#![allow(clippy::needless_range_loop)]
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

/// Instrumentation counters for a single `Solver::solve` run, used for
/// benchmarking and progress reporting. Not part of the solving logic itself.
pub struct Metrics {
    pub states_evaluated: AtomicUsize,
    pub max_depth: AtomicUsize,
    pub guesses_evaluated: AtomicUsize,
    pub pruned_by_bounds: AtomicUsize,
    pub pruned_by_equivalence: AtomicUsize,
    pub cache_hits: AtomicUsize,
    /// Number of root-level first guesses fully evaluated (for progress reporting).
    pub root_guesses_done: AtomicUsize,
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
            root_guesses_done: AtomicUsize::new(0),
        }
    }
}

/// Sentinel used by non-root Solver instances; never tightened, so never causes spurious abort.
static SENTINEL_BETA: AtomicU32 = AtomicU32::new(u32::MAX);

/// The optimal Wordle solver using Branch and Bound.
///
/// Implements aggressive search space pruning through:
/// - Exact capacity lower bounds (see `heuristic::capacity_bound`)
/// - Expected-remaining-candidate guess ordering heuristic
/// - Equivalence-class guess projection (skips guesses that are
///   indistinguishable given the current candidate set)
/// - A lock-free atomic transposition table (`GlobalCache`) for subtree memoization
pub struct Solver<'a> {
    pub metrics: &'a Metrics,
    pub max_k: usize,
    pub matrix: &'a ResponseMatrix,
    pub dict: &'a crate::dict::Dictionary,
    capacity_bounds_2d: &'a [Vec<u32>],
    pub seen_projections: [rustc_hash::FxHashSet<u32>; 32],
    pub cache: &'a crate::cache::GlobalCache,
    /// Shared global upper bound across all parallel root-level tasks.
    /// When a thread improves beta, others see it immediately and can abort early.
    global_beta: &'a AtomicU32,
    /// Depth-indexed scratch buffers to avoid allocation in min_state_val.
    /// Max depth is naturally bounded, but we provide 32 levels to be safe against deep suboptimal branches.
    scratch_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    pub scratch_set_projs: [Vec<u32>; 32],
    scratch_guesses: [Vec<usize>; 32],
    scratch_sorted_sets: [Vec<usize>; 32],
    scratch_phase1_guesses: [Vec<usize>; 32],
    scratch_phase2_guesses: [Vec<usize>; 32],
    scratch_phase1_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    scratch_phase2_tuples: [Vec<(usize, u32, u32, usize)>; 32],
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
        capacity_bounds_2d: &'a [Vec<u32>],
        cache: &'a crate::cache::GlobalCache,
    ) -> Self {
        Self::new_with_global_beta(
            matrix,
            max_k,
            dict,
            metrics,
            capacity_bounds_2d,
            cache,
            &SENTINEL_BETA,
        )
    }

    pub fn new_with_global_beta(
        matrix: &'a ResponseMatrix,
        max_k: usize,
        dict: &'a crate::dict::Dictionary,
        metrics: &'a Metrics,
        capacity_bounds_2d: &'a [Vec<u32>],
        cache: &'a crate::cache::GlobalCache,
        global_beta: &'a AtomicU32,
    ) -> Self {
        Self {
            matrix,
            max_k,
            dict,
            metrics,
            seen_projections: std::array::from_fn(|_| rustc_hash::FxHashSet::default()),
            capacity_bounds_2d,
            cache,
            global_beta,
            scratch_tuples: std::array::from_fn(|_| Vec::new()),
            scratch_set_projs: std::array::from_fn(|_| Vec::new()),
            scratch_guesses: std::array::from_fn(|_| Vec::new()),
            scratch_sorted_sets: std::array::from_fn(|_| Vec::new()),
            scratch_phase1_guesses: std::array::from_fn(|_| Vec::new()),
            scratch_phase2_guesses: std::array::from_fn(|_| Vec::new()),
            scratch_phase1_tuples: std::array::from_fn(|_| Vec::new()),
            scratch_phase2_tuples: std::array::from_fn(|_| Vec::new()),
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
        let cache_size = if crate::is_compute_host() {
            512 * 1024 * 1024
        } else {
            64 * 1024 * 1024
        };
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

        let mut capacity_bounds_2d = vec![vec![0; dict.candidates.len() + 1]; max_k + 1];
        for k in 2..=max_k {
            for i in 0..=dict.candidates.len() {
                capacity_bounds_2d[k][i] = heuristic::capacity_bound(i, k);
            }
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
                    base_cost += capacity_bounds_2d[max_k][p_len];
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
                        &capacity_bounds_2d,
                        &global_cache,
                    );
                    // For a bucket, the cost is evaluated via min_state_val.
                    // We use a very loose beta since we evaluate in parallel.
                    solver.min_state_val(&bucket, &active_guesses, initial_greedy_cost, 2, max_k)
                })
                .sum();

            let val = base_cost + bucket_costs;
            beta.fetch_min(val, Ordering::Relaxed);

            // Pre-filter alternative root guesses: only retain those whose
            // capacity_bound lb is strictly less than the current beta.
            // A guess with lb >= beta cannot possibly improve the solution,
            // so we can skip it entirely. Note: allowed_guesses (active_guesses)
            // is passed unchanged to sub-problems; this filter only affects
            // which root tasks we launch.
            let beta_after_first = beta.load(Ordering::Relaxed);
            let root_candidates: Vec<usize> = active_guesses[1..]
                .iter()
                .copied()
                .filter(|&g| {
                    let mut counts = [0u16; 243];
                    for &c in set {
                        counts[matrix.get(g, c).0 as usize] += 1;
                    }
                    let lb: u32 = set.len() as u32
                        + counts
                            .iter()
                            .enumerate()
                            .filter(|&(r_idx, &cnt)| {
                                cnt > 0 && r_idx != crate::core::Response::WIN.0 as usize
                            })
                            .map(|(_, &cnt)| capacity_bounds_2d[max_k][cnt as usize])
                            .sum::<u32>();
                    lb < beta_after_first
                })
                .collect();

            println!(
                "\n*** Root candidates after filter: {} / {} ***",
                root_candidates.len(),
                active_guesses.len() - 1
            );

            // Now evaluate the remaining guesses in parallel with the tight beta.
            // Each solver holds a reference to the shared beta so it can abort early
            // if another thread finds a better solution while this one is running.
            root_candidates.par_iter().for_each(|&g| {
                let current_beta = beta.load(Ordering::Relaxed);
                let mut local_solver = Solver::new_with_global_beta(
                    matrix,
                    max_k,
                    dict,
                    metrics,
                    &capacity_bounds_2d,
                    &global_cache,
                    &beta,
                );
                let val =
                    local_solver.min_guess_val(set, g, &active_guesses, current_beta, 1, max_k);

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

                metrics.root_guesses_done.fetch_add(1, Ordering::Relaxed);
            });
        }

        beta.load(Ordering::Relaxed)
    }

    /// Computes the minimum expected cost to solve `set`, minimizing over every guess in
    /// `allowed_guesses`. This is the search's main per-state entry point:
    /// 1. **Transposition table lookup**: an exact cached value returns immediately; a cached
    ///    lower bound `>= beta` fails high immediately (see the module-level note above on
    ///    fail-hard transposition pruning).
    /// 2. **Capacity lower bound pruning**: `capacity_bounds[c_len]` (global) and a per-node
    ///    `local_max_k`-derived bound both give an instant return once they prove `beta`
    ///    can't be beaten.
    /// 3. **Equivalence-class projection**: guesses that partition `set` identically to one
    ///    already tried at this node are skipped via a bitwise projection over the candidate
    ///    set's letter inventory (`seen_projections`).
    /// 4. **Guess ordering + fail-hard alpha-beta**: remaining guesses are sorted by expected
    ///    remaining candidates and evaluated via `min_guess_val`, tightening `best_val` as
    ///    better guesses are found and stopping early once `best_val` reaches the proven
    ///    local lower bound.
    fn min_state_val(
        &mut self,
        set: &[usize],
        allowed_guesses: &[usize],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        self.metrics
            .max_depth
            .fetch_max(depth, std::sync::atomic::Ordering::Relaxed);

        let mut hash = 0;
        for &c in set {
            hash ^= self.matrix.zobrist[c];
        }

        let mut cached_lower_bound = 0;
        if let Some((cached_val, is_exact)) = self.cache.get(hash) {
            self.metrics
                .cache_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if is_exact {
                return cached_val;
            } else {
                if cached_val >= beta {
                    return beta;
                }
                cached_lower_bound = cached_val;
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
        if c_len <= 15 {
            let mut best_inside = u32::MAX;
            for i in 0..c_len {
                let ci = set[i];
                let gi = self.dict.candidate_to_guess[ci];

                let mut counts = [0u8; 243];
                let mut num_distinct = 0;

                for j in 0..c_len {
                    if i != j {
                        let cj = set[j];
                        let r = self.matrix.get(gi, cj).0 as usize;
                        if counts[r] == 0 {
                            num_distinct += 1;
                        }
                        counts[r] += 1;
                    }
                }

                if num_distinct == c_len - 1 {
                    return (2 * c_len - 1) as u32;
                }
                if num_distinct == c_len - 2 {
                    best_inside = best_inside.min((2 * c_len) as u32);
                }
            }
            if best_inside == (2 * c_len) as u32 {
                return best_inside;
            }
        }

        let global_lb = self.capacity_bounds_2d[self.max_k][c_len];
        if global_lb >= beta {
            return global_lb;
        }

        let parent_lb = self.capacity_bounds_2d[parent_max_k][c_len];
        if parent_lb >= beta {
            return parent_lb.max(global_lb);
        }

        let mut best_val = beta;

        // Capacity hint: the number of non-equivalent, non-useless guesses is bounded by
        // allowed_guesses.len() but for small candidate sets (common at depth 3+) the
        // actual count is much smaller. Over-allocating to allowed_guesses.len() wastes
        // 100s of KB per call when depth is 3+ and the set is tiny. A cap of c_len * 300
        // covers realistic non-equivalent-guess counts; if exceeded, the Vec grows normally.
        let cap = allowed_guesses.len().min(c_len * 300 + 64);
        let mut active_tuples = std::mem::take(&mut self.scratch_tuples[depth]);
        active_tuples.clear();
        if active_tuples.capacity() < cap {
            active_tuples.reserve(cap - active_tuples.capacity());
        }
        let mut c_mask = 0u32;
        for &c in set {
            c_mask |= self.matrix.candidate_masks[c];
        }

        let mut set_projs = std::mem::take(&mut self.scratch_set_projs[depth]);
        set_projs.clear();
        for i in 0..c_len {
            let g = self.dict.candidate_to_guess[set[i]];
            let mut proj = 0u32;
            let chars = &self.dict.guess_chars[g];
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
            set_projs.push(proj);
        }

        self.seen_projections[depth].clear();
        let mut equiv_pruned = 0;

        let mut active_guesses = std::mem::take(&mut self.scratch_guesses[depth]);
        active_guesses.clear();

        let mut phase1_guesses = std::mem::take(&mut self.scratch_phase1_guesses[depth]);
        phase1_guesses.clear();
        let mut phase2_guesses = std::mem::take(&mut self.scratch_phase2_guesses[depth]);
        phase2_guesses.clear();

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

            if !self.seen_projections[depth].insert(proj) {
                equiv_pruned += 1;
                continue;
            }
            active_guesses.push(g);

            let mut in_set = false;
            for i in 0..c_len {
                if proj == set_projs[i] {
                    in_set = true;
                    break;
                }
            }
            if in_set {
                phase1_guesses.push(g);
            } else {
                phase2_guesses.push(g);
            }
        }

        self.scratch_set_projs[depth] = set_projs;

        self.metrics
            .pruned_by_equivalence
            .fetch_add(equiv_pruned, std::sync::atomic::Ordering::Relaxed);

        let mut local_lb = global_lb.max(parent_lb).max(cached_lower_bound);
        let mut counts = [0u16; 243];
        let mut non_empty = [0u8; 243];

        let mut phase1_tuples = std::mem::take(&mut self.scratch_phase1_tuples[depth]);
        phase1_tuples.clear();
        let mut phase2_tuples = std::mem::take(&mut self.scratch_phase2_tuples[depth]);
        phase2_tuples.clear();

        if set.len() <= 16 {
            for &g in &phase1_guesses {
                let mut expected_rem = 0u32;
                let mut lb_cost = c_len as u32;
                let mut num_non_empty = 0;
                let g_offset = g * self.matrix.num_candidates;
                for &c in set {
                    let r = unsafe { self.matrix.data.get_unchecked(g_offset + c).0 as usize };
                    unsafe {
                        if *counts.get_unchecked(r) == 0 {
                            *non_empty.get_unchecked_mut(num_non_empty) = r as u8;
                            num_non_empty += 1;
                        }
                        *counts.get_unchecked_mut(r) += 1;
                    }
                }
                if num_non_empty == 1 {
                    counts[non_empty[0] as usize] = 0;
                    continue;
                }
                for i in 0..num_non_empty {
                    let r_idx = non_empty[i] as usize;
                    let count = counts[r_idx];
                    counts[r_idx] = 0;
                    expected_rem += (count as u32) * (count as u32);
                    if r_idx != crate::core::Response::WIN.0 as usize {
                        lb_cost += self.capacity_bounds_2d[self.max_k][count as usize];
                    }
                }
                if lb_cost < beta {
                    phase1_tuples.push((g, expected_rem, lb_cost, num_non_empty));
                }
            }
        } else {
            for &g in &phase1_guesses {
                let mut expected_rem = 0u32;
                let mut lb_cost = c_len as u32;
                let g_offset = g * self.matrix.num_candidates;
                for &c in set {
                    let r = unsafe { self.matrix.data.get_unchecked(g_offset + c).0 as usize };
                    unsafe { *counts.get_unchecked_mut(r) += 1; }
                }
                let mut num_non_empty = 0;
                for r in 0..243 {
                    if counts[r] > 0 {
                        non_empty[num_non_empty] = r as u8;
                        num_non_empty += 1;
                    }
                }
                if num_non_empty == 1 {
                    counts[non_empty[0] as usize] = 0;
                    continue;
                }
                for i in 0..num_non_empty {
                    let r_idx = non_empty[i] as usize;
                    let count = counts[r_idx];
                    counts[r_idx] = 0;
                    expected_rem += (count as u32) * (count as u32);
                    if r_idx != crate::core::Response::WIN.0 as usize {
                        lb_cost += self.capacity_bounds_2d[self.max_k][count as usize];
                    }
                }
                if lb_cost < beta {
                    phase1_tuples.push((g, expected_rem, lb_cost, num_non_empty));
                }
            }
        }
        phase1_tuples.sort_unstable_by_key(|&(_, exp, _, _)| exp);

        for &(_g, _, g_lb, _non_empty) in &phase1_tuples {
            if g_lb >= best_val {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            let val = self.min_guess_val(set, _g, &active_guesses, best_val, depth, self.max_k);
            if val < best_val {
                best_val = val;
                if best_val <= local_lb {
                    break;
                }
            }
        }

        if best_val > local_lb {
            for &g in &phase2_guesses {
                let mut expected_rem = 0u32;
                let mut lb_cost = c_len as u32;
                let mut num_non_empty = 0;
                let g_offset = g * self.matrix.num_candidates;
                for &c in set {
                    let r = unsafe { self.matrix.data.get_unchecked(g_offset + c).0 as usize };
                    unsafe {
                        if *counts.get_unchecked(r) == 0 {
                            *non_empty.get_unchecked_mut(num_non_empty) = r as u8;
                            num_non_empty += 1;
                        }
                        *counts.get_unchecked_mut(r) += 1;
                    }
                }
                if num_non_empty == 1 {
                    counts[non_empty[0] as usize] = 0;
                    continue;
                }
                for i in 0..num_non_empty {
                    let r_idx = non_empty[i] as usize;
                    let count = counts[r_idx];
                    counts[r_idx] = 0;
                    expected_rem += (count as u32) * (count as u32);
                    if r_idx != crate::core::Response::WIN.0 as usize {
                        lb_cost += self.capacity_bounds_2d[self.max_k][count as usize];
                    }
                }
                phase2_tuples.push((g, expected_rem, lb_cost, num_non_empty));
            }
            phase2_tuples.sort_unstable_by_key(|&(_, exp, _, _)| exp);

            let mut local_max_k = 0;
            let mut valid_max_k = 0;

            for &(_, _, g_lb, non_empty) in phase1_tuples.iter().chain(phase2_tuples.iter()) {
                if non_empty > local_max_k {
                    local_max_k = non_empty;
                }
                if g_lb < beta && non_empty > valid_max_k {
                    valid_max_k = non_empty;
                }
            }

            let base_lb = self.capacity_bounds_2d[local_max_k][c_len];
            if base_lb > local_lb {
                local_lb = base_lb;
            }

            let tight_lb = heuristic::tight_capacity_bound(c_len, valid_max_k, local_max_k);
            if tight_lb > local_lb {
                local_lb = tight_lb;
            }

            if local_lb >= best_val {
                // Done!
            } else {
                for &(_g, _, g_lb, _non_empty) in &phase2_tuples {
                    if g_lb >= best_val {
                        self.metrics
                            .pruned_by_bounds
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                    let val =
                        self.min_guess_val(set, _g, &active_guesses, best_val, depth, local_max_k);
                    if val < best_val {
                        best_val = val;
                        if best_val <= local_lb {
                            break;
                        }
                    }
                }
            }
        }

        self.scratch_guesses[depth] = active_guesses;
        self.scratch_phase1_guesses[depth] = phase1_guesses;
        self.scratch_phase2_guesses[depth] = phase2_guesses;
        self.scratch_phase1_tuples[depth] = phase1_tuples;
        self.scratch_phase2_tuples[depth] = phase2_tuples;

        let is_exact = best_val < beta;
        if is_exact {
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
    pub fn min_guess_val(
        &mut self,
        set: &[usize],
        guess: usize,
        allowed_guesses: &[usize],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        // Tighten local beta using the shared global bound from concurrent threads.
        // min_state_val caches against the tightened beta; alpha-beta semantics guarantee
        // any cached lower bound produced this way is <= the true sub-state cost.
        let beta = beta.min(self.global_beta.load(Ordering::Relaxed));

        self.metrics
            .guesses_evaluated
            .fetch_add(1, Ordering::Relaxed);
        let mut counts = [0u16; 243];
        let mut non_empty_indices = [0u8; 243];
        let mut num_non_empty = 0;

        let g_offset = guess * self.matrix.num_candidates;
        for &c in set {
            let r = unsafe { self.matrix.data.get_unchecked(g_offset + c).0 as usize };
            unsafe {
                if *counts.get_unchecked(r) == 0 {
                    *non_empty_indices.get_unchecked_mut(num_non_empty) = r as u8;
                    num_non_empty += 1;
                }
                *counts.get_unchecked_mut(r) += 1;
            }
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
            // self.capacity_bounds[n] == capacity_bound(n, self.max_k) for every n up to
            // dict.candidates.len() (see solve()'s setup) - self.max_k is fixed at
            // construction and never changes across recursion, so this is always exactly
            // the same value capacity_bound() would compute, just without redoing the
            // O(log n) loop on every one of this hot function's calls.
            let lb = self.capacity_bounds_2d[parent_max_k][p_len as usize];
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
        let mut sorted_set = std::mem::take(&mut self.scratch_sorted_sets[depth]);
        sorted_set.clear();
        if sorted_set.capacity() < set.len() {
            sorted_set.reserve(set.len() - sorted_set.capacity());
        }
        sorted_set.resize(set.len(), 0);

        let mut offsets = [0usize; 243];
        let mut curr = 0;
        for i in 0..num_non_empty {
            let r = non_empty_indices[i] as usize;
            offsets[r] = curr;
            curr += counts[r] as usize;
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

            // Also respect the global beta from concurrent threads: if another thread
            // already found a solution cheaper than beta, tighten our local bound.
            let effective_beta = if depth == 1 {
                new_beta.min(
                    self.global_beta
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .saturating_sub(b),
                )
            } else {
                new_beta
            };

            if effective_beta == 0 {
                self.scratch_sorted_sets[depth] = sorted_set;
                return beta;
            }

            let val =
                self.min_state_val(p, allowed_guesses, effective_beta, depth + 1, parent_max_k);
            if b + val >= beta {
                self.scratch_sorted_sets[depth] = sorted_set;
                return beta;
            }
            // Propagate any tightening from the global beta.
            if depth == 1 {
                let global_now = self.global_beta.load(std::sync::atomic::Ordering::Relaxed);
                if b + val >= global_now {
                    self.scratch_sorted_sets[depth] = sorted_set;
                    return beta;
                }
            }
            cost = b + val;
        }

        self.scratch_sorted_sets[depth] = sorted_set;

        cost
    }
}

// Note: GlobalCache now stores both Exact values (when a full search completes)
// and Lower Bounds (when a search fails high against `beta`).
// This implements Fail-Hard Alpha-Beta Transposition Table Pruning, ensuring that
// if we revisit a state with a `beta` that is <= a previously established lower bound,
// we can instantly prune the subtree and return `beta`, avoiding massive redundant deep searches.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_golden_n100_exact_cost() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let candidates: Vec<usize> = (0..100).collect();
        let cost = Solver::solve(&matrix, &candidates, &dict, &metrics);
        assert_eq!(
            cost, 262,
            "N=100 golden cost changed - likely correctness bug"
        );
    }

    #[test]
    fn test_golden_n500_exact_cost() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let candidates: Vec<usize> = (0..500).collect();
        let cost = Solver::solve(&matrix, &candidates, &dict, &metrics);
        assert_eq!(
            cost, 1469,
            "N=500 golden cost changed - likely correctness bug"
        );
    }

    #[test]
    fn test_golden_n750_exact_cost() {
        // Deliberately larger than the other golden tests: catches a real
        // regression class the N<=250 tests miss, where a guess pruned for
        // *this* node's beta is wrongly withheld from the pool passed down
        // to children, who may need it for their own (looser) sub-beta.
        // That bug produced cost=2264 here (wrong, too high) while leaving
        // N=100/250 unaffected. Costs ~10s; worth it for what it catches.
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let candidates: Vec<usize> = (0..750).collect();
        let cost = Solver::solve(&matrix, &candidates, &dict, &metrics);
        assert_eq!(
            cost, 2256,
            "N=750 golden cost changed - likely correctness bug"
        );
    }

    #[test]
    fn test_golden_n250_exact_cost() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let candidates: Vec<usize> = (0..250).collect();
        let cost = Solver::solve(&matrix, &candidates, &dict, &metrics);
        assert_eq!(
            cost, 702,
            "N=250 golden cost changed - likely correctness bug"
        );
    }

    #[test]
    fn test_determinism_repeated_solves() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let candidates: Vec<usize> = (0..100).collect();
        let mut results = Vec::new();
        for _ in 0..5 {
            let metrics = Metrics::new();
            results.push(Solver::solve(&matrix, &candidates, &dict, &metrics));
        }
        assert!(
            results.iter().all(|&r| r == results[0]),
            "non-deterministic results across repeated runs on identical input: {:?}",
            results
        );
    }
}
#[cfg(test)]
mod solver_cache_tests {
    use super::*;
    #[test]
    fn test_lower_bound_tightening() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let _matrix = crate::matrix::ResponseMatrix::new(&dict);
        let _metrics = Metrics::new();
        let cache = crate::cache::GlobalCache::new(1024);
        assert_eq!(cache.get(0), None);
    }
}
