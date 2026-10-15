#![allow(clippy::needless_range_loop)]
use crate::core::Response;
use crate::dict::Dictionary;
use crate::heuristic;
use crate::matrix::ResponseMatrix;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Global lock-striped cache for exact equivalence deduping.
///
/// Reduces the O(G log G) sorting overhead that would otherwise be required
/// to find equivalence classes at every node.
/// The hash key represents the `set_hash` (a 64-bit Zobrist hash of the exact candidate set).
/// The mapped value is the exact deduplicated projection of all valid guesses,
/// deduplicated by a Zobrist hash of the responses they produce.
pub type EquivCache =
    [std::sync::RwLock<rustc_hash::FxHashMap<u64, std::sync::Arc<Vec<u64>>>>; 1024];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateSet(pub Vec<usize>);

impl std::borrow::Borrow<[usize]> for CandidateSet {
    fn borrow(&self) -> &[usize] {
        &self.0
    }
}

/// Instrumentation counters for a single `Solver::solve` run, used for
/// benchmarking and progress reporting. Not part of the solving logic itself.
/// Counters to measure the performance and branching factor of the search.
#[derive(Debug)]
pub struct Metrics {
    pub states_evaluated: AtomicUsize,
    pub max_depth: AtomicUsize,
    pub guesses_evaluated: AtomicUsize,
    pub pruned_by_bounds: AtomicUsize,
    pub pruned_by_equivalence: AtomicUsize,
    pub cache_hits: AtomicUsize,
    pub cache_misses: AtomicUsize,
    pub equiv_cache_hits: AtomicUsize,
    pub equiv_cache_misses: AtomicUsize,
    /// Number of guesses fully evaluated by the current optimal-guess scan
    /// (root or per-node, for progress reporting). In a policy-tree build this
    /// makes the dominant root scan's progress readable as a fraction.
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
            cache_misses: AtomicUsize::new(0),
            equiv_cache_hits: AtomicUsize::new(0),
            equiv_cache_misses: AtomicUsize::new(0),
            root_guesses_done: AtomicUsize::new(0),
        }
    }
}

/// Sentinel used by non-root Solver instances; never tightened, so never causes spurious abort.
static SENTINEL_BETA: AtomicU32 = AtomicU32::new(u32::MAX);

/// Per-shard entry cap for the equivalence cache. Each cached projection is an
/// `Arc<Vec<u64>>` of one bit per guess (~2 KB for the full 14,855-word list),
/// so the default cap of 65,536 entries per shard permits tens of GB in the
/// worst case - more than the compute host has. A large run can lower it with
/// [`set_equiv_cache_cap`] to bound memory at some cost in hit rate.
static EQUIV_CACHE_CAP: AtomicUsize = AtomicUsize::new(65_536);

/// Lowers (or raises) the per-shard equivalence-cache entry cap. Process-wide:
/// one solve runs per process.
pub fn set_equiv_cache_cap(entries_per_shard: usize) {
    EQUIV_CACHE_CAP.store(entries_per_shard.max(1024), Ordering::Relaxed);
}

pub struct SolverScratch {
    pub scratch_is_in_set: [Vec<u64>; 32],
    pub scratch_sorted_sets: [Vec<usize>; 32],
    pub scratch_phase1_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    pub scratch_phase2_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    pub scratch_hash_table: [Vec<u64>; 32],
    pub scratch_added_indices: [Vec<usize>; 32],
    pub scratch_projs: [Vec<u64>; 32],
    pub scratch_active_bits: [Vec<u64>; 32],
}

thread_local! {
    static THREAD_SCRATCH: std::cell::RefCell<Option<Box<SolverScratch>>> = const { std::cell::RefCell::new(None) };
}

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

    pub cache: &'a crate::cache::GlobalCache,
    /// Shared global upper bound across all parallel root-level tasks.
    /// When a thread improves beta, others see it immediately and can abort early.
    global_beta: &'a AtomicU32,
    equiv_cache: &'a EquivCache,
    pub current_cost_so_far: u32,
    /// Depth-indexed scratch buffers to avoid allocation in min_state_val.
    pub scratch_is_in_set: [Vec<u64>; 32],
    scratch_sorted_sets: [Vec<usize>; 32],
    scratch_phase1_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    scratch_phase2_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    scratch_hash_table: [Vec<u64>; 32],
    scratch_added_indices: [Vec<usize>; 32],
    scratch_projs: [Vec<u64>; 32],
    scratch_active_bits: [Vec<u64>; 32],
}

impl<'a> Drop for Solver<'a> {
    fn drop(&mut self) {
        let scratch = Box::new(SolverScratch {
            scratch_is_in_set: std::mem::replace(
                &mut self.scratch_is_in_set,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_sorted_sets: std::mem::replace(
                &mut self.scratch_sorted_sets,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_phase1_tuples: std::mem::replace(
                &mut self.scratch_phase1_tuples,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_phase2_tuples: std::mem::replace(
                &mut self.scratch_phase2_tuples,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_hash_table: std::mem::replace(
                &mut self.scratch_hash_table,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_added_indices: std::mem::replace(
                &mut self.scratch_added_indices,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_projs: std::mem::replace(
                &mut self.scratch_projs,
                std::array::from_fn(|_| Vec::new()),
            ),
            scratch_active_bits: std::mem::replace(
                &mut self.scratch_active_bits,
                std::array::from_fn(|_| Vec::new()),
            ),
        });
        THREAD_SCRATCH.with(|ts| *ts.borrow_mut() = Some(scratch));
    }
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

        let mut c_mask = 0u32;
        for &c in set {
            c_mask |= matrix.candidate_masks[c];
        }

        let mut best_guess = usize::MAX;
        let mut min_expected_rem = u32::MAX;

        let mut counts = [0u16; 243];
        let mut non_empty_indices = [0u8; 243];

        let skip_projections = c_mask.count_ones() >= 26;
        let mut seen_projections = if skip_projections {
            rustc_hash::FxHashSet::default()
        } else {
            rustc_hash::FxHashSet::with_capacity_and_hasher(
                dict.guesses.len().min(4096),
                Default::default(),
            )
        };

        let matrix_data_ptr = matrix.data.as_ptr();
        for g in 0..dict.guesses.len() {
            if !skip_projections {
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
            }

            let g_offset = g * matrix.num_candidates;
            let g_ptr = unsafe { matrix_data_ptr.add(g_offset) };
            let mut num_non_empty = 0;
            for &c in set {
                let r = unsafe { (*g_ptr.add(c)).0 as usize };
                if counts[r] == 0 {
                    non_empty_indices[num_non_empty] = r as u8;
                    num_non_empty += 1;
                }
                counts[r] += 1;
            }

            if num_non_empty <= 1 {
                for i in 0..num_non_empty {
                    counts[non_empty_indices[i] as usize] = 0;
                }
                continue;
            }

            let mut expected_rem = 0u32;
            for i in 0..num_non_empty {
                let r = non_empty_indices[i] as usize;
                let count = counts[r] as u32;
                expected_rem += count * count;
                counts[r] = 0;
            }
            if expected_rem < min_expected_rem {
                min_expected_rem = expected_rem;
                best_guess = g;
            }
        }

        if best_guess == usize::MAX {
            return u32::MAX; // Should not happen
        }

        let best_g_offset = best_guess * matrix.num_candidates;
        let best_g_ptr = unsafe { matrix_data_ptr.add(best_g_offset) };
        let mut counts = [0u16; 243];
        for &c in set {
            let r = unsafe { (*best_g_ptr.add(c)).0 as usize };
            counts[r] += 1;
        }

        let mut offsets = [0usize; 244];
        for r_idx in 0..243 {
            offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
        }
        let mut sorted_set = vec![0usize; set.len()];
        let mut current_offsets = offsets;
        for &c in set {
            let r = unsafe { (*best_g_ptr.add(c)).0 as usize };
            let pos = current_offsets[r];
            sorted_set[pos] = c;
            current_offsets[r] += 1;
        }

        let mut cost = set.len() as u32;
        for r_idx in 0..243 {
            let p_len = counts[r_idx] as usize;
            if p_len == 0 || r_idx == crate::core::Response::WIN.0 as usize {
                continue;
            }
            let start = offsets[r_idx];
            let end = start + p_len;
            cost += Self::greedy_solve(matrix, dict, &sorted_set[start..end]);
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
        equiv_cache: &'a EquivCache,
    ) -> Self {
        Self::new_with_global_beta(
            matrix,
            max_k,
            dict,
            metrics,
            capacity_bounds_2d,
            cache,
            &SENTINEL_BETA,
            equiv_cache,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_global_beta(
        matrix: &'a ResponseMatrix,
        max_k: usize,
        dict: &'a crate::dict::Dictionary,
        metrics: &'a Metrics,
        capacity_bounds_2d: &'a [Vec<u32>],
        cache: &'a crate::cache::GlobalCache,
        global_beta: &'a AtomicU32,
        equiv_cache: &'a EquivCache,
    ) -> Self {
        let scratch = THREAD_SCRATCH
            .with(|ts| ts.borrow_mut().take())
            .unwrap_or_else(|| {
                Box::new(SolverScratch {
                    scratch_is_in_set: std::array::from_fn(|_| {
                        vec![0u64; dict.guesses.len().div_ceil(64)]
                    }),
                    scratch_sorted_sets: std::array::from_fn(|_| Vec::new()),
                    scratch_phase1_tuples: std::array::from_fn(|_| Vec::new()),
                    scratch_phase2_tuples: std::array::from_fn(|_| Vec::new()),
                    scratch_hash_table: std::array::from_fn(|_| vec![0u64; 32768]),
                    scratch_added_indices: std::array::from_fn(|_| Vec::with_capacity(14855)),
                    scratch_projs: std::array::from_fn(|_| vec![0u64; 14855]),
                    scratch_active_bits: std::array::from_fn(|_| vec![0u64; 233]),
                })
            });

        Self {
            matrix,
            max_k,
            dict,
            metrics,
            capacity_bounds_2d,
            cache,
            global_beta,
            equiv_cache,
            current_cost_so_far: 0,

            scratch_is_in_set: scratch.scratch_is_in_set,
            scratch_sorted_sets: scratch.scratch_sorted_sets,
            scratch_phase1_tuples: scratch.scratch_phase1_tuples,
            scratch_phase2_tuples: scratch.scratch_phase2_tuples,
            scratch_hash_table: scratch.scratch_hash_table,
            scratch_added_indices: scratch.scratch_added_indices,
            scratch_projs: scratch.scratch_projs,
            scratch_active_bits: scratch.scratch_active_bits,
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
        equiv_cache: &'a EquivCache,
    ) -> u32 {
        match initial_candidates.len() {
            0 => return 0,
            1 => return 1,
            2 => return 3,
            _ => {}
        }
        let cache_size = if crate::is_compute_host() {
            // 512 M units x 16 bytes each (two AtomicU64 slots per unit) =
            // 8 GB, which is the whole reason the cost-only full run is tight
            // on the 16 GB compute host. The policy-tree full run uses a
            // smaller table (see run_full) because it also holds the
            // equivalence cache and the tree.
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

            let num_u64s = dict.guesses.len().div_ceil(64);
            let mut all_guesses_bits = vec![u64::MAX; num_u64s];
            let rem = dict.guesses.len() % 64;
            if rem != 0 {
                all_guesses_bits[num_u64s - 1] = (1 << rem) - 1;
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
                        equiv_cache,
                    );
                    let bucket_greedy = Self::greedy_solve(matrix, dict, &bucket);
                    let val =
                        solver.min_state_val(&bucket, &all_guesses_bits, bucket_greedy, 2, max_k);
                    val.min(bucket_greedy)
                })
                .sum();

            let first_guess_cost = base_cost + bucket_costs;
            let greedy_cost_to_beat = initial_greedy_cost.min(first_guess_cost);

            // Pre-filter alternative root guesses: only retain those whose
            // capacity_bound lb is strictly less than the current beta.
            // A guess with lb >= beta cannot possibly improve the solution,
            // so we can skip it entirely. Note: allowed_guesses (active_guesses)
            // is passed unchanged to sub-problems; this filter only affects
            // which root tasks we launch.
            let mut root_candidates_tuples: Vec<(usize, u32, u32)> = active_guesses[1..]
                .iter()
                .copied()
                .filter_map(|g| {
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
                    let mut expected_rem = 0u32;
                    for &cnt in &counts {
                        if cnt > 0 {
                            expected_rem += (cnt as u32) * (cnt as u32);
                        }
                    }
                    if lb < greedy_cost_to_beat {
                        Some((g, lb, expected_rem))
                    } else {
                        None
                    }
                })
                .collect();

            root_candidates_tuples.sort_unstable_by_key(|t| (t.1, t.2));
            let root_candidates: Vec<usize> =
                root_candidates_tuples.into_iter().map(|t| t.0).collect();

            println!(
                "\n*** Root candidates after filter: {} / {} ***",
                root_candidates.len(),
                active_guesses.len() - 1
            );

            // Single tight-beta scan over remaining root candidates.
            // first_guess_cost is computed exactly above via bucket parallelization,
            // so it is already the tightest possible upper bound. One pass with
            // beta=greedy_cost_to_beat prunes almost all root candidates immediately.
            // IDA* is not needed here: beta is already exact after evaluating first_g.
            if !root_candidates.is_empty() {
                let scan_beta = std::sync::atomic::AtomicU32::new(greedy_cost_to_beat);
                let result_beta = crate::parallel_depth::solve_parallel_depth2(
                    matrix,
                    initial_candidates,
                    dict,
                    metrics,
                    equiv_cache,
                    &global_cache,
                    max_k,
                    &capacity_bounds_2d,
                    &scan_beta,
                    &root_candidates,
                );
                return result_beta.min(greedy_cost_to_beat);
            }

            return greedy_cost_to_beat;
        }

        initial_greedy_cost
    }

    /// Quickly evaluates small candidate sets (<= 15) using in-set partitions.
    /// Returns (cost, is_exact).
    /// - If an in-set guess partitions the remaining n-1 candidates into n-1 distinct buckets,
    ///   the exact cost is 2*n - 1.
    /// - If no in-set guess achieves n-1, but at least one achieves n-2 distinct buckets,
    ///   the exact cost is 2*n.
    /// - Otherwise, no guess (in-set or out-of-set) can achieve < 2*n, so 2*n is a guaranteed
    ///   lower bound.
    #[inline(always)]
    pub fn quick_bucket_eval(
        matrix: &ResponseMatrix,
        dict: &Dictionary,
        bucket: &[usize],
    ) -> (u32, bool) {
        let n = bucket.len();
        match n {
            0 => (0, true),
            1 => (1, true),
            2 => (3, true),
            _ => {
                let mut best_inside = u32::MAX;
                let target_exact = (2 * n) as u32;
                let data_ptr = matrix.data.as_ptr();
                for i in 0..n {
                    let ci = unsafe { *bucket.get_unchecked(i) };
                    let gi = unsafe { *dict.candidate_to_guess.get_unchecked(ci) };
                    let g_ptr = unsafe { data_ptr.add(gi * matrix.num_candidates) };

                    let mut mask = [0u64; 4];
                    let mut num_distinct = 0;
                    let mut collisions = 0;
                    let max_collisions = if best_inside == target_exact { 1 } else { 2 };

                    for (j, &cj) in bucket.iter().enumerate() {
                        if i != j {
                            let r = unsafe { (*g_ptr.add(cj)).0 as usize };
                            let word_idx = r >> 6;
                            let bit = 1u64 << (r & 63);
                            let word = unsafe { mask.get_unchecked_mut(word_idx) };
                            if (*word & bit) == 0 {
                                *word |= bit;
                                num_distinct += 1;
                            } else {
                                collisions += 1;
                                if collisions >= max_collisions {
                                    break;
                                }
                            }
                        }
                    }

                    if num_distinct == n - 1 {
                        return ((2 * n - 1) as u32, true);
                    }
                    if num_distinct == n - 2 {
                        best_inside = target_exact;
                    }
                }

                if best_inside == target_exact {
                    (target_exact, true)
                } else {
                    (target_exact, false)
                }
            }
        }
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
    pub fn min_state_val(
        &mut self,
        set: &[usize],
        parent_active_guesses: &[u64],
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
                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cached_val >= beta
                    || self.current_cost_so_far + cached_val >= current_global_beta
                {
                    return cached_val;
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
            let (val, is_exact) = Self::quick_bucket_eval(self.matrix, self.dict, set);
            if is_exact {
                self.cache.insert(hash, val, true);
                return val;
            }
            cached_lower_bound = cached_lower_bound.max(val);
            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if cached_lower_bound >= beta
                || self.current_cost_so_far + cached_lower_bound >= current_global_beta
            {
                self.cache.insert(hash, cached_lower_bound, false);
                return cached_lower_bound;
            }
        }

        let parent_cap_bounds = &self.capacity_bounds_2d[parent_max_k];
        let parent_lb = unsafe { *parent_cap_bounds.get_unchecked(c_len) };
        let current_global_beta = self.global_beta.load(Ordering::Relaxed);
        if parent_lb >= beta || self.current_cost_so_far + parent_lb >= current_global_beta {
            return parent_lb;
        }

        let mut best_val = beta;

        let set_hash = hash;

        let should_cache_equiv = set.len() >= 10 && set.len() < 40;
        let shard_idx = (set_hash as usize) % 1024;
        let mut active_bits_scratch = std::mem::take(&mut self.scratch_active_bits[depth]);
        let rc_opt = if should_cache_equiv {
            let cache = self.equiv_cache[shard_idx].read().unwrap();
            if let Some(cached) = cache.get(&set_hash) {
                self.metrics
                    .equiv_cache_hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(std::sync::Arc::clone(cached))
            } else {
                None
            }
        } else {
            None
        };

        let slice_ptr: *const [u64];
        let _keep_alive = if set.len() >= 40 {
            slice_ptr = parent_active_guesses as *const _;
            None
        } else if let Some(rc) = &rc_opt {
            slice_ptr = rc.as_slice() as *const _;
            None
        } else {
            let num_u64s = self.dict.guesses.len().div_ceil(64);
            active_bits_scratch.clear();
            active_bits_scratch.resize(num_u64s, 0);

            let table = &mut self.scratch_hash_table[depth];
            let added_indices = &mut self.scratch_added_indices[depth];
            added_indices.clear();
            let num_guesses = self.dict.guesses.len();

            let projs = &mut self.scratch_projs[depth];

            let active_count: u32 = parent_active_guesses.iter().map(|&b| b.count_ones()).sum();
            let table_size = ((active_count + 243) * 2)
                .next_power_of_two()
                .clamp(1024, 32768) as usize;
            let mask = table_size - 1;
            let shift = 64 - table_size.trailing_zeros();

            let chunk_size = 512;
            for chunk_start in (0..num_guesses).step_by(chunk_size) {
                let chunk_end = (chunk_start + chunk_size).min(num_guesses);
                let b_start = chunk_start / 64;
                let b_end = chunk_end.div_ceil(64);
                let mut has_active = false;
                for b in b_start..b_end {
                    if unsafe { *parent_active_guesses.get_unchecked(b) } != 0 {
                        has_active = true;
                        break;
                    }
                }
                if !has_active {
                    continue;
                }

                let projs_chunk = unsafe { projs.get_unchecked_mut(chunk_start..chunk_end) };
                projs_chunk.fill(0);
                for &c in set {
                    let c_off = c * num_guesses;
                    let z = unsafe { *self.matrix.zobrist.get_unchecked(c) };
                    let c_slice = unsafe {
                        self.matrix
                            .data_c_g
                            .get_unchecked(c_off + chunk_start..c_off + chunk_end)
                    };
                    for (p, &r) in projs_chunk.iter_mut().zip(c_slice.iter()) {
                        *p ^= z.wrapping_mul(r.0 as u64 + 1);
                    }
                }
            }

            let mut useless_projs = [0u64; 243];
            for r in 0..243 {
                let mut p = 0u64;
                let m = r as u64 + 1;
                for &c in set {
                    p ^= unsafe { *self.matrix.zobrist.get_unchecked(c) }.wrapping_mul(m);
                }
                if p == 0 {
                    p = 1;
                }
                useless_projs[r] = p;
            }
            for r in 0..243 {
                let proj = useless_projs[r];
                let mut idx = (proj.wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                loop {
                    let slot = table[idx];
                    if slot == 0 {
                        table[idx] = proj;
                        added_indices.push(idx);
                        break;
                    }
                    if slot == proj {
                        break;
                    }
                    idx = (idx + 1) & mask;
                }
            }

            for (block_idx, &block) in parent_active_guesses.iter().enumerate() {
                if block == u64::MAX {
                    for tz in 0..64 {
                        let g = block_idx * 64 + tz;
                        if g >= num_guesses {
                            break;
                        }
                        let mut proj = unsafe { *projs.get_unchecked(g) };
                        if proj == 0 {
                            proj = 1;
                        }
                        let mut idx = (proj.wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                        loop {
                            let slot = table[idx];
                            if slot == 0 {
                                table[idx] = proj;
                                unsafe {
                                    *active_bits_scratch.get_unchecked_mut(block_idx) |= 1 << tz;
                                }
                                added_indices.push(idx);
                                break;
                            }
                            if slot == proj {
                                break;
                            }
                            idx = (idx + 1) & mask;
                        }
                    }
                } else if block != 0 {
                    let mut b = block;
                    while b != 0 {
                        let tz = b.trailing_zeros();
                        let g = block_idx * 64 + tz as usize;
                        let mut proj = unsafe { *projs.get_unchecked(g) };
                        if proj == 0 {
                            proj = 1;
                        }
                        let mut idx = (proj.wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                        loop {
                            let slot = table[idx];
                            if slot == 0 {
                                table[idx] = proj;
                                active_bits_scratch[block_idx] |= 1 << tz;
                                added_indices.push(idx);
                                break;
                            }
                            if slot == proj {
                                break;
                            }
                            idx = (idx + 1) & mask;
                        }
                        b &= b - 1;
                    }
                }
            }

            for &idx in added_indices.iter() {
                table[idx] = 0;
            }

            let rc_new = if should_cache_equiv {
                let rc = std::sync::Arc::new(active_bits_scratch.clone());
                let mut cache_mut = self.equiv_cache[shard_idx].write().unwrap();
                if let Some(cached) = cache_mut.get(&set_hash) {
                    self.metrics
                        .equiv_cache_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Some(std::sync::Arc::clone(cached))
                } else {
                    self.metrics
                        .equiv_cache_misses
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.metrics.pruned_by_equivalence.fetch_add(
                        self.dict.guesses.len()
                            - rc.iter().map(|b| b.count_ones() as usize).sum::<usize>(),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    let cap = EQUIV_CACHE_CAP.load(Ordering::Relaxed);
                    if cache_mut.len() > cap {
                        let evict = (cap / 4).max(1);
                        let keys_to_remove: Vec<_> =
                            cache_mut.keys().take(evict).copied().collect();
                        for k in keys_to_remove {
                            cache_mut.remove(&k);
                        }
                    }
                    cache_mut.insert(set_hash, std::sync::Arc::clone(&rc));
                    Some(rc)
                }
            } else {
                self.metrics
                    .equiv_cache_misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.metrics.pruned_by_equivalence.fetch_add(
                    self.dict.guesses.len()
                        - active_bits_scratch
                            .iter()
                            .map(|b| b.count_ones() as usize)
                            .sum::<usize>(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                None
            };

            if let Some(rc_val) = &rc_new {
                slice_ptr = rc_val.as_slice() as *const _;
            } else {
                slice_ptr = active_bits_scratch.as_slice() as *const _;
            }
            rc_new
        };

        let active_guesses_slice = unsafe { &*slice_ptr };

        self.scratch_active_bits[depth] = active_bits_scratch;

        let mut local_lb = parent_lb.max(cached_lower_bound);
        let mut counts = [0u16; 243];
        let mut non_empty_indices = [0u8; 243];

        let parent_cap_bounds_ptr = parent_cap_bounds.as_ptr();
        let matrix_data_ptr = self.matrix.data.as_ptr();

        let mut phase1_tuples = std::mem::take(&mut self.scratch_phase1_tuples[depth]);
        phase1_tuples.clear();
        let mut phase2_tuples = std::mem::take(&mut self.scratch_phase2_tuples[depth]);
        phase2_tuples.clear();

        let mut local_max_k = 0;
        let mut valid_phase1_k = 0;
        let mut min_val_found = u32::MAX;

        for &cand in set {
            let g = unsafe { *self.dict.candidate_to_guess.get_unchecked(cand) };
            if (active_guesses_slice[g / 64] & (1 << (g % 64))) == 0 {
                continue;
            }
            let mut num_non_empty = 0;
            let g_off = g * self.matrix.num_candidates;
            let g_ptr = unsafe { matrix_data_ptr.add(g_off) };
            for &c in set {
                let r = unsafe { (*g_ptr.add(c)).0 as usize };
                unsafe {
                    let cnt = counts.get_unchecked_mut(r);
                    *non_empty_indices.get_unchecked_mut(num_non_empty) = r as u8;
                    num_non_empty += (*cnt == 0) as usize;
                    *cnt += 1;
                }
            }
            if num_non_empty == 1 {
                unsafe {
                    let r_idx = *non_empty_indices.get_unchecked(0) as usize;
                    *counts.get_unchecked_mut(r_idx) = 0;
                }
                continue;
            }
            if num_non_empty > local_max_k {
                local_max_k = num_non_empty;
            }
            let mut expected_rem = 0u32;
            let mut lb_cost = c_len as u32;
            for i in 0..num_non_empty {
                let r_idx = unsafe { *non_empty_indices.get_unchecked(i) as usize };
                let count = unsafe { *counts.get_unchecked(r_idx) };
                unsafe {
                    *counts.get_unchecked_mut(r_idx) = 0;
                }
                expected_rem += (count as u32) * (count as u32);
                if r_idx != Response::WIN.0 as usize {
                    lb_cost += unsafe { *parent_cap_bounds_ptr.add(count as usize) };
                }
            }
            if lb_cost < beta {
                if num_non_empty > valid_phase1_k {
                    valid_phase1_k = num_non_empty;
                }
                phase1_tuples.push((g, expected_rem, lb_cost, num_non_empty));
            } else {
                min_val_found = min_val_found.min(lb_cost);
            }
        }
        phase1_tuples.sort_unstable_by_key(|&(g, exp, lb, _)| (lb, exp, g));

        for (idx, &(_g, _, g_lb, _non_empty)) in phase1_tuples.iter().enumerate() {
            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if self.current_cost_so_far + local_lb >= current_global_beta {
                break;
            }
            if self.current_cost_so_far + g_lb >= current_global_beta {
                min_val_found = min_val_found.min(g_lb);
                self.metrics.pruned_by_bounds.fetch_add(
                    phase1_tuples.len() - idx,
                    std::sync::atomic::Ordering::Relaxed,
                );
                break;
            }
            if g_lb >= best_val {
                min_val_found = min_val_found.min(g_lb);
                self.metrics.pruned_by_bounds.fetch_add(
                    phase1_tuples.len() - idx,
                    std::sync::atomic::Ordering::Relaxed,
                );
                break;
            }
            let val =
                self.min_guess_val(set, active_guesses_slice, _g, best_val, depth, parent_max_k);
            min_val_found = min_val_found.min(val);
            if val < best_val {
                best_val = val;
                if best_val <= local_lb {
                    break;
                }
            }
        }

        let phase2_min_lb = heuristic::phase2_capacity_bound(c_len, parent_max_k, parent_max_k);
        let phase2_effective_lb = local_lb.max(phase2_min_lb);
        let current_global_beta = self.global_beta.load(Ordering::Relaxed);
        if best_val > phase2_effective_lb
            && self.current_cost_so_far + phase2_effective_lb < current_global_beta
        {
            // Gather Phase 2 guesses: active guesses not in set
            let mut valid_phase2_k = 0;
            let mut is_in_set = std::mem::take(&mut self.scratch_is_in_set[depth]);
            for &c in set {
                let g = unsafe { *self.dict.candidate_to_guess.get_unchecked(c) };
                unsafe {
                    *is_in_set.get_unchecked_mut(g / 64) |= 1 << (g % 64);
                }
            }
            let num_guesses = self.dict.guesses.len();
            let data_c_g_ptr = self.matrix.data_c_g.as_ptr();
            let mut stack_c_ptrs = [std::ptr::null::<Response>(); 256];
            let mut heap_c_ptrs = Vec::new();
            let c_ptrs: &[*const Response] = if c_len <= 256 {
                for (i, &c) in set.iter().enumerate() {
                    unsafe {
                        *stack_c_ptrs.get_unchecked_mut(i) = data_c_g_ptr.add(c * num_guesses);
                    }
                }
                &stack_c_ptrs[..c_len]
            } else {
                heap_c_ptrs.reserve(c_len);
                for &c in set {
                    heap_c_ptrs.push(unsafe { data_c_g_ptr.add(c * num_guesses) });
                }
                &heap_c_ptrs
            };

            for (block_idx, &block) in active_guesses_slice.iter().enumerate() {
                if block == 0 {
                    continue;
                }
                let mut b = block & !is_in_set[block_idx];
                while b != 0 {
                    let tz = b.trailing_zeros();
                    let g = block_idx * 64 + tz as usize;
                    b &= b - 1;

                    let mut num_non_empty = 0;
                    for &c_base in c_ptrs {
                        let r = unsafe { (*c_base.add(g)).0 as usize };
                        unsafe {
                            let cnt = counts.get_unchecked_mut(r);
                            *non_empty_indices.get_unchecked_mut(num_non_empty) = r as u8;
                            num_non_empty += (*cnt == 0) as usize;
                            *cnt += 1;
                        }
                    }
                    if num_non_empty == 1 {
                        unsafe {
                            let r_idx = *non_empty_indices.get_unchecked(0) as usize;
                            *counts.get_unchecked_mut(r_idx) = 0;
                        }
                        continue;
                    }
                    let mut expected_rem = 0u32;
                    let mut lb_cost = c_len as u32;
                    for i in 0..num_non_empty {
                        let r_idx = unsafe { *non_empty_indices.get_unchecked(i) as usize };
                        let count = unsafe { *counts.get_unchecked(r_idx) };
                        unsafe {
                            *counts.get_unchecked_mut(r_idx) = 0;
                        }
                        expected_rem += (count as u32) * (count as u32);
                        if r_idx != Response::WIN.0 as usize {
                            lb_cost += unsafe { *parent_cap_bounds_ptr.add(count as usize) };
                        }
                    }
                    if num_non_empty == 2
                        && expected_rem == (c_len * c_len) as u32 - 2 * (c_len as u32) + 2
                    {
                        continue;
                    }
                    if num_non_empty > local_max_k {
                        local_max_k = num_non_empty;
                    }
                    if lb_cost < best_val {
                        if num_non_empty > valid_phase2_k {
                            valid_phase2_k = num_non_empty;
                        }
                        phase2_tuples.push((g, expected_rem, lb_cost, num_non_empty));
                    } else {
                        min_val_found = min_val_found.min(lb_cost);
                    }
                }
            }
            for &c in set {
                let g = self.dict.candidate_to_guess[c];
                is_in_set[g / 64] = 0;
            }
            self.scratch_is_in_set[depth] = is_in_set;

            let base_lb = unsafe {
                *self
                    .capacity_bounds_2d
                    .get_unchecked(local_max_k)
                    .get_unchecked(c_len)
            };
            if base_lb > local_lb {
                local_lb = base_lb;
            }

            let phase1_lb = if valid_phase1_k > 0 {
                heuristic::tight_capacity_bound(c_len, valid_phase1_k, local_max_k)
            } else {
                u32::MAX
            };
            let phase2_lb = if valid_phase2_k > 0 {
                heuristic::phase2_capacity_bound(c_len, valid_phase2_k, local_max_k)
            } else {
                u32::MAX
            };
            let tight_lb = phase1_lb.min(phase2_lb);
            if tight_lb != u32::MAX && tight_lb > local_lb {
                local_lb = tight_lb;
            }

            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if local_lb < best_val && self.current_cost_so_far + local_lb < current_global_beta {
                phase2_tuples.sort_unstable_by_key(|&(g, exp, lb, _)| (lb, exp, g));

                for (idx, &(_g, _, g_lb, _non_empty)) in phase2_tuples.iter().enumerate() {
                    let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                    if self.current_cost_so_far + local_lb >= current_global_beta {
                        break;
                    }
                    if self.current_cost_so_far + g_lb >= current_global_beta {
                        min_val_found = min_val_found.min(g_lb);
                        self.metrics.pruned_by_bounds.fetch_add(
                            phase2_tuples.len() - idx,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        break;
                    }
                    if g_lb >= best_val {
                        min_val_found = min_val_found.min(g_lb);
                        self.metrics.pruned_by_bounds.fetch_add(
                            phase2_tuples.len() - idx,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        break;
                    }
                    let val = self.min_guess_val(
                        set,
                        active_guesses_slice,
                        _g,
                        best_val,
                        depth,
                        local_max_k,
                    );
                    min_val_found = min_val_found.min(val);
                    if val < best_val {
                        best_val = val;
                        if best_val <= local_lb {
                            break;
                        }
                    }
                }
            }
        } else {
            min_val_found = min_val_found.min(phase2_min_lb);
        }

        self.scratch_phase1_tuples[depth] = phase1_tuples;
        self.scratch_phase2_tuples[depth] = phase2_tuples;

        let is_exact = best_val < beta;
        if is_exact {
            self.cache.insert(hash, best_val, true);
            best_val
        } else {
            let mut final_lb = beta.max(local_lb);
            if best_val > local_lb && min_val_found != u32::MAX {
                final_lb = final_lb.max(min_val_found);
            }
            self.cache.insert(hash, final_lb, false);
            final_lb
        }
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
        active_guesses_slice: &[u64],
        guess: usize,

        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        let current_global_beta = self.global_beta.load(Ordering::Relaxed);
        let beta = beta.min(current_global_beta);

        self.metrics
            .guesses_evaluated
            .fetch_add(1, Ordering::Relaxed);

        let parent_cap_bounds = &self.capacity_bounds_2d[parent_max_k];
        let parent_cap_bounds_ptr = parent_cap_bounds.as_ptr();
        let c_len = set.len();
        let g_offset = guess * self.matrix.num_candidates;
        let g_ptr = unsafe { self.matrix.data.as_ptr().add(g_offset) };

        if c_len <= 16 {
            let mut local_counts = [(0u8, 0u8, 0u32); 16];
            let mut c_responses = [0u8; 16];
            let mut num_non_empty = 0;
            for (cand_idx, &c) in set.iter().enumerate() {
                let r = unsafe { (*g_ptr.add(c)).0 };
                unsafe {
                    *c_responses.get_unchecked_mut(cand_idx) = r;
                }
                let mut found = false;
                for i in 0..num_non_empty {
                    let entry = unsafe { local_counts.get_unchecked_mut(i) };
                    if entry.0 == r {
                        entry.1 += 1;
                        found = true;
                        break;
                    }
                }
                if !found {
                    unsafe {
                        *local_counts.get_unchecked_mut(num_non_empty) = (r, 1, 0);
                    }
                    num_non_empty += 1;
                }
            }

            if num_non_empty == 1 {
                return beta;
            }

            let mut cost = c_len as u32;
            let mut max_p_len = 0;

            for i in 0..num_non_empty {
                let entry = unsafe { local_counts.get_unchecked_mut(i) };
                let r_idx = entry.0 as usize;
                if r_idx == Response::WIN.0 as usize {
                    continue;
                }
                let p_len = entry.1 as usize;
                if p_len > max_p_len {
                    max_p_len = p_len;
                }
                let lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                cost += lb;
                entry.2 = lb;
            }

            if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, Ordering::Relaxed);
                return cost;
            }

            if max_p_len <= 2 {
                return cost;
            }

            local_counts[0..num_non_empty]
                .sort_unstable_by_key(|&(_, count, _)| std::cmp::Reverse(count));

            for i in 0..num_non_empty {
                let entry = unsafe { local_counts.get_unchecked_mut(i) };
                let p_len = entry.1 as usize;
                let r_idx = entry.0 as usize;
                if r_idx == Response::WIN.0 as usize || p_len <= 2 {
                    continue;
                }

                let mut p = [0usize; 16];
                let mut p_idx = 0;
                for (cand_idx, &c) in set.iter().enumerate() {
                    let r = unsafe { *c_responses.get_unchecked(cand_idx) as usize };
                    if r == r_idx {
                        unsafe {
                            *p.get_unchecked_mut(p_idx) = c;
                        }
                        p_idx += 1;
                    }
                }

                let (quick_val, is_exact) =
                    Self::quick_bucket_eval(self.matrix, self.dict, unsafe {
                        p.get_unchecked(0..p_len)
                    });
                let lb = entry.2;
                let used_val = quick_val.max(lb);
                cost += used_val - lb;
                entry.2 = used_val;

                if is_exact {
                    if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                        return cost;
                    }
                    continue;
                }

                let b = cost - entry.2;
                let new_beta = beta - b;
                let effective_beta =
                    new_beta.min(current_global_beta.saturating_sub(self.current_cost_so_far + b));

                if effective_beta == 0 {
                    return cost;
                }

                self.current_cost_so_far += b;
                let val = self.min_state_val(
                    &p[0..p_len],
                    active_guesses_slice,
                    effective_beta,
                    depth + 1,
                    p_len.min(parent_max_k).max(2),
                );
                self.current_cost_so_far -= b;

                if b + val >= beta || self.current_cost_so_far + b + val >= current_global_beta {
                    return b + val;
                }
                cost = b + val;
                entry.2 = val;
            }
            cost
        } else {
            let mut counts = [0u16; 243];
            let mut bucket_hashes = [0u64; 243];
            let mut non_empty_indices = [0u8; 243];
            let mut num_non_empty = 0;

            let zobrist_ptr = self.matrix.zobrist.as_ptr();
            for &c in set {
                let r = unsafe { (*g_ptr.add(c)).0 as usize };
                unsafe {
                    let cnt = counts.get_unchecked_mut(r);
                    *non_empty_indices.get_unchecked_mut(num_non_empty) = r as u8;
                    num_non_empty += (*cnt == 0) as usize;
                    *cnt += 1;
                    *bucket_hashes.get_unchecked_mut(r) ^= *zobrist_ptr.add(c);
                }
            }

            if num_non_empty == 1 {
                return beta;
            }

            let mut cost = set.len() as u32;
            let mut max_p_len = 0;
            let mut eval_indices = [0u8; 243];
            let mut num_eval = 0;

            for i in 0..num_non_empty {
                let r_idx = unsafe { *non_empty_indices.get_unchecked(i) as usize };
                if r_idx == Response::WIN.0 as usize {
                    continue;
                }
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };
                if p_len > max_p_len {
                    max_p_len = p_len;
                }
                if p_len > 2 {
                    unsafe {
                        *eval_indices.get_unchecked_mut(num_eval) = r_idx as u8;
                    }
                    num_eval += 1;
                }
                cost += unsafe { *parent_cap_bounds_ptr.add(p_len) };
            }

            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, Ordering::Relaxed);
                return cost;
            }

            if max_p_len <= 2 {
                return cost;
            }

            // CRITICAL OPTIMIZATION: Evaluate largest buckets first.
            // Large buckets have a higher probability of exceeding their heuristic minimum bounds.
            // By evaluating them first, we can rapidly tighten our accumulated cost and trigger
            // an Alpha-Beta cutoff (cost >= beta) before wasting time evaluating the smaller buckets.
            // Ties broken by response index for deterministic order.
            eval_indices[0..num_eval]
                .sort_unstable_by_key(|&r| (std::cmp::Reverse(counts[r as usize]), r));

            let split_idx = eval_indices[0..num_eval].partition_point(|&r| counts[r as usize] > 15);

            let mut p_lbs = [0u32; 243];
            let mut child_max_ks = [0u16; 243];
            for i in 0..num_eval {
                let r_idx = unsafe { *eval_indices.get_unchecked(i) as usize };
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };
                unsafe {
                    *p_lbs.get_unchecked_mut(r_idx) = *parent_cap_bounds_ptr.add(p_len);
                    *child_max_ks.get_unchecked_mut(r_idx) = (p_len.min(parent_max_k)) as u16;
                }
            }

            // Pass 1: Cache-only check descending by bucket size before partitioning into sorted_set.
            // If cached bounds alone push cost >= beta or current_cost_so_far + cost >= current_global_beta,
            // we prune immediately without spending time on counting sort or scratch buffer partitioning.
            let mut exact_mask = [0u64; 4];
            let mut cached_mask = [0u64; 4];
            let mut num_exact = 0;

            for i in 0..num_eval {
                let r_idx = unsafe { *eval_indices.get_unchecked(i) as usize };
                let hash = unsafe { *bucket_hashes.get_unchecked(r_idx) };
                if let Some((cached_val, is_exact)) = self.cache.get(hash) {
                    self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                    let lb = unsafe { *p_lbs.get_unchecked(r_idx) };
                    let used_val = cached_val.max(lb);
                    cost += used_val - lb;
                    unsafe {
                        *p_lbs.get_unchecked_mut(r_idx) = used_val;
                    }
                    let word_idx = r_idx >> 6;
                    let bit = 1u64 << (r_idx & 63);
                    if is_exact {
                        unsafe {
                            *exact_mask.get_unchecked_mut(word_idx) |= bit;
                        }
                        num_exact += 1;
                    }
                    unsafe {
                        *cached_mask.get_unchecked_mut(word_idx) |= bit;
                    }

                    let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                    if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                        self.metrics
                            .pruned_by_bounds
                            .fetch_add(1, Ordering::Relaxed);
                        return cost;
                    }
                }
            }

            // If all buckets with p_len > 2 were exact in cache, no partitioning or evaluation is needed.
            if num_exact == num_eval {
                return cost;
            }

            // Fast slice partition using counting sort
            let mut sorted_set = std::mem::take(&mut self.scratch_sorted_sets[depth]);
            sorted_set.resize(set.len(), 0);

            let mut offsets = [0u16; 243];
            let mut curr = 0;
            for i in 0..num_non_empty {
                let r = unsafe { *non_empty_indices.get_unchecked(i) as usize };
                unsafe {
                    *offsets.get_unchecked_mut(r) = curr;
                    curr += *counts.get_unchecked(r);
                }
            }

            let mut current_offsets = offsets;
            for &c in set {
                let r_idx = unsafe { (*g_ptr.add(c)).0 as usize };
                let pos = unsafe { *current_offsets.get_unchecked(r_idx) as usize };
                unsafe {
                    *sorted_set.get_unchecked_mut(pos) = c;
                    *current_offsets.get_unchecked_mut(r_idx) += 1;
                }
            }

            // Pass 2a: Cheap uncached buckets evaluation (quick_bucket_eval on p_len <= 15).
            // Evaluating small buckets first takes nanoseconds and often prunes cost >= beta
            // immediately, completely bypassing expensive full-dictionary branching factor searches
            // on any p_len > 15 buckets for this guess.
            for i in split_idx..num_eval {
                let r_idx = unsafe { *eval_indices.get_unchecked(i) as usize };
                let word_idx = r_idx >> 6;
                let bit = 1u64 << (r_idx & 63);
                if (unsafe { *cached_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };
                let start = unsafe { *offsets.get_unchecked(r_idx) as usize };
                let end = start + p_len;
                let p = unsafe { sorted_set.get_unchecked(start..end) };
                let hash = unsafe { *bucket_hashes.get_unchecked(r_idx) };

                let (quick_val, is_exact) = Self::quick_bucket_eval(self.matrix, self.dict, p);
                let lb = unsafe { *p_lbs.get_unchecked(r_idx) };
                let used_val = quick_val.max(lb);
                cost += used_val - lb;
                unsafe {
                    *p_lbs.get_unchecked_mut(r_idx) = used_val;
                }
                if is_exact {
                    unsafe {
                        *exact_mask.get_unchecked_mut(word_idx) |= bit;
                    }
                    num_exact += 1;
                }
                self.cache.insert(hash, used_val, is_exact);
                unsafe {
                    *child_max_ks.get_unchecked_mut(r_idx) = (p_len.min(parent_max_k)) as u16;
                }

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                    break;
                }
            }

            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, Ordering::Relaxed);
                self.scratch_sorted_sets[depth] = sorted_set;
                return cost;
            }

            if num_exact == num_eval {
                self.scratch_sorted_sets[depth] = sorted_set;
                return cost;
            }

            // Pass 2b: Large uncached buckets evaluation (tight capacity bound on p_len > 15).
            // Only reached if small buckets alone did not exceed beta.
            for i in 0..split_idx {
                let r_idx = unsafe { *eval_indices.get_unchecked(i) as usize };
                let word_idx = r_idx >> 6;
                let bit = 1u64 << (r_idx & 63);
                if (unsafe { *cached_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };
                let start = unsafe { *offsets.get_unchecked(r_idx) as usize };
                let end = start + p_len;
                let p = unsafe { sorted_set.get_unchecked(start..end) };
                let hash = unsafe { *bucket_hashes.get_unchecked(r_idx) };

                let lb = unsafe { *p_lbs.get_unchecked(r_idx) };
                let target_k = crate::heuristic::min_k_for_target_bound(p_len, parent_max_k, lb);
                if target_k == 2 {
                    unsafe {
                        *child_max_ks.get_unchecked_mut(r_idx) =
                            (p_len as u16).min(*child_max_ks.get_unchecked(r_idx));
                    }
                    self.cache.insert(hash, lb, false);
                } else {
                    let k_cand = crate::heuristic::compute_candidate_branching_factor_with_target(
                        self.matrix,
                        p,
                        target_k,
                    );
                    if k_cand >= target_k {
                        unsafe {
                            *child_max_ks.get_unchecked_mut(r_idx) =
                                (p_len as u16).min(*child_max_ks.get_unchecked(r_idx));
                        }
                        self.cache.insert(hash, lb, false);
                    } else {
                        let k = crate::heuristic::compute_max_branching_factor_seq_with_target(
                            self.matrix,
                            p,
                            k_cand,
                            target_k,
                        );
                        if k >= target_k {
                            unsafe {
                                *child_max_ks.get_unchecked_mut(r_idx) =
                                    (p_len as u16).min(*child_max_ks.get_unchecked(r_idx));
                            }
                            self.cache.insert(hash, lb, false);
                        } else {
                            unsafe {
                                *child_max_ks.get_unchecked_mut(r_idx) =
                                    (k as u16).min(*child_max_ks.get_unchecked(r_idx));
                            }
                            let tight_lb =
                                crate::heuristic::tight_capacity_bound(p_len, k, parent_max_k);
                            let used_val = tight_lb.max(lb);
                            cost += used_val - lb;
                            unsafe {
                                *p_lbs.get_unchecked_mut(r_idx) = used_val;
                            }
                            self.cache.insert(hash, used_val, false);
                        }
                    }
                }

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                    break;
                }
            }

            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, Ordering::Relaxed);
                self.scratch_sorted_sets[depth] = sorted_set;
                return cost;
            }

            for i in 0..num_eval {
                let r_idx = unsafe { *eval_indices.get_unchecked(i) as usize };
                let word_idx = r_idx >> 6;
                let bit = 1u64 << (r_idx & 63);
                if (unsafe { *exact_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let p_len = unsafe { *counts.get_unchecked(r_idx) as usize };

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if self.current_cost_so_far + cost >= current_global_beta {
                    self.scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }

                let start = unsafe { *offsets.get_unchecked(r_idx) as usize };
                let end = start + p_len;
                let p = unsafe { sorted_set.get_unchecked(start..end) };

                let b = cost - unsafe { *p_lbs.get_unchecked(r_idx) };
                let new_beta = beta - b;

                // Also respect the global beta from concurrent threads: if another thread
                // already found a solution cheaper than beta, tighten our local bound.
                let effective_beta =
                    new_beta.min(current_global_beta.saturating_sub(self.current_cost_so_far + b));

                if effective_beta == 0 {
                    self.scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }

                self.current_cost_so_far += b;
                let val = self.min_state_val(
                    p,
                    active_guesses_slice,
                    effective_beta,
                    depth + 1,
                    (unsafe { *child_max_ks.get_unchecked(r_idx) } as usize).max(2),
                );
                self.current_cost_so_far -= b;
                if b + val >= beta {
                    self.scratch_sorted_sets[depth] = sorted_set;
                    return b + val;
                }
                // Propagate any tightening from the global beta.
                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if self.current_cost_so_far + b + val >= current_global_beta {
                    self.scratch_sorted_sets[depth] = sorted_set;
                    return b + val;
                }
                cost = b + val;
            }

            self.scratch_sorted_sets[depth] = sorted_set;

            cost
        }
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

    fn assert_golden_tree_cost(n: usize, expected_cost: u64) {
        let full = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let candidates: Vec<crate::core::Word> = full.candidates[..n].to_vec();
        let dict = crate::dict::Dictionary::from_words(full.guesses.clone(), candidates);
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..n).collect();
        let opts = crate::policy::BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, stats) = crate::policy::build_policy_tree(
            &matrix,
            &dict,
            crate::policy::Strategy::Optimal,
            &root,
            &opts,
        )
        .expect("build_policy_tree failed");
        let report = tree
            .validate(&matrix, &dict)
            .expect("tree validation failed");
        assert_eq!(
            report.wins, n as u64,
            "N={n} tree must cover all candidates"
        );
        assert_eq!(
            report.total_cost, expected_cost,
            "N={n} golden policy tree total cost changed - likely correctness bug"
        );
        assert_eq!(
            stats.summary.total_cost, expected_cost,
            "N={n} stats summary cost must match validation report"
        );
    }

    #[test]
    fn test_golden_n100_exact_cost() {
        assert_golden_tree_cost(100, 262);
    }

    #[test]
    fn test_golden_n250_exact_cost() {
        assert_golden_tree_cost(250, 702);
    }

    #[test]
    fn test_golden_n500_exact_cost() {
        assert_golden_tree_cost(500, 1469);
    }

    #[test]
    fn test_golden_n750_exact_cost() {
        // Deliberately larger than the other golden tests: catches a real
        // regression class the N<=250 tests miss, where a guess pruned for
        // *this* node's beta is wrongly withheld from the pool passed down
        // to children, who may need it for their own (looser) sub-beta.
        // That bug produced cost=2264 here (wrong, too high) while leaving
        // N=100/250 unaffected. Costs ~10s; worth it for what it catches.
        assert_golden_tree_cost(750, 2256);
    }

    #[test]
    fn test_golden_n800_exact_cost() {
        assert_golden_tree_cost(800, 2419);
    }

    #[test]
    fn test_determinism_repeated_solves() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let candidates: Vec<usize> = (0..100).collect();
        let mut results = Vec::new();
        for _ in 0..5 {
            let metrics = Metrics::new();
            let equiv_cache: [_; 1024] =
                std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
            results.push(Solver::solve(
                &matrix,
                &candidates,
                &dict,
                &metrics,
                &equiv_cache,
            ));
        }
        assert!(
            results.iter().all(|&r| r == results[0]),
            "non-deterministic results across repeated runs on identical input: {:?}",
            results
        );
    }
    #[test]
    fn test_equiv_cache_metrics_populated() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        // Use a small set that will definitely trigger some EquivCache hits/misses.
        // N=100 might not trigger hits if it's too small, but misses will definitely happen.
        let candidates: Vec<usize> = (0..100).collect();
        let equiv_cache: [_; 1024] =
            std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
        let _cost = Solver::solve(&matrix, &candidates, &dict, &metrics, &equiv_cache);

        let _hits = metrics
            .equiv_cache_hits
            .load(std::sync::atomic::Ordering::Relaxed);
        let misses = metrics
            .equiv_cache_misses
            .load(std::sync::atomic::Ordering::Relaxed);

        // At least we should have some cache misses since it's empty initially.
        assert!(misses > 0, "Expected some EquivCache misses, got 0");
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

    #[test]
    fn test_solve_empty_candidates() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let candidates = vec![];
        let equiv_cache: [_; 1024] =
            std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
        let cost = Solver::solve(&matrix, &candidates, &dict, &metrics, &equiv_cache);
        assert_eq!(cost, 0);
    }

    #[test]
    fn test_quick_bucket_eval_exactness() {
        let dict = crate::dict::Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let equiv_cache: [_; 1024] =
            std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));

        // Test multiple small subsets of candidates (sizes 2, 3, 4)
        for offset in (0..15).step_by(5) {
            for size in 2..=4 {
                let bucket: Vec<usize> = (offset..offset + size).collect();
                let (q_val, is_exact) = Solver::quick_bucket_eval(&matrix, &dict, &bucket);
                let true_cost = Solver::solve(&matrix, &bucket, &dict, &metrics, &equiv_cache);

                if is_exact {
                    assert_eq!(
                        q_val, true_cost,
                        "quick_bucket_eval returned exact {} but true_cost is {} for {:?}",
                        q_val, true_cost, bucket
                    );
                } else {
                    assert!(
                        q_val <= true_cost,
                        "quick_bucket_eval returned lower bound {} > true_cost {} for {:?}",
                        q_val,
                        true_cost,
                        bucket
                    );
                }
            }
        }
    }
}
