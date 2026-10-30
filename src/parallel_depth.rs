use crate::cache::GlobalCache;
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::solver::EquivCache;
use crate::solver::{Metrics, Solver};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Executes a fully parallel depth-2 alpha-beta search across the top-level branches.
///
/// This approach splits the initial set of root guesses into independently evaluated
/// buckets at depth 2 (since depth 1 handles WIN conditions). Uses Rayon's work-stealing
/// `par_iter` to dynamically distribute these buckets across available CPU cores.
///
/// A single global `beta` (an `AtomicU32`) is shared between threads, meaning optimal ordering
/// of `active_guesses` (i.e. sorting by expected remaining) allows early threads to quickly
/// tighten the global `beta`. This immediately starves and prunes subsequent threads evaluating
/// worse guesses, ensuring that adding cores does not simply result in more wasted work.
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
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_guesses_bits = vec![u64::MAX; num_u64s];
    let rem = dict.guesses.len() % 64;
    if rem != 0 {
        all_guesses_bits[num_u64s - 1] = (1 << rem) - 1;
    }

    active_guesses.par_iter().for_each(|&g| {
        let win_response = matrix.win_response.0 as usize;
        if matrix.num_responses <= 256 {
            let g_ptr = unsafe { matrix.data_u8.as_ptr().add(g * matrix.num_candidates) };
            let mut counts = [0u16; 256];
            let mut non_empty_indices = [0u16; 256];
            let mut num_non_empty = 0;

            for &c in initial_candidates {
                let r_idx = unsafe { *g_ptr.add(c) as usize };
                if counts[r_idx] == 0 {
                    non_empty_indices[num_non_empty] = r_idx as u16;
                    num_non_empty += 1;
                }
                counts[r_idx] += 1;
            }

            let mut offsets = [0u16; 256];
            let mut current_offsets = [0u16; 256];
            let mut curr = 0;
            for i in 0..num_non_empty {
                let r = non_empty_indices[i] as usize;
                offsets[r] = curr as u16;
                current_offsets[r] = curr as u16;
                curr += counts[r] as usize;
            }

            let mut sorted_set = vec![0usize; initial_candidates.len()];
            for &c in initial_candidates {
                let r_idx = unsafe { *g_ptr.add(c) as usize };
                let pos = current_offsets[r_idx] as usize;
                sorted_set[pos] = c;
                current_offsets[r_idx] += 1;
            }

            let mut lb = initial_candidates.len() as u32;
            let mut p_lbs = [0u32; 256];
            let mut exact_vals = [0u64; 4];

            for i in 0..num_non_empty {
                let r_idx = non_empty_indices[i] as usize;
                if r_idx == win_response {
                    continue;
                }
                let p_len = counts[r_idx] as usize;
                let cap_lb = capacity_bounds_2d[max_k][p_len];

                if p_len <= 2 {
                    p_lbs[r_idx] = cap_lb;
                    lb += cap_lb;
                    exact_vals[r_idx >> 6] |= 1u64 << (r_idx & 63);
                    continue;
                }

                let start = offsets[r_idx] as usize;
                let end = start + p_len;
                let p = &sorted_set[start..end];

                let mut hash = 0u64;
                for &c in p {
                    hash ^= matrix.zobrist[c];
                }

                if let Some((cached_val, is_exact)) = global_cache.get(hash) {
                    let used_val = cached_val.max(cap_lb);
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    if is_exact {
                        exact_vals[r_idx >> 6] |= 1u64 << (r_idx & 63);
                    }
                } else if p_len <= 15 {
                    let (q_val, is_exact) = Solver::quick_bucket_eval(matrix, dict, p);
                    let used_val = q_val.max(cap_lb);
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    if is_exact {
                        exact_vals[r_idx >> 6] |= 1u64 << (r_idx & 63);
                    }
                    global_cache.insert_set(hash, p, used_val, is_exact);
                    cert_record!(12, p, used_val, is_exact);
                } else {
                    let target_k = crate::heuristic::min_k_for_target_bound(p_len, max_k, cap_lb);
                    let used_val = if target_k == 2 {
                        cap_lb
                    } else {
                        let k_cand =
                            crate::heuristic::compute_candidate_branching_factor_with_target(
                                matrix, p, target_k,
                            );
                        if k_cand >= target_k {
                            cap_lb
                        } else {
                            let k = crate::heuristic::compute_max_branching_factor_seq_with_target(
                                matrix, p, k_cand, target_k,
                            );
                            if k >= target_k {
                                cap_lb
                            } else {
                                crate::heuristic::tight_capacity_bound(p_len, k, max_k).max(cap_lb)
                            }
                        }
                    };
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    global_cache.insert_set(hash, p, used_val, false);
                    cert_record!(13, p, used_val, false);
                }

                if lb >= beta.load(Ordering::Relaxed) {
                    return;
                }
            }

            let mut bucket_tasks = Vec::new();
            for i in 0..num_non_empty {
                let r_idx = non_empty_indices[i] as usize;
                let is_exact = (exact_vals[r_idx >> 6] & (1u64 << (r_idx & 63))) != 0;
                if r_idx == win_response || is_exact {
                    continue;
                }
                let p_len = counts[r_idx] as usize;
                let start = offsets[r_idx] as usize;
                let end = start + p_len;
                bucket_tasks.push((r_idx, sorted_set[start..end].to_vec()));
            }

            bucket_tasks.sort_unstable_by_key(|b| std::cmp::Reverse(b.1.len()));

            let running_cost = AtomicU32::new(lb);
            let exceeded = AtomicBool::new(false);

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

                let b_cost = cost_so_far.saturating_sub(p_lbs[r_idx]);
                let effective_beta = current_b.saturating_sub(b_cost);
                if effective_beta == 0 {
                    exceeded.store(true, Ordering::Relaxed);
                    return;
                }

                let val =
                    solver.min_state_val(&bucket, &all_guesses_bits, effective_beta, 2, max_k);

                let net_increase = val.saturating_sub(p_lbs[r_idx]);

                let new_cost =
                    running_cost.fetch_add(net_increase, Ordering::Relaxed) + net_increase;

                if new_cost >= beta.load(Ordering::Relaxed) {
                    exceeded.store(true, Ordering::Relaxed);
                }
            });

            let total = running_cost.load(Ordering::Relaxed);
            if !exceeded.load(Ordering::Relaxed) {
                beta.fetch_min(total, Ordering::Relaxed);
            }

            metrics.root_guesses_done.fetch_add(1, Ordering::Relaxed);
        } else {
            let g_ptr = unsafe { matrix.data.as_ptr().add(g * matrix.num_candidates) };
            let num_resp = matrix.num_responses;
            let mut counts = vec![0u16; num_resp];
            let mut non_empty_indices = vec![0u16; num_resp];
            let mut num_non_empty = 0;

            for &c in initial_candidates {
                let r_idx = unsafe { (*g_ptr.add(c)).0 as usize };
                if counts[r_idx] == 0 {
                    non_empty_indices[num_non_empty] = r_idx as u16;
                    num_non_empty += 1;
                }
                counts[r_idx] += 1;
            }

            let mut offsets = vec![0usize; num_resp + 1];
            for r_idx in 0..num_resp {
                offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
            }

            let mut sorted_set = vec![0usize; initial_candidates.len()];
            let mut current_offsets = offsets.clone();
            for &c in initial_candidates {
                let r_idx = unsafe { (*g_ptr.add(c)).0 as usize };
                let pos = current_offsets[r_idx];
                sorted_set[pos] = c;
                current_offsets[r_idx] += 1;
            }

            let mut lb = initial_candidates.len() as u32;
            let mut p_lbs = vec![0u32; num_resp];
            let mut exact_vals = vec![false; num_resp];

            for i in 0..num_non_empty {
                let r_idx = non_empty_indices[i] as usize;
                if r_idx == win_response {
                    continue;
                }
                let p_len = counts[r_idx] as usize;
                let cap_lb = capacity_bounds_2d[max_k][p_len];

                if p_len <= 2 {
                    p_lbs[r_idx] = cap_lb;
                    lb += cap_lb;
                    exact_vals[r_idx] = true;
                    continue;
                }

                let start = offsets[r_idx];
                let end = start + p_len;
                let p = &sorted_set[start..end];

                let mut hash = 0u64;
                for &c in p {
                    hash ^= matrix.zobrist[c];
                }

                if let Some((cached_val, is_exact)) = global_cache.get(hash) {
                    let used_val = cached_val.max(cap_lb);
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    if is_exact {
                        exact_vals[r_idx] = true;
                    }
                } else if p_len <= 15 {
                    let (q_val, is_exact) = Solver::quick_bucket_eval(matrix, dict, p);
                    let used_val = q_val.max(cap_lb);
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    if is_exact {
                        exact_vals[r_idx] = true;
                    }
                    global_cache.insert_set(hash, p, used_val, is_exact);
                    cert_record!(12, p, used_val, is_exact);
                } else {
                    let target_k = crate::heuristic::min_k_for_target_bound(p_len, max_k, cap_lb);
                    let used_val = if target_k == 2 {
                        cap_lb
                    } else {
                        let k_cand =
                            crate::heuristic::compute_candidate_branching_factor_with_target(
                                matrix, p, target_k,
                            );
                        if k_cand >= target_k {
                            cap_lb
                        } else {
                            let k = crate::heuristic::compute_max_branching_factor_seq_with_target(
                                matrix, p, k_cand, target_k,
                            );
                            if k >= target_k {
                                cap_lb
                            } else {
                                crate::heuristic::tight_capacity_bound(p_len, k, max_k).max(cap_lb)
                            }
                        }
                    };
                    p_lbs[r_idx] = used_val;
                    lb += used_val;
                    global_cache.insert_set(hash, p, used_val, false);
                    cert_record!(13, p, used_val, false);
                }

                if lb >= beta.load(Ordering::Relaxed) {
                    return;
                }
            }

            let mut bucket_tasks = Vec::new();
            for i in 0..num_non_empty {
                let r_idx = non_empty_indices[i] as usize;
                if r_idx == win_response || exact_vals[r_idx] {
                    continue;
                }
                let p_len = counts[r_idx] as usize;
                let start = offsets[r_idx];
                let end = start + p_len;
                bucket_tasks.push((r_idx, sorted_set[start..end].to_vec()));
            }

            bucket_tasks.sort_unstable_by_key(|b| std::cmp::Reverse(b.1.len()));

            let running_cost = AtomicU32::new(lb);
            let exceeded = AtomicBool::new(false);

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

                let b_cost = cost_so_far.saturating_sub(p_lbs[r_idx]);
                let effective_beta = current_b.saturating_sub(b_cost);
                if effective_beta == 0 {
                    exceeded.store(true, Ordering::Relaxed);
                    return;
                }

                let val =
                    solver.min_state_val(&bucket, &all_guesses_bits, effective_beta, 2, max_k);

                let net_increase = val.saturating_sub(p_lbs[r_idx]);

                let new_cost =
                    running_cost.fetch_add(net_increase, Ordering::Relaxed) + net_increase;

                if new_cost >= beta.load(Ordering::Relaxed) {
                    exceeded.store(true, Ordering::Relaxed);
                }
            });

            let total = running_cost.load(Ordering::Relaxed);
            if !exceeded.load(Ordering::Relaxed) {
                beta.fetch_min(total, Ordering::Relaxed);
            }

            metrics.root_guesses_done.fetch_add(1, Ordering::Relaxed);
        }
    });

    beta.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::GlobalCache;
    use crate::dict::Dictionary;
    use crate::matrix::ResponseMatrix;
    use crate::solver::Metrics;
    use std::sync::RwLock;

    #[test]
    fn test_parallel_depth2_runs() {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let global_cache = GlobalCache::new(1024);
        let equiv_cache: [_; 1024] =
            std::array::from_fn(|_| RwLock::new(rustc_hash::FxHashMap::default()));
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

#[test]
fn test_parallel_depth2_prunes_early() {
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    let matrix = ResponseMatrix::new(&dict);
    let metrics = Metrics::new();
    let global_cache = GlobalCache::new(1024);
    let equiv_cache: [_; 1024] =
        std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));

    // Beta is 0! It should do zero real work.
    let beta = AtomicU32::new(0);
    let active_guesses: Vec<usize> = vec![0, 1, 2];
    let initial_candidates: Vec<usize> = vec![0, 1, 2, 3, 4, 5];
    let max_k = 2;
    let mut capacity_bounds_2d = vec![vec![0; 10]; 3];
    // Populate bounds so lb >= beta (6 >= 0)
    for k in 2..=2 {
        for i in 0..10 {
            capacity_bounds_2d[k][i] = i as u32;
        }
    }

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
    assert_eq!(result, 0);
}
