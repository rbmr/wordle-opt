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
    if matrix.num_responses <= 256 {
        let mut counts = [0u16; 256];
        for &c in candidates {
            let r = matrix.get(guess, c);
            counts[r.0 as usize] += 1;
        }

        let mut score = 0;
        for &c in candidates {
            let r = matrix.get(guess, c);
            let count = counts[r.0 as usize];
            if count > 0 {
                score += (count as u32) * (count as u32);
                counts[r.0 as usize] = 0;
            }
        }
        score
    } else {
        let mut counts = [0u16; 2187];
        for &c in candidates {
            let r = matrix.get(guess, c);
            counts[r.0 as usize] += 1;
        }

        let mut score = 0;
        for &c in candidates {
            let r = matrix.get(guess, c);
            let count = counts[r.0 as usize];
            if count > 0 {
                score += (count as u32) * (count as u32);
                counts[r.0 as usize] = 0;
            }
        }
        score
    }
}

/// Sorts a slice of guesses in-place by their `expected_remaining` heuristic score.
pub fn sort_guesses_by_expected_remaining(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    guesses: &mut [usize],
) {
    guesses.sort_by_cached_key(|&g| compute_expected_remaining(matrix, candidates, g));
}

#[inline(always)]
fn combine_and_count_u8(mask: &[u64; 8]) -> usize {
    let m0 = mask[0] | mask[4];
    let m1 = mask[1] | mask[5];
    let m2 = mask[2] | mask[6];
    let m3 = mask[3] | mask[7];
    (m0.count_ones() + m1.count_ones() + m2.count_ones() + m3.count_ones()) as usize
}

#[inline(always)]
fn count_unique_responses_from_row_u8(g_ptr: *const u8, candidates: &[usize], mask: &mut [u64; 8]) {
    let n = candidates.len();
    let mut c_it = candidates.as_ptr();
    let c_end = unsafe { c_it.add(n & !1) };
    let mask_ptr = mask.as_mut_ptr();
    while c_it < c_end {
        let c0 = unsafe { *c_it };
        let c1 = unsafe { *c_it.add(1) };
        let r0 = unsafe { *g_ptr.add(c0) as usize };
        let r1 = unsafe { *g_ptr.add(c1) as usize };
        unsafe {
            *mask_ptr.add(r0 >> 6) |= 1u64 << (r0 & 63);
            *mask_ptr.add(4 + (r1 >> 6)) |= 1u64 << (r1 & 63);
            c_it = c_it.add(2);
        }
    }
    if (n & 1) != 0 {
        let c0 = unsafe { *c_end };
        let r0 = unsafe { *g_ptr.add(c0) as usize };
        unsafe {
            *mask_ptr.add(r0 >> 6) |= 1u64 << (r0 & 63);
        }
    }
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

    if matrix.num_responses <= 256 {
        (0..matrix.num_guesses)
            .into_par_iter()
            .map(|g| {
                let g_ptr = unsafe { matrix.data_u8.as_ptr().add(g * matrix.num_candidates) };
                let mut mask = [0u64; 8];
                count_unique_responses_from_row_u8(g_ptr, candidates, &mut mask);
                combine_and_count_u8(&mask)
            })
            .max()
            .unwrap_or(0)
    } else {
        (0..matrix.num_guesses)
            .into_par_iter()
            .map(|g| {
                let g_ptr = unsafe { matrix.data.as_ptr().add(g * matrix.num_candidates) };
                let mut mask = [0u64; 36];
                for &c in candidates {
                    let r = unsafe { (*g_ptr.add(c)).0 as usize };
                    unsafe {
                        *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
                    }
                }
                mask.iter().map(|w| w.count_ones() as usize).sum::<usize>()
            })
            .max()
            .unwrap_or(0)
    }
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
    let effective_target = target_k.min(candidates.len()).min(matrix.num_responses);
    let mut max_k = 0;
    if matrix.num_responses <= 256 {
        let data_ptr = matrix.data_u8.as_ptr();
        for &c in candidates {
            let g = unsafe { *matrix.candidate_to_guess.get_unchecked(c) };
            let g_ptr = unsafe { data_ptr.add(g * matrix.num_candidates) };
            let mut mask = [0u64; 8];
            count_unique_responses_from_row_u8(g_ptr, candidates, &mut mask);
            let k = combine_and_count_u8(&mask);
            if k > max_k {
                max_k = k;
                if max_k >= effective_target {
                    return max_k;
                }
            }
        }
    } else {
        let data_ptr = matrix.data.as_ptr();
        for &c in candidates {
            let g = unsafe { *matrix.candidate_to_guess.get_unchecked(c) };
            let g_ptr = unsafe { data_ptr.add(g * matrix.num_candidates) };
            let mut mask = [0u64; 36];
            for &other in candidates {
                let r = unsafe { (*g_ptr.add(other)).0 as usize };
                unsafe {
                    *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
                }
            }
            let k: usize = mask.iter().map(|w| w.count_ones() as usize).sum();
            if k > max_k {
                max_k = k;
                if max_k >= effective_target {
                    return max_k;
                }
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
    compute_candidate_branching_factor_with_target(
        matrix,
        candidates,
        candidates.len().min(matrix.num_responses),
    )
}

/// Finds the smallest branching factor `k` in 2..=n.min(num_responses) such that
/// `tight_capacity_bound(n, k, max_k) <= target_bound`.
/// If no such `k` exists (even for k = n.min(num_responses)), returns `n.min(num_responses) + 1`.
pub fn min_k_for_target_bound(n: usize, max_k: usize, target_bound: u32) -> usize {
    if n >= 3 && n <= max_k {
        let tb = target_bound as usize;
        if tb < 2 * n - 1 {
            return n + 1;
        }
        return (3 * n - 1).saturating_sub(tb).max(2);
    }
    let max_possible_k = n.min(max_k);
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

#[inline(always)]
fn min_non_overlap_for_len(word_len: usize, max_k: usize) -> u32 {
    let min_vp = if max_k >= 2187 {
        8
    } else if max_k >= 729 {
        7
    } else if max_k >= 243 {
        6
    } else if max_k >= 81 {
        5
    } else if max_k >= 27 {
        4
    } else if max_k >= 9 {
        3
    } else if max_k >= 3 {
        2
    } else {
        1
    };
    (word_len.saturating_sub(min_vp) + 1) as u32
}

/// Sequential version of `compute_max_branching_factor` with early stopping and target_k.
#[inline(always)]
fn count_unique_responses_u8(ptrs: &[*const u8], g: usize, mask: &mut [u64; 8]) {
    let n = ptrs.len();
    let mut p_it = ptrs.as_ptr();
    let p_end = unsafe { p_it.add(n & !1) };
    let mask_ptr = mask.as_mut_ptr();
    while p_it < p_end {
        let c_ptr0 = unsafe { *p_it };
        let c_ptr1 = unsafe { *p_it.add(1) };
        let r0 = unsafe { *c_ptr0.add(g) as usize };
        let r1 = unsafe { *c_ptr1.add(g) as usize };
        unsafe {
            *mask_ptr.add(r0 >> 6) |= 1u64 << (r0 & 63);
            *mask_ptr.add(4 + (r1 >> 6)) |= 1u64 << (r1 & 63);
            p_it = p_it.add(2);
        }
    }
    if (n & 1) != 0 {
        let c_ptr0 = unsafe { *p_end };
        let r0 = unsafe { *c_ptr0.add(g) as usize };
        unsafe {
            *mask_ptr.add(r0 >> 6) |= 1u64 << (r0 & 63);
        }
    }
}

pub fn compute_max_branching_factor_seq_with_target(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    initial_max_k: usize,
    target_k: usize,
) -> usize {
    if candidates.len() <= 2 {
        return candidates.len();
    }
    let effective_target = target_k.min(candidates.len()).min(matrix.num_responses);
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

    let word_len = matrix.word_len;
    let mut c_mask = 0u32;
    let mut c_all_mask = u32::MAX;
    let mut c_dup_mask = 0u32;
    let mut at_mask = [0u32; 8];
    let mut elsewhere_mask = [0u32; 8];
    let mut dup_elsewhere_mask = [0u32; 8];
    for &c in candidates {
        let m = unsafe { *matrix.candidate_masks.get_unchecked(c) };
        c_mask |= m;
        c_all_mask &= m;
        if m != u32::MAX && word_len <= 8 {
            let dup = unsafe { *matrix.candidate_dup_masks.get_unchecked(c) };
            c_dup_mask |= dup;
            let c_at = unsafe { matrix.candidate_at_masks.get_unchecked(c) };
            for i in 0..word_len {
                let bit = c_at[i];
                at_mask[i] |= bit;
                elsewhere_mask[i] |= m & !bit;
                dup_elsewhere_mask[i] |= dup & !bit;
            }
        }
    }
    let has_profile = c_mask != u32::MAX && word_len <= 8;
    let mut m_tab = [[1u8; 96]; 8];
    let mut active_letter_mask = 0u32;
    if has_profile {
        let b_u = !c_all_mask;
        let b_1_base = b_u | c_dup_mask;
        for i in 0..word_len {
            let a = at_mask[i];
            let e = elsewhere_mask[i];
            let e_dup = dup_elsewhere_mask[i];
            let b_r = if (a & (a - 1)) == 0 { !a } else { u32::MAX };
            let b_1 = b_r & b_1_base;
            let ae = a & e;
            let a_or_e = a | e;
            let ge2_u = ae | (a_or_e & b_u);
            let eq3_u = ae & b_u;
            let ge2_1 = ae | (a_or_e & b_1);
            let eq3_1 = ae & b_1;
            let ae_dup = a & e_dup;
            let a_or_e_dup = a | e_dup;
            let ge2_2 = ae_dup | (a_or_e_dup & b_r);
            let eq3_2 = ae_dup & b_r;
            active_letter_mask |= ge2_u | ge2_1 | ge2_2;

            let tab_i = &mut m_tab[i];
            let mut m = ge2_u;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                tab_i[l] = 2 + (((eq3_u >> l) & 1) as u8);
            }
            let mut m = ge2_1;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                tab_i[l + 32] = 2 + (((eq3_1 >> l) & 1) as u8);
            }
            let mut m = ge2_2;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                tab_i[l + 64] = 2 + (((eq3_2 >> l) & 1) as u8);
            }
        }
    }

    let mid = candidates.len() / 2;
    let rem_len = candidates.len() - mid;
    let num_guesses = matrix.num_guesses;
    if matrix.num_responses <= 256 {
        let data_c_g_ptr = matrix.data_c_g_u8.as_ptr();
        let mut stack_ptrs = std::mem::MaybeUninit::<[*const u8; 256]>::uninit();
        let stack_ptrs_ptr = stack_ptrs.as_mut_ptr() as *mut *const u8;
        let mut heap_ptrs = Vec::new();
        let (all_ptrs, c_half1_ptrs, c_half2_ptrs) = if candidates.len() <= 256 {
            for (i, &c) in candidates.iter().enumerate() {
                unsafe {
                    *stack_ptrs_ptr.add(i) = data_c_g_ptr.add(c * num_guesses);
                }
            }
            let all = unsafe { std::slice::from_raw_parts(stack_ptrs_ptr, candidates.len()) };
            (all, &all[..mid], &all[mid..])
        } else {
            heap_ptrs.reserve(candidates.len());
            for &c in candidates {
                heap_ptrs.push(unsafe { data_c_g_ptr.add(c * num_guesses) });
            }
            (&heap_ptrs[..], &heap_ptrs[..mid], &heap_ptrs[mid..])
        };

        if has_profile && word_len == 5 {
            let non_c_mask = !active_letter_mask;
            let mut min_non_overlap = min_non_overlap_for_len(5, max_k);
            let tab0 = &m_tab[0];
            let tab1 = &m_tab[1];
            let tab2 = &m_tab[2];
            let tab3 = &m_tab[3];
            let tab4 = &m_tab[4];
            for (g, p) in matrix.guess_profiles.iter().enumerate() {
                if (p.mask & non_c_mask).count_ones() >= min_non_overlap {
                    continue;
                }
                let prod = unsafe {
                    (*tab0.get_unchecked(p.chars[0] as usize) as u32)
                        * (*tab1.get_unchecked(p.chars[1] as usize) as u32)
                        * (*tab2.get_unchecked(p.chars[2] as usize) as u32)
                        * (*tab3.get_unchecked(p.chars[3] as usize) as u32)
                        * (*tab4.get_unchecked(p.chars[4] as usize) as u32)
                };
                if prod as usize <= max_k {
                    continue;
                }

                let mut mask = [0u64; 8];
                if rem_len < max_k {
                    count_unique_responses_u8(c_half1_ptrs, g, &mut mask);
                    let half_unique = combine_and_count_u8(&mask);
                    if half_unique + rem_len <= max_k {
                        continue;
                    }
                    count_unique_responses_u8(c_half2_ptrs, g, &mut mask);
                } else {
                    count_unique_responses_u8(all_ptrs, g, &mut mask);
                }
                let k = combine_and_count_u8(&mask);
                if k > max_k {
                    max_k = k;
                    if max_k >= effective_target {
                        break;
                    }
                    min_non_overlap = min_non_overlap_for_len(5, max_k);
                }
            }
        } else if has_profile {
            let non_c_mask = !active_letter_mask;
            let mut min_non_overlap = min_non_overlap_for_len(word_len, max_k);
            for (g, p) in matrix.guess_profiles.iter().enumerate() {
                if (p.mask & non_c_mask).count_ones() >= min_non_overlap {
                    continue;
                }
                let mut prod = 1u32;
                for i in 0..word_len {
                    prod *= unsafe { *m_tab[i].get_unchecked(p.chars[i] as usize) as u32 };
                }
                if prod as usize <= max_k {
                    continue;
                }

                let mut mask = [0u64; 8];
                if rem_len < max_k {
                    count_unique_responses_u8(c_half1_ptrs, g, &mut mask);
                    let half_unique = combine_and_count_u8(&mask);
                    if half_unique + rem_len <= max_k {
                        continue;
                    }
                    count_unique_responses_u8(c_half2_ptrs, g, &mut mask);
                } else {
                    count_unique_responses_u8(all_ptrs, g, &mut mask);
                }
                let k = combine_and_count_u8(&mask);
                if k > max_k {
                    max_k = k;
                    if max_k >= effective_target {
                        break;
                    }
                    min_non_overlap = min_non_overlap_for_len(word_len, max_k);
                }
            }
        } else {
            for (g, p) in matrix.guess_profiles.iter().enumerate() {
                let overlap = p.mask & c_mask;
                if overlap == 0 {
                    continue;
                }
                let mut mask = [0u64; 8];
                if rem_len < max_k {
                    count_unique_responses_u8(c_half1_ptrs, g, &mut mask);
                    let half_unique = combine_and_count_u8(&mask);
                    if half_unique + rem_len <= max_k {
                        continue;
                    }
                    count_unique_responses_u8(c_half2_ptrs, g, &mut mask);
                } else {
                    count_unique_responses_u8(all_ptrs, g, &mut mask);
                }
                let k = combine_and_count_u8(&mask);
                if k > max_k {
                    max_k = k;
                    if max_k >= effective_target {
                        break;
                    }
                }
            }
        }
    } else {
        let data_c_g_ptr = matrix.data_c_g.as_ptr();
        let mut stack_ptrs = std::mem::MaybeUninit::<[*const crate::core::Response; 256]>::uninit();
        let stack_ptrs_ptr = stack_ptrs.as_mut_ptr() as *mut *const crate::core::Response;
        let mut heap_ptrs = Vec::new();
        let (all_ptrs, c_half1_ptrs, c_half2_ptrs) = if candidates.len() <= 256 {
            for (i, &c) in candidates.iter().enumerate() {
                unsafe {
                    *stack_ptrs_ptr.add(i) = data_c_g_ptr.add(c * num_guesses);
                }
            }
            let all = unsafe { std::slice::from_raw_parts(stack_ptrs_ptr, candidates.len()) };
            (all, &all[..mid], &all[mid..])
        } else {
            heap_ptrs.reserve(candidates.len());
            for &c in candidates {
                heap_ptrs.push(unsafe { data_c_g_ptr.add(c * num_guesses) });
            }
            (&heap_ptrs[..], &heap_ptrs[..mid], &heap_ptrs[mid..])
        };

        let non_c_mask = !active_letter_mask;
        let mut min_non_overlap = min_non_overlap_for_len(word_len, max_k);
        for (g, p) in matrix.guess_profiles.iter().enumerate() {
            if has_profile {
                if (p.mask & non_c_mask).count_ones() >= min_non_overlap {
                    continue;
                }
                let mut prod = 1u32;
                for i in 0..word_len {
                    prod *= unsafe { *m_tab[i].get_unchecked(p.chars[i] as usize) as u32 };
                }
                if prod as usize <= max_k {
                    continue;
                }
            } else if (p.mask & c_mask) == 0 {
                continue;
            }

            let mut mask = [0u64; 36];
            if rem_len < max_k {
                for &c_base in c_half1_ptrs {
                    let r = unsafe { (*c_base.add(g)).0 as usize };
                    unsafe {
                        *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
                    }
                }
                let half_unique: usize = mask.iter().map(|w| w.count_ones() as usize).sum();
                if half_unique + rem_len <= max_k {
                    continue;
                }
                for &c_base in c_half2_ptrs {
                    let r = unsafe { (*c_base.add(g)).0 as usize };
                    unsafe {
                        *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
                    }
                }
            } else {
                for &c_base in all_ptrs {
                    let r = unsafe { (*c_base.add(g)).0 as usize };
                    unsafe {
                        *mask.get_unchecked_mut(r >> 6) |= 1u64 << (r & 63);
                    }
                }
            }
            let k: usize = mask.iter().map(|w| w.count_ones() as usize).sum();
            if k > max_k {
                max_k = k;
                if max_k >= effective_target {
                    break;
                }
                if has_profile {
                    min_non_overlap = min_non_overlap_for_len(word_len, max_k);
                }
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
    compute_max_branching_factor_seq_with_target(
        matrix,
        candidates,
        0,
        candidates.len().min(matrix.num_responses),
    )
}

/// Lower bound on the total cost of solving `n` candidates when no guess
/// splits them, or any subset of them, into more than `k` responses.
///
/// See [`forest_bound`] for the argument; this is the case of a single root.
pub fn capacity_bound(n: usize, k: usize) -> u32 {
    match n {
        0 => return 0,
        1 => return 1,
        2 => return 3,
        _ => {}
    }
    if k <= 1 {
        panic!("no strategy separates {n} candidates with branching factor {k}");
    }
    saturate(forest_bound(n as u64, 1, k as u64))
}

fn saturate(cost: u64) -> u32 {
    cost.min(u32::MAX as u64) as u32
}

/// Lower bound on the summed depths of `m` candidates solved by a forest of at
/// most `roots` decision trees whose roots sit at depth 1, where no guess has
/// more than `k >= 2` responses on any set it is asked about.
///
/// A guess that is one of the remaining candidates spends one of its `k`
/// responses on the win, so it has at most `k - 1` children; any other guess
/// solves nothing at its own depth but may have `k` children. With `a_d`
/// candidates solved at depth `d` and `m_d` nodes there, `m_1 <= roots`,
/// `a_d <= m_d` and `m_{d+1} <= k * m_d - a_d`. Unrolling,
///
/// ```text
/// A_D = a_1 + ... + a_D <= roots * k^(D-1) - sum_{d <= D-2} (k^(D-1-d) - 1) * a_d,
/// ```
///
/// and dropping every `a_d` but `a_1` (their coefficients are not negative)
/// leaves `A_D <= U_D(a_1) = roots * k^(D-1) - (k^(D-2) - 1) * a_1` for
/// `D >= 2`, with `A_1 = a_1`. The cost is the number of unsolved candidates
/// summed over depths, at least
///
/// ```text
/// g(a_1) = m + (m - a_1) + sum_{D >= 2} max(0, m - U_D(a_1)),
/// ```
///
/// so the bound is the minimum of `g` over `0 <= a_1 <= min(roots, m)`. `g` is
/// convex and piecewise linear, so that minimum is at an end of the range or
/// next to a kink.
///
/// When every node is a candidate the counting is exact (depth `d` then holds
/// `roots * (k-1)^(d-1)` candidates), but only up to
/// `m = roots * (k^2 - k + 1) + 1`. Past that, guesses outside the set pay for
/// themselves: one solved candidate moves a level down in exchange for a whole
/// extra subtree one level below, so the all-candidates fill overestimates.
/// Up to that point the minimum of `g` equals the all-candidates fill, which
/// is computed directly because it is cheaper.
pub fn forest_bound(m: u64, roots: u64, k: u64) -> u64 {
    if m == 0 {
        return 0;
    }
    if roots == 0 {
        return u64::MAX;
    }
    if m <= roots {
        return m;
    }
    debug_assert!(k >= 2);
    if m <= roots * (k * k - k + 1) + 1 {
        return all_candidates_fill(m, roots, k);
    }
    let hi = roots.min(m);
    let mut best = forest_g(m, roots, k, 0).min(forest_g(m, roots, k, hi));
    // Kinks of g, one per depth whose term can become active in [0, hi].
    let mut pow = k; // k^(D-2) for D = 3
    loop {
        let coeff = pow - 1;
        let cap = roots.saturating_mul(pow.saturating_mul(k)); // roots * k^(D-1)
        if cap.saturating_sub(coeff.saturating_mul(hi)) >= m {
            break;
        }
        if cap >= m {
            let kink = (cap - m) / coeff;
            for a in [kink, kink + 1] {
                if a <= hi {
                    best = best.min(forest_g(m, roots, k, a));
                }
            }
        }
        pow = pow.saturating_mul(k);
    }
    best
}

/// `g(a_1)` from [`forest_bound`].
fn forest_g(m: u64, roots: u64, k: u64, a1: u64) -> u64 {
    let mut cost = m + (m - a1);
    let mut pow = 1u64; // k^(D-2) for D = 2
    loop {
        let cap = roots.saturating_mul(pow.saturating_mul(k));
        let used = (pow - 1).saturating_mul(a1);
        let reach = cap.saturating_sub(used);
        if reach >= m {
            return cost;
        }
        cost += m - reach;
        pow = pow.saturating_mul(k);
    }
}

/// Summed depths when depth `d` holds `roots * (k-1)^(d-1)` candidates.
fn all_candidates_fill(m: u64, roots: u64, k: u64) -> u64 {
    let mut remaining = m;
    let mut cost = 0;
    let mut depth = 1;
    let mut capacity_at_depth = roots;
    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(k - 1);
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
        let dict = Dictionary::from_words(words.clone(), words.clone());
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

/// Lower bound on the cost of `n` candidates when the first guess is not one
/// of them and has at most `root_k` responses, and no later guess has more
/// than `max_k`.
///
/// The first guess solves nothing, so every candidate is at depth 2 or deeper,
/// below a forest of at most `root_k` subtrees (see [`forest_bound`]).
pub fn phase2_capacity_bound(n: usize, root_k: usize, max_k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if root_k <= 1 || max_k <= 1 {
        return capacity_bound(n, max_k);
    }
    if n <= max_k {
        return (3 * n - root_k.min(n)) as u32;
    }
    saturate(n as u64 + forest_bound(n as u64, root_k as u64, max_k as u64))
}

/// Lower bound on the cost of `n` candidates when the first guess is one of
/// them and has at most `root_k` responses, and no later guess has more than
/// `max_k`.
///
/// The first guess solves one candidate at depth 1 and leaves the other
/// `n - 1` below a forest of at most `root_k - 1` subtrees (see
/// [`forest_bound`]).
pub fn tight_capacity_bound(n: usize, root_k: usize, max_k: usize) -> u32 {
    match n {
        0 => return 0,
        1 => return 1,
        2 => return 3,
        _ => {}
    }
    if root_k <= 1 || max_k <= 1 {
        // No guess separates the candidates; the caller prunes on the result.
        return capacity_bound(n, max_k);
    }
    if n <= max_k {
        return (3 * n - 1 - root_k.min(n)) as u32;
    }
    saturate(n as u64 + forest_bound(n as u64 - 1, root_k as u64 - 1, max_k as u64))
}

#[cfg(test)]
mod forest_bound_tests {
    use super::*;

    /// Exact minima over abstract trees, by dynamic programming over every
    /// way to split a set: `f[n]` for one root, `p[j][m]` for at most `j`
    /// roots sharing `m` candidates. A node that is a candidate solves it and
    /// splits the rest into at most `k - 1` parts; any other node splits all
    /// of them into between 2 and `k` parts.
    fn exact(max_n: usize, k: usize) -> (Vec<u64>, Vec<Vec<u64>>) {
        let inf = u64::MAX / 4;
        let mut f = vec![0u64; max_n + 1];
        let mut p = vec![vec![inf; max_n + 1]; k + 1];
        for row in p.iter_mut() {
            row[0] = 0;
        }
        for m in 1..=max_n {
            if m == 1 {
                f[1] = 1;
            } else {
                let solve_one = p[k - 1][m - 1];
                let split = (1..m).map(|x| f[x] + p[k - 1][m - x]).min().unwrap();
                f[m] = m as u64 + solve_one.min(split);
            }
            for j in 1..=k {
                let best = (1..=m).map(|x| f[x] + p[j - 1][m - x]).min().unwrap();
                p[j][m] = p[j - 1][m].min(best);
            }
        }
        (f, p)
    }

    /// `forest_bound` without the shortcut, minimizing `g` over every `a_1`.
    fn brute_g_min(m: u64, roots: u64, k: u64) -> u64 {
        if m == 0 {
            return 0;
        }
        if m <= roots {
            return m;
        }
        (0..=roots.min(m))
            .map(|a| forest_g(m, roots, k, a))
            .min()
            .unwrap()
    }

    #[test]
    fn never_exceeds_the_exact_minimum() {
        for k in 2..=7usize {
            let max_n = (k * k * k + 40).min(260);
            let (f, p) = exact(max_n, k);
            for n in 0..=max_n {
                assert!(
                    capacity_bound(n, k) as u64 <= f[n],
                    "capacity_bound({n}, {k}) = {} > exact {}",
                    capacity_bound(n, k),
                    f[n]
                );
            }
            for roots in 1..=k {
                for m in 0..=max_n {
                    let lb = forest_bound(m as u64, roots as u64, k as u64);
                    assert!(
                        lb <= p[roots][m],
                        "forest_bound({m}, {roots}, {k}) = {lb} > exact {}",
                        p[roots][m]
                    );
                }
            }
            for root_k in 2..=k {
                for n in 3..=max_n {
                    // Candidate root: one solved, the rest under root_k - 1 subtrees.
                    let tight = n as u64 + p[root_k - 1][n - 1];
                    assert!(tight_capacity_bound(n, root_k, k) as u64 <= tight);
                    // Other root: everything under root_k subtrees.
                    let phase2 = n as u64 + p[root_k][n];
                    assert!(phase2_capacity_bound(n, root_k, k) as u64 <= phase2);
                }
            }
        }
    }

    #[test]
    fn the_old_fill_overestimates_from_k_squared_minus_k_plus_3() {
        for k in 2..=7usize {
            let n = k * k - k + 3;
            let (f, _) = exact(n, k);
            assert!(all_candidates_fill(n as u64, 1, k as u64) > f[n]);
            assert_eq!(all_candidates_fill(n as u64 - 1, 1, k as u64), f[n - 1]);
        }
    }

    #[test]
    fn shortcut_and_kinks_match_the_full_minimum() {
        for k in 2..=40u64 {
            for roots in [1u64, 2, 3, 5, k - 1, k] {
                let limit = roots * (k * k - k + 1) + 3 * k + 10;
                for m in 0..=limit.min(5000) {
                    assert_eq!(
                        forest_bound(m, roots, k),
                        brute_g_min(m, roots, k),
                        "m={m} roots={roots} k={k}"
                    );
                }
            }
        }
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = 2 + x % 242;
            let roots = 1 + (x >> 8) % k;
            let m = (x >> 20) % 200_000;
            assert_eq!(
                forest_bound(m, roots, k),
                brute_g_min(m, roots, k),
                "m={m} roots={roots} k={k}"
            );
        }
    }

    /// The bounds as they were before guesses outside the set were accounted
    /// for: every node below the first guess spends a response on a win.
    fn old_tight(n: u64, root_k: u64, max_k: u64) -> u64 {
        1 + (n - 1) + all_candidates_fill(n - 1, root_k - 1, max_k)
    }

    fn old_phase2(n: u64, root_k: u64, max_k: u64) -> u64 {
        n + all_candidates_fill(n, root_k, max_k)
    }

    #[test]
    fn unchanged_wherever_the_old_fill_was_valid() {
        for k in 2..=243u64 {
            for n in 0..=(k * k - k + 2).min(20_000) {
                let old = all_candidates_fill(n, 1, k);
                assert_eq!(capacity_bound(n as usize, k as usize) as u64, old);
            }
        }
        for max_k in (2..=243u64).step_by(7).chain([3, 4, 5, 243]) {
            for root_k in (2..=max_k).step_by(5).chain([2, max_k]) {
                let valid = |m: u64, roots: u64| m <= roots * (max_k * max_k - max_k + 1) + 1;
                for n in 3..=6000u64 {
                    if valid(n - 1, root_k - 1) {
                        assert_eq!(
                            tight_capacity_bound(n as usize, root_k as usize, max_k as usize)
                                as u64,
                            old_tight(n, root_k, max_k)
                        );
                    }
                    if valid(n, root_k) {
                        assert_eq!(
                            phase2_capacity_bound(n as usize, root_k as usize, max_k as usize)
                                as u64,
                            old_phase2(n, root_k, max_k)
                        );
                    }
                }
            }
        }
    }
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
                let var_pos = if g_ones == matrix.word_len as u32 {
                    overlap_count
                } else {
                    let chars = &matrix.guess_chars[g];
                    let mut vp = 0u32;
                    for i in 0..matrix.word_len.min(8) {
                        vp += (c_mask >> chars[i]) & 1;
                    }
                    vp
                };

                let mut mask = [0u64; 4];
                for &c in subset {
                    let r = matrix.get(g, c).0 as usize;
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

    #[test]
    fn test_per_letter_profile_bound_soundness() {
        use crate::dict::Dictionary;
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let mut rng = fastrand::Rng::with_seed(98765);
        for size in [4, 12, 20, 35] {
            let mut candidates: Vec<usize> = (0..matrix.num_candidates).collect();
            rng.shuffle(&mut candidates);
            let subset = &candidates[..size];

            let mut c_all_mask = u32::MAX;
            let mut c_dup_mask = 0u32;
            let mut at_mask = [0u32; 8];
            let mut elsewhere_mask = [0u32; 8];
            let mut dup_elsewhere_mask = [0u32; 8];
            for &c in subset {
                let m = matrix.candidate_masks[c];
                c_all_mask &= m;
                let dup = matrix.candidate_dup_masks[c];
                c_dup_mask |= dup;
                let c_at = &matrix.candidate_at_masks[c];
                for i in 0..5 {
                    let bit = c_at[i];
                    at_mask[i] |= bit;
                    elsewhere_mask[i] |= m & !bit;
                    dup_elsewhere_mask[i] |= dup & !bit;
                }
            }
            let b_u = !c_all_mask;
            let b_1_base = b_u | c_dup_mask;
            let mut m_tab = [[1u8; 96]; 8];
            let mut active_letter_mask = 0u32;
            for i in 0..5 {
                let a = at_mask[i];
                let e = elsewhere_mask[i];
                let e_dup = dup_elsewhere_mask[i];
                let b_r = if (a & (a - 1)) == 0 { !a } else { u32::MAX };
                let b_1 = b_r & b_1_base;
                let ae = a & e;
                let a_or_e = a | e;
                let ge2_u = ae | (a_or_e & b_u);
                let eq3_u = ae & b_u;
                let ge2_1 = ae | (a_or_e & b_1);
                let eq3_1 = ae & b_1;
                let ae_dup = a & e_dup;
                let a_or_e_dup = a | e_dup;
                let ge2_2 = ae_dup | (a_or_e_dup & b_r);
                let eq3_2 = ae_dup & b_r;
                active_letter_mask |= ge2_u | ge2_1 | ge2_2;

                let tab_i = &mut m_tab[i];
                let mut m = ge2_u;
                while m != 0 {
                    let l = m.trailing_zeros() as usize;
                    m &= m - 1;
                    tab_i[l] = 2 + (((eq3_u >> l) & 1) as u8);
                }
                let mut m = ge2_1;
                while m != 0 {
                    let l = m.trailing_zeros() as usize;
                    m &= m - 1;
                    tab_i[l + 32] = 2 + (((eq3_1 >> l) & 1) as u8);
                }
                let mut m = ge2_2;
                while m != 0 {
                    let l = m.trailing_zeros() as usize;
                    m &= m - 1;
                    tab_i[l + 64] = 2 + (((eq3_2 >> l) & 1) as u8);
                }
            }

            let non_c_mask = !active_letter_mask;
            for (g, chars) in matrix.guess_profile_chars.iter().enumerate() {
                let mut prod = 1usize;
                for i in 0..5 {
                    let l = chars[i] as usize;
                    prod *= m_tab[i][l] as usize;
                }
                let non_overlap = (matrix.guess_masks[g] & non_c_mask).count_ones();
                let mut mask = [0u64; 4];
                for &c in subset {
                    let r = matrix.get(g, c).0 as usize;
                    mask[r >> 6] |= 1u64 << (r & 63);
                }
                let actual_k = (mask[0].count_ones()
                    + mask[1].count_ones()
                    + mask[2].count_ones()
                    + mask[3].count_ones()) as usize;
                assert!(
                    actual_k <= prod,
                    "g={} ({}) actual_k={} > prod={}",
                    g,
                    dict.guesses[g],
                    actual_k,
                    prod
                );
                assert!(
                    actual_k <= 3usize.pow(5u32.saturating_sub(non_overlap)),
                    "g={} ({}) actual_k={} > 3^(5 - {})",
                    g,
                    dict.guesses[g],
                    actual_k,
                    non_overlap
                );
            }
            assert_eq!(
                compute_max_branching_factor_seq(&matrix, subset),
                compute_max_branching_factor(&matrix, subset)
            );
        }

        // Also a subset sharing fixed letters at the last two positions.
        let er_subset: Vec<usize> = (0..matrix.num_candidates)
            .filter(|&c| dict.candidates[c].ends_with(b"er"))
            .take(30)
            .collect();
        assert_eq!(
            compute_max_branching_factor_seq(&matrix, &er_subset),
            compute_max_branching_factor(&matrix, &er_subset)
        );
    }
}
