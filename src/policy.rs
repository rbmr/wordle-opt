#![allow(clippy::needless_range_loop)]
//! Building policy trees: the strategies and the builder that captures one
//! into a [`PolicyTree`]. The tree format and its validation are in
//! `crate::tree`, re-exported here.

use crate::cache::GlobalCache;
use crate::dict::Dictionary;
use crate::heuristic;
use crate::matrix::ResponseMatrix;
use crate::solver::{EquivCache, Metrics, Solver};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub use crate::tree::*;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// The built-in strategies that can be captured into a policy tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The exact cost-minimizing strategy, computed with the branch-and-bound
    /// solver. Expensive: building the full tree requires an exact solve of
    /// every reachable state.
    Optimal,
    /// Greedy heuristic: pick the guess that minimizes the expected number of
    /// remaining candidates, `sum_r |C_{g,r}|^2`.
    MinRemaining,
    /// Greedy heuristic: pick the guess maximizing the summed letter frequency
    /// of its distinct letters over the current candidate set.
    MaxFreq,
}

impl Strategy {
    pub fn parse(s: &str) -> Option<Strategy> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "optimal" => Some(Strategy::Optimal),
            "min-remaining" | "min-guesses-remaining" | "minremaining" => {
                Some(Strategy::MinRemaining)
            }
            "max-freq" | "max-frequency" | "max-letter-freq" | "maxfreq" => Some(Strategy::MaxFreq),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Strategy::Optimal => "optimal",
            Strategy::MinRemaining => "min-remaining",
            Strategy::MaxFreq => "max-freq",
        }
    }

    pub fn all() -> [Strategy; 3] {
        [Strategy::Optimal, Strategy::MinRemaining, Strategy::MaxFreq]
    }
}

/// A candidates policy: given the set of still-possible candidates, returns
/// the guess to make. This is the abstraction the tree builder consumes, so a
/// new strategy only has to implement this one method to be captured.
///
/// The returned index refers to `Dictionary::guesses`. A policy is only
/// required to be *useful* (strictly shrink the candidate set) when more than
/// one candidate remains; the builder special-cases the singleton state (the
/// only winning move is to guess the remaining candidate).
pub trait CandidatesPolicy {
    fn guess(&self, candidates: &[usize]) -> usize;
}

/// Shared helper: the projection used to deduplicate guesses that partition
/// the candidate set identically (same scheme as `Solver::greedy_solve`).
fn guess_projection(dict: &Dictionary, c_mask: u32, guess: usize) -> u64 {
    let chars = &dict.guess_chars[guess];
    let mut proj = 0u64;
    for i in 0..dict.word_len.min(8) {
        let l = chars[i] as u64;
        proj |= ((l + 1) * (((c_mask >> l) & 1) as u64)) << (5 * i);
    }
    proj
}

/// The active (non-useless), projection-deduplicated guesses for `candidates`,
/// sorted by ascending expected-remaining then index (deterministic).
pub(crate) fn active_guesses(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    candidates: &[usize],
    capacity_bounds_2d: &[Vec<u32>],
    max_k: usize,
) -> Vec<(usize, u32, u32)> {
    use rayon::prelude::*;

    let mut c_mask = 0u32;
    for &c in candidates {
        c_mask |= matrix.candidate_masks[c];
    }

    let skip_projections = c_mask.count_ones() >= 26;
    let surviving_guesses: Vec<usize> = if skip_projections {
        (0..dict.guesses.len()).collect()
    } else {
        let mut seen = rustc_hash::FxHashSet::with_capacity_and_hasher(
            dict.guesses.len().min(4096),
            Default::default(),
        );
        (0..dict.guesses.len())
            .filter(|&g| seen.insert(guess_projection(dict, c_mask, g)))
            .collect()
    };

    let cap_bounds = &capacity_bounds_2d[max_k];
    let mut out: Vec<(usize, u32, u32)> = if candidates.len() >= 48 {
        if matrix.num_responses <= 256 {
            surviving_guesses
                .par_chunks(256)
                .flat_map_iter(|chunk| {
                    let mut counts = [0u16; 256];
                    let mut non_empty_indices = std::mem::MaybeUninit::<[u16; 256]>::uninit();
                    let non_empty_indices_ptr = non_empty_indices.as_mut_ptr() as *mut u16;
                    let cap_bounds_ptr = cap_bounds.as_ptr();
                    let matrix_data_ptr = matrix.data_u8.as_ptr();
                    let mut chunk_out = Vec::with_capacity(chunk.len());

                    for &g in chunk {
                        let g_ptr = unsafe { matrix_data_ptr.add(g * matrix.num_candidates) };
                        let mut num_non_empty = 0;
                        for &c in candidates {
                            let r = unsafe { *g_ptr.add(c) as usize };
                            let cnt = unsafe { *counts.get_unchecked(r) };
                            if cnt == 0 {
                                unsafe {
                                    *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                                }
                                num_non_empty += 1;
                            }
                            unsafe {
                                *counts.get_unchecked_mut(r) = cnt + 1;
                            }
                        }
                        if num_non_empty <= 1 {
                            for i in 0..num_non_empty {
                                let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                                unsafe {
                                    *counts.get_unchecked_mut(r) = 0;
                                }
                            }
                            continue;
                        }
                        let mut expected_rem = 0u32;
                        let mut base_lb = candidates.len() as u32;
                        for i in 0..num_non_empty {
                            let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                            let v = unsafe { *counts.get_unchecked(r) as usize };
                            expected_rem += (v as u32) * (v as u32);
                            if r != matrix.win_response.0 as usize {
                                base_lb += unsafe { *cap_bounds_ptr.add(v) };
                            }
                            unsafe {
                                *counts.get_unchecked_mut(r) = 0;
                            }
                        }
                        chunk_out.push((g, expected_rem, base_lb));
                    }
                    chunk_out
                })
                .collect()
        } else {
            surviving_guesses
                .par_chunks(256)
                .flat_map_iter(|chunk| {
                    let mut counts = [0u16; 2187];
                    let mut non_empty_indices = std::mem::MaybeUninit::<[u16; 2187]>::uninit();
                    let non_empty_indices_ptr = non_empty_indices.as_mut_ptr() as *mut u16;
                    let cap_bounds_ptr = cap_bounds.as_ptr();
                    let matrix_data_ptr = matrix.data.as_ptr();
                    let mut chunk_out = Vec::with_capacity(chunk.len());

                    for &g in chunk {
                        let g_ptr = unsafe { matrix_data_ptr.add(g * matrix.num_candidates) };
                        let mut num_non_empty = 0;
                        for &c in candidates {
                            let r = unsafe { (*g_ptr.add(c)).0 as usize };
                            let cnt = unsafe { *counts.get_unchecked(r) };
                            if cnt == 0 {
                                unsafe {
                                    *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                                }
                                num_non_empty += 1;
                            }
                            unsafe {
                                *counts.get_unchecked_mut(r) = cnt + 1;
                            }
                        }
                        if num_non_empty <= 1 {
                            for i in 0..num_non_empty {
                                let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                                unsafe {
                                    *counts.get_unchecked_mut(r) = 0;
                                }
                            }
                            continue;
                        }
                        let mut expected_rem = 0u32;
                        let mut base_lb = candidates.len() as u32;
                        for i in 0..num_non_empty {
                            let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                            let v = unsafe { *counts.get_unchecked(r) as usize };
                            expected_rem += (v as u32) * (v as u32);
                            if r != matrix.win_response.0 as usize {
                                base_lb += unsafe { *cap_bounds_ptr.add(v) };
                            }
                            unsafe {
                                *counts.get_unchecked_mut(r) = 0;
                            }
                        }
                        chunk_out.push((g, expected_rem, base_lb));
                    }
                    chunk_out
                })
                .collect()
        }
    } else {
        let mut out = Vec::with_capacity(surviving_guesses.len());
        let cap_bounds_ptr = cap_bounds.as_ptr();
        let num_guesses = matrix.num_guesses;

        if matrix.num_responses <= 256 {
            let data_c_g_ptr = matrix.data_c_g_u8.as_ptr();
            let mut stack_c_ptrs = std::mem::MaybeUninit::<[*const u8; 48]>::uninit();
            let stack_c_ptrs_ptr = stack_c_ptrs.as_mut_ptr() as *mut *const u8;
            for (i, &c) in candidates.iter().enumerate() {
                unsafe {
                    *stack_c_ptrs_ptr.add(i) = data_c_g_ptr.add(c * num_guesses);
                }
            }
            let c_ptrs = unsafe { std::slice::from_raw_parts(stack_c_ptrs_ptr, candidates.len()) };

            let mut counts = [0u16; 256];
            let mut non_empty_indices = std::mem::MaybeUninit::<[u16; 256]>::uninit();
            let non_empty_indices_ptr = non_empty_indices.as_mut_ptr() as *mut u16;

            for g in surviving_guesses {
                let mut num_non_empty = 0;
                for &c_base in c_ptrs {
                    let r = unsafe { *c_base.add(g) as usize };
                    let cnt = unsafe { *counts.get_unchecked(r) };
                    if cnt == 0 {
                        unsafe {
                            *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                        }
                        num_non_empty += 1;
                    }
                    unsafe {
                        *counts.get_unchecked_mut(r) = cnt + 1;
                    }
                }
                if num_non_empty <= 1 {
                    for i in 0..num_non_empty {
                        let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                        unsafe {
                            *counts.get_unchecked_mut(r) = 0;
                        }
                    }
                    continue;
                }
                let mut expected_rem = 0u32;
                let mut base_lb = candidates.len() as u32;
                for i in 0..num_non_empty {
                    let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                    let v = unsafe { *counts.get_unchecked(r) as usize };
                    expected_rem += (v as u32) * (v as u32);
                    if r != matrix.win_response.0 as usize {
                        base_lb += unsafe { *cap_bounds_ptr.add(v) };
                    }
                    unsafe {
                        *counts.get_unchecked_mut(r) = 0;
                    }
                }
                out.push((g, expected_rem, base_lb));
            }
        } else {
            let data_c_g_ptr = matrix.data_c_g.as_ptr();
            let mut stack_c_ptrs =
                std::mem::MaybeUninit::<[*const crate::core::Response; 48]>::uninit();
            let stack_c_ptrs_ptr = stack_c_ptrs.as_mut_ptr() as *mut *const crate::core::Response;
            for (i, &c) in candidates.iter().enumerate() {
                unsafe {
                    *stack_c_ptrs_ptr.add(i) = data_c_g_ptr.add(c * num_guesses);
                }
            }
            let c_ptrs = unsafe { std::slice::from_raw_parts(stack_c_ptrs_ptr, candidates.len()) };

            let mut counts = [0u16; 2187];
            let mut non_empty_indices = std::mem::MaybeUninit::<[u16; 2187]>::uninit();
            let non_empty_indices_ptr = non_empty_indices.as_mut_ptr() as *mut u16;

            for g in surviving_guesses {
                let mut num_non_empty = 0;
                for &c_base in c_ptrs {
                    let r = unsafe { (*c_base.add(g)).0 as usize };
                    let cnt = unsafe { *counts.get_unchecked(r) };
                    if cnt == 0 {
                        unsafe {
                            *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                        }
                        num_non_empty += 1;
                    }
                    unsafe {
                        *counts.get_unchecked_mut(r) = cnt + 1;
                    }
                }
                if num_non_empty <= 1 {
                    for i in 0..num_non_empty {
                        let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                        unsafe {
                            *counts.get_unchecked_mut(r) = 0;
                        }
                    }
                    continue;
                }
                let mut expected_rem = 0u32;
                let mut base_lb = candidates.len() as u32;
                for i in 0..num_non_empty {
                    let r = unsafe { *non_empty_indices_ptr.add(i) as usize };
                    let v = unsafe { *counts.get_unchecked(r) as usize };
                    expected_rem += (v as u32) * (v as u32);
                    if r != matrix.win_response.0 as usize {
                        base_lb += unsafe { *cap_bounds_ptr.add(v) };
                    }
                    unsafe {
                        *counts.get_unchecked_mut(r) = 0;
                    }
                }
                out.push((g, expected_rem, base_lb));
            }
        }
        out
    };

    out.sort_unstable_by_key(|&(g, e, _)| (e, g));
    out
}

/// Whether `guess` strictly partitions `candidates` (produces at least two
/// distinct responses). A guess that doesn't is *useless*: it leaves the
/// candidate set unchanged and can never terminate a policy.
fn guess_is_useful(matrix: &ResponseMatrix, candidates: &[usize], guess: usize) -> bool {
    if candidates.len() <= 1 {
        return false;
    }
    let first = matrix.get(guess, candidates[0]);
    for &c in &candidates[1..] {
        if matrix.get(guess, c) != first {
            return true;
        }
    }
    false
}

/// "Min guesses remaining": minimize `sum_r |C_{g,r}|^2`, the expected number
/// of remaining candidates (up to the constant factor `|C|`). Only guesses that
/// partition the candidate set are considered, so the policy always terminates.
pub struct MinRemainingPolicy<'a> {
    matrix: &'a ResponseMatrix,
}

impl<'a> MinRemainingPolicy<'a> {
    pub fn new(matrix: &'a ResponseMatrix) -> Self {
        Self { matrix }
    }
}

impl CandidatesPolicy for MinRemainingPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        let mut best_guess = usize::MAX;
        let mut best_score = u64::MAX;
        if self.matrix.num_responses <= 256 {
            let mut counts = [0u16; 256];
            for g in 0..self.matrix.num_guesses {
                let mut distinct = 0;
                for &c in candidates {
                    let r = self.matrix.get(g, c).0 as usize;
                    if counts[r] == 0 {
                        distinct += 1;
                    }
                    counts[r] += 1;
                }
                if distinct <= 1 {
                    for &c in candidates {
                        let r = self.matrix.get(g, c).0 as usize;
                        counts[r] = 0;
                    }
                    continue; // useless
                }
                let mut score = 0u64;
                for &c in candidates {
                    let r = self.matrix.get(g, c).0 as usize;
                    let v = counts[r];
                    if v > 0 {
                        score += (v as u64) * (v as u64);
                        counts[r] = 0;
                    }
                }
                if score < best_score {
                    best_score = score;
                    best_guess = g;
                }
            }
        } else {
            let mut counts = [0u16; 2187];
            for g in 0..self.matrix.num_guesses {
                let mut distinct = 0;
                for &c in candidates {
                    let r = self.matrix.get(g, c).0 as usize;
                    if counts[r] == 0 {
                        distinct += 1;
                    }
                    counts[r] += 1;
                }
                if distinct <= 1 {
                    for &c in candidates {
                        let r = self.matrix.get(g, c).0 as usize;
                        counts[r] = 0;
                    }
                    continue; // useless
                }
                let mut score = 0u64;
                for &c in candidates {
                    let r = self.matrix.get(g, c).0 as usize;
                    let v = counts[r];
                    if v > 0 {
                        score += (v as u64) * (v as u64);
                        counts[r] = 0;
                    }
                }
                if score < best_score {
                    best_score = score;
                    best_guess = g;
                }
            }
        }
        best_guess
    }
}

/// "Max letter frequency": maximize the summed frequency, over the candidate
/// set, of the distinct letters appearing in the guess.
///
/// The best-scoring guess that actually partitions the set is used: a
/// top-scoring guess can be useless (for example, one whose letters all yield
/// the same response), and such a guess cannot be part of a terminating policy.
pub struct MaxFreqPolicy<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
}

impl<'a> MaxFreqPolicy<'a> {
    pub fn new(matrix: &'a ResponseMatrix, dict: &'a Dictionary) -> Self {
        Self { matrix, dict }
    }
}

impl CandidatesPolicy for MaxFreqPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        // Counted per byte, so any symbol alphabet works.
        let mut letter_freq = [0u32; 256];
        for &c in candidates {
            let w = self.dict.candidates[c].as_bytes();
            for (i, &b) in w.iter().enumerate() {
                if !w[..i].contains(&b) {
                    letter_freq[b as usize] += 1;
                }
            }
        }
        let mut scored: Vec<(u32, usize)> = (0..self.dict.guesses.len())
            .map(|g| {
                let w = self.dict.guesses[g].as_bytes();
                let mut score = 0u32;
                for (i, &b) in w.iter().enumerate() {
                    if !w[..i].contains(&b) {
                        score += letter_freq[b as usize];
                    }
                }
                (score, g)
            })
            .collect();
        scored.sort_unstable_by_key(|&(s, g)| (std::cmp::Reverse(s), g));
        for (_, g) in scored {
            if guess_is_useful(self.matrix, candidates, g) {
                return g;
            }
        }
        usize::MAX
    }
}

/// The exact, cost-minimizing policy.
///
/// For a state `C` it returns `(guess, T*(C))` where `guess` achieves the
/// minimum total cost and `T*` is that cost. It reuses a single persistent
/// `GlobalCache`/`EquivCache` across every state it is asked about, so building
/// a whole tree shares memoization between nodes.
///
/// Cost model: `T*(C) = min_g [ |C| + sum_{r != WIN} T*(C_{g,r}) ]` - the
/// integer reformulation of the expected-guess objective (see `ideas/Wordle.md`).
pub struct OptimalPolicy<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
    metrics: &'a Metrics,
    cache: &'a GlobalCache,
    equiv_cache: &'a EquivCache,
    capacity_bounds_2d: &'a [Vec<u32>],
    max_k: usize,
    all_guesses_bits: &'a [u64],
}

impl<'a> OptimalPolicy<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        matrix: &'a ResponseMatrix,
        dict: &'a Dictionary,
        metrics: &'a Metrics,
        cache: &'a GlobalCache,
        equiv_cache: &'a EquivCache,
        capacity_bounds_2d: &'a [Vec<u32>],
        max_k: usize,
        all_guesses_bits: &'a [u64],
    ) -> Self {
        Self {
            matrix,
            dict,
            metrics,
            cache,
            equiv_cache,
            capacity_bounds_2d,
            max_k,
            all_guesses_bits,
        }
    }

    /// Fast-path for small candidate sets (n <= 15) that can be solved in 2n - 1 total cost
    /// (the absolute theoretical minimum possible cost) by an in-set guess that isolates
    /// all other n - 1 candidates into distinct responses.
    ///
    /// Tie-breaking in scan sorts active guesses by (expected_rem, g). Since any guess that
    /// partitions into singletons has the absolute minimum possible expected_rem (n), scan
    /// deterministically selects the in-set guess achieving 2n - 1 with the smallest guess index g.
    /// Testing candidate guesses in ascending order of g preserves exact tie-breaking.
    fn quick_exact_guess(&self, candidates: &[usize]) -> Option<(usize, u32)> {
        let n = candidates.len();
        if n > 15 {
            return None;
        }

        let mut cand_guesses = [0usize; 16];
        for i in 0..n {
            cand_guesses[i] = self.dict.candidate_to_guess[candidates[i]];
        }
        cand_guesses[0..n].sort_unstable();

        if self.matrix.num_responses <= 256 {
            let matrix_data_ptr = self.matrix.data_u8.as_ptr();
            for &g in &cand_guesses[0..n] {
                let g_ptr = unsafe { matrix_data_ptr.add(g * self.matrix.num_candidates) };

                let mut seen_mask = [0u64; 4];
                let mut has_collision = false;

                for &c in candidates {
                    let r = unsafe { *g_ptr.add(c) as usize };
                    let word_idx = r >> 6;
                    let bit = 1u64 << (r & 63);
                    if (seen_mask[word_idx] & bit) != 0 {
                        has_collision = true;
                        break;
                    }
                    seen_mask[word_idx] |= bit;
                }

                if !has_collision {
                    return Some((g, (2 * n - 1) as u32));
                }
            }
        } else {
            let matrix_data_ptr = self.matrix.data.as_ptr();
            for &g in &cand_guesses[0..n] {
                let g_ptr = unsafe { matrix_data_ptr.add(g * self.matrix.num_candidates) };

                let mut seen = [0u16; 16];
                let mut has_collision = false;

                for (seen_len, &c) in candidates.iter().enumerate() {
                    let r = unsafe { (*g_ptr.add(c)).0 };
                    for &prev in &seen[..seen_len] {
                        if prev == r {
                            has_collision = true;
                            break;
                        }
                    }
                    if has_collision {
                        break;
                    }
                    seen[seen_len] = r;
                }

                if !has_collision {
                    return Some((g, (2 * n - 1) as u32));
                }
            }
        }

        None
    }

    /// The exact optimal guess and its total cost for `candidates`.
    pub fn guess_and_cost(&self, candidates: &[usize]) -> (usize, u32) {
        match candidates.len() {
            0 => (usize::MAX, 0),
            1 => (self.dict.candidate_to_guess[candidates[0]], 1),
            // Any in-set guess is optimal for two candidates (1 + 2 = 3).
            2 => (self.dict.candidate_to_guess[candidates[0]], 3),
            _ => {
                if let Some(res) = self.quick_exact_guess(candidates) {
                    return res;
                }
                self.scan(candidates)
            }
        }
    }

    fn scan(&self, candidates: &[usize]) -> (usize, u32) {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

        let active = active_guesses(
            self.matrix,
            self.dict,
            candidates,
            self.capacity_bounds_2d,
            self.max_k,
        );

        // For large candidate sets (especially root), evaluating the best-ranked guess
        // across all cores via bucket parallelization quickly establishes an exact tight upper bound.
        // This tight bound lets us prune >90% of alternative root guesses before any deep search.
        if candidates.len() >= 48 && !active.is_empty() {
            let cap_bounds = &self.capacity_bounds_2d[self.max_k];
            let first_g = active[0].0;

            let num_resp = self.matrix.num_responses;
            let mut counts = vec![0u16; num_resp];
            let mut bucket_hashes = vec![0u64; num_resp];
            let mut non_empty_indices = vec![0u16; num_resp];
            let mut num_non_empty = 0;

            let zobrist_ptr = self.matrix.zobrist.as_ptr();
            let cap_bounds_ptr = cap_bounds.as_ptr();

            let (first_g_u8_ptr, first_g_u16_ptr) = if self.matrix.num_responses <= 256 {
                (
                    unsafe {
                        self.matrix
                            .data_u8
                            .as_ptr()
                            .add(first_g * self.matrix.num_candidates)
                    },
                    std::ptr::null(),
                )
            } else {
                (std::ptr::null(), unsafe {
                    self.matrix
                        .data
                        .as_ptr()
                        .add(first_g * self.matrix.num_candidates)
                })
            };

            for &c in candidates {
                let r_idx = if !first_g_u8_ptr.is_null() {
                    unsafe { *first_g_u8_ptr.add(c) as usize }
                } else {
                    unsafe { (*first_g_u16_ptr.add(c)).0 as usize }
                };
                unsafe {
                    let cnt = *counts.get_unchecked(r_idx);
                    if cnt == 0 {
                        *non_empty_indices.get_unchecked_mut(num_non_empty) = r_idx as u16;
                        num_non_empty += 1;
                    }
                    *counts.get_unchecked_mut(r_idx) = cnt + 1;
                    *bucket_hashes.get_unchecked_mut(r_idx) ^= *zobrist_ptr.add(c);
                }
            }

            let mut offsets = vec![0usize; num_resp + 1];
            for r_idx in 0..num_resp {
                unsafe {
                    *offsets.get_unchecked_mut(r_idx + 1) =
                        *offsets.get_unchecked(r_idx) + *counts.get_unchecked(r_idx) as usize;
                }
            }
            let mut sorted_set = vec![0usize; candidates.len()];
            let mut current_offsets = offsets.clone();
            for &c in candidates {
                let r_idx = if !first_g_u8_ptr.is_null() {
                    unsafe { *first_g_u8_ptr.add(c) as usize }
                } else {
                    unsafe { (*first_g_u16_ptr.add(c)).0 as usize }
                };
                let pos = unsafe { *current_offsets.get_unchecked(r_idx) };
                unsafe {
                    *sorted_set.get_unchecked_mut(pos) = c;
                    *current_offsets.get_unchecked_mut(r_idx) += 1;
                }
            }

            let mut bucket_tasks = Vec::new();
            let mut base_cost = candidates.len() as u32;
            for i in 0..num_non_empty {
                let r_idx = unsafe { *non_empty_indices.get_unchecked(i) as usize };
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };
                if r_idx == self.matrix.win_response.0 as usize {
                    continue;
                }
                if p_len <= 2 {
                    base_cost += unsafe { *cap_bounds_ptr.add(p_len) };
                    continue;
                }
                let hash = unsafe { *bucket_hashes.get_unchecked(r_idx) };
                if let Some((cached_val, true)) = self.cache.get(hash) {
                    base_cost += cached_val;
                    continue;
                }
                let start = unsafe { *offsets.get_unchecked(r_idx) };
                let end = start + p_len;
                let p = &sorted_set[start..end];
                bucket_tasks.push((p, hash));
            }

            bucket_tasks.sort_unstable_by_key(|b| std::cmp::Reverse(b.0.len()));

            let bucket_costs: u32 = bucket_tasks
                .into_par_iter()
                .map(|(bucket, hash)| {
                    if let Some((cached_val, true)) = self.cache.get(hash) {
                        return cached_val;
                    }
                    let mut known_quick = 0;
                    if bucket.len() <= 15 {
                        let (q_val, is_exact) =
                            Solver::quick_bucket_eval(self.matrix, self.dict, bucket);
                        if is_exact {
                            self.cache.insert_set(hash, bucket, q_val, true);
                            cert_record!(14, bucket, q_val, true);
                            return q_val;
                        }
                        known_quick = q_val;
                    }
                    let mut solver = Solver::new(
                        self.matrix,
                        self.max_k,
                        self.dict,
                        self.metrics,
                        self.capacity_bounds_2d,
                        self.cache,
                        self.equiv_cache,
                    );
                    let bucket_greedy = Solver::greedy_solve(self.matrix, self.dict, bucket);
                    let bucket_k = bucket.len().min(self.max_k).max(2);
                    let val = solver.min_state_val_with_hash(
                        bucket,
                        hash,
                        self.all_guesses_bits,
                        bucket_greedy,
                        2,
                        bucket_k,
                        known_quick,
                    );
                    let exact_val = val.min(bucket_greedy);
                    self.cache.insert_set(hash, bucket, exact_val, true);
                    cert_record!(15, bucket, exact_val, true);
                    exact_val
                })
                .sum();

            let first_guess_cost = base_cost + bucket_costs;
            self.metrics
                .root_guesses_done
                .fetch_add(1, Ordering::Relaxed);

            let global_beta = AtomicU32::new(first_guess_cost);
            let global_best = AtomicU64::new((first_guess_cost as u64) << 32);

            let filtered_active: Vec<(usize, usize, u32)> = active[1..]
                .iter()
                .enumerate()
                .filter(|&(_, &(_, _, base_lb))| base_lb < first_guess_cost)
                .map(|(i, &(g, _, base_lb))| (i + 1, g, base_lb))
                .collect();

            const CHUNK_SIZE: usize = 4;
            let next = AtomicUsize::new(0);
            (0..rayon::current_num_threads())
                .into_par_iter()
                .with_max_len(1)
                .for_each(|_| {
                    let mut solver = Solver::new_with_global_beta(
                        self.matrix,
                        self.max_k,
                        self.dict,
                        self.metrics,
                        self.capacity_bounds_2d,
                        self.cache,
                        &global_beta,
                        self.equiv_cache,
                    );

                    loop {
                        let start_idx = next.fetch_add(CHUNK_SIZE, Ordering::Relaxed);
                        if start_idx >= filtered_active.len() {
                            break;
                        }
                        let end_idx = (start_idx + CHUNK_SIZE).min(filtered_active.len());

                        for i in start_idx..end_idx {
                            let (orig_idx, g, base_lb) =
                                unsafe { *filtered_active.get_unchecked(i) };

                            let best_packed = global_best.load(Ordering::Relaxed);
                            let best_val = (best_packed >> 32) as u32;
                            let best_idx = (best_packed & 0xFFFFFFFF) as usize;
                            let guess_beta = if orig_idx < best_idx {
                                best_val + 1
                            } else {
                                best_val
                            };

                            if base_lb >= guess_beta {
                                continue;
                            }

                            let v = solver.min_guess_val(
                                candidates,
                                self.all_guesses_bits,
                                g,
                                guess_beta,
                                1,
                                self.max_k,
                            );
                            self.metrics
                                .root_guesses_done
                                .fetch_add(1, Ordering::Relaxed);

                            if v < guess_beta {
                                let packed = ((v as u64) << 32) | (orig_idx as u64);
                                let old = global_best.fetch_min(packed, Ordering::Relaxed);
                                let old_val = (old >> 32) as u32;
                                if v < old_val {
                                    global_beta.fetch_min(v + 1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                });

            let final_best = global_best.load(Ordering::Relaxed);
            let best_val = (final_best >> 32) as u32;
            let best_idx = (final_best & 0xFFFFFFFF) as u32;

            let best_guess = if best_idx == u32::MAX {
                usize::MAX
            } else {
                active[best_idx as usize].0
            };

            return (best_guess, best_val);
        }

        let initial_best_val = Solver::greedy_solve(self.matrix, self.dict, candidates);

        // For very small candidate sets, avoid thread overhead
        if candidates.len() <= 10 {
            let mut solver = Solver::new(
                self.matrix,
                self.max_k,
                self.dict,
                self.metrics,
                self.capacity_bounds_2d,
                self.cache,
                self.equiv_cache,
            );
            let mut best_val = initial_best_val;
            let mut best_guess = usize::MAX;
            for &(g, _, base_lb) in &active {
                let guess_beta = if best_guess == usize::MAX {
                    best_val + 1
                } else {
                    best_val
                };
                if base_lb >= guess_beta {
                    continue;
                }
                let v = solver.min_guess_val(
                    candidates,
                    self.all_guesses_bits,
                    g,
                    guess_beta,
                    1,
                    self.max_k,
                );
                self.metrics
                    .root_guesses_done
                    .fetch_add(1, Ordering::Relaxed);
                if v < best_val {
                    best_val = v;
                    best_guess = g;
                } else if v == best_val && best_guess == usize::MAX {
                    best_guess = g;
                }
            }
            return (best_guess, best_val);
        }

        let global_beta = AtomicU32::new(initial_best_val + 1);
        let global_best = AtomicU64::new(((initial_best_val as u64) << 32) | (u32::MAX as u64));

        // Dynamic, rank-ordered claiming: every worker repeatedly takes the
        // next-best-ranked unevaluated guess from a shared counter. The
        // guesses' costs are wildly uneven, so a static one-chunk-per-thread
        // split left most cores idle while a few long evaluations finished
        // (measured: busy cores fell from ~13 to ~1 over the root scan), and
        // it also let the late chunks run with a loose beta while only the
        // first chunk held the best-ranked guesses. Claiming in rank order
        // keeps all workers on the most promising guesses first, so the
        // bound tightens as early as possible.
        //
        // The outcome is independent of evaluation order: a guess whose true
        // cost is <= the best is always evaluated exactly (beta is always
        // best + 1 >= its cost + 1 at that point), and the packed
        // (cost, index) minimum breaks ties by heuristic rank.
        let next = AtomicUsize::new(0);
        (0..rayon::current_num_threads())
            .into_par_iter()
            .with_max_len(1)
            .for_each(|_| {
                let mut solver = Solver::new_with_global_beta(
                    self.matrix,
                    self.max_k,
                    self.dict,
                    self.metrics,
                    self.capacity_bounds_2d,
                    self.cache,
                    &global_beta,
                    self.equiv_cache,
                );

                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(g, _, base_lb)) = active.get(idx) else {
                        break;
                    };

                    let best_packed = global_best.load(Ordering::Relaxed);
                    let best_val = (best_packed >> 32) as u32;
                    let best_idx = (best_packed & 0xFFFFFFFF) as usize;
                    let guess_beta = if idx < best_idx {
                        best_val + 1
                    } else {
                        best_val
                    };

                    if base_lb >= guess_beta {
                        continue;
                    }

                    let v = solver.min_guess_val(
                        candidates,
                        self.all_guesses_bits,
                        g,
                        guess_beta,
                        1,
                        self.max_k,
                    );
                    self.metrics
                        .root_guesses_done
                        .fetch_add(1, Ordering::Relaxed);

                    if v < guess_beta {
                        let packed = ((v as u64) << 32) | (idx as u64);
                        let old = global_best.fetch_min(packed, Ordering::Relaxed);
                        let old_val = (old >> 32) as u32;
                        if v < old_val {
                            global_beta.fetch_min(v + 1, Ordering::Relaxed);
                        }
                    }
                }
            });

        let final_best = global_best.load(Ordering::Relaxed);
        let best_val = (final_best >> 32) as u32;
        let best_idx = (final_best & 0xFFFFFFFF) as u32;

        let best_guess = if best_idx == u32::MAX {
            usize::MAX
        } else {
            active[best_idx as usize].0
        };

        (best_guess, best_val)
    }
}

impl CandidatesPolicy for OptimalPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        self.guess_and_cost(candidates).0
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Live counters the builder updates as it works, so a caller can watch a long
/// build instead of seeing nothing until it finishes: the root scan alone can
/// run for hours, and the builder's own periodic sampling only happens at node
/// boundaries, which that scan never reaches until it completes.
///
/// A caller-owned `Arc<BuildObserver>` passed through [`BuildOptions`] is
/// updated in place (the caller keeps its own handle for a progress thread).
/// When none is passed, the builder uses a private one and the counters are
/// simply never read.
#[derive(Debug, Default)]
pub struct BuildObserver {
    /// The same instrumentation counters `Solver` uses, so live search
    /// throughput (`states_evaluated`, `guesses_evaluated`, `cache_hits`, ...)
    /// is visible during a long node evaluation.
    pub metrics: Metrics,
    /// Tree-shape counters, updated at node boundaries.
    pub tree: LiveTreeStats,
}

/// Tree-shape counters, updated at node boundaries during a build. Before the
/// first node completes these are the root's initial values (1 node, no edges).
#[derive(Debug, Default)]
pub struct LiveTreeStats {
    pub nodes: AtomicUsize,
    pub edges: AtomicUsize,
    pub frontier: AtomicUsize,
    pub depth: AtomicUsize,
    pub leaves: AtomicUsize,
}

/// Options controlling a tree build.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Print periodic progress to stderr.
    pub progress: bool,
    pub progress_interval: Duration,
    /// Transposition-table size (entries; must be a power of two).
    pub cache_entries: usize,
    /// Optional caller-owned observer, updated in place while the build runs.
    pub observer: Option<Arc<BuildObserver>>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            progress: true,
            progress_interval: Duration::from_secs(2),
            cache_entries: 1 << 22,
            observer: None,
        }
    }
}

/// Final totals for a build.
#[derive(Debug, Clone)]
pub struct BuildSummary {
    pub nodes: u64,
    pub edges: u64,
    /// Structural terminal nodes (no children). A tree has fewer of these than
    /// candidates: a candidate can also be won at an internal node whose guess
    /// is one of several remaining candidates.
    pub leaves: u64,
    /// Number of (node, candidate) wins - one per candidate, so this must equal
    /// the candidate count for a complete tree.
    pub wins: u64,
    pub max_depth: usize,
    pub total_cost: u64,
    pub total_ms: f64,
    pub pick_ms_total: f64,
    pub states_evaluated: u64,
    pub guesses_evaluated: u64,
    pub cache_hits: u64,
    pub pruned_by_bounds: u64,
    pub pruned_by_equivalence: u64,
}

/// Result of a build.
pub struct BuildStats {
    pub summary: BuildSummary,
}

/// Builds a complete policy tree for `root_candidates` using `strategy`.
pub fn build_policy_tree(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    strategy: Strategy,
    root_candidates: &[usize],
    opts: &BuildOptions,
) -> Result<(PolicyTree, BuildStats), PolicyError> {
    if dict.guesses.len() > u16::MAX as usize {
        return Err(PolicyError::TooManyGuesses {
            count: dict.guesses.len(),
        });
    }
    let max_k = heuristic::compute_max_branching_factor(matrix, root_candidates);
    let mut capacity_bounds_2d = vec![vec![0u32; root_candidates.len() + 1]; max_k + 1];
    for k in 2..=max_k {
        for i in 0..=root_candidates.len() {
            capacity_bounds_2d[k][i] = heuristic::capacity_bound(i, k);
        }
    }
    let observer = opts.observer.clone().unwrap_or_default();
    let metrics = &observer.metrics;
    let global_cache = GlobalCache::new(opts.cache_entries);
    let equiv_cache: EquivCache =
        std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_guesses_bits = vec![u64::MAX; num_u64s];
    let rem = dict.guesses.len() % 64;
    if rem != 0 {
        all_guesses_bits[num_u64s - 1] = (1 << rem) - 1;
    }

    let optimal;
    let min_remaining;
    let max_freq;
    let policy: &dyn CandidatesPolicy = match strategy {
        Strategy::Optimal => {
            optimal = OptimalPolicy::new(
                matrix,
                dict,
                metrics,
                &global_cache,
                &equiv_cache,
                &capacity_bounds_2d,
                max_k,
                &all_guesses_bits,
            );
            &optimal
        }
        Strategy::MinRemaining => {
            min_remaining = MinRemainingPolicy::new(matrix);
            &min_remaining
        }
        Strategy::MaxFreq => {
            max_freq = MaxFreqPolicy::new(matrix, dict);
            &max_freq
        }
    };

    let mut tree = PolicyTree::new(strategy.name(), dictionary_hash(dict), dict);
    let stats = build_into(
        &mut tree,
        matrix,
        dict,
        policy,
        root_candidates,
        opts,
        &observer,
    )?;
    Ok((tree, stats))
}

/// DFS construction of the arena. Uses an explicit stack (not recursion) so a
/// pathologically deep tree can't overflow the stack.
#[allow(clippy::too_many_arguments)]
fn build_into(
    tree: &mut PolicyTree,
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    policy: &dyn CandidatesPolicy,
    root_candidates: &[usize],
    opts: &BuildOptions,
    observer: &BuildObserver,
) -> Result<BuildStats, PolicyError> {
    let metrics = &observer.metrics;
    let start = Instant::now();
    let mut stats = BuildStats {
        summary: BuildSummary {
            nodes: 0,
            edges: 0,
            leaves: 0,
            wins: 0,
            max_depth: 0,
            total_cost: 0,
            total_ms: 0.0,
            pick_ms_total: 0.0,
            states_evaluated: 0,
            guesses_evaluated: 0,
            cache_hits: 0,
            pruned_by_bounds: 0,
            pruned_by_equivalence: 0,
        },
    };
    let mut last_report = Instant::now();

    let root = tree.nodes.len() as u32;
    tree.nodes.push(PolicyNode {
        guess: 0,
        edge_start: 0,
        edge_count: 0,
    });
    tree.root = root;
    observer
        .tree
        .nodes
        .store(tree.nodes.len(), Ordering::Relaxed);
    // (node index, depth, response into node, candidates)
    let mut stack: Vec<(u32, usize, i32, Vec<usize>)> =
        vec![(root, 0, -1, root_candidates.to_vec())];
    // Edges are collected as (parent, response, child) and laid out
    // contiguously per node (ascending id, ascending response) at the end, so
    // the arena matches the canonical layout regardless of DFS order.
    let mut pending_edges: Vec<(u32, u16, u32)> = Vec::new();

    // Reusable buckets array across iterations (indexed by response, 0..num_responses).
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); matrix.num_responses];

    while let Some((node_idx, depth, _response, candidates)) = stack.pop() {
        if candidates.is_empty() {
            return Err(PolicyError::EmptyCandidateSet { depth });
        }
        stats.summary.max_depth = stats.summary.max_depth.max(depth);

        // A singleton is always won by guessing the remaining candidate; this
        // bypasses the policy so a heuristic can't loop forever here.
        let pick_start = Instant::now();
        let guess = if candidates.len() == 1 {
            dict.candidate_to_guess[candidates[0]]
        } else {
            policy.guess(&candidates)
        };
        stats.summary.pick_ms_total += pick_start.elapsed().as_secs_f64() * 1000.0;
        if guess >= dict.guesses.len() {
            return Err(PolicyError::GuessOutOfRange { guess });
        }

        if candidates.len() == 1 {
            let node = &mut tree.nodes[node_idx as usize];
            node.guess = guess as u16;
            node.edge_count = 0;
            stats.summary.leaves += 1;
            stats.summary.wins += 1;
            stats.summary.total_cost += depth as u64 + 1;
        } else {
            // Partition candidates by response.
            for &c in &candidates {
                let r = matrix.get(guess, c).0 as usize;
                buckets[r].push(c);
            }

            // Non-win buckets become edges, in ascending response order.
            let win = matrix.win_response.0 as usize;
            let mut child_specs: Vec<(u16, Vec<usize>)> = Vec::new();
            for r in 0..matrix.num_responses {
                if r == win || buckets[r].is_empty() {
                    continue;
                }
                // A non-win bucket equal to the whole set means the guess did not
                // partition - the strategy would never terminate.
                if buckets[r].len() == candidates.len() {
                    return Err(PolicyError::UselessGuess {
                        guess,
                        candidates: candidates.len(),
                        depth,
                    });
                }
                child_specs.push((r as u16, std::mem::take(&mut buckets[r])));
            }

            let wins_here = buckets[win].len() as u64;
            buckets[win].clear();
            stats.summary.wins += wins_here;
            stats.summary.total_cost += wins_here * (depth as u64 + 1);

            if child_specs.is_empty() {
                // A structural leaf must be exactly a win.
                if candidates.len() != 1 || wins_here != 1 {
                    return Err(PolicyError::Shape(format!(
                        "node {node_idx} is a leaf but its guess does not win the {} remaining candidate(s)",
                        candidates.len()
                    )));
                }
                stats.summary.leaves += 1;
            }

            let mut child_ids = Vec::with_capacity(child_specs.len());
            for _ in &child_specs {
                child_ids.push(tree.nodes.len() as u32);
                tree.nodes.push(PolicyNode {
                    guess: 0,
                    edge_start: 0,
                    edge_count: 0,
                });
            }
            for (i, (r, _)) in child_specs.iter().enumerate() {
                pending_edges.push((node_idx, *r, child_ids[i]));
            }
            let node = &mut tree.nodes[node_idx as usize];
            node.guess = guess as u16;
            node.edge_count = child_specs.len() as u16;

            // Push children in reverse so ascending response order is processed first.
            for (i, (r, bucket)) in child_specs.into_iter().enumerate().rev() {
                stack.push((child_ids[i], depth + 1, r as i32, bucket));
            }
        }

        observer
            .tree
            .nodes
            .store(tree.nodes.len(), Ordering::Relaxed);
        observer
            .tree
            .edges
            .store(pending_edges.len(), Ordering::Relaxed);
        observer.tree.frontier.store(stack.len(), Ordering::Relaxed);
        observer.tree.depth.store(depth, Ordering::Relaxed);
        observer
            .tree
            .leaves
            .store(stats.summary.leaves as usize, Ordering::Relaxed);

        if opts.progress && last_report.elapsed() >= opts.progress_interval {
            last_report = Instant::now();
            eprintln!(
                "[tree] nodes={} edges={} frontier={} depth={} leaves={} elapsed={:.1}s pick_time={:.1}s",
                tree.nodes.len(),
                pending_edges.len(),
                stack.len(),
                depth,
                stats.summary.leaves,
                start.elapsed().as_secs_f64(),
                stats.summary.pick_ms_total / 1000.0,
            );
        }
    }

    // Canonical edge layout: contiguous per node, ascending node id then
    // ascending response. Edges for a given parent were pushed in ascending
    // response order, so a stable sort by parent preserves that.
    pending_edges.sort_by_key(|&(p, _, _)| p);
    tree.edges.clear();
    tree.edges.reserve(pending_edges.len());
    for (_, r, child) in &pending_edges {
        tree.edges.push(PolicyEdge {
            response: *r,
            child: *child,
        });
    }
    let mut cursor = 0u32;
    for n in tree.nodes.iter_mut() {
        n.edge_start = cursor;
        cursor += n.edge_count as u32;
    }

    observer
        .tree
        .nodes
        .store(tree.nodes.len(), Ordering::Relaxed);
    observer
        .tree
        .edges
        .store(tree.edges.len(), Ordering::Relaxed);
    observer.tree.frontier.store(0, Ordering::Relaxed);

    let m = &mut stats.summary;
    m.nodes = tree.nodes.len() as u64;
    m.edges = tree.edges.len() as u64;
    m.total_ms = start.elapsed().as_secs_f64() * 1000.0;
    m.states_evaluated = metrics.states_evaluated.load(Ordering::Relaxed) as u64;
    m.guesses_evaluated = metrics.guesses_evaluated.load(Ordering::Relaxed) as u64;
    m.cache_hits = metrics.cache_hits.load(Ordering::Relaxed) as u64;
    m.pruned_by_bounds = metrics.pruned_by_bounds.load(Ordering::Relaxed) as u64;
    m.pruned_by_equivalence = metrics.pruned_by_equivalence.load(Ordering::Relaxed) as u64;

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Response;

    fn dict_from(guesses: &[&str], candidates: &[&str]) -> Dictionary {
        Dictionary::from_words(
            guesses.iter().map(|w| crate::core::Word::new(w)).collect(),
            candidates
                .iter()
                .map(|w| crate::core::Word::new(w))
                .collect(),
        )
    }

    #[test]
    fn response_string_roundtrip() {
        for r in 0..243u16 {
            let s = response_to_string(r);
            assert_eq!(response_from_string(&s), Some(r));
        }
        assert_eq!(response_to_string(Response::WIN.0), "ggggg");
        assert_eq!(response_to_string(0), "bbbbb");
        assert_eq!(
            response_from_string("bgybg"),
            Some(Response::new(0, 1, 2, 0, 1).0)
        );
    }

    #[test]
    fn strategy_parse_roundtrip() {
        for s in Strategy::all() {
            assert_eq!(Strategy::parse(s.name()), Some(s));
        }
        assert_eq!(Strategy::parse("nonsense"), None);
    }

    #[test]
    fn dictionary_hash_is_stable_and_distinguishes() {
        let a = dict_from(&["abcde", "fghij"], &["abcde"]);
        let b = dict_from(&["abcde", "fghij"], &["abcde"]);
        let c = dict_from(&["abcde", "fghij"], &["fghij"]);
        assert_eq!(dictionary_hash(&a), dictionary_hash(&b));
        assert_ne!(dictionary_hash(&a), dictionary_hash(&c));
    }

    /// Build a tiny real dictionary (first N candidates of the shipped list).
    fn real_dict(n: usize) -> Dictionary {
        let full = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let candidates: Vec<crate::core::Word> = full.candidates[..n].to_vec();
        Dictionary::from_words(full.guesses.clone(), candidates)
    }

    #[test]
    fn heuristic_trees_build_and_validate() {
        let dict = real_dict(12);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        for strategy in [Strategy::MinRemaining, Strategy::MaxFreq] {
            let opts = BuildOptions {
                progress: false,
                ..Default::default()
            };
            let (tree, stats) = build_policy_tree(&matrix, &dict, strategy, &root, &opts).unwrap();
            let report = tree.validate(&matrix, &dict).unwrap();
            assert_eq!(report.wins, dict.candidates.len() as u64);
            assert_eq!(report.total_cost, stats.summary.total_cost);
            assert!(report.max_depth >= 1);
        }
    }

    /// Builds the exact optimal tree for the deterministic first-N candidate
    /// subset (the same convention the golden solver tests in `solver.rs`
    /// use), validates it, and returns the report plus what the caller needs
    /// to re-derive the solver's optimum for the same subset.
    fn build_and_validate_optimal_tree(n: usize) -> (Dictionary, ResponseMatrix, ValidationReport) {
        let dict = real_dict(n);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..n).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, stats) =
            build_policy_tree(&matrix, &dict, Strategy::Optimal, &root, &opts).unwrap();
        let report = tree.validate(&matrix, &dict).unwrap();
        assert_eq!(report.wins, n as u64);
        assert_eq!(stats.summary.total_cost, report.total_cost);
        (dict, matrix, report)
    }

    /// The exact correctness criterion for an optimal tree: it must be valid
    /// (every candidate terminates, and an edge exists iff its response is
    /// possible) and its total cost must equal the solver's exact optimal
    /// cost. Together those imply optimality, since no strategy can beat the
    /// solver's optimum. Checked at a non-trivial size as well as a tiny one,
    /// because a depth-3 tree exercises partitioning the tiny case does not.
    #[test]
    fn optimal_tree_cost_matches_solver() {
        for n in [10, 100] {
            let (dict, matrix, report) = build_and_validate_optimal_tree(n);
            let root: Vec<usize> = (0..n).collect();
            let metrics = Metrics::new();
            let equiv: EquivCache =
                std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
            let solver_cost = Solver::solve(&matrix, &root, &dict, &metrics, &equiv);
            assert_eq!(
                report.total_cost, solver_cost as u64,
                "optimal policy tree total cost must equal the solver's optimal cost at N={n}"
            );
        }
    }

    /// The optimal tree must not depend on how the per-node scan is scheduled.
    /// The scan claims guesses dynamically across however many worker threads
    /// exist, so evaluation order differs between runs and thread counts; the
    /// tree - including which of several equally-optimal guesses is chosen -
    /// must be byte-identical regardless.
    #[test]
    fn optimal_tree_is_independent_of_thread_count() {
        let n = 100;
        let dict = real_dict(n);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..n).collect();
        let build_json = |threads: usize| -> String {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                let opts = BuildOptions {
                    progress: false,
                    ..Default::default()
                };
                let (tree, _) =
                    build_policy_tree(&matrix, &dict, Strategy::Optimal, &root, &opts).unwrap();
                tree.to_readable(&dict).unwrap().to_json()
            })
        };
        let reference = build_json(1);
        for threads in [3, 8] {
            assert_eq!(
                build_json(threads),
                reference,
                "optimal tree differs between 1 and {threads} threads"
            );
        }
    }

    #[test]
    fn readable_roundtrip_is_deterministic() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (t1, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        let (t2, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        assert_eq!(
            t1.to_readable(&dict).unwrap().to_json(),
            t2.to_readable(&dict).unwrap().to_json()
        );

        let json = t1.to_readable(&dict).unwrap().to_json();
        let parsed = ReadableTreeFile::from_json(&json).unwrap();
        let back = parsed.to_tree().unwrap();
        assert_eq!(back.to_readable(&dict).unwrap().to_json(), json);
        back.validate(&matrix, &dict).unwrap();
    }

    #[test]
    fn readable_roundtrip_validates() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        let readable = tree.to_readable(&dict).unwrap();
        let json = readable.to_json();
        let parsed = ReadableTreeFile::from_json(&json).unwrap();
        let embedded = parsed.embedded_dictionary().unwrap();
        let back = parsed.to_tree().unwrap();
        back.validate(&matrix, &embedded).unwrap();
        assert_eq!(
            back.to_readable(&embedded).unwrap().to_json(),
            tree.to_readable(&dict).unwrap().to_json()
        );
    }

    #[test]
    fn validator_rejects_tampering() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        tree.validate(&matrix, &dict).unwrap();

        // 1. Missing an edge.
        let mut missing = tree.clone();
        let n0 = missing.nodes[missing.root as usize];
        let last = n0.edge_start as usize + n0.edge_count as usize - 1;
        missing.edges.remove(last);
        missing.nodes[missing.root as usize].edge_count -= 1;
        assert!(matches!(
            missing.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 2. An impossible response on an edge.
        let mut impossible = tree.clone();
        let n0 = impossible.nodes[impossible.root as usize];
        let start = n0.edge_start as usize;
        let mut used: Vec<u16> = impossible.edges[start..start + n0.edge_count as usize]
            .iter()
            .map(|e| e.response)
            .collect();
        let spare = (0..243u16)
            .find(|r| *r != WIN_RESPONSE && !used.contains(r))
            .unwrap();
        used.clear();
        impossible.edges[start].response = spare;
        assert!(matches!(
            impossible.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 3. Orphan node.
        let mut orphan = tree.clone();
        orphan.nodes.push(PolicyNode {
            guess: 0,
            edge_start: 0,
            edge_count: 0,
        });
        assert!(matches!(
            orphan.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 4. Wrong dictionary hash.
        let mut wrong_hash = tree.clone();
        wrong_hash.dictionary_hash ^= 1;
        assert!(matches!(
            wrong_hash.validate(&matrix, &dict),
            Err(PolicyError::BadHeader(_))
        ));
    }
}
