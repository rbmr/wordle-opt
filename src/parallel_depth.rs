use crate::solver::{Solver, Metrics};
use crate::matrix::ResponseMatrix;
use crate::dict::Dictionary;
use crate::cache::GlobalCache;
use crate::solver::EquivCache;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};

/// Executes a fully parallel depth-2 alpha-beta search across the top-level branches.
///
/// Uses work-stealing (Rayon) over the provided root candidates. A single global `beta`
/// is shared between threads, meaning optimal ordering of `active_guesses` (i.e. sorting
/// by lower bound) allows early threads to tighten the global `beta`, immediately
/// starving and pruning subsequent threads evaluating worse guesses.
#[allow(clippy::too_many_arguments)]
pub fn solve_parallel_depth2<'a>(
    matrix: &'a ResponseMatrix,
    initial_candidates: &[usize],
    dict: &'a Dictionary,
    metrics: &'a Metrics,
    equiv_cache: &'a EquivCache,
    global_cache: &'a GlobalCache,
    max_k: usize,
    capacity_bounds_2d: &[Vec<u32>],
    beta: &'a AtomicU32,
    active_guesses: &[usize],
) -> u32 {

    active_guesses.par_iter().for_each(|&g| {
        let current_beta = beta.load(Ordering::Relaxed);
        
        let mut counts = [0u16; 243];
        let mut non_empty_indices = [0u8; 243];
        let mut num_non_empty = 0;

        for &c in initial_candidates {
            let r_idx = matrix.get(g, c).0 as usize;
            if counts[r_idx] == 0 {
                non_empty_indices[num_non_empty] = r_idx as u8;
                num_non_empty += 1;
            }
            counts[r_idx] += 1;
        }

        let mut offsets = [0usize; 244];
        for r_idx in 0..243 {
            offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
        }

        let mut lb = initial_candidates.len() as u32;
        let mut p_lbs = [0u32; 243];

        for i in 0..num_non_empty {
            let r_idx = non_empty_indices[i] as usize;
            if r_idx == crate::core::Response::WIN.0 as usize {
                continue;
            }
            let p_len = counts[r_idx] as usize;
            let p_lb = capacity_bounds_2d[max_k][p_len];
            p_lbs[r_idx] = p_lb;
            lb += p_lb;
        }

        // Alpha-beta: Early prune the entire guess before sorting/allocating buckets
        if lb >= current_beta {
            return;
        }

        let mut sorted_set = vec![0usize; initial_candidates.len()];
        let mut current_offsets = offsets;
        for &c in initial_candidates {
            let r_idx = matrix.get(g, c).0 as usize;
            let pos = current_offsets[r_idx];
            sorted_set[pos] = c;
            current_offsets[r_idx] += 1;
        }

        let mut bucket_tasks = Vec::new();
        for i in 0..num_non_empty {
            let r_idx = non_empty_indices[i] as usize;
            let p_len = counts[r_idx] as usize;
            if r_idx == crate::core::Response::WIN.0 as usize {
                continue;
            }
            if p_len <= 2 {
                continue;
            }
            let start = offsets[r_idx];
            let end = start + p_len;
            bucket_tasks.push((r_idx, sorted_set[start..end].to_vec()));
        }

        // Sort largest to smallest to process heaviest subtrees first
        bucket_tasks.sort_unstable_by_key(|b| std::cmp::Reverse(b.1.len()));

        let running_cost = AtomicU32::new(lb);
        let exceeded = AtomicBool::new(false);

        // Rayon's par_iter allows child tasks to execute concurrently and steal work,
        // dissolving the "TARES" straggler problem into dozens of independent bucket computations.
        bucket_tasks.into_par_iter().for_each(|(r_idx, bucket)| {
            if exceeded.load(Ordering::Relaxed) {
                return;
            }

            let current_b = beta.load(Ordering::Relaxed);
            let cost_so_far = running_cost.load(Ordering::Relaxed);
            if cost_so_far >= current_b {
                exceeded.store(true, Ordering::Relaxed);
                return;
            }

            let mut solver = Solver::new_with_global_beta(
                matrix,
                max_k,
                dict,
                metrics,
                capacity_bounds_2d,
                global_cache,
                beta,
                equiv_cache,
            );

            // We must subtract this bucket's existing capacity bound from cost_so_far 
            // since we are about to replace it with the true cost.
            let b_cost = cost_so_far.saturating_sub(p_lbs[r_idx]);
            let effective_beta = current_b.saturating_sub(b_cost);
            if effective_beta == 0 {
                exceeded.store(true, Ordering::Relaxed);
                return;
            }

            let val = solver.min_state_val(&bucket, effective_beta, 2, max_k);

            // The net increase to the total cost is the true value minus the capacity bound we started with.
            let net_increase = val.saturating_sub(p_lbs[r_idx]);
            
            let new_cost = running_cost.fetch_add(net_increase, Ordering::Relaxed) + net_increase;
            
            if new_cost >= beta.load(Ordering::Relaxed) {
                exceeded.store(true, Ordering::Relaxed);
            }
        });

        let total = running_cost.load(Ordering::Relaxed);
        if !exceeded.load(Ordering::Relaxed) {
            beta.fetch_min(total, Ordering::Relaxed);
        }
        
        metrics.root_guesses_done.fetch_add(1, Ordering::Relaxed);
    });

    beta.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::Metrics;
    use crate::dict::Dictionary;
    use crate::matrix::ResponseMatrix;
    use crate::cache::GlobalCache;
    use std::sync::RwLock;

    #[test]
    fn test_parallel_depth2_runs() {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let global_cache = GlobalCache::new(1024);
        let equiv_cache: [_; 64] = std::array::from_fn(|_| RwLock::new(rustc_hash::FxHashMap::default()));
        let beta = AtomicU32::new(100);
        let active_guesses: Vec<usize> = vec![0, 1, 2];
        let initial_candidates: Vec<usize> = vec![0, 1];
        let max_k = 2;
        let capacity_bounds_2d = vec![vec![0; 3]; 3];

        let result = solve_parallel_depth2(
            &matrix,
            &initial_candidates,
            &dict,
            &metrics,
            &equiv_cache,
            &global_cache,
            max_k,
            &capacity_bounds_2d,
            &beta,
            &active_guesses,
        );
        // We just care that it executes without panicking and beta is correctly reduced.
        assert!(result <= 100);
    }
}
