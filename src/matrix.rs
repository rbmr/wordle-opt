use crate::core::Response;
use crate::dict::Dictionary;

pub struct ResponseMatrix {
    pub num_guesses: usize,
    pub num_candidates: usize,
    pub guess_masks: Vec<u32>,
    pub candidate_masks: Vec<u32>,
    data: Vec<Response>,
}

impl ResponseMatrix {
    pub fn new(dict: &Dictionary) -> Self {
        let num_guesses = dict.guesses.len();
        let num_candidates = dict.candidates.len();
        let mut data = Vec::with_capacity(num_guesses * num_candidates);
        let mut guess_masks = Vec::with_capacity(num_guesses);
        let mut candidate_masks = Vec::with_capacity(num_candidates);

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

        for guess in &dict.guesses {
            for candidate in &dict.candidates {
                data.push(Response::compute(candidate, guess));
            }
        }

        Self {
            num_guesses,
            num_candidates,
            guess_masks,
            candidate_masks,
            data,
        }
    }

    #[inline]
    pub fn get(&self, guess_idx: usize, candidate_idx: usize) -> Response {
        self.data[guess_idx * self.num_candidates + candidate_idx]
    }
}
