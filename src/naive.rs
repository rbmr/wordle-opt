use crate::matrix::ResponseMatrix;
use crate::dict::Dictionary;
use crate::core::Response;

pub struct NaiveSolver<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
}

impl<'a> NaiveSolver<'a> {
    pub fn new(matrix: &'a ResponseMatrix, dict: &'a Dictionary) -> Self {
        Self { matrix, dict }
    }

    pub fn solve(&self, set: &[usize]) -> u32 {
        let allowed_guesses: Vec<usize> = (0..self.dict.guesses.len()).collect();
        self.min_state_val(set, &allowed_guesses, u32::MAX)
    }

    fn min_state_val(&self, set: &[usize], allowed_guesses: &[usize], beta: u32) -> u32 {
        let c_len = set.len();
        if c_len == 0 {
            return 0;
        }
        if c_len == 1 {
            return 1;
        }
        if c_len == 2 {
            return 3;
        }

        let mut best_val = u32::MAX;
        
        for &g in allowed_guesses {
            let mut counts = [0u16; 243];
            let mut num_non_empty = 0;
            for &c in set {
                let r = self.matrix.get(g, c).0 as usize;
                if counts[r] == 0 {
                    num_non_empty += 1;
                }
                counts[r] += 1;
            }

            // A useless guess doesn't partition anything and isn't one of the candidates
            let useless = num_non_empty == 1;
            if useless {
                continue;
            }

            let mut cost = set.len() as u32;
            let mut possible = true;
            
            for r_idx in 0..243 {
                if r_idx == Response::WIN.0 as usize {
                    continue;
                }
                let p_len = counts[r_idx] as usize;
                if p_len == 0 {
                    continue;
                }
                
                let mut subset = Vec::with_capacity(p_len);
                for &c in set {
                    if self.matrix.get(g, c).0 as usize == r_idx {
                        subset.push(c);
                    }
                }
                
                
                cost += self.min_state_val(&subset, allowed_guesses, best_val);
                if cost >= best_val { break; }
            }
            
            if cost < best_val {
                best_val = cost;
            }
        }
        
        best_val
    }
}
