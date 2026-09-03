use crate::core::Response;
use crate::dict::Dictionary;

pub struct ResponseMatrix {
    pub num_guesses: usize,
    pub num_candidates: usize,
    data: Vec<Response>,
}

impl ResponseMatrix {
    pub fn new(dict: &Dictionary) -> Self {
        let num_guesses = dict.guesses.len();
        let num_candidates = dict.candidates.len();
        let mut data = Vec::with_capacity(num_guesses * num_candidates);

        for guess in &dict.guesses {
            for candidate in &dict.candidates {
                data.push(Response::compute(candidate, guess));
            }
        }

        Self {
            num_guesses,
            num_candidates,
            data,
        }
    }

    #[inline]
    pub fn get(&self, guess_idx: usize, candidate_idx: usize) -> Response {
        self.data[guess_idx * self.num_candidates + candidate_idx]
    }
}
