use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
static EVAL_COUNT: AtomicUsize = AtomicUsize::new(0);
use crate::matrix::ResponseMatrix;
use crate::core::Response;
use crate::heuristic;
use rustc_hash::FxHashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateSet(pub Vec<usize>);

pub struct Solver<'a> {
    pub max_k: usize,
    pub matrix: &'a ResponseMatrix,
    pub dict: &'a crate::dict::Dictionary,
    pub cache: FxHashMap<CandidateSet, u32>,
}

impl<'a> Solver<'a> {
    pub fn new(matrix: &'a ResponseMatrix, max_k: usize, dict: &'a crate::dict::Dictionary) -> Self {
        Self {
            matrix,
            max_k,
            dict,
            cache: FxHashMap::default(),
        }
    }

    pub fn solve(matrix: &'a ResponseMatrix, initial_candidates: &[usize], dict: &'a crate::dict::Dictionary) -> u32 {
        let max_k = heuristic::compute_max_branching_factor(matrix, initial_candidates);
        
        let mut guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(matrix, initial_candidates, &mut guesses);
        
        let set = CandidateSet(initial_candidates.to_vec());
        
        
        let beta = AtomicU32::new(u32::MAX);
        let progress = AtomicUsize::new(0);
        
        
        // Filter active guesses
        let mut active_guesses = Vec::with_capacity(guesses.len());
        for &g in &guesses {
            let mut first_r = None;
            let mut useless = true;
            for &c in &set.0 {
                let r = matrix.get(g, c);
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
        
        heuristic::sort_guesses_by_expected_remaining(matrix, &set.0, &mut active_guesses);
        let total_active = active_guesses.len();

        active_guesses.par_iter().for_each(|&g| {
            let current_beta = beta.load(Ordering::Relaxed);
            let mut solver = Solver::new(matrix, max_k, dict);
            let val = solver.min_guess_val(&set, g, &active_guesses, current_beta);
            

            // atomic min
            let mut current = beta.load(Ordering::Relaxed);
            while val < current {
                match beta.compare_exchange_weak(current, val, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(new_current) => current = new_current,
                }
            }
            let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
            println!("Progress: {}/{} root guesses evaluated. Current best bound: {}", done, total_active, beta.load(Ordering::Relaxed));

        });
        
        beta.load(Ordering::Relaxed)
    }

    fn min_state_val(&mut self, set: &CandidateSet, allowed_guesses: &[usize], beta: u32) -> u32 {
        
        let count = EVAL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 10_000_000 == 0 {
            println!("Evaluated {} states... Cache size: {}", count, self.cache.len());
        }

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

let lb = heuristic::capacity_bound(c_len, self.max_k);
        if lb >= beta {
            return beta;
        }

        let mut best_val = beta;
        
        // In deeper layers we want to only iterate allowed_guesses, but we also want to sort them.
        // For performance, we sort top N guesses, or just filter useless guesses.
        // Let's filter out useless guesses and sort the remaining.
        let mut active_guesses = Vec::with_capacity(allowed_guesses.len());
        
        let mut c_mask = 0u32;
        for &c in &set.0 {
            c_mask |= self.matrix.candidate_masks[c];
        }
        
        let mut seen_projections = rustc_hash::FxHashSet::default();

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
                let mut proj = 0u32;
                for i in 0..5 {
                    let letter = self.dict.guesses[g].0[i] - b'a';
                    if (c_mask & (1 << letter)) != 0 {
                        proj |= ((letter as u32) + 1) << (i * 5);
                    }
                }
                if seen_projections.insert(proj) {
                    active_guesses.push(g);
                }
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
        let mut counts = [0u16; 243];
        let mut num_non_empty = 0;
        
        for &c in &set.0 {
            let r = self.matrix.get(guess, c).0 as usize;
            if counts[r] == 0 {
                num_non_empty += 1;
            }
            counts[r] += 1;
        }

        if num_non_empty == 1 {
            return beta;
        }

        let mut cost = set.0.len() as u32;
        let mut p_lbs = [0u32; 243];
        
        for r_idx in 0..243 {
            let p_len = counts[r_idx] as u32;
            if p_len == 0 || r_idx == Response::WIN.0 as usize {
                continue;
            }
            let lb = heuristic::capacity_bound(p_len as usize, self.max_k);
            cost += lb;
            p_lbs[r_idx] = lb;
        }

        if cost >= beta {
            return beta;
        }

        // Now we actually need the partitions, so we construct them.
        let mut partitions: [Vec<usize>; 243] = std::array::from_fn(|_| Vec::new());
        // Preallocate capacity
        for r_idx in 0..243 {
            if counts[r_idx] > 0 {
                partitions[r_idx].reserve_exact(counts[r_idx] as usize);
            }
        }
        for &c in &set.0 {
            let r = self.matrix.get(guess, c).0 as usize;
            partitions[r].push(c);
        }

        for r_idx in 0..243 {
            let p = &partitions[r_idx];
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
