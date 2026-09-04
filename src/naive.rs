use crate::core::Response;
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;

pub struct NaiveSolver<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
}

impl<'a> NaiveSolver<'a> {
    pub fn new(matrix: &'a ResponseMatrix, dict: &'a Dictionary) -> Self {
        Self { matrix, dict }
    }

    pub fn solve(&self, set: &[usize]) -> u32 {
        let mut allowed_guesses: Vec<usize> = (0..self.dict.guesses.len()).collect();
        // A single static sort at the root to ensure alpha-beta isn't worst-case.
        crate::heuristic::sort_guesses_by_expected_remaining(
            self.matrix,
            set,
            &mut allowed_guesses,
        );
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

        let mut best_val = beta;

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

            let useless = num_non_empty == 1;
            if useless {
                continue;
            }

            let mut cost = set.len() as u32;
            if cost >= best_val {
                continue;
            }

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

                let b = best_val - cost;
                let val = self.min_state_val(&subset, allowed_guesses, b);
                cost += val;
                if cost >= best_val {
                    break;
                }
            }

            if cost < best_val {
                best_val = cost;
            }
        }

        best_val
    }
}
