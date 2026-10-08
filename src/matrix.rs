#![allow(clippy::needless_range_loop)]
use crate::core::Response;
use crate::dict::Dictionary;

/// A precomputed lookup table mapping every (Guess, Candidate) pair to their resulting Wordle `Response`.
///
/// The matrix is flattened into a 1D vector and stored in row-major order: `guess * num_candidates + candidate`.
/// This gives O(1) lookup, and keeps a single guess's row contiguous for the inner solver loops that
/// iterate candidates for a fixed guess.
pub struct ResponseMatrix {
    pub num_guesses: usize,
    pub num_candidates: usize,
    pub guess_masks: Vec<u32>,
    pub candidate_masks: Vec<u32>,
    pub zobrist: Vec<u64>,
    pub data: Vec<Response>,
    /// Transposed matrix: `data_c_g[candidate * num_guesses + guess]`.
    ///
    /// This layout provides coalesced, cache-friendly memory access when looping over all guesses
    /// for a fixed candidate. This is critical for the `is_equivalent` check which compares two
    /// candidates across all possible guesses.
    pub data_c_g: Vec<Response>,
}

impl ResponseMatrix {
    pub fn new(dict: &Dictionary) -> Self {
        let num_guesses = dict.guesses.len();
        let num_candidates = dict.candidates.len();
        let mut data = Vec::with_capacity(num_guesses * num_candidates);
        let mut guess_masks = Vec::with_capacity(num_guesses);
        let mut candidate_masks = Vec::with_capacity(num_candidates);
        let mut zobrist = Vec::with_capacity(num_candidates);
        let mut rng = fastrand::Rng::with_seed(42);
        for _ in 0..num_candidates {
            zobrist.push(rng.u64(..));
        }

        for guess in &dict.guesses {
            let mut mask = 0u32;
            for &b in &guess.0 {
                mask |= 1 << (b - b'a');
            }
            guess_masks.push(mask);
        }

        for candidate in &dict.candidates {
            let mut mask = 0u32;
            for &b in &candidate.0 {
                mask |= 1 << (b - b'a');
            }
            candidate_masks.push(mask);
        }

        use rayon::prelude::*;

        // Pre-allocate the data array
        data.resize(dict.guesses.len() * dict.candidates.len(), Response(0));

        let num_candidates = dict.candidates.len();
        let guesses = &dict.guesses;
        let candidates = &dict.candidates;

        // Compute rows in parallel
        data.par_chunks_mut(num_candidates)
            .enumerate()
            .for_each(|(g, row)| {
                let guess = &guesses[g];
                for (c, candidate) in candidates.iter().enumerate() {
                    row[c] = Response::compute(candidate, guess);
                }
            });

        let mut data_c_g = vec![Response(0); num_candidates * num_guesses];
        for c in 0..num_candidates {
            for g in 0..num_guesses {
                data_c_g[c * num_guesses + g] = data[g * num_candidates + c];
            }
        }

        Self {
            num_guesses,
            num_candidates,
            guess_masks,
            candidate_masks,
            zobrist,
            data,
            data_c_g,
        }
    }

    #[inline]
    pub fn get(&self, guess_idx: usize, candidate_idx: usize) -> Response {
        self.data[guess_idx * self.num_candidates + candidate_idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Response, Word};
    use crate::dict::Dictionary;

    #[test]
    fn test_matrix_computation() {
        let mut guesses = Vec::new();
        let mut candidates = Vec::new();
        guesses.push(Word::new("apple"));
        guesses.push(Word::new("berry"));
        candidates.push(Word::new("apple"));
        candidates.push(Word::new("maple"));

        let dict = Dictionary {
            guess_chars: guesses
                .iter()
                .map(|w| {
                    [
                        w.0[0] - b'a',
                        w.0[1] - b'a',
                        w.0[2] - b'a',
                        w.0[3] - b'a',
                        w.0[4] - b'a',
                    ]
                })
                .collect(),
            guesses,
            candidates,
            candidate_to_guess: vec![0, 1],
            guess_to_candidate: vec![u16::MAX; 2],
        };

        let matrix = ResponseMatrix::new(&dict);
        assert_eq!(matrix.num_guesses, 2);
        assert_eq!(matrix.num_candidates, 2);

        // guess 0: apple, candidate 0: apple -> WIN
        assert_eq!(matrix.get(0, 0), Response::WIN);

        // guess 0: apple, candidate 1: maple (secret=maple, guess=apple):
        // a: not green, secret's only 'a' (index 1) unused -> yellow
        // p: green (index 2) -> consumes secret's only 'p', second 'p' has none left -> black
        // l: green, e: green
        // => [2, 0, 1, 1, 1] -> 2 + 0*3 + 1*9 + 1*27 + 1*81 = 119
        assert_eq!(matrix.get(0, 1), Response::new(2, 0, 1, 1, 1));
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;
    use crate::core::Word;

    #[test]
    fn test_transpose() {
        let guesses = vec![Word::new("abcde"), Word::new("fghij")];
        let candidates = vec![Word::new("abcde"), Word::new("xyzab"), Word::new("fghij")];
        let dict = Dictionary {
            guess_chars: guesses
                .iter()
                .map(|w| {
                    [
                        w.0[0] - b'a',
                        w.0[1] - b'a',
                        w.0[2] - b'a',
                        w.0[3] - b'a',
                        w.0[4] - b'a',
                    ]
                })
                .collect(),
            guesses,
            candidates,
            candidate_to_guess: vec![0, 2, 1],
            guess_to_candidate: vec![0, 2],
        };
        let matrix = ResponseMatrix::new(&dict);

        for g in 0..matrix.num_guesses {
            for c in 0..matrix.num_candidates {
                let r1 = matrix.data[g * matrix.num_candidates + c];
                let r2 = matrix.data_c_g[c * matrix.num_guesses + g];
                assert_eq!(r1, r2, "Mismatch at g={}, c={}", g, c);
            }
        }
    }
}
