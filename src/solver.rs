use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
static EVAL_COUNT: AtomicUsize = AtomicUsize::new(0);
use crate::matrix::ResponseMatrix;
use crate::core::Response;
use crate::heuristic;

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
pub struct Metrics {
    pub states_evaluated: AtomicUsize,
    pub guesses_evaluated: AtomicUsize,
    pub pruned_by_bounds: AtomicUsize,
    pub pruned_by_equivalence: AtomicUsize,
    pub cache_hits: AtomicUsize,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            states_evaluated: AtomicUsize::new(0),
            guesses_evaluated: AtomicUsize::new(0),
            pruned_by_bounds: AtomicUsize::new(0),
            pruned_by_equivalence: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
        }
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
    pub metrics: &'a Metrics,
    pub max_k: usize,
    pub matrix: &'a ResponseMatrix,
    pub dict: &'a crate::dict::Dictionary,
    pub global_beta: Option<&'a std::sync::atomic::AtomicU32>,
    pub seen_projections: rustc_hash::FxHashSet<u32>,
}

impl<'a> Solver<'a> {

    pub fn greedy_solve(matrix: &ResponseMatrix, max_k: usize, dict: &'a crate::dict::Dictionary, set: &[usize], metrics: &'a Metrics) -> u32 {
        if set.len() <= 2 {
            return (set.len() * (set.len() + 1) / 2) as u32;
        }

        let mut active_tuples = Vec::with_capacity(dict.guesses.len());
        let mut c_mask = 0u32;
        for &c in set {
            c_mask |= matrix.candidate_masks[c];
        }

        let mut seen_projections = rustc_hash::FxHashSet::default();
        for g in 0..dict.guesses.len() {
            let chars = &dict.guess_chars[g];
            let mut proj = 0u32;
            let l0 = chars[0] as u32;
            proj |= (l0 + 1) * ((c_mask >> l0) & 1);
            let l1 = chars[1] as u32;
            proj |= ((l1 + 1) * ((c_mask >> l1) & 1)) << 5;
            let l2 = chars[2] as u32;
            proj |= ((l2 + 1) * ((c_mask >> l2) & 1)) << 10;
            let l3 = chars[3] as u32;
            proj |= ((l3 + 1) * ((c_mask >> l3) & 1)) << 15;
            let l4 = chars[4] as u32;
            proj |= ((l4 + 1) * ((c_mask >> l4) & 1)) << 20;
            if !seen_projections.insert(proj) {
                continue;
            }

            let mut counts = [0u16; 243];
            let mut num_non_empty = 0;
            for &c in set {
                let r = matrix.get(g, c).0 as usize;
                if counts[r] == 0 {
                    num_non_empty += 1;
                }
                counts[r] += 1;
            }

            let useless = num_non_empty == 1 && !set.contains(&g);
            if useless { continue; }

            let mut expected_rem = 0u32;
            for &count in &counts {
                if count > 0 {
                    expected_rem += (count as u32) * (count as u32);
                }
            }
            active_tuples.push((g, expected_rem));
        }

        active_tuples.sort_unstable_by_key(|&(_, exp)| exp);
        if active_tuples.is_empty() {
            return u32::MAX; // Should not happen
        }

        let best_guess = active_tuples[0].0;
        
        let mut counts = [0u16; 243];
        for &c in set {
            counts[matrix.get(best_guess, c).0 as usize] += 1;
        }

        let mut cost = set.len() as u32;
        for r_idx in 0..243 {
            let p_len = counts[r_idx] as usize;
            if p_len == 0 || r_idx == crate::core::Response::WIN.0 as usize {
                continue;
            }
            let mut subset = Vec::with_capacity(p_len);
            for &c in set {
                if matrix.get(best_guess, c).0 as usize == r_idx {
                    subset.push(c);
                }
            }
            cost += Self::greedy_solve(matrix, max_k, dict, &subset, metrics);
        }
        cost
    }
    pub fn new(matrix: &'a ResponseMatrix, max_k: usize, dict: &'a crate::dict::Dictionary, metrics: &'a Metrics, global_beta: Option<&'a AtomicU32>) -> Self {
        Self {
            matrix,
            max_k,
            dict,
            metrics,
            global_beta,
            seen_projections: rustc_hash::FxHashSet::default(),
        }
    }

    /// Solves the given candidate subset to minimize the total expected guesses.
    ///
    /// Evaluates all initial guesses in parallel using Rayon, sharing the global
    /// best upper bound (`beta`) atomically for heavy cross-thread pruning.
    pub fn solve(matrix: &'a ResponseMatrix, initial_candidates: &[usize], dict: &'a crate::dict::Dictionary, metrics: &'a Metrics) -> u32 {
        let max_k = heuristic::compute_max_branching_factor(matrix, initial_candidates);
        
        let mut guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(matrix, initial_candidates, &mut guesses);
        
        let set = initial_candidates;
        
        
        let initial_greedy_cost = Self::greedy_solve(matrix, max_k, dict, initial_candidates, metrics);
        let beta = AtomicU32::new(initial_greedy_cost);
        
        
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

        active_guesses.par_iter().for_each(|&g| {
            let current_beta = beta.load(Ordering::Relaxed);
            let mut solver = Solver::new(matrix, max_k, dict, metrics, Some(&beta));
            let val = solver.min_guess_val(set, g, &active_guesses, current_beta);
            

            // atomic min
            let mut current = beta.load(Ordering::Relaxed);
            while val < current {
                match beta.compare_exchange_weak(current, val, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(new_current) => current = new_current,
                }
            }
            

        });
        
        beta.load(Ordering::Relaxed)
    }

    fn min_state_val(&mut self, set: &[usize], allowed_guesses: &[usize], beta: u32) -> u32 {
        self.metrics.states_evaluated.fetch_add(1, Ordering::Relaxed);
        
        if let Some(gb) = self.global_beta {
            let current_global = gb.load(Ordering::Relaxed);
            if current_global < beta {
                beta = current_global;
            }
        }
        
        let count = EVAL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 10_000_000 == 0 {
            println!("Evaluated {} states... Cache size: 0", count);
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

        

let lb = heuristic::capacity_bound(c_len, self.max_k);
        if lb >= beta {
            return beta;
        }

        let mut best_val = beta;
        
        // In deeper layers we want to only iterate allowed_guesses, but we also want to sort them.
        // For performance, we sort top N guesses, or just filter useless guesses.
        // Let's filter out useless guesses and sort the remaining.
        let mut active_tuples = Vec::with_capacity(allowed_guesses.len());
        let mut c_mask = 0u32;
        for &c in set {
            c_mask |= self.matrix.candidate_masks[c];
        }
        
        self.seen_projections.clear();
        let mut equiv_pruned = 0;

        for &g in allowed_guesses {
            let chars = &self.dict.guess_chars[g];
            let mut proj = 0u32;
            let l0 = chars[0] as u32;
            proj |= (l0 + 1) * ((c_mask >> l0) & 1);
            let l1 = chars[1] as u32;
            proj |= ((l1 + 1) * ((c_mask >> l1) & 1)) << 5;
            let l2 = chars[2] as u32;
            proj |= ((l2 + 1) * ((c_mask >> l2) & 1)) << 10;
            let l3 = chars[3] as u32;
            proj |= ((l3 + 1) * ((c_mask >> l3) & 1)) << 15;
            let l4 = chars[4] as u32;
            proj |= ((l4 + 1) * ((c_mask >> l4) & 1)) << 20;
            
            if !self.seen_projections.insert(proj) {
                equiv_pruned += 1;
                continue;
            }

            let mut counts = [0u16; 243];
            let mut num_non_empty = 0;
            for &c in set {
                let r = self.matrix.get(g, c).0 as usize;
                if counts[r] == 0 {
                    num_non_empty += 1;
                }
                counts[r] += 1;
            }

            let useless = num_non_empty == 1 && !set.contains(&g);
            if useless {
                continue;
            }

            let mut expected_rem = 0u32;
            for &count in &counts {
                if count > 0 {
                    expected_rem += (count as u32) * (count as u32);
                }
            }

            active_tuples.push((g, expected_rem));
        }
        
        self.metrics.pruned_by_equivalence.fetch_add(equiv_pruned, Ordering::Relaxed);
        active_tuples.sort_unstable_by_key(|&(_, exp)| exp);

        
        let active_guesses: Vec<usize> = active_tuples.iter().map(|&(g, _)| g).collect();
        for &g in &active_guesses {
            best_val = self.min_guess_val(set, g, &active_guesses, best_val);
        }




        best_val
    }

    fn min_guess_val(&mut self, set: &[usize], guess: usize, allowed_guesses: &[usize], beta: u32) -> u32 {
        self.metrics.guesses_evaluated.fetch_add(1, Ordering::Relaxed);
        let mut counts = [0u16; 243];
        let mut non_empty_indices = [0u8; 243];
        let mut num_non_empty = 0;
        
        for &c in set {
            let r = self.matrix.get(guess, c).0 as usize;
            if counts[r] == 0 {
                non_empty_indices[num_non_empty] = r as u8;
                num_non_empty += 1;
            }
            counts[r] += 1;
        }

        if num_non_empty == 1 {
            return beta;
        }

        // Evaluate largest buckets first to trigger Alpha-Beta cutoffs earlier
        non_empty_indices[0..num_non_empty].sort_unstable_by_key(|&r| std::cmp::Reverse(counts[r as usize]));

        let mut cost = set.len() as u32;
        let mut p_lbs = [0u32; 243];
        
        for i in 0..num_non_empty {
            let r_idx = non_empty_indices[i] as usize;
            if r_idx == Response::WIN.0 as usize {
                continue;
            }
            let p_len = counts[r_idx] as u32;
            let lb = heuristic::capacity_bound(p_len as usize, self.max_k);
            cost += lb;
            p_lbs[r_idx] = lb;
        }

        if cost >= beta {
            self.metrics.pruned_by_bounds.fetch_add(1, Ordering::Relaxed);
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

        for i in 0..num_non_empty {
            let r_idx = non_empty_indices[i] as usize;
            let p_len = counts[r_idx] as usize;
            if r_idx == Response::WIN.0 as usize {
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
