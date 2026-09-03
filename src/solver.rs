use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
static EVAL_COUNT: AtomicUsize = AtomicUsize::new(0);
use crate::matrix::ResponseMatrix;
use crate::core::Response;
use crate::heuristic;
use rustc_hash::FxHashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateSet(pub Vec<usize>);

impl std::borrow::Borrow<[usize]> for CandidateSet {
    fn borrow(&self) -> &[usize] {
        &self.0
    }
}

/// The optimal Wordle solver using Branch and Bound.
///
/// Implements aggressive search space pruning through:
/// - Exact Capacity Lower Bounds
/// - Expected Remaining Candidate Heuristics
/// - Equivalence Class Guess Projections
/// - `FxHashMap` based subtree memoization
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

    /// Solves the given candidate subset to minimize the total expected guesses.
    ///
    /// Evaluates all initial guesses in parallel using Rayon, sharing the global
    /// best upper bound (`beta`) atomically for heavy cross-thread pruning.
    pub fn solve(matrix: &'a ResponseMatrix, initial_candidates: &[usize], dict: &'a crate::dict::Dictionary) -> u32 {
        let max_k = heuristic::compute_max_branching_factor(matrix, initial_candidates);
        
        let mut guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(matrix, initial_candidates, &mut guesses);
        
        let set = initial_candidates;
        
        
        let beta = AtomicU32::new(u32::MAX);
        let progress = AtomicUsize::new(0);
        
        
        // Filter active guesses
        let mut active_guesses = Vec::with_capacity(guesses.len());
        for &g in &guesses {
            let mut first_r = None;
            let mut useless = true;
            for &c in set {
                let r = matrix.get(g, c);
                if first_r.is_none() {
                    first_r = Some(r);
                } else if first_r != Some(r) {
                    useless = false;
                    break;
                }
            }
            if !useless || set.contains(&g) {
                active_guesses.push(g);
            }
        }
        
        heuristic::sort_guesses_by_expected_remaining(matrix, set, &mut active_guesses);
        let total_active = active_guesses.len();

        active_guesses.par_iter().for_each(|&g| {
            let current_beta = beta.load(Ordering::Relaxed);
            let mut solver = Solver::new(matrix, max_k, dict);
            let val = solver.min_guess_val(set, g, &active_guesses, current_beta);
            

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

    fn min_state_val(&mut self, set: &[usize], allowed_guesses: &[usize], beta: u32) -> u32 {
        
        let count = EVAL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 10_000_000 == 0 {
            println!("Evaluated {} states... Cache size: {}", count, self.cache.len());
        }

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
        for &c in set {
            c_mask |= self.matrix.candidate_masks[c];
        }
        
        let mut seen_projections = rustc_hash::FxHashSet::default();

        for &g in allowed_guesses {
            // A guess is useless if it doesn't partition `set`
            let mut first_r = None;
            let mut useless = true;
            for &c in set {
                let r = self.matrix.get(g, c);
                if first_r.is_none() {
                    first_r = Some(r);
                } else if first_r != Some(r) {
                    useless = false;
                    break;
                }
            }
            if !useless || set.contains(&g) {
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
        heuristic::sort_guesses_by_expected_remaining(self.matrix, set, &mut active_guesses);

        for &g in &active_guesses {
            best_val = self.min_guess_val(set, g, &active_guesses, best_val);
        }

        if best_val < beta {
            self.cache.insert(CandidateSet(set.to_vec()), best_val);
        }

        best_val
    }

    fn min_guess_val(&mut self, set: &[usize], guess: usize, allowed_guesses: &[usize], beta: u32) -> u32 {
        let mut counts = [0u16; 243];
        let mut num_non_empty = 0;
        
        for &c in set {
            let r = self.matrix.get(guess, c).0 as usize;
            if counts[r] == 0 {
                num_non_empty += 1;
            }
            counts[r] += 1;
        }

        if num_non_empty == 1 {
            return beta;
        }

        let mut cost = set.len() as u32;
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

        // Fast slice partition using counting sort
        let mut sorted_set = vec![0; set.len()];
        let mut offsets = [0usize; 244];
        for r_idx in 0..243 {
            offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
        }
        
        let mut current_offsets = offsets;
        for &c in set {
            let r_idx = self.matrix.get(guess, c).0 as usize;
            let pos = current_offsets[r_idx];
            sorted_set[pos] = c;
            current_offsets[r_idx] += 1;
        }

        for r_idx in 0..243 {
            let p_len = counts[r_idx] as usize;
            if p_len == 0 || r_idx == Response::WIN.0 as usize {
                continue;
            }
            if p_len <= 2 {
                continue;
            }

            let start = offsets[r_idx];
            let end = start + p_len;
            let p = &sorted_set[start..end];

            let b = cost - p_lbs[r_idx];
            let new_beta = beta - b;
            
            let val = self.min_state_val(p, allowed_guesses, new_beta);
            if b + val >= beta {
                return beta;
            }
            cost = b + val;
        }

        cost
    }
}
