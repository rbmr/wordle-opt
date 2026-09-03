use crate::matrix::ResponseMatrix;
use crate::core::Response;
use crate::heuristic;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateSet(pub Vec<usize>);

pub struct Solver<'a> {
    pub matrix: &'a ResponseMatrix,
    pub cache: HashMap<CandidateSet, u32>,
}

impl<'a> Solver<'a> {
    pub fn new(matrix: &'a ResponseMatrix) -> Self {
        Self {
            matrix,
            cache: HashMap::new(),
        }
    }

    pub fn solve(&mut self, initial_candidates: &[usize]) -> u32 {
        let mut guesses: Vec<usize> = (0..self.matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(self.matrix, initial_candidates, &mut guesses);
        
        let set = CandidateSet(initial_candidates.to_vec());
        
        // Initial upper bound (heuristic cost)
        let beta = u32::MAX; // We can improve this by simulating the heuristic first
        
        self.min_state_val(&set, &guesses, beta)
    }

    fn min_state_val(&mut self, set: &CandidateSet, allowed_guesses: &[usize], beta: u32) -> u32 {
        let c_len = set.0.len();
        if c_len == 0 {
            return 0;
        }
        if c_len == 1 {
            return 1;
        }
        if c_len == 2 {
            return 3;
        }

        if let Some(&val) = self.cache.get(set) {
            if val >= beta {
                return beta;
            }
            return val;
        }

        let lb2 = (2 * c_len as u32).saturating_sub(1);
        if lb2 >= beta {
            return beta;
        }

        let mut best_val = beta;
        
        // In deeper layers we want to only iterate allowed_guesses, but we also want to sort them.
        // For performance, we sort top N guesses, or just filter useless guesses.
        // Let's filter out useless guesses and sort the remaining.
        let mut active_guesses = Vec::with_capacity(allowed_guesses.len());
        for &g in allowed_guesses {
            // A guess is useless if it doesn't partition `set`
            let mut first_r = None;
            let mut useless = true;
            for &c in &set.0 {
                let r = self.matrix.get(g, c);
                if first_r.is_none() {
                    first_r = Some(r);
                } else if first_r != Some(r) {
                    useless = false;
                    break;
                }
            }
            if !useless || set.0.contains(&g) {
                active_guesses.push(g);
            }
        }
        
        // Sort active guesses
        heuristic::sort_guesses_by_expected_remaining(self.matrix, &set.0, &mut active_guesses);

        for &g in &active_guesses {
            best_val = self.min_guess_val(set, g, &active_guesses, best_val);
        }

        if best_val < beta {
            self.cache.insert(set.clone(), best_val);
        }

        best_val
    }

    fn min_guess_val(&mut self, set: &CandidateSet, guess: usize, allowed_guesses: &[usize], beta: u32) -> u32 {
        let mut partitions = vec![Vec::new(); 243];
        for &c in &set.0 {
            let r = self.matrix.get(guess, c);
            partitions[r.0 as usize].push(c);
        }

        let mut cost = set.0.len() as u32;
        let mut p_lbs = Vec::with_capacity(243);
        
        for (r_idx, p) in partitions.iter().enumerate() {
            if p.is_empty() {
                p_lbs.push(0);
                continue;
            }
            if r_idx == Response::WIN.0 as usize {
                p_lbs.push(0);
                continue;
            }
            let p_len = p.len() as u32;
            let lb = if p_len == 1 {
                1
            } else if p_len == 2 {
                3
            } else {
                (2 * p_len).saturating_sub(1)
            };
            cost += lb;
            p_lbs.push(lb);
        }

        if cost >= beta {
            return beta;
        }

        for (r_idx, p) in partitions.iter().enumerate() {
            if p.is_empty() || r_idx == Response::WIN.0 as usize {
                continue;
            }
            if p.len() <= 2 {
                continue;
            }

            let b = cost - p_lbs[r_idx];
            let new_beta = beta - b;
            
            let val = self.min_state_val(&CandidateSet(p.clone()), allowed_guesses, new_beta);
            if b + val >= beta {
                return beta;
            }
            cost = b + val;
        }

        cost
    }
}
