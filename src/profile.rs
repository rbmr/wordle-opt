//! A cheap upper bound on how many distinct responses a guess can give on a
//! set of words, from which letters the set has at each position.

/// One bit per byte value.
type ByteSet = [u64; 4];

fn has(set: &ByteSet, b: u8) -> bool {
    set[(b >> 6) as usize] >> (b & 63) & 1 == 1
}

fn add(set: &mut ByteSet, b: u8) {
    set[(b >> 6) as usize] |= 1 << (b & 63);
}

/// The most positions a profile tracks: the capacity of a [`crate::core::Word`].
const MAX_POS: usize = 15;

/// For one set of words, which letters occur where.
///
/// Position `i` of a guess with letter `l` can only be green if some word has
/// `l` at `i`, only yellow if some word has `l` elsewhere and not at `i`, and
/// only black if some word lacks `l` (or, when the guess repeats `l`, does not
/// have `l` at `i`). The response is the marks at every position together, so
/// at most the product of the per-position counts of possible marks can occur.
/// This holds for words of any length; every position is counted.
pub struct SetProfile {
    words: usize,
    /// Letters some word has at position `i`.
    at: [ByteSet; MAX_POS],
    /// Letters some word lacks at position `i`.
    not_at: [ByteSet; MAX_POS],
    /// Letters some word has, but not at position `i`.
    elsewhere: [ByteSet; MAX_POS],
    /// Letters some word lacks entirely.
    missing: ByteSet,
}

impl SetProfile {
    pub fn new<'a, T: AsRef<[u8]> + 'a>(words: impl IntoIterator<Item = &'a T>) -> Self {
        let mut p = SetProfile {
            words: 0,
            at: [[0; 4]; MAX_POS],
            not_at: [[0; 4]; MAX_POS],
            elsewhere: [[0; 4]; MAX_POS],
            missing: [0; 4],
        };
        // Letters every word has, and letters every word has at each
        // position; `missing` and `not_at` are their complements.
        let mut in_all: ByteSet = [u64::MAX; 4];
        let mut at_in_all: [ByteSet; MAX_POS] = [[u64::MAX; 4]; MAX_POS];
        for w in words {
            let slice = w.as_ref();
            assert!(slice.len() <= MAX_POS, "word longer than {MAX_POS} bytes");
            p.words += 1;
            let mut seen: ByteSet = [0; 4];
            for (i, &b) in slice.iter().enumerate() {
                add(&mut p.at[i], b);
                add(&mut seen, b);
                let mut only: ByteSet = [0; 4];
                add(&mut only, b);
                for (x, o) in at_in_all[i].iter_mut().zip(only) {
                    *x &= o;
                }
            }
            for (x, s) in in_all.iter_mut().zip(seen) {
                *x &= s;
            }
            for (i, &own) in slice.iter().enumerate() {
                for &b in slice {
                    if b != own {
                        add(&mut p.elsewhere[i], b);
                    }
                }
            }
        }
        for (m, x) in p.missing.iter_mut().zip(in_all) {
            *m = !x;
        }
        for (not_at, all) in p.not_at.iter_mut().zip(&at_in_all) {
            for (n, x) in not_at.iter_mut().zip(all) {
                *n = !x;
            }
        }
        p
    }

    /// Upper bound on the distinct responses `guess` gives on the set,
    /// never more than the number of words.
    pub fn max_responses(&self, guess: &[u8]) -> usize {
        let mut product = 1usize;
        for (i, &l) in guess.iter().enumerate() {
            let repeated = guess.iter().filter(|&&b| b == l).count() > 1;
            let green = has(&self.at[i], l) as usize;
            let yellow = has(&self.elsewhere[i], l) as usize;
            let black = if repeated {
                has(&self.not_at[i], l)
            } else {
                has(&self.missing, l)
            } as usize;
            product = product.saturating_mul(green + yellow + black);
        }
        product.min(self.words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The responses, written out directly (letter counting).
    fn response(guess: &[u8; 5], secret: &[u8; 5]) -> u8 {
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

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    #[test]
    fn bound_holds_for_random_sets_and_guesses() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let mut tight = 0;
        for trial in 0..20_000 {
            let alphabet = 2 + rng.below(8);
            let word = |rng: &mut Rng| -> [u8; 5] {
                std::array::from_fn(|_| b'a' + rng.below(alphabet) as u8)
            };
            let set: Vec<[u8; 5]> = (0..1 + rng.below(40)).map(|_| word(&mut rng)).collect();
            let profile = SetProfile::new(&set);
            for _ in 0..8 {
                let g = word(&mut rng);
                let mut seen = [false; 243];
                let actual = set
                    .iter()
                    .filter(|s| !std::mem::replace(&mut seen[response(&g, s) as usize], true))
                    .count();
                let bound = profile.max_responses(&g);
                assert!(
                    actual <= bound,
                    "trial {trial}: guess {:?} gives {actual} responses on {:?}, bound {bound}",
                    String::from_utf8_lossy(&g),
                    set.iter()
                        .map(|w| String::from_utf8_lossy(w).into_owned())
                        .collect::<Vec<_>>()
                );
                tight += (actual == bound) as usize;
            }
        }
        // Not a requirement for soundness, only a sign the bound is useful.
        assert!(tight > 20_000, "bound met with equality only {tight} times");
    }

    /// The responses for words of any length, as a base-3 number.
    fn response_any(guess: &[u8], secret: &[u8]) -> u64 {
        let n = guess.len();
        let mut marks = vec![0u8; n];
        let mut unmatched = [0u8; 256];
        for i in 0..n {
            if guess[i] == secret[i] {
                marks[i] = 1;
            } else {
                unmatched[secret[i] as usize] += 1;
            }
        }
        for i in 0..n {
            let l = guess[i] as usize;
            if marks[i] == 0 && unmatched[l] > 0 {
                marks[i] = 2;
                unmatched[l] -= 1;
            }
        }
        marks.iter().rev().fold(0, |acc, &m| acc * 3 + m as u64)
    }

    /// The bound must hold at every word length the dictionary accepts, not
    /// only for five letters: positions past the fifth vary the response too.
    #[test]
    fn bound_holds_for_every_word_length() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for len in 1..=10usize {
            for trial in 0..3_000 {
                let alphabet = 2 + rng.below(6);
                let word = |rng: &mut Rng| -> Vec<u8> {
                    (0..len).map(|_| b'a' + rng.below(alphabet) as u8).collect()
                };
                let set: Vec<Vec<u8>> = (0..1 + rng.below(30)).map(|_| word(&mut rng)).collect();
                let profile = SetProfile::new(&set);
                for _ in 0..6 {
                    let g = word(&mut rng);
                    let mut seen = std::collections::HashSet::new();
                    let actual = set
                        .iter()
                        .filter(|s| seen.insert(response_any(&g, s)))
                        .count();
                    let bound = profile.max_responses(&g);
                    assert!(
                        actual <= bound,
                        "length {len}, trial {trial}: guess {:?} gives {actual} responses on {:?}, bound {bound}",
                        String::from_utf8_lossy(&g),
                        set.iter()
                            .map(|w| String::from_utf8_lossy(w).into_owned())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }

    /// Words that differ only after the fifth position.
    #[test]
    fn late_positions_count() {
        let set: Vec<&[u8]> = vec![b"aaaaab", b"aaaaac"];
        let profile = SetProfile::new(&set);
        assert_eq!(profile.max_responses(b"aaaaab"), 2);
    }
}
