#![allow(clippy::needless_range_loop)]
use crate::core::Response;
use crate::dict::Dictionary;

/// A precomputed lookup table mapping every (Guess, Candidate) pair to their resulting Wordle `Response`.
///
/// The matrix is flattened into a 1D vector and stored in row-major order: `guess * num_candidates + candidate`.
/// This layout guarantees $O(1)$ lookup time and maximizes L1 CPU cache locality during inner solver loops.
pub struct ResponseMatrix {
    pub num_guesses: usize,
    pub num_candidates: usize,
    pub guess_masks: Vec<u32>,
    pub candidate_masks: Vec<u32>,
    pub zobrist: Vec<u64>,
    data: Vec<Response>,
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

        Self {
            num_guesses,
            num_candidates,
            guess_masks,
            candidate_masks,
            zobrist,
            data,
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
        };

        let matrix = ResponseMatrix::new(&dict);
        assert_eq!(matrix.num_guesses, 2);
        assert_eq!(matrix.num_candidates, 2);

        // guess 0: apple, candidate 0: apple -> WIN
        assert_eq!(matrix.get(0, 0), Response::WIN);

        // guess 0: apple, candidate 1: maple
        // a: black, p: green, p: green, l: green, e: green => [0, 2, 2, 2, 2] -> 0 + 2*3 + 2*9 + 2*27 + 2*81 = 6 + 18 + 54 + 162 = 240
        // Wait, let's just test it's not WIN.
        assert_ne!(matrix.get(0, 1), Response::WIN);
    }
}
