use crate::core::Response;
use crate::heuristic;
use crate::matrix::ResponseMatrix;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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
    pub max_depth: AtomicUsize,
    pub guesses_evaluated: AtomicUsize,
    pub pruned_by_bounds: AtomicUsize,
    pub pruned_by_equivalence: AtomicUsize,
    pub cache_hits: AtomicUsize,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            states_evaluated: AtomicUsize::new(0),
            max_depth: AtomicUsize::new(0),
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
    capacity_bounds: &'a [u32],
    pub seen_projections: rustc_hash::FxHashSet<u32>,
    pub cache: &'a crate::cache::GlobalCache,
}

impl<'a> Solver<'a> {
    /// A fast, non-optimal greedy solver used exclusively to seed the initial `beta` upper bound.
    /// It recursively selects the guess with the lowest Expected Remaining Candidates heuristic,
    /// generating a highly efficient (though mathematically suboptimal) decision tree.
    pub fn greedy_solve(
        matrix: &ResponseMatrix,
        dict: &'a crate::dict::Dictionary,
        set: &[usize],
    ) -> u32 {
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

            let useless = num_non_empty == 1;
            if useless {
                continue;
            }

            let mut expected_rem = 0u32;
            for &count in &counts {
                expected_rem += (count as u32) * (count as u32);
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
            cost += Self::greedy_solve(matrix, dict, &subset);
        }
        cost
    }
    pub fn new(
        matrix: &'a ResponseMatrix,
        max_k: usize,
        dict: &'a crate::dict::Dictionary,
        metrics: &'a Metrics,
        capacity_bounds: &'a [u32],
        cache: &'a crate::cache::GlobalCache,
    ) -> Self {
        Self {
            matrix,
            max_k,
            dict,
            metrics,
            seen_projections: rustc_hash::FxHashSet::default(),
            capacity_bounds,
            cache,
        }
    }

    /// Solves the given candidate subset to minimize the total expected guesses.
    ///
    /// Evaluates all initial guesses in parallel using Rayon, sharing the global
    /// best upper bound (`beta`) atomically for heavy cross-thread pruning.
    pub fn solve(
        matrix: &'a ResponseMatrix,
        initial_candidates: &[usize],
        dict: &'a crate::dict::Dictionary,
        metrics: &'a Metrics,
    ) -> u32 {
        let is_compute = std::fs::read_to_string("/etc/hostname").map(|s| s.trim() == "ubuntu-main" || s.trim() == "compute").unwrap_or(false);
        let cache_size = if is_compute { 512 * 1024 * 1024 } else { 64 * 1024 * 1024 };
        let global_cache = crate::cache::GlobalCache::new(cache_size);
        let max_k = heuristic::compute_max_branching_factor(matrix, initial_candidates);

        let mut guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(matrix, initial_candidates, &mut guesses);

        let set = initial_candidates;

        let initial_greedy_cost = Self::greedy_solve(matrix, dict, initial_candidates);
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
            if !useless {
                active_guesses.push(g);
            }
        }

        heuristic::sort_guesses_by_expected_remaining(matrix, set, &mut active_guesses);

        let mut capacity_bounds = Vec::with_capacity(dict.candidates.len() + 1);
        for i in 0..=dict.candidates.len() {
            capacity_bounds.push(heuristic::capacity_bound(i, max_k));
        }

        if !active_guesses.is_empty() {
            let first_g = active_guesses[0];

            // To prevent 7 cores from sitting idle while evaluating the massive first guess,
            // we parallelize its buckets! Since this is the best guess, it's very unlikely to be pruned,
            // so we don't lose much alpha-beta efficiency by evaluating buckets in parallel.

            let mut counts = [0u16; 243];
            let mut non_empty_indices = [0u8; 243];
            let mut num_non_empty = 0;

            for &c in set {
                let r_idx = matrix.get(first_g, c).0 as usize;
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
            let mut sorted_set = vec![0usize; set.len()];
            let mut current_offsets = offsets;
            for &c in set {
                let r_idx = matrix.get(first_g, c).0 as usize;
                let pos = current_offsets[r_idx];
                sorted_set[pos] = c;
                current_offsets[r_idx] += 1;
            }

            let mut bucket_tasks = Vec::new();
            let mut base_cost = set.len() as u32;
            for i in 0..num_non_empty {
                let r_idx = non_empty_indices[i] as usize;
                let p_len = counts[r_idx] as usize;
                if r_idx == crate::core::Response::WIN.0 as usize {
                    
                    continue;
                }
                if p_len <= 2 {
                    let lb = heuristic::capacity_bound(p_len, max_k);
                    base_cost += lb;
                    continue;
                }
                let start = offsets[r_idx];
                let end = start + p_len;
                bucket_tasks.push(sorted_set[start..end].to_vec());
            }

            bucket_tasks.sort_unstable_by_key(|b| std::cmp::Reverse(b.len()));

            let bucket_costs: u32 = bucket_tasks
                .into_par_iter()
                .map(|bucket| {
                    let mut solver = Solver::new(
                        matrix,
                        max_k,
                        dict,
                        metrics,
                        &capacity_bounds,
                        &global_cache,
                    );
                    // For a bucket, the cost is evaluated via min_state_val.
                    // We use a very loose beta since we evaluate in parallel.
                    solver.min_state_val(&bucket, &active_guesses, initial_greedy_cost, 2)
                })
                .sum();

            let val = base_cost + bucket_costs;
            beta.fetch_min(val, Ordering::Relaxed);

            // Now evaluate the remaining guesses in parallel with the tight beta
            active_guesses[1..].par_iter().for_each(|&g| {
                let current_beta = beta.load(Ordering::Relaxed);
                let mut local_solver = Solver::new(
                    matrix,
                    max_k,
                    dict,
                    metrics,
                    &capacity_bounds,
                    &global_cache,
                );
                let val = local_solver.min_guess_val(set, g, &active_guesses, current_beta, 1);

                // atomic min
                let mut current = beta.load(Ordering::Relaxed);
                while val < current {
                    match beta.compare_exchange_weak(
                        current,
                        val,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => current = actual,
                    }
                }
            });
        }

        beta.load(Ordering::Relaxed)
    }

    /// Computes the minimum expected cost to solve a subset of candidates using ANY valid guess.
    ///
    /// This function performs the top-level iteration over all available `allowed_guesses`.
    /// To maximize alpha-beta pruning, guesses are first evaluated heuristically and sorted
    /// by their expected capacity. We also aggressively prune symmetrically equivalent guesses
    /// using a bitwise projection filter `seen_projections`.
    /// Evaluates the minimum expected cost of the entire state given a set of candidates.
    ///
    /// This function acts as the `Max` node in the Min-Max tree (maximizing our efficiency/
    /// minimizing the expected cost). It uses several heavy optimization techniques:
    /// 1. **Transposition Table (Cache)**: Caches deep identical subtrees using a lock-free thread-local Zobrist hash.
    /// 2. **Alpha-Beta Bounds Pruning**: Uses `global_lb` and dynamically computes `local_lb` to instantly prune search if the mathematical optimum is reached.
    /// 3. **Young Brothers Wait Concept (YBWC)**: For `depth == 1`, evaluates the most promising root guess sequentially to establish a strict bound, then evaluates the rest in parallel using Rayon.
    /// 4. **Equivalence Class Projection**: Skips identical guesses using a bitwise character projection and an O(1) generation array.
    fn min_state_val(
        &mut self,
        set: &[usize],
        allowed_guesses: &[usize],
        beta: u32,
        depth: usize,
    ) -> u32 {
        self.metrics
            .max_depth
            .fetch_max(depth, std::sync::atomic::Ordering::Relaxed);

        let mut hash = 0;
        for &c in set {
            hash ^= self.matrix.zobrist[c];
        }

        if let Some((value, is_exact)) = self.cache.get(hash) {
            self.metrics
                .cache_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if is_exact {
                return value;
            }
            if value >= beta {
                return beta;
            }
        }

        self.metrics
            .states_evaluated
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

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

        let global_lb = self.capacity_bounds[c_len];
        if global_lb >= beta {
            return beta;
        }

        let mut best_val = beta;

        let mut active_tuples = Vec::with_capacity(allowed_guesses.len());
        let mut c_mask = 0u32;
        for &c in set {
            c_mask |= self.matrix.candidate_masks[c];
        }

        self.seen_projections.clear();
        let mut local_max_k = 0;
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
            let mut non_empty = [0u8; 243];
            let mut num_non_empty = 0;
            for &c in set {
                let r = self.matrix.get(g, c).0 as usize;
                if counts[r] == 0 {
                    non_empty[num_non_empty] = r as u8;
                    num_non_empty += 1;
                }
                counts[r] += 1;
            }

            if num_non_empty > local_max_k {
                local_max_k = num_non_empty;
            }

            if num_non_empty == 1 {
                continue;
            }

            let mut expected_rem = 0u32;
            let mut lb_cost = set.len() as u32;
            for i in 0..num_non_empty {
                let r_idx = non_empty[i] as usize;
                let count = counts[r_idx];
                expected_rem += (count as u32) * (count as u32);
                if r_idx != crate::core::Response::WIN.0 as usize {
                    lb_cost += self.capacity_bounds[count as usize];
                }
            }

            active_tuples.push((g, expected_rem, lb_cost));
        }
        self.metrics
            .pruned_by_equivalence
            .fetch_add(equiv_pruned, std::sync::atomic::Ordering::Relaxed);

        active_tuples.sort_unstable_by_key(|&(_, exp, _)| exp);

        let local_lb = heuristic::capacity_bound(c_len, local_max_k);
        if local_lb >= beta {
            return beta;
        }

        let active_guesses: Vec<usize> = active_tuples.iter().map(|&(g, _, _)| g).collect();

        if depth == 1 && active_guesses.len() > 1 {
            let shared_best = std::sync::atomic::AtomicU32::new(best_val);
            let first_g = active_guesses[0];
            let val = self.min_guess_val(
                set,
                first_g,
                &active_guesses,
                shared_best.load(std::sync::atomic::Ordering::Relaxed),
                depth,
            );
            shared_best.fetch_min(val, std::sync::atomic::Ordering::Relaxed);

            if shared_best.load(std::sync::atomic::Ordering::Relaxed) <= local_lb {
                best_val = shared_best.load(std::sync::atomic::Ordering::Relaxed);
            } else {
                use rayon::prelude::*;
                active_tuples[1..].par_iter().for_each(|&(g, _, g_lb)| {
                    let current_best = shared_best.load(std::sync::atomic::Ordering::Relaxed);
                    if g_lb >= current_best {
                        self.metrics.pruned_by_bounds.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    let current_best = shared_best.load(std::sync::atomic::Ordering::Relaxed);
                    if current_best <= local_lb {
                        return;
                    }

                    let mut local_solver = Solver::new(
                        self.matrix,
                        self.max_k,
                        self.dict,
                        self.metrics,
                        self.capacity_bounds,
                        self.cache,
                    );

                    let val =
                        local_solver.min_guess_val(set, g, &active_guesses, current_best, depth);
                    shared_best.fetch_min(val, std::sync::atomic::Ordering::Relaxed);
                });
                best_val = shared_best.load(std::sync::atomic::Ordering::Relaxed);
            }
        } else {
            for &(g, _, g_lb) in &active_tuples {
                if g_lb >= best_val {
                    self.metrics.pruned_by_bounds.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }
                let val = self.min_guess_val(set, g, &active_guesses, best_val, depth);
                if val < best_val {
                    best_val = val;
                    if best_val <= local_lb {
                        break;
                    }
                }
            }
        }

        if best_val < beta {
            self.cache.insert(hash, best_val, true);
        } else {
            self.cache.insert(hash, beta, false);
        }
        best_val
    }

    /// Evaluates the true cost of making a specific `guess` given the current `set` of candidates.
    ///
    /// This mathematically partitions the candidates into up to 243 ternary response buckets.
    /// It recursively queries `min_state_val` on each sub-bucket. Alpha-beta pruning is applied
    /// at the bucket level: if the cumulative cost of resolved buckets plus the theoretical
    /// heuristic minimum cost of the remaining unresolved buckets exceeds `beta`, evaluation
    /// is immediately aborted.
    fn min_guess_val(
        &mut self,
        set: &[usize],
        guess: usize,
        allowed_guesses: &[usize],
        beta: u32,
        depth: usize,
    ) -> u32 {
        self.metrics
            .guesses_evaluated
            .fetch_add(1, Ordering::Relaxed);
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

        // CRITICAL OPTIMIZATION: Evaluate largest buckets first.
        // Large buckets have a higher probability of exceeding their heuristic minimum bounds.
        // By evaluating them first, we can rapidly tighten our accumulated cost and trigger
        // an Alpha-Beta cutoff (cost >= beta) before wasting time evaluating the smaller buckets.
        // Benchmarks show this sorting step halves the total number of evaluated states.
        non_empty_indices[0..num_non_empty]
            .sort_unstable_by_key(|&r| std::cmp::Reverse(counts[r as usize]));

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
            self.metrics
                .pruned_by_bounds
                .fetch_add(1, Ordering::Relaxed);
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

            let val = self.min_state_val(p, allowed_guesses, new_beta, depth + 1);
            if b + val >= beta {
                return beta;
            }
            cost = b + val;
        }

        cost
    }
}

// Note: GlobalCache now stores both Exact values (when a full search completes)
// and Lower Bounds (when a search fails high against `beta`).
// This implements Fail-Hard Alpha-Beta Transposition Table Pruning, ensuring that
// if we revisit a state with a `beta` that is <= a previously established lower bound,
// we can instantly prune the subtree and return `beta`, avoiding massive redundant deep searches.
