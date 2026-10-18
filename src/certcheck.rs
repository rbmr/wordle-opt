//! Independent checker for optimality certificates.
//!
//! A certificate is a list of claims `LB(S) >= b` about candidate sets `S`.
//! This module checks one against a readable policy tree using nothing from
//! the solver: its own feedback function (written like the one in
//! `tests/optimality_audit.rs`), its own tree walker, no transposition table,
//! no search and no pruning shortcuts. If every claim checks and the root claim
//! bounds the whole candidate list by the tree's cost, the tree is optimal.
//!
//! Each claim names its set explicitly, as the root (every candidate) or as
//! one bucket of an earlier claim's set (`parent`, `guess`, `response`), and
//! the checker recomputes the members. Claims are looked up by their exact
//! member list, never by a hash alone. Each claim cites one rule:
//!
//! * **base**: `|S| = 1` costs 1 and `|S| = 2` costs 3.
//! * **capacity**: `b <= L3(|S|, k)`, where the checker computes `k`, the most
//!   distinct responses any allowed guess gives on `S`. See [`l3`].
//! * **expansion**: for *every* allowed guess `g` (the checker enumerates the
//!   whole list itself) that splits `S`, `|S| + sum over non-win buckets B of
//!   LB(B) >= b`, where `LB(B)` is the largest claim on exactly `B`, or the
//!   default `base` / `L3(|B|, k(S))` bound if that is larger. A guess that
//!   does not split `S` returns to the same set one guess later, so it never
//!   helps and is skipped.
//! * **assumed**: not checked. A shallow certificate stops at a depth and
//!   leaves its deepest claims to other evidence; the report says how many.
//!
//! Soundness is induction on `|S|`: the buckets an expansion uses are strict
//! subsets, so every claim it relies on is about a smaller set.

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

type W = [u8; 5];

/// All-green response, base 3 with green = 1 at every position.
const WIN: u8 = 121;

pub const MAGIC: &[u8; 8] = b"WOCERT01";

pub const RULE_BASE: u8 = 0;
pub const RULE_CAPACITY: u8 = 1;
pub const RULE_EXPANSION: u8 = 2;
pub const RULE_ASSUMED: u8 = 3;

/// Per position 0 black, 1 green, 2 yellow, position 0 least significant
/// (the policy JSON's `b`/`g`/`y` key order).
fn feedback(guess: &W, secret: &W) -> u8 {
    let mut marks = [0u8; 5];
    let mut unmatched = [0u8; 256];
    for i in 0..5 {
        if guess[i] == secret[i] {
            marks[i] = 1;
        } else {
            unmatched[secret[i] as usize] += 1;
        }
    }
    for i in 0..5 {
        let l = guess[i] as usize;
        if marks[i] == 0 && unmatched[l] > 0 {
            marks[i] = 2;
            unmatched[l] -= 1;
        }
    }
    marks.iter().rev().fold(0, |acc, &m| acc * 3 + m)
}

fn code_key(code: u8) -> String {
    let mut v = code;
    (0..5)
        .map(|_| {
            let c = b"bgy"[(v % 3) as usize] as char;
            v /= 3;
            c
        })
        .collect()
}

/// Lower bound on the cost of any policy for `n` candidates when no allowed
/// guess gives more than `k` distinct responses on them.
///
/// Proof: the first guess wins at most one candidate (`w1 <= 1`). It leaves
/// at most `k - 1` non-win buckets if it is a candidate and at most `k` if it
/// is not (then `w1 = 0`); each bucket's first guess wins at most one more, so
/// `w2 <= k - 1` (or `k`) and `w2 <= n - w1`. Everything else costs at least 3,
/// so the cost is at least `3n - 2 w1 - w2`, minimized at
/// `max(3n - k - 1, 2n - 1)` by the in-set case.
///
/// This is `heuristic::capacity_bound` truncated at depth 3. That function also
/// charges depth 4 and beyond as if every node were a candidate, which is not a
/// valid lower bound once `n >= k^2 - k + 3`: a non-candidate guess trades its
/// own win for one more bucket, and from depth 4 on that extra bucket is worth
/// more than the win (for example `n = 9, k = 3`: 24 is achievable, the
/// formula says 25).
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
pub fn digest<'a>(
    guesses: impl IntoIterator<Item = &'a [u8; 5]>,
    candidates: impl IntoIterator<Item = &'a [u8; 5]>,
) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    eat(b'G');
    for w in guesses {
        w.iter().for_each(|&b| eat(b));
    }
    eat(b'C');
    for w in candidates {
        w.iter().for_each(|&b| eat(b));
    }
    h
}

#[derive(Debug, Default)]
pub struct Report {
    pub tree_cost: u64,
    pub claims: usize,
    pub by_rule: [usize; 4],
    pub root_bound: u64,
    pub assumed_max_size: usize,
    /// Response reads the check performed in its main loops: `|G| |S|` per
    /// capacity claim, `2 |G| |S|` per expansion.
    pub lookups: u64,
    /// Every failed check. Empty means the certificate is valid.
    pub failures: Vec<String>,
}

impl Report {
    /// Optimal outright: valid, and no claim was left unchecked.
    pub fn proves_optimal(&self) -> bool {
        self.failures.is_empty() && self.by_rule[RULE_ASSUMED as usize] == 0
    }
}

struct Lists {
    guesses: Vec<W>,
    candidates: Vec<W>,
    /// Feedback of guess `g` on candidate `c` at `g * candidates.len() + c`.
    fb: Vec<u8>,
    /// The same table candidate-major: `c * guesses.len() + g`.
    fb_by_candidate: Vec<u8>,
}

impl Lists {
    fn row(&self, g: usize) -> &[u8] {
        let n = self.candidates.len();
        &self.fb[g * n..(g + 1) * n]
    }
    fn column(&self, c: usize) -> &[u8] {
        let n = self.guesses.len();
        &self.fb_by_candidate[c * n..(c + 1) * n]
    }
}

fn to_word(v: &serde_json::Value) -> Result<W, String> {
    let s = v.as_str().ok_or("word is not a string")?;
    s.as_bytes()
        .try_into()
        .map_err(|_| format!("{s:?} is not a 5-byte word"))
}

fn words(file: &serde_json::Value, key: &str) -> Result<Vec<W>, String> {
    file.get(key)
        .and_then(|v| v.as_array())
        .ok_or(format!("tree has no {key} list"))?
        .iter()
        .map(to_word)
        .collect()
}

/// Walks the tree, checking that every node plays an allowed guess, that its
/// children are exactly its possible non-win responses, and that each
/// candidate is won exactly once (implied by partitioning). Returns the cost.
fn walk(
    node: &serde_json::Value,
    set: &[u16],
    lists: &Lists,
    index: &HashMap<W, usize>,
    path: &str,
) -> Result<u64, String> {
    let guess = to_word(node.get("guess").ok_or(format!("{path}: no guess"))?)?;
    let &g = index
        .get(&guess)
        .ok_or(format!("{path}: guess is not allowed"))?;
    let row = lists.row(g);
    let mut buckets: HashMap<u8, Vec<u16>> = HashMap::new();
    for &c in set {
        buckets.entry(row[c as usize]).or_default().push(c);
    }
    buckets.remove(&WIN);
    let empty = serde_json::Map::new();
    let children = match node.get("children") {
        None => &empty,
        Some(v) => v.as_object().ok_or(format!("{path}: children not a map"))?,
    };
    let mut want: Vec<String> = buckets.keys().map(|&r| code_key(r)).collect();
    want.sort_unstable();
    let mut have: Vec<String> = children.keys().cloned().collect();
    have.sort_unstable();
    if want != have {
        return Err(format!("{path}: children are not the possible responses"));
    }
    if buckets.len() == 1 && buckets.values().next().unwrap().len() == set.len() {
        return Err(format!("{path}: guess does not split its set"));
    }
    let mut cost = set.len() as u64;
    for (r, bucket) in buckets {
        let key = code_key(r);
        let child_path = format!("{path}/{key}");
        cost += walk(&children[&key], &bucket, lists, index, &child_path)?;
    }
    Ok(cost)
}

struct ClaimRec {
    set: Vec<u16>,
    bound: u64,
    rule: u8,
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

fn parse_claims(cert: &[u8], lists: &Lists) -> Result<Vec<ClaimRec>, String> {
    let mut r = Reader(cert);
    if r.take(8)? != MAGIC {
        return Err("not a certificate (bad magic)".into());
    }
    if r.u64()? != digest(&lists.guesses, &lists.candidates) {
        return Err("certificate is for different word lists".into());
    }
    if r.u32()? as usize != lists.guesses.len() || r.u32()? as usize != lists.candidates.len() {
        return Err("certificate list sizes differ from the tree's".into());
    }
    let count = r.u32()? as usize;
    let mut claims: Vec<ClaimRec> = Vec::with_capacity(count);
    for i in 0..count {
        let set = match r.u8()? {
            0 => (0..lists.candidates.len() as u16).collect(),
            1 => {
                let parent = r.u32()? as usize;
                let g = r.u32()? as usize;
                let resp = r.u8()?;
                if parent >= i || g >= lists.guesses.len() || resp >= 243 || resp == WIN {
                    return Err(format!("claim {i}: bad set definition"));
                }
                let row = lists.row(g);
                let set: Vec<u16> = claims[parent]
                    .set
                    .iter()
                    .copied()
                    .filter(|&c| row[c as usize] == resp)
                    .collect();
                if set.is_empty() {
                    return Err(format!("claim {i}: empty set"));
                }
                set
            }
            t => return Err(format!("claim {i}: unknown set tag {t}")),
        };
        let bound = r.u32()? as u64;
        let rule = r.u8()?;
        if rule > RULE_ASSUMED {
            return Err(format!("claim {i}: unknown rule {rule}"));
        }
        claims.push(ClaimRec { set, bound, rule });
    }
    if !r.0.is_empty() {
        return Err("trailing bytes after the last claim".into());
    }
    Ok(claims)
}

/// Whether no allowed guess gives more than `limit` distinct responses on
/// `set`. A guess position whose letter occurs in no word of `set` is gray for
/// every candidate in it, so a guess with `v` other positions gives at most
/// `min(3^v, |set|)` responses; only guesses where that exceeds `limit` are
/// read, from the candidate-major table.
fn branching_at_most(set: &[u16], lists: &Lists, limit: usize) -> bool {
    if set.len() <= limit {
        return true;
    }
    let mut present = [false; 256];
    for &c in set {
        for &b in &lists.candidates[c as usize] {
            present[b as usize] = true;
        }
    }
    let risky: Vec<usize> = (0..lists.guesses.len())
        .filter(|&g| {
            let v = lists.guesses[g]
                .iter()
                .filter(|&&b| present[b as usize])
                .count();
            3usize.pow(v as u32) > limit
        })
        .collect();
    let mut seen = [[0u64; 4]; 256];
    for chunk in risky.chunks(256) {
        let seen = &mut seen[..chunk.len()];
        seen.fill([0; 4]);
        for &c in set {
            let column = lists.column(c as usize);
            for (s, &g) in seen.iter_mut().zip(chunk) {
                let r = column[g];
                s[(r >> 6) as usize] |= 1 << (r & 63);
            }
        }
        if seen
            .iter()
            .any(|s| s.iter().map(|w| w.count_ones() as usize).sum::<usize>() > limit)
        {
            return false;
        }
    }
    true
}

fn default_bound(n: usize, k: usize) -> u64 {
    l3(n, k)
}

/// Checks one expansion claim; `Err` names the first guess that beats it.
fn check_expansion(
    set: &[u16],
    bound: u64,
    lists: &Lists,
    best: &HashMap<&[u16], u64>,
) -> Result<(), String> {
    let n = set.len();
    // One pass over the candidate-major table, in chunks of guesses, finds both
    // `k` and every guess's default sum. A bucket of size b >= 1 defaults to
    // L3(b, k) = (2b - 1) + max(0, b - k), and k is at least the number of
    // buckets m of any guess, so only buckets larger than m can contribute the
    // second term: those are kept until `k` is known.
    const CHUNK: usize = 256;
    let ng = lists.guesses.len();
    let mut counts = vec![0u16; CHUNK * 243];
    let mut used: Vec<Vec<u8>> = (0..CHUNK).map(|_| Vec::new()).collect();
    // Per guess: splits it, base sum n + sum(2b - 1), and its large buckets.
    // Guesses whose base sum n + sum(2b - 1) falls short of the bound: only they
    // can need the max(0, b - k) terms, so only their large buckets are kept.
    let mut pending: Vec<(usize, u64, Vec<u16>)> = Vec::new();
    let mut k = 0;
    for start in (0..ng).step_by(CHUNK) {
        let end = (start + CHUNK).min(ng);
        for &c in set {
            for (j, &r) in lists.column(c as usize)[start..end].iter().enumerate() {
                let slot = &mut counts[j * 243 + r as usize];
                if *slot == 0 {
                    used[j].push(r);
                }
                *slot += 1;
            }
        }
        for (j, u) in used[..end - start].iter_mut().enumerate() {
            let m = u.len();
            k = k.max(m);
            let splits = m > 1 || u[0] == WIN;
            let mut sum = n as u64;
            let mut big: Vec<u16> = Vec::new();
            for &r in u.iter() {
                let slot = &mut counts[j * 243 + r as usize];
                let b = *slot as usize;
                if r != WIN {
                    sum += 2 * b as u64 - 1;
                    if b > m {
                        big.push(b as u16);
                    }
                }
                *slot = 0;
            }
            u.clear();
            if splits && sum < bound {
                pending.push((start + j, sum, big));
            }
        }
    }
    // A guess that does not split the set is never useful. Only guesses whose
    // default sum falls short of `bound` are looked at again, with the claims
    // on their buckets.
    let short: Vec<usize> = pending
        .into_iter()
        .filter(|(_, sum, big)| {
            let extra: u64 = big
                .iter()
                .map(|&b| (b as u64).saturating_sub(k as u64))
                .sum();
            sum + extra < bound
        })
        .map(|(g, _, _)| g)
        .collect();
    let mut seen_rows: HashSet<Vec<u8>> = HashSet::new();
    for g in short {
        let row = lists.row(g);
        // Defaults are not enough: use the claims on this guess's buckets.
        let key: Vec<u8> = set.iter().map(|&c| row[c as usize]).collect();
        if !seen_rows.insert(key) {
            continue; // identical partition, already checked
        }
        let mut buckets: HashMap<u8, Vec<u16>> = HashMap::new();
        for &c in set {
            let r = row[c as usize];
            if r != WIN {
                buckets.entry(r).or_default().push(c);
            }
        }
        let mut sum = n as u64;
        for b in buckets.values() {
            let d = default_bound(b.len(), k);
            sum += best.get(b.as_slice()).map_or(d, |&v| v.max(d));
        }
        if sum < bound {
            let w = lists.guesses[g];
            return Err(format!(
                "guess {} gives only {sum} < {bound}",
                String::from_utf8_lossy(&w)
            ));
        }
    }
    Ok(())
}

/// Checks `cert` against the readable policy tree `tree_json`.
pub fn check(tree_json: &str, cert: &[u8]) -> Result<Report, String> {
    let file: serde_json::Value =
        serde_json::from_str(tree_json).map_err(|e| format!("tree is not JSON: {e}"))?;
    let guesses = words(&file, "guesses")?;
    let candidates = words(&file, "candidates")?;
    if candidates.is_empty() || candidates.len() > u16::MAX as usize {
        return Err("unsupported candidate count".into());
    }
    let n = candidates.len();
    let fb: Vec<u8> = guesses
        .par_iter()
        .flat_map_iter(|g| candidates.iter().map(move |c| feedback(g, c)))
        .collect();
    let fb_by_candidate: Vec<u8> = candidates
        .par_iter()
        .flat_map_iter(|c| guesses.iter().map(move |g| feedback(g, c)))
        .collect();
    let lists = Lists {
        guesses,
        candidates,
        fb,
        fb_by_candidate,
    };
    let index: HashMap<W, usize> = lists
        .guesses
        .iter()
        .enumerate()
        .map(|(i, &w)| (w, i))
        .collect();
    let all: Vec<u16> = (0..n as u16).collect();
    let root = file.get("root").ok_or("tree has no root")?;
    let tree_cost = walk(root, &all, &lists, &index, "")?;

    let claims = parse_claims(cert, &lists)?;
    let mut report = Report {
        tree_cost,
        claims: claims.len(),
        ..Default::default()
    };
    let mut best: HashMap<&[u16], u64> = HashMap::new();
    for c in &claims {
        report.by_rule[c.rule as usize] += 1;
        let g = lists.guesses.len() as u64;
        report.lookups += match c.rule {
            RULE_CAPACITY => g * c.set.len() as u64,
            RULE_EXPANSION => 2 * g * c.set.len() as u64,
            _ => 0,
        };
        let e = best.entry(c.set.as_slice()).or_insert(0);
        *e = (*e).max(c.bound);
        if c.rule == RULE_ASSUMED {
            report.assumed_max_size = report.assumed_max_size.max(c.set.len());
        }
    }

    let failures: Vec<String> = claims
        .par_iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let size = c.set.len();
            let res = match c.rule {
                RULE_BASE => match size {
                    1 | 2 if c.bound <= l3(size, 0) => Ok(()),
                    _ => Err("base rule needs |S| <= 2 and b <= 1 or 3".to_string()),
                },
                RULE_CAPACITY => {
                    // b <= L3(n, k) = max(2n - 1, 3n - k - 1): either b <= 2n - 1,
                    // or no guess may give more than 3n - 1 - b responses.
                    let n = size as u64;
                    let limit = (3 * n).saturating_sub(1 + c.bound) as usize;
                    if c.bound < 2 * n || branching_at_most(&c.set, &lists, limit) {
                        Ok(())
                    } else {
                        Err(format!("some guess gives more than {limit} responses"))
                    }
                }
                RULE_EXPANSION => check_expansion(&c.set, c.bound, &lists, &best),
                _ => Ok(()),
            };
            res.err()
                .map(|e| format!("claim {i} (|S| = {size}, b = {}): {e}", c.bound))
        })
        .collect();
    report.failures = failures;

    match claims.first() {
        Some(c) if c.set.len() == n => {
            report.root_bound = c.bound;
            if c.bound < tree_cost {
                report.failures.push(format!(
                    "root claim {} is below the tree's cost {tree_cost}",
                    c.bound
                ));
            }
        }
        _ => report
            .failures
            .push("the first claim must be about every candidate".into()),
    }
    Ok(report)
}

/// `wordle-opt check-cert <tree.json> <cert.bin>`
pub fn run_cli(args: &[String]) {
    let (Some(tree_path), Some(cert_path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: wordle-opt check-cert <tree.json> <certificate>");
        std::process::exit(2);
    };
    let start = std::time::Instant::now();
    let tree = std::fs::read_to_string(tree_path).unwrap_or_else(|e| {
        eprintln!("check-cert: cannot read {tree_path}: {e}");
        std::process::exit(1);
    });
    let cert = std::fs::read(cert_path).unwrap_or_else(|e| {
        eprintln!("check-cert: cannot read {cert_path}: {e}");
        std::process::exit(1);
    });
    match check(&tree, &cert) {
        Ok(r) => {
            println!("Tree cost: {}", r.tree_cost);
            println!("Root claim: LB >= {}", r.root_bound);
            println!(
                "Claims: {} (base {}, capacity {}, expansion {}, assumed {})",
                r.claims, r.by_rule[0], r.by_rule[1], r.by_rule[2], r.by_rule[3]
            );
            println!("Certificate bytes: {}", cert.len());
            println!(
                "Check volume: {} response reads ({:.2e})",
                r.lookups, r.lookups as f64
            );
            println!("Check time: {:.3}s", start.elapsed().as_secs_f64());
            for f in r.failures.iter().take(20) {
                println!("FAIL: {f}");
            }
            if !r.failures.is_empty() {
                println!("CERTIFICATE INVALID ({} failures)", r.failures.len());
                std::process::exit(1);
            }
            if r.proves_optimal() {
                println!("CERTIFICATE VALID: the tree is optimal");
            } else {
                println!(
                    "CERTIFICATE VALID GIVEN {} ASSUMED CLAIMS (largest |S| = {})",
                    r.by_rule[3], r.assumed_max_size
                );
                std::process::exit(3);
            }
        }
        Err(e) => {
            eprintln!("check-cert: {e}");
            std::process::exit(1);
        }
    }
}
