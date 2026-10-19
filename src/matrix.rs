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
    pub word_len: usize,
    pub num_responses: usize,
    pub win_response: Response,
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
    pub candidate_to_guess: Vec<usize>,
    pub guess_chars: Vec<[u8; 8]>,
}

impl ResponseMatrix {
    pub fn new(dict: &Dictionary) -> Self {
        let num_guesses = dict.guesses.len();
        let num_candidates = dict.candidates.len();
        let word_len = dict.word_len;
        let num_responses = 3usize.pow(word_len.min(10) as u32);
        let win_response = Response::win_for_len(word_len);
        let mut data = Vec::with_capacity(num_guesses * num_candidates);
        let mut guess_masks = Vec::with_capacity(num_guesses);
        let mut candidate_masks = Vec::with_capacity(num_candidates);
        let mut zobrist = Vec::with_capacity(num_candidates);
        let mut rng = fastrand::Rng::with_seed(42);
        for _ in 0..num_candidates {
            zobrist.push(rng.u64(..));
        }

        // Letter masks use the dictionary's injective letter index. Past 31
        // symbols there is none, and every mask is all ones instead: each guess
        // then looks like it shares every letter with every candidate, which
        // only disables the letter-based shortcuts (projection deduplication,
        // the varying-position branching bound) and never prunes wrongly.
        let index = crate::dict::letter_index(dict.guesses.iter().chain(&dict.candidates));
        let mask_of = |w: &crate::core::Word| match &index {
            Some(ix) => w
                .as_bytes()
                .iter()
                .fold(0u32, |m, &b| m | 1 << ix[b as usize]),
            None => u32::MAX,
        };

        for guess in &dict.guesses {
            guess_masks.push(mask_of(guess));
        }

        for candidate in &dict.candidates {
            candidate_masks.push(mask_of(candidate));
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

        let candidate_to_guess = dict.candidate_to_guess.clone();
        let guess_chars = dict.guess_chars.clone();

        Self {
            num_guesses,
            num_candidates,
            word_len,
            num_responses,
            win_response,
            guess_masks,
            candidate_masks,
            zobrist,
            data,
            data_c_g,
            candidate_to_guess,
            guess_chars,
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

        let dict = Dictionary::from_words(guesses, candidates);

        let matrix = ResponseMatrix::new(&dict);
        assert_eq!(matrix.num_guesses, 3); // apple, berry, maple
        assert_eq!(matrix.num_candidates, 2);

        // candidate 0 is apple. Find guess for apple
        let apple_g = dict.guesses.binary_search(&Word::new("apple")).unwrap();
        let apple_c = dict.candidates.binary_search(&Word::new("apple")).unwrap();
        assert_eq!(matrix.get(apple_g, apple_c), Response::WIN);

        // guess: apple, candidate: maple (secret=maple, guess=apple):
        // => [2, 0, 1, 1, 1] -> 2 + 0*3 + 1*9 + 1*27 + 1*81 = 119
        let maple_c = dict.candidates.binary_search(&Word::new("maple")).unwrap();
        assert_eq!(matrix.get(apple_g, maple_c), Response::new(2, 0, 1, 1, 1));
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
        let dict = Dictionary::from_words(guesses, candidates);
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
