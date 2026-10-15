#![allow(clippy::needless_range_loop)]
use crate::matrix::ResponseMatrix;

/// Computes a heuristic score for a guess based on the sum of squares of bucket sizes.
///
/// A lower score is better, as it indicates the guess splits the candidates more evenly
/// across the 243 possible response patterns.
pub fn compute_expected_remaining(
    matrix: &ResponseMatrix,
    candidates: &[usize],
    guess: usize,
) -> u32 {
    let mut counts = [0u32; 243];
    for &c in candidates {
        let r = matrix.get(guess, c);
        counts[r.0 as usize] += 1;
    }

    let mut score = 0;
    for &count in &counts {
        score += count * count;
    }
    score
}

/// Sorts a slice of guesses in-place by their `expected_remaining` heuristic score.
pub fn sort_guesses_by_expected_remaining(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    guesses: &mut [usize],
) {
    guesses.sort_by_cached_key(|&g| compute_expected_remaining(matrix, candidates, g));
}

/// Computes the maximum number of distinct response patterns (branches)
/// that ANY guess can generate across the given candidate set.
///
/// This provides the `max_k` value used to calculate capacity bounds.
pub fn compute_max_branching_factor(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
) -> usize {
    use rayon::prelude::*;

    (0..matrix.num_guesses)
        .into_par_iter()
        .map(|g| {
            let g_offset = g * matrix.num_candidates;
            let mut mask = [0u64; 4];
            for &c in candidates {
                let r = unsafe { matrix.data.get_unchecked(g_offset + c).0 as usize };
                mask[r >> 6] |= 1u64 << (r & 63);
            }
            (mask[0].count_ones()
                + mask[1].count_ones()
                + mask[2].count_ones()
                + mask[3].count_ones()) as usize
        })
        .max()
        .unwrap_or(0)
}

/// Computes the maximum branching factor achievable among candidate words only.
/// Early exits if `target_k` is reached.
pub fn compute_candidate_branching_factor_with_target(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    target_k: usize,
) -> usize {
    if candidates.len() <= 2 {
        return candidates.len();
    }
    let effective_target = target_k.min(candidates.len()).min(243);
    let mut max_k = 0;
    for &c in candidates {
        let g = matrix.candidate_to_guess[c];
        let g_offset = g * matrix.num_candidates;
        let g_row = unsafe {
            matrix
                .data
                .get_unchecked(g_offset..g_offset + matrix.num_candidates)
        };
        let mut mask = [0u64; 4];
        for &other in candidates {
            let r = unsafe { g_row.get_unchecked(other).0 as usize };
            unsafe {
                *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
            }
        }
        let k = (mask[0].count_ones()
            + mask[1].count_ones()
            + mask[2].count_ones()
            + mask[3].count_ones()) as usize;
        if k > max_k {
            max_k = k;
            if max_k >= effective_target {
                return max_k;
            }
        }
    }
    max_k
}

/// Computes the maximum branching factor achievable among candidate words only.
pub fn compute_candidate_branching_factor(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
) -> usize {
    compute_candidate_branching_factor_with_target(matrix, candidates, candidates.len().min(243))
}

/// Finds the smallest branching factor `k` in 2..=n.min(243) such that
/// `tight_capacity_bound(n, k, max_k) <= target_bound`.
/// If no such `k` exists (even for k = n.min(243)), returns `n.min(243) + 1`.
pub fn min_k_for_target_bound(n: usize, max_k: usize, target_bound: u32) -> usize {
    let max_possible_k = n.min(max_k).min(243);
    let cap_lb = capacity_bound(n, max_k);
    if target_bound < cap_lb {
        return max_possible_k + 1;
    }
    if tight_capacity_bound(n, 2, max_k) <= target_bound {
        return 2;
    }
    if tight_capacity_bound(n, max_possible_k, max_k) > target_bound {
        return max_possible_k + 1;
    }
    let mut low = 2;
    let mut high = max_possible_k;
    while low + 1 < high {
        let mid = (low + high) / 2;
        if tight_capacity_bound(n, mid, max_k) <= target_bound {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

/// Sequential version of `compute_max_branching_factor` with early stopping and target_k.
pub fn compute_max_branching_factor_seq_with_target(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    initial_max_k: usize,
    target_k: usize,
) -> usize {
    if candidates.len() <= 2 {
        return candidates.len();
    }
    let effective_target = target_k.min(candidates.len()).min(243);
    let mut max_k = initial_max_k;
    if max_k >= effective_target {
        return max_k;
    }

    if max_k == 0 {
        max_k =
            compute_candidate_branching_factor_with_target(matrix, candidates, effective_target);
        if max_k >= effective_target {
            return max_k;
        }
    }

    let mut c_mask = 0u32;
    for &c in candidates {
        c_mask |= matrix.candidate_masks[c];
    }

    let mid = candidates.len() / 2;

    for g in 0..matrix.num_guesses {
        let g_mask = matrix.guess_masks[g];
        let overlap = g_mask & c_mask;
        if overlap == 0 {
            continue;
        }
        let g_ones = g_mask.count_ones();
        let overlap_count = overlap.count_ones();

        let var_pos = if g_ones == 5 {
            overlap_count
        } else {
            let [c0, c1, c2, c3, c4] = unsafe { *matrix.guess_chars.get_unchecked(g) };
            ((c_mask >> c0) & 1)
                + ((c_mask >> c1) & 1)
                + ((c_mask >> c2) & 1)
                + ((c_mask >> c3) & 1)
                + ((c_mask >> c4) & 1)
        };

        if (var_pos <= 1 && max_k >= 3)
            || (var_pos <= 2 && (max_k >= 9 || (overlap_count == 1 && max_k >= 8)))
            || (var_pos <= 3 && (max_k >= 27 || (overlap_count == 1 && max_k >= 12)))
            || (var_pos <= 4 && max_k >= 81)
        {
            continue;
        }

        let g_offset = g * matrix.num_candidates;
        let g_row = unsafe {
            matrix
                .data
                .get_unchecked(g_offset..g_offset + matrix.num_candidates)
        };
        let mut mask = [0u64; 4];
        for &c in &candidates[..mid] {
            let r = unsafe { g_row.get_unchecked(c).0 as usize };
            unsafe {
                *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
            }
        }
        let half_unique = (mask[0].count_ones()
            + mask[1].count_ones()
            + mask[2].count_ones()
            + mask[3].count_ones()) as usize;
        if half_unique + (candidates.len() - mid) <= max_k {
            continue;
        }
        for &c in &candidates[mid..] {
            let r = unsafe { g_row.get_unchecked(c).0 as usize };
            unsafe {
                *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
            }
        }
        let k = (mask[0].count_ones()
            + mask[1].count_ones()
            + mask[2].count_ones()
            + mask[3].count_ones()) as usize;
        if k > max_k {
            max_k = k;
            if max_k >= effective_target {
                break;
            }
        }
    }
    max_k
}

/// Sequential version of `compute_max_branching_factor` with early stopping,
/// suitable for execution inside worker threads without thread contention.
pub fn compute_max_branching_factor_seq(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
) -> usize {
    compute_max_branching_factor_seq_with_target(matrix, candidates, 0, candidates.len().min(243))
}

/// Computes the absolute minimum total cost to solve a subset of size `n`
/// assuming a maximum branching factor `k`.
///
/// This mathematically models the exact capacity of a uniform tree with degree `k-1`
/// (since 1 branch is reserved for the 'WIN' response).
pub fn capacity_bound(n: usize, k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    if n == 2 {
        return 3;
    }

    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 1;
    if k <= 1 {
        panic!("Infinite loop: n={}, k={}", n, k);
    }
    let mut capacity_at_depth = 1u32;

    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(k as u32 - 1);
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capacity_bound() {
        assert_eq!(capacity_bound(1, 3), 1);
        assert_eq!(capacity_bound(2, 3), 3); // 1 win at depth 1, 1 at depth 2 (cost 1 + 2)
        assert_eq!(capacity_bound(3, 3), 5); // 1 at depth 1, 2 at depth 2 (cost 1 + 2 + 2)
        assert_eq!(capacity_bound(4, 3), 8); // 1 at depth 1, 2 at depth 2, 1 at depth 3 (cost 1+4+3)
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn test_capacity_bound_scaling() {
        // Test edge cases
        assert_eq!(capacity_bound(0, 100), 0);
        assert_eq!(capacity_bound(1, 100), 1);
        assert_eq!(capacity_bound(2, 100), 3);

        // Test tight branching (k=3)
        // n=7: 1 at depth 1, 2 at depth 2 (cost 4), 4 at depth 3 (cost 12) -> total 17
        assert_eq!(capacity_bound(7, 3), 1 + 4 + 12);

        // Test wide branching (k=100) for n=20
        // 1 at depth 1, 19 at depth 2 -> 1 + 38 = 39
        assert_eq!(capacity_bound(20, 100), 39);
    }
}

#[cfg(test)]
mod expected_remaining_tests {
    use super::*;
    use crate::dict::Dictionary;

    #[test]
    fn test_compute_expected_remaining() {
        // Create a fake dictionary and matrix for testing expected remaining logic
        let words = vec![
            crate::core::Word::new("abcde"),
            crate::core::Word::new("abcdf"),
            crate::core::Word::new("xyzab"),
        ];
        let dict = Dictionary {
            guesses: words.clone(),
            candidates: words.clone(),
            guess_chars: vec![[0, 1, 2, 3, 4], [0, 1, 2, 3, 5], [23, 24, 25, 0, 1]],
            candidate_to_guess: vec![0, 1, 2],
        };
        let matrix = ResponseMatrix::new(&dict);
        let set = vec![0, 1, 2];

        // Guess 0 (abcde) against candidate 0 (abcde) is WIN
        // Guess 0 against candidate 1 (abcdf) is 4 greens, 1 black
        // Guess 0 against candidate 2 (xyzab) is 2 yellows (ab), 3 blacks
        // So the 3 candidates fall into 3 distinct buckets.
        // sum(1^2 + 1^2 + 1^2) = 3
        let exp0 = compute_expected_remaining(&matrix, &set, 0);
        assert_eq!(exp0, 3);
    }
}

/// Computes a lower bound on the cost when the root guess is a Phase 2 guess
/// (i.e. not in the remaining candidate set).
///
/// A Phase 2 guess cannot possibly hit the correct word at depth 1.
/// Thus, all `n` candidates must be resolved at depth 2 or deeper.
/// `root_k` is the maximum branching factor of the root guess,
/// while `max_k` is the maximum branching factor for all subsequent depths.
pub fn phase2_capacity_bound(n: usize, root_k: usize, max_k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if root_k <= 1 || max_k <= 1 {
        return capacity_bound(n, max_k);
    }
    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 2; // Phase 2 guess means no WIN at depth 1

    let mut capacity_at_depth = root_k as u32;

    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(max_k as u32 - 1);
    }
    cost
}

/// Computes a lower bound for a Phase 1 guess where the root branching factor
/// (`root_k`) is restricted compared to the global max branching factor (`max_k`).
///
/// Since it is a Phase 1 guess, 1 candidate can be a WIN at depth 1.
/// The remaining branches at depth 1 are at most `root_k - 1`.
/// All deeper levels have branching factor `max_k - 1`.
pub fn tight_capacity_bound(n: usize, root_k: usize, max_k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    if n == 2 {
        return 3;
    }

    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 1;
    if root_k <= 1 || max_k <= 1 {
        // If max_k <= 1 or root_k <= 1 and we have n > 2, it's impossible to solve
        // but we return a large finite cost to let alpha-beta prune it.
        return capacity_bound(n, max_k);
    }

    let take = remaining.min(1);
    cost += take * depth;
    remaining -= take;
    depth += 1;

    let mut capacity_at_depth = (root_k as u32).saturating_sub(1);

    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(max_k as u32 - 1);
    }
    cost
}

#[cfg(test)]
mod tight_capacity_bounds_tests {
    use super::*;

    #[test]
    fn test_tight_capacity_bound_is_tighter() {
        // If max_k is large but valid_max_k is small, tight_capacity_bound should give a higher cost.
        let n = 20;
        let global_max_k = 10;
        let valid_max_k = 3;

        let loose_cost = capacity_bound(n, global_max_k);
        let tight_cost = tight_capacity_bound(n, valid_max_k, global_max_k);

        // Because the root is restricted to only 3 branches instead of 10,
        // it must push more nodes to deeper levels, resulting in a higher total cost.
        assert!(
            tight_cost > loose_cost,
            "Tight cost {} should be > loose cost {}",
            tight_cost,
            loose_cost
        );
    }
}

#[cfg(test)]
mod phase2_capacity_bounds_tests {
    use super::*;

    #[test]
    fn test_phase2_capacity_bound_is_correct() {
        // We proved mathematically that if n=5, valid_max_k=2, local_max_k=3,
        // a Phase 2 guess can achieve 12, but tight_capacity_bound gives 13.
        let phase1_lb = tight_capacity_bound(5, 2, 3);
        assert_eq!(phase1_lb, 13, "Phase 1 bound is 13");

        let phase2_lb = phase2_capacity_bound(5, 3, 3);
        assert_eq!(phase2_lb, 12, "Phase 2 bound is 12");

        let tight_lb = phase1_lb.min(phase2_lb);
        assert_eq!(
            tight_lb, 12,
            "Minimum bound correctly prevents Phase 2 pruning"
        );
    }

    #[test]
    fn test_phase2_zero_candidates() {
        assert_eq!(phase2_capacity_bound(0, 3, 3), 0);
    }
}

#[cfg(test)]
mod target_bound_tests {
    use super::*;

    #[test]
    fn test_tight_capacity_bound_monotonicity() {
        for n in 3..50 {
            for max_k in 5..50 {
                let limit = n.min(243);
                for k in 2..limit {
                    let cost_k = tight_capacity_bound(n, k, max_k);
                    let cost_k_plus_1 = tight_capacity_bound(n, k + 1, max_k);
                    assert!(
                        cost_k_plus_1 <= cost_k,
                        "Monotonicity violated for n={}, max_k={}: k={} gives {}, k+1={} gives {}",
                        n,
                        max_k,
                        k,
                        cost_k,
                        k + 1,
                        cost_k_plus_1
                    );
                }
            }
        }
    }

    #[test]
    fn test_min_k_for_target_bound_correctness() {
        for n in 3..40 {
            for max_k in 10..40 {
                let cap_lb = capacity_bound(n, max_k);
                let max_bound = tight_capacity_bound(n, 2, max_k) + 3;
                for target_bound in cap_lb.saturating_sub(2)..max_bound {
                    let min_k = min_k_for_target_bound(n, max_k, target_bound);
                    let limit = n.min(max_k).min(243);
                    if min_k <= limit {
                        assert!(
                            tight_capacity_bound(n, min_k, max_k) <= target_bound,
                            "min_k={} should satisfy target_bound={} for n={}, max_k={}",
                            min_k,
                            target_bound,
                            n,
                            max_k
                        );
                        if min_k > 2 {
                            assert!(
                                tight_capacity_bound(n, min_k - 1, max_k) > target_bound,
                                "min_k - 1 = {} should exceed target_bound={} for n={}, max_k={}",
                                min_k - 1,
                                target_bound,
                                n,
                                max_k
                            );
                        }
                    } else {
                        for k in 2..=limit {
                            assert!(
                                tight_capacity_bound(n, k, max_k) > target_bound,
                                "If min_k > limit, no k should satisfy target_bound: k={} gave {}",
                                k,
                                tight_capacity_bound(n, k, max_k)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_branching_factor_positional_varying_bound() {
        use crate::dict::Dictionary;
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let mut rng = fastrand::Rng::with_seed(12345);
        for size in [5, 15, 30, 50] {
            let mut candidates: Vec<usize> = (0..matrix.num_candidates).collect();
            rng.shuffle(&mut candidates);
            let subset = &candidates[..size];

            let mut c_mask = 0u32;
            for &c in subset {
                c_mask |= matrix.candidate_masks[c];
            }

            for g in (0..matrix.num_guesses).step_by(17) {
                let g_mask = matrix.guess_masks[g];
                let overlap = g_mask & c_mask;
                let overlap_count = overlap.count_ones();
                let g_ones = g_mask.count_ones();
                let var_pos = if g_ones == 5 {
                    overlap_count
                } else {
                    let [c0, c1, c2, c3, c4] = matrix.guess_chars[g];
                    ((c_mask >> c0) & 1)
                        + ((c_mask >> c1) & 1)
                        + ((c_mask >> c2) & 1)
                        + ((c_mask >> c3) & 1)
                        + ((c_mask >> c4) & 1)
                };

                let mut mask = [0u64; 4];
                let g_offset = g * matrix.num_candidates;
                for &c in subset {
                    let r = matrix.data[g_offset + c].0 as usize;
                    mask[r >> 6] |= 1u64 << (r & 63);
                }
                let actual_k = (mask[0].count_ones()
                    + mask[1].count_ones()
                    + mask[2].count_ones()
                    + mask[3].count_ones()) as usize;

                if var_pos <= 1 {
                    assert!(actual_k <= 3, "g={} var_pos=1 actual_k={}", g, actual_k);
                }
                if var_pos <= 2 {
                    if overlap_count == 1 {
                        assert!(
                            actual_k <= 8,
                            "g={} var_pos=2 overlap_count=1 actual_k={}",
                            g,
                            actual_k
                        );
                    } else {
                        assert!(actual_k <= 9, "g={} var_pos=2 actual_k={}", g, actual_k);
                    }
                }
                if var_pos <= 3 {
                    if overlap_count == 1 {
                        assert!(
                            actual_k <= 12,
                            "g={} var_pos=3 overlap_count=1 actual_k={}",
                            g,
                            actual_k
                        );
                    } else {
                        assert!(actual_k <= 27, "g={} var_pos=3 actual_k={}", g, actual_k);
                    }
                }
                if var_pos <= 4 {
                    assert!(actual_k <= 81, "g={} var_pos=4 actual_k={}", g, actual_k);
                }
            }
        }
    }
}
