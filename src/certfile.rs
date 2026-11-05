//! The optimality-certificate file format, written by the solver and read by
//! the checker.
//!
//! A certificate is a list of claims `LB(S) >= b` about candidate sets `S`.
//! Each claim names its set as the root (every candidate) or as one bucket of
//! an earlier claim's set (`parent`, `guess`, `response`), and cites the rule
//! that justifies it (see `crate::certcheck` for what each rule requires).
//!
//! Layout, little-endian:
//!
//! ```text
//! magic "WOCERT01" | digest u64 | guesses u32 | candidates u32 | claims u32
//! per claim: tag u8 (0 root, 1 bucket) [parent u32, guess u32, response u8]
//!            bound u32 | rule u8
//! ```

/// First eight bytes of every certificate.
pub const MAGIC: &[u8; 8] = b"WOCERT01";

/// `|S| = 1` costs 1 and `|S| = 2` costs 3.
pub const RULE_BASE: u8 = 0;
/// `b <= L3(|S|, k)` for the most responses `k` any guess gives on `S`.
pub const RULE_CAPACITY: u8 = 1;
/// Every guess that splits `S` costs at least `b`, given the claims on its buckets.
pub const RULE_EXPANSION: u8 = 2;
/// Not checked: left to other evidence.
pub const RULE_ASSUMED: u8 = 3;

/// How a claim names its set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetDef {
    /// Every candidate.
    Root,
    /// The candidates of claim `parent`'s set that give `response` to `guess`.
    Child {
        parent: u32,
        guess: u32,
        response: u8,
    },
}

/// One claim: `LB(set) >= bound`, by `rule`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub def: SetDef,
    pub bound: u32,
    pub rule: u8,
}

/// The word lists a certificate is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// [`digest`] of both lists.
    pub digest: u64,
    pub guesses: u32,
    pub candidates: u32,
}

/// Lower bound on the cost of any policy for `n` candidates when no allowed
/// guess gives more than `k` distinct responses on them.
///
/// The first guess wins at most one candidate (`w1 <= 1`). It leaves at most
/// `k - 1` non-win buckets if it is a candidate and at most `k` if it is not
/// (then `w1 = 0`); each bucket's first guess wins at most one more, so
/// `w2 <= k - w1` and `w2 <= n - w1`. Everything else costs at least 3, so the
/// cost is at least `3n - 2 w1 - w2`, which is smallest at
/// `max(3n - k - 1, 2n - 1)`.
pub fn l3(n: usize, k: usize) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        2 => 3,
        _ => {
            let n = n as u64;
            (2 * n - 1).max((3 * n).saturating_sub(k as u64 + 1))
        }
    }
}

/// FNV-1a over both word lists, binding a certificate to the exact lists.
pub fn digest<'a, T: AsRef<[u8]> + 'a>(
    guesses: impl IntoIterator<Item = &'a T>,
    candidates: impl IntoIterator<Item = &'a T>,
) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    eat(b'G');
    for w in guesses {
        w.as_ref().iter().for_each(|&b| eat(b));
    }
    eat(b'C');
    for w in candidates {
        w.as_ref().iter().for_each(|&b| eat(b));
    }
    h
}

/// Serializes a certificate.
pub fn encode(header: &Header, claims: &[Claim]) -> Vec<u8> {
    let mut out = Vec::with_capacity(28 + 14 * claims.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&header.digest.to_le_bytes());
    out.extend_from_slice(&header.guesses.to_le_bytes());
    out.extend_from_slice(&header.candidates.to_le_bytes());
    out.extend_from_slice(&(claims.len() as u32).to_le_bytes());
    for c in claims {
        match c.def {
            SetDef::Root => out.push(0),
            SetDef::Child {
                parent,
                guess,
                response,
            } => {
                out.push(1);
                out.extend_from_slice(&parent.to_le_bytes());
                out.extend_from_slice(&guess.to_le_bytes());
                out.push(response);
            }
        }
        out.extend_from_slice(&c.bound.to_le_bytes());
        out.push(c.rule);
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.0.len() < n {
            return Err("certificate is truncated".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Parses a certificate. Checks the layout only: whether the header matches
/// the lists, and what the claims mean, is up to the reader.
pub fn decode(bytes: &[u8]) -> Result<(Header, Vec<Claim>), String> {
    let mut r = Reader(bytes);
    if r.take(8)? != MAGIC {
        return Err("not a certificate (bad magic)".into());
    }
    let header = Header {
        digest: r.u64()?,
        guesses: r.u32()?,
        candidates: r.u32()?,
    };
    let count = r.u32()? as usize;
    let mut claims = Vec::with_capacity(count.min(bytes.len() / 6));
    for i in 0..count {
        let def = match r.u8()? {
            0 => SetDef::Root,
            1 => SetDef::Child {
                parent: r.u32()?,
                guess: r.u32()?,
                response: r.u8()?,
            },
            t => return Err(format!("claim {i}: unknown set tag {t}")),
        };
        let bound = r.u32()?;
        let rule = r.u8()?;
        if rule > RULE_ASSUMED {
            return Err(format!("claim {i}: unknown rule {rule}"));
        }
        claims.push(Claim { def, bound, rule });
    }
    if !r.0.is_empty() {
        return Err("trailing bytes after the last claim".into());
    }
    Ok((header, claims))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Header, Vec<Claim>) {
        let header = Header {
            digest: 0x0123_4567_89ab_cdef,
            guesses: 14855,
            candidates: 2340,
        };
        let claims = vec![
            Claim {
                def: SetDef::Root,
                bound: 7920,
                rule: RULE_EXPANSION,
            },
            Claim {
                def: SetDef::Child {
                    parent: 0,
                    guess: 14854,
                    response: 242,
                },
                bound: u32::MAX,
                rule: RULE_ASSUMED,
            },
            Claim {
                def: SetDef::Child {
                    parent: 1,
                    guess: 0,
                    response: 0,
                },
                bound: 1,
                rule: RULE_BASE,
            },
        ];
        (header, claims)
    }

    #[test]
    fn decode_inverts_encode() {
        let (header, claims) = sample();
        let bytes = encode(&header, &claims);
        assert_eq!(decode(&bytes).unwrap(), (header, claims));
    }

    #[test]
    fn decode_rejects_every_truncation_and_trailing_bytes() {
        let (header, claims) = sample();
        let bytes = encode(&header, &claims);
        for len in 0..bytes.len() {
            assert!(
                decode(&bytes[..len]).is_err(),
                "accepted {len} of {} bytes",
                bytes.len()
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
    }

    #[test]
    fn decode_rejects_bad_magic_tags_and_rules() {
        let (header, claims) = sample();
        let bytes = encode(&header, &claims);
        let mut bad = bytes.clone();
        bad[0] ^= 1;
        assert!(decode(&bad).is_err());
        // The first claim's set tag sits right after the 28-byte header.
        let mut bad = bytes.clone();
        bad[28] = 2;
        assert!(decode(&bad).is_err());
        // Its rule is the byte after the tag and the 4-byte bound.
        let mut bad = bytes;
        bad[28 + 1 + 4] = RULE_ASSUMED + 1;
        assert!(decode(&bad).is_err());
    }

    /// `l3` is a lower bound: no tree of any shape with branching at most `k`
    /// beats it (exhaustive over small trees, by dynamic programming over the
    /// sizes of the first guess's buckets).
    #[test]
    fn l3_never_exceeds_the_exact_minimum() {
        for k in 2..=6usize {
            let max_n = 60;
            let inf = u64::MAX / 4;
            let mut f = vec![0u64; max_n + 1];
            let mut p = vec![vec![inf; max_n + 1]; k + 1];
            for row in p.iter_mut() {
                row[0] = 0;
            }
            for m in 1..=max_n {
                if m == 1 {
                    f[1] = 1;
                } else {
                    let win = p[k - 1][m - 1];
                    let split = (1..m).map(|x| f[x] + p[k - 1][m - x]).min().unwrap();
                    f[m] = m as u64 + win.min(split);
                }
                for j in 1..=k {
                    let best = (1..=m).map(|x| f[x] + p[j - 1][m - x]).min().unwrap();
                    p[j][m] = p[j - 1][m].min(best);
                }
            }
            for (n, &exact) in f.iter().enumerate() {
                assert!(l3(n, k) <= exact, "l3({n}, {k}) = {} > {exact}", l3(n, k));
            }
        }
    }
}
