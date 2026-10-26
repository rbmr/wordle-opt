#![allow(clippy::needless_range_loop)]
use crate::core::{Response, ResponseElem};
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
/// worst case. A large run can lower it with
/// [`set_equiv_cache_cap`] to bound memory at some cost in hit rate.
static EQUIV_CACHE_CAP: AtomicUsize = AtomicUsize::new(65_536);

/// Lowers (or raises) the per-shard equivalence-cache entry cap. Process-wide:
/// one solve runs per process.
pub fn set_equiv_cache_cap(entries_per_shard: usize) {
    EQUIV_CACHE_CAP.store(entries_per_shard.max(1024), Ordering::Relaxed);
}

/// Maximum number of ternary responses supported (3^7 = 2187).
pub const MAX_RESPONSES: usize = 2187;

pub struct SolverScratch {
    pub scratch_is_in_set: [Vec<u64>; 32],
    pub scratch_sorted_sets: [Vec<usize>; 32],
    pub scratch_phase1_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    pub scratch_phase2_tuples: [Vec<(usize, u32, u32, usize)>; 32],
    pub scratch_hash_table: [Vec<u64>; 32],
    pub scratch_added_indices: [Vec<usize>; 32],
    pub scratch_projs: [Vec<u64>; 32],
    pub scratch_active_bits: [Vec<u64>; 32],
    pub scratch_guess_counts: [Vec<u16>; 32],
    pub scratch_guess_hashes: [Vec<u64>; 32],
    pub scratch_guess_p_lbs: [Vec<u32>; 32],
    pub scratch_guess_child_max_ks: [Vec<u16>; 32],
    pub scratch_z_tables: [Vec<u64>; 32],
    pub scratch_state_counts: Vec<u16>,
    pub scratch_useless_projs: Vec<u64>,
}

impl SolverScratch {
    pub fn new(num_guesses: usize, num_responses: usize) -> Self {
        let mut s = Self {
            scratch_is_in_set: std::array::from_fn(|_| Vec::new()),
            scratch_sorted_sets: std::array::from_fn(|_| Vec::new()),
            scratch_phase1_tuples: std::array::from_fn(|_| Vec::new()),
            scratch_phase2_tuples: std::array::from_fn(|_| Vec::new()),
            scratch_hash_table: std::array::from_fn(|_| Vec::new()),
            scratch_added_indices: std::array::from_fn(|_| Vec::with_capacity(14855)),
            scratch_projs: std::array::from_fn(|_| Vec::new()),
            scratch_active_bits: std::array::from_fn(|_| Vec::new()),
            scratch_guess_counts: std::array::from_fn(|_| Vec::new()),
            scratch_guess_hashes: std::array::from_fn(|_| Vec::new()),
            scratch_guess_p_lbs: std::array::from_fn(|_| Vec::new()),
            scratch_guess_child_max_ks: std::array::from_fn(|_| Vec::new()),
            scratch_z_tables: std::array::from_fn(|_| Vec::new()),
            scratch_state_counts: Vec::new(),
            scratch_useless_projs: Vec::new(),
        };
        s.fit_dictionary(num_guesses, num_responses);
        s
    }

    /// Grows the buffers to fit a dictionary of `num_guesses` and response domain `num_responses`.
    fn fit_dictionary(&mut self, num_guesses: usize, num_responses: usize) {
        let words = num_guesses.div_ceil(64);
        let table_len = ((num_guesses + num_responses) * 2)
            .next_power_of_two()
            .max(32768);
        for d in 0..32 {
            if self.scratch_is_in_set[d].len() < words {
                self.scratch_is_in_set[d].resize(words, 0);
            }
            if self.scratch_sorted_sets[d].len() < num_guesses {
                self.scratch_sorted_sets[d].resize(num_guesses, 0);
            }
            if self.scratch_projs[d].len() < num_guesses {
                self.scratch_projs[d].resize(num_guesses, 0);
            }
            if self.scratch_hash_table[d].len() < table_len {
                self.scratch_hash_table[d].resize(table_len, 0);
            }
            if self.scratch_active_bits[d].len() < words {
                self.scratch_active_bits[d].resize(words, 0);
            }
            if self.scratch_guess_counts[d].len() < num_responses {
                self.scratch_guess_counts[d].resize(num_responses, 0);
            }
            if self.scratch_guess_hashes[d].len() < num_responses {
                self.scratch_guess_hashes[d].resize(num_responses, 0);
            }
            if self.scratch_guess_p_lbs[d].len() < num_responses {
                self.scratch_guess_p_lbs[d].resize(num_responses, 0);
            }
            if self.scratch_guess_child_max_ks[d].len() < num_responses {
                self.scratch_guess_child_max_ks[d].resize(num_responses, 0);
            }
            if self.scratch_z_tables[d].len() < 40 * num_responses {
                self.scratch_z_tables[d].resize(40 * num_responses, 0);
            }
        }
        if self.scratch_state_counts.len() < num_responses {
            self.scratch_state_counts.resize(num_responses, 0);
        }
        if self.scratch_useless_projs.len() < num_responses {
            self.scratch_useless_projs.resize(num_responses, 0);
        }
    }
}

thread_local! {
    static THREAD_SCRATCH: std::cell::RefCell<Option<Box<SolverScratch>>> = const { std::cell::RefCell::new(None) };
}

/// Specialized solver for a concrete word length with compile-time response parameters:
/// - `R`: Total response count (3^word_len)
/// - `M`: Bitmask u64 words count (div_ceil(R, 64))
/// - `WIN`: Response code for the all-green win response
/// - `Elem`: Storage element type (`u8` for len <= 5, `Response` for len > 5)
pub struct SpecializedSolver<
    'a,
    const R: usize,
    const M: usize,
    const WIN: usize,
    Elem: ResponseElem,
> {
    pub metrics: &'a Metrics,
    pub max_k: usize,
    pub matrix: &'a ResponseMatrix,
    pub dict: &'a crate::dict::Dictionary,
    capacity_bounds_2d: &'a [Vec<u32>],
    pub cache: &'a crate::cache::GlobalCache,
    global_beta: &'a AtomicU32,
    equiv_cache: &'a EquivCache,
    pub current_cost_so_far: u32,
    pub scratch: Option<Box<SolverScratch>>,
    _phantom: std::marker::PhantomData<Elem>,
}

impl<'a, const R: usize, const M: usize, const WIN: usize, Elem: ResponseElem> Drop
    for SpecializedSolver<'a, R, M, WIN, Elem>
{
    fn drop(&mut self) {
        if let Some(scratch) = self.scratch.take() {
            THREAD_SCRATCH.with(|ts| *ts.borrow_mut() = Some(scratch));
        }
    }
}

impl<'a, const R: usize, const M: usize, const WIN: usize, Elem: ResponseElem>
    SpecializedSolver<'a, R, M, WIN, Elem>
{
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
        let mut scratch = THREAD_SCRATCH
            .with(|ts| ts.borrow_mut().take())
            .unwrap_or_else(|| Box::new(SolverScratch::new(dict.guesses.len(), R)));
        scratch.fit_dictionary(dict.guesses.len(), R);

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
            scratch: Some(scratch),
            _phantom: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub fn quick_bucket_eval_specialized(
        matrix: &ResponseMatrix,
        dict: &Dictionary,
        bucket: &[usize],
    ) -> (u32, bool) {
        let n = bucket.len();
        if n > 15 {
            return (2 * n as u32, false);
        }
        match n {
            0 => (0, true),
            1 => (1, true),
            2 => (3, true),
            3 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                let g0 = unsafe { *cand_to_guess_ptr.add(c0) };
                let g0_ptr = unsafe { data_ptr.add(g0 * num_cand) };
                if unsafe { *g0_ptr.add(c1) != *g0_ptr.add(c2) } {
                    return (5, true);
                }

                let g1 = unsafe { *cand_to_guess_ptr.add(c1) };
                let g1_ptr = unsafe { data_ptr.add(g1 * num_cand) };
                if unsafe { *g1_ptr.add(c0) != *g1_ptr.add(c2) } {
                    return (5, true);
                }

                let g2 = unsafe { *cand_to_guess_ptr.add(c2) };
                let g2_ptr = unsafe { data_ptr.add(g2 * num_cand) };
                if unsafe { *g2_ptr.add(c0) != *g2_ptr.add(c1) } {
                    return (5, true);
                }

                (6, true)
            }
            4 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT4: [[usize; 3]; 4] = [[1, 2, 3], [2, 3, 0], [3, 0, 1], [0, 1, 2]];
                let c = [c0, c1, c2, c3];
                let mut best_inside = u32::MAX;

                for i in 0..4 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic] = ROT4[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };

                    if ra != rb && ra != rc && rb != rc {
                        return (7, true);
                    }

                    if best_inside != 8 && (ra != rb || rb != rc) {
                        best_inside = 8;
                    }
                }

                if best_inside == 8 {
                    (8, true)
                } else {
                    (8, false)
                }
            }
            5 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let c4 = unsafe { *bucket_ptr.add(4) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT5: [[usize; 4]; 5] = [
                    [1, 2, 3, 4],
                    [2, 3, 4, 0],
                    [3, 4, 0, 1],
                    [4, 0, 1, 2],
                    [0, 1, 2, 3],
                ];
                let c = [c0, c1, c2, c3, c4];
                let mut best_inside = u32::MAX;

                for i in 0..5 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic, id] = ROT5[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];
                    let cd = c[id];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };
                    let rd = unsafe { *gi_ptr.add(cd) };

                    let eq_ab = ra == rb;
                    let eq_ac = ra == rc;
                    let eq_ad = ra == rd;
                    let eq_bc = rb == rc;
                    let eq_bd = rb == rd;
                    let eq_cd = rc == rd;

                    let coll_mask = (eq_ab as u8)
                        | ((eq_ac as u8) << 1)
                        | ((eq_ad as u8) << 2)
                        | ((eq_bc as u8) << 3)
                        | ((eq_bd as u8) << 4)
                        | ((eq_cd as u8) << 5);

                    if coll_mask == 0 {
                        return (9, true);
                    }

                    if best_inside != 10 && (coll_mask & (coll_mask - 1)) == 0 {
                        best_inside = 10;
                    }
                }

                if best_inside == 10 {
                    (10, true)
                } else {
                    (10, false)
                }
            }
            6 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let c4 = unsafe { *bucket_ptr.add(4) };
                let c5 = unsafe { *bucket_ptr.add(5) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT6: [[usize; 5]; 6] = [
                    [1, 2, 3, 4, 5],
                    [2, 3, 4, 5, 0],
                    [3, 4, 5, 0, 1],
                    [4, 5, 0, 1, 2],
                    [5, 0, 1, 2, 3],
                    [0, 1, 2, 3, 4],
                ];
                let c = [c0, c1, c2, c3, c4, c5];
                let mut best_inside = u32::MAX;

                for i in 0..6 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic, id, ie] = ROT6[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];
                    let cd = c[id];
                    let ce = c[ie];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };
                    let rd = unsafe { *gi_ptr.add(cd) };
                    let re = unsafe { *gi_ptr.add(ce) };

                    let eq_ab = ra == rb;
                    let eq_ac = ra == rc;
                    let eq_ad = ra == rd;
                    let eq_ae = ra == re;
                    let eq_bc = rb == rc;
                    let eq_bd = rb == rd;
                    let eq_be = rb == re;
                    let eq_cd = rc == rd;
                    let eq_ce = rc == re;
                    let eq_de = rd == re;

                    let coll_mask = (eq_ab as u16)
                        | ((eq_ac as u16) << 1)
                        | ((eq_ad as u16) << 2)
                        | ((eq_ae as u16) << 3)
                        | ((eq_bc as u16) << 4)
                        | ((eq_bd as u16) << 5)
                        | ((eq_be as u16) << 6)
                        | ((eq_cd as u16) << 7)
                        | ((eq_ce as u16) << 8)
                        | ((eq_de as u16) << 9);

                    if coll_mask == 0 {
                        return (11, true);
                    }

                    if best_inside != 12 && (coll_mask & (coll_mask - 1)) == 0 {
                        best_inside = 12;
                    }
                }

                if best_inside == 12 {
                    (12, true)
                } else {
                    (12, false)
                }
            }
            7 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let c4 = unsafe { *bucket_ptr.add(4) };
                let c5 = unsafe { *bucket_ptr.add(5) };
                let c6 = unsafe { *bucket_ptr.add(6) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT7: [[usize; 6]; 7] = [
                    [1, 2, 3, 4, 5, 6],
                    [2, 3, 4, 5, 6, 0],
                    [3, 4, 5, 6, 0, 1],
                    [4, 5, 6, 0, 1, 2],
                    [5, 6, 0, 1, 2, 3],
                    [6, 0, 1, 2, 3, 4],
                    [0, 1, 2, 3, 4, 5],
                ];
                let c = [c0, c1, c2, c3, c4, c5, c6];
                let mut best_inside = u32::MAX;

                for i in 0..7 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic, id, ie, if_] = ROT7[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];
                    let cd = c[id];
                    let ce = c[ie];
                    let cf = c[if_];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };
                    let rd = unsafe { *gi_ptr.add(cd) };
                    let re = unsafe { *gi_ptr.add(ce) };
                    let rf = unsafe { *gi_ptr.add(cf) };

                    let eq_ab = ra == rb;
                    let eq_ac = ra == rc;
                    let eq_ad = ra == rd;
                    let eq_ae = ra == re;
                    let eq_af = ra == rf;
                    let eq_bc = rb == rc;
                    let eq_bd = rb == rd;
                    let eq_be = rb == re;
                    let eq_bf = rb == rf;
                    let eq_cd = rc == rd;
                    let eq_ce = rc == re;
                    let eq_cf = rc == rf;
                    let eq_de = rd == re;
                    let eq_df = rd == rf;
                    let eq_ef = re == rf;

                    let coll_mask = (eq_ab as u16)
                        | ((eq_ac as u16) << 1)
                        | ((eq_ad as u16) << 2)
                        | ((eq_ae as u16) << 3)
                        | ((eq_af as u16) << 4)
                        | ((eq_bc as u16) << 5)
                        | ((eq_bd as u16) << 6)
                        | ((eq_be as u16) << 7)
                        | ((eq_bf as u16) << 8)
                        | ((eq_cd as u16) << 9)
                        | ((eq_ce as u16) << 10)
                        | ((eq_cf as u16) << 11)
                        | ((eq_de as u16) << 12)
                        | ((eq_df as u16) << 13)
                        | ((eq_ef as u16) << 14);

                    if coll_mask == 0 {
                        return (13, true);
                    }

                    if best_inside != 14 && (coll_mask & (coll_mask - 1)) == 0 {
                        best_inside = 14;
                    }
                }

                if best_inside == 14 {
                    (14, true)
                } else {
                    (14, false)
                }
            }
            8 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let c4 = unsafe { *bucket_ptr.add(4) };
                let c5 = unsafe { *bucket_ptr.add(5) };
                let c6 = unsafe { *bucket_ptr.add(6) };
                let c7 = unsafe { *bucket_ptr.add(7) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT8: [[usize; 7]; 8] = [
                    [1, 2, 3, 4, 5, 6, 7],
                    [2, 3, 4, 5, 6, 7, 0],
                    [3, 4, 5, 6, 7, 0, 1],
                    [4, 5, 6, 7, 0, 1, 2],
                    [5, 6, 7, 0, 1, 2, 3],
                    [6, 7, 0, 1, 2, 3, 4],
                    [7, 0, 1, 2, 3, 4, 5],
                    [0, 1, 2, 3, 4, 5, 6],
                ];
                let c = [c0, c1, c2, c3, c4, c5, c6, c7];
                let mut best_inside = u32::MAX;

                for i in 0..8 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic, id, ie, if_, ig] = ROT8[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];
                    let cd = c[id];
                    let ce = c[ie];
                    let cf = c[if_];
                    let cg = c[ig];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };
                    let rd = unsafe { *gi_ptr.add(cd) };
                    let re = unsafe { *gi_ptr.add(ce) };
                    let rf = unsafe { *gi_ptr.add(cf) };
                    let rg = unsafe { *gi_ptr.add(cg) };

                    let eq_ab = ra == rb;
                    let eq_ac = ra == rc;
                    let eq_ad = ra == rd;
                    let eq_ae = ra == re;
                    let eq_af = ra == rf;
                    let eq_ag = ra == rg;

                    let eq_bc = rb == rc;
                    let eq_bd = rb == rd;
                    let eq_be = rb == re;
                    let eq_bf = rb == rf;
                    let eq_bg = rb == rg;

                    let eq_cd = rc == rd;
                    let eq_ce = rc == re;
                    let eq_cf = rc == rf;
                    let eq_cg = rc == rg;

                    let eq_de = rd == re;
                    let eq_df = rd == rf;
                    let eq_dg = rd == rg;

                    let eq_ef = re == rf;
                    let eq_eg = re == rg;

                    let eq_fg = rf == rg;

                    let coll_mask = (eq_ab as u32)
                        | ((eq_ac as u32) << 1)
                        | ((eq_ad as u32) << 2)
                        | ((eq_ae as u32) << 3)
                        | ((eq_af as u32) << 4)
                        | ((eq_ag as u32) << 5)
                        | ((eq_bc as u32) << 6)
                        | ((eq_bd as u32) << 7)
                        | ((eq_be as u32) << 8)
                        | ((eq_bf as u32) << 9)
                        | ((eq_bg as u32) << 10)
                        | ((eq_cd as u32) << 11)
                        | ((eq_ce as u32) << 12)
                        | ((eq_cf as u32) << 13)
                        | ((eq_cg as u32) << 14)
                        | ((eq_de as u32) << 15)
                        | ((eq_df as u32) << 16)
                        | ((eq_dg as u32) << 17)
                        | ((eq_ef as u32) << 18)
                        | ((eq_eg as u32) << 19)
                        | ((eq_fg as u32) << 20);

                    if coll_mask == 0 {
                        return (15, true);
                    }

                    if best_inside != 16 && (coll_mask & (coll_mask - 1)) == 0 {
                        best_inside = 16;
                    }
                }

                if best_inside == 16 {
                    (16, true)
                } else {
                    (16, false)
                }
            }
            9 => {
                let bucket_ptr = bucket.as_ptr();
                let c0 = unsafe { *bucket_ptr };
                let c1 = unsafe { *bucket_ptr.add(1) };
                let c2 = unsafe { *bucket_ptr.add(2) };
                let c3 = unsafe { *bucket_ptr.add(3) };
                let c4 = unsafe { *bucket_ptr.add(4) };
                let c5 = unsafe { *bucket_ptr.add(5) };
                let c6 = unsafe { *bucket_ptr.add(6) };
                let c7 = unsafe { *bucket_ptr.add(7) };
                let c8 = unsafe { *bucket_ptr.add(8) };
                let num_cand = matrix.num_candidates;
                let data_ptr = matrix.data_ptr::<Elem>();
                let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();

                const ROT9: [[usize; 8]; 9] = [
                    [1, 2, 3, 4, 5, 6, 7, 8],
                    [2, 3, 4, 5, 6, 7, 8, 0],
                    [3, 4, 5, 6, 7, 8, 0, 1],
                    [4, 5, 6, 7, 8, 0, 1, 2],
                    [5, 6, 7, 8, 0, 1, 2, 3],
                    [6, 7, 8, 0, 1, 2, 3, 4],
                    [7, 8, 0, 1, 2, 3, 4, 5],
                    [8, 0, 1, 2, 3, 4, 5, 6],
                    [0, 1, 2, 3, 4, 5, 6, 7],
                ];
                let c = [c0, c1, c2, c3, c4, c5, c6, c7, c8];
                let mut best_inside = u32::MAX;

                for i in 0..9 {
                    let gi = unsafe { *cand_to_guess_ptr.add(c[i]) };
                    let gi_ptr = unsafe { data_ptr.add(gi * num_cand) };

                    let [ia, ib, ic, id, ie, if_, ig, ih] = ROT9[i];
                    let ca = c[ia];
                    let cb = c[ib];
                    let cc = c[ic];
                    let cd = c[id];
                    let ce = c[ie];
                    let cf = c[if_];
                    let cg = c[ig];
                    let ch = c[ih];

                    let ra = unsafe { *gi_ptr.add(ca) };
                    let rb = unsafe { *gi_ptr.add(cb) };
                    let rc = unsafe { *gi_ptr.add(cc) };
                    let rd = unsafe { *gi_ptr.add(cd) };
                    let re = unsafe { *gi_ptr.add(ce) };
                    let rf = unsafe { *gi_ptr.add(cf) };
                    let rg = unsafe { *gi_ptr.add(cg) };
                    let rh = unsafe { *gi_ptr.add(ch) };

                    let eq_ab = ra == rb;
                    let eq_ac = ra == rc;
                    let eq_ad = ra == rd;
                    let eq_ae = ra == re;
                    let eq_af = ra == rf;
                    let eq_ag = ra == rg;
                    let eq_ah = ra == rh;

                    let eq_bc = rb == rc;
                    let eq_bd = rb == rd;
                    let eq_be = rb == re;
                    let eq_bf = rb == rf;
                    let eq_bg = rb == rg;
                    let eq_bh = rb == rh;

                    let eq_cd = rc == rd;
                    let eq_ce = rc == re;
                    let eq_cf = rc == rf;
                    let eq_cg = rc == rg;
                    let eq_ch = rc == rh;

                    let eq_de = rd == re;
                    let eq_df = rd == rf;
                    let eq_dg = rd == rg;
                    let eq_dh = rd == rh;

                    let eq_ef = re == rf;
                    let eq_eg = re == rg;
                    let eq_eh = re == rh;

                    let eq_fg = rf == rg;
                    let eq_fh = rf == rh;

                    let eq_gh = rg == rh;

                    let coll_mask = (eq_ab as u32)
                        | ((eq_ac as u32) << 1)
                        | ((eq_ad as u32) << 2)
                        | ((eq_ae as u32) << 3)
                        | ((eq_af as u32) << 4)
                        | ((eq_ag as u32) << 5)
                        | ((eq_ah as u32) << 6)
                        | ((eq_bc as u32) << 7)
                        | ((eq_bd as u32) << 8)
                        | ((eq_be as u32) << 9)
                        | ((eq_bf as u32) << 10)
                        | ((eq_bg as u32) << 11)
                        | ((eq_bh as u32) << 12)
                        | ((eq_cd as u32) << 13)
                        | ((eq_ce as u32) << 14)
                        | ((eq_cf as u32) << 15)
                        | ((eq_cg as u32) << 16)
                        | ((eq_ch as u32) << 17)
                        | ((eq_de as u32) << 18)
                        | ((eq_df as u32) << 19)
                        | ((eq_dg as u32) << 20)
                        | ((eq_dh as u32) << 21)
                        | ((eq_ef as u32) << 22)
                        | ((eq_eg as u32) << 23)
                        | ((eq_eh as u32) << 24)
                        | ((eq_fg as u32) << 25)
                        | ((eq_fh as u32) << 26)
                        | ((eq_gh as u32) << 27);

                    if coll_mask == 0 {
                        return (17, true);
                    }

                    if best_inside != 18 && (coll_mask & (coll_mask - 1)) == 0 {
                        best_inside = 18;
                    }
                }

                if best_inside == 18 {
                    (18, true)
                } else {
                    (18, false)
                }
            }
            _ => {
                let mut best_inside = u32::MAX;
                let target_exact = (2 * n) as u32;
                let data_ptr = matrix.data_ptr::<Elem>();

                if M <= 4 {
                    let bucket_ptr = bucket.as_ptr();
                    let cand_to_guess_ptr = dict.candidate_to_guess.as_ptr();
                    for &ci in bucket {
                        let gi = unsafe { *cand_to_guess_ptr.add(ci) };
                        let g_ptr = unsafe { data_ptr.add(gi * matrix.num_candidates) };

                        let mut seen_mask = [0u64; M];
                        let seen_ptr = seen_mask.as_mut_ptr();
                        let mut collisions = 0;
                        let max_collisions = if best_inside == target_exact { 1 } else { 2 };

                        for i in 0..n {
                            let c = unsafe { *bucket_ptr.add(i) };
                            let r = unsafe { (*g_ptr.add(c)).to_usize() };
                            let word_idx = r >> 6;
                            let bit = 1u64 << (r & 63);
                            let word = unsafe { &mut *seen_ptr.add(word_idx) };
                            if (*word & bit) != 0 {
                                collisions += 1;
                                if collisions >= max_collisions {
                                    break;
                                }
                            } else {
                                *word |= bit;
                            }
                        }

                        if collisions == 0 {
                            return ((2 * n - 1) as u32, true);
                        }
                        if collisions == 1 && max_collisions == 2 {
                            best_inside = target_exact;
                        }
                    }
                } else {
                    let bucket_ptr = bucket.as_ptr();
                    for &ci in bucket {
                        let gi = unsafe { *dict.candidate_to_guess.get_unchecked(ci) };
                        let g_ptr = unsafe { data_ptr.add(gi * matrix.num_candidates) };

                        let mut seen = [0u16; 16];
                        let mut seen_len = 0;
                        let mut collisions = 0;
                        let max_collisions = if best_inside == target_exact { 1 } else { 2 };

                        for i in 0..n {
                            let c = unsafe { *bucket_ptr.add(i) };
                            let r = unsafe { (*g_ptr.add(c)).to_usize() as u16 };
                            let mut duplicate = false;
                            for &prev in &seen[..seen_len] {
                                if prev == r {
                                    duplicate = true;
                                    break;
                                }
                            }
                            if duplicate {
                                collisions += 1;
                                if collisions >= max_collisions {
                                    break;
                                }
                            } else {
                                seen[seen_len] = r;
                                seen_len += 1;
                            }
                        }

                        if collisions == 0 {
                            return ((2 * n - 1) as u32, true);
                        }
                        if collisions == 1 && max_collisions == 2 {
                            best_inside = target_exact;
                        }
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

    #[inline(always)]
    pub fn min_state_val(
        &mut self,
        set: &[usize],
        parent_active_guesses: &[u64],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        let mut hash = 0;
        let zobrist_ptr = self.matrix.zobrist.as_ptr();
        for &c in set {
            hash ^= unsafe { *zobrist_ptr.add(c) };
        }
        self.min_state_val_with_hash(
            set,
            hash,
            parent_active_guesses,
            beta,
            depth,
            parent_max_k,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn min_state_val_with_hash(
        &mut self,
        set: &[usize],
        hash: u64,
        parent_active_guesses: &[u64],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
        known_quick_lb: u32,
    ) -> u32 {
        let c_len = set.len();
        if c_len <= 2 {
            return match c_len {
                0 => 0,
                1 => 1,
                _ => 3,
            };
        }

        self.metrics
            .max_depth
            .fetch_max(depth, std::sync::atomic::Ordering::Relaxed);

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
        if known_quick_lb != 0 {
            cached_lower_bound = cached_lower_bound.max(known_quick_lb);
        } else if c_len <= 15 {
            let (val, is_exact) = Self::quick_bucket_eval_specialized(self.matrix, self.dict, set);
            if is_exact {
                self.cache.insert_set(hash, set, val, true);
                cert_record!(1, set, val, true);
                return val;
            }
            cached_lower_bound = cached_lower_bound.max(val);
        }

        let parent_cap_bounds = &self.capacity_bounds_2d[parent_max_k];
        let parent_lb = unsafe { *parent_cap_bounds.get_unchecked(c_len) };
        cached_lower_bound = cached_lower_bound.max(parent_lb);
        let current_global_beta = self.global_beta.load(Ordering::Relaxed);
        if cached_lower_bound >= beta
            || self.current_cost_so_far + cached_lower_bound >= current_global_beta
        {
            self.cache.insert_set(hash, set, cached_lower_bound, false);
            cert_record!(2, set, cached_lower_bound, false);
            return cached_lower_bound;
        }

        let mut best_val = beta;
        let mut cut_by_global = false;
        let set_hash = hash;

        let should_cache_equiv = set.len() >= 10 && set.len() < 40;
        let shard_idx = (set_hash as usize) % 1024;
        let mut active_bits_scratch: Option<Vec<u64>> = None;
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
            let mut bits =
                std::mem::take(&mut self.scratch.as_mut().unwrap().scratch_active_bits[depth]);
            let num_u64s = self.dict.guesses.len().div_ceil(64);
            bits.clear();
            bits.resize(num_u64s, 0);

            let scratch_ref = self.scratch.as_mut().unwrap();
            let table = &mut scratch_ref.scratch_hash_table[depth];
            let table_ptr = table.as_mut_ptr();
            let added_indices = &mut scratch_ref.scratch_added_indices[depth];
            added_indices.clear();
            let num_guesses = self.dict.guesses.len();

            let projs = &mut scratch_ref.scratch_projs[depth];
            let projs_raw_ptr = projs.as_ptr();

            let active_count: u32 = parent_active_guesses.iter().map(|&b| b.count_ones()).sum();
            let table_size = (((active_count + R as u32) * 2).next_power_of_two() as usize)
                .clamp(1024, table.len());
            let mask = table_size - 1;
            let shift = 64 - table_size.trailing_zeros();

            let data_cg_ptr = self.matrix.data_cg_ptr::<Elem>();
            let zobrist_ptr = self.matrix.zobrist.as_ptr();
            let set_ptr = set.as_ptr();

            let z_tables_ptr = scratch_ref.scratch_z_tables[depth].as_mut_ptr();
            let mut c_vals = [0usize; 40];
            for c_idx in 0..c_len {
                let c = unsafe { *set_ptr.add(c_idx) };
                c_vals[c_idx] = c;
                let z = unsafe { *zobrist_ptr.add(c) };
                let c_table_ptr = unsafe { z_tables_ptr.add(c_idx * R) };
                for r in 0..R {
                    unsafe {
                        *c_table_ptr.add(r) = z.wrapping_mul(r as u64 + 1);
                    }
                }
            }

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

                let projs_chunk = &mut projs[chunk_start..chunk_end];
                projs_chunk.fill(0);
                let chunk_len = chunk_end - chunk_start;
                let projs_ptr = projs_chunk.as_mut_ptr();
                for c_idx in 0..c_len {
                    let c = unsafe { *c_vals.get_unchecked(c_idx) };
                    let c_table_ptr = unsafe { z_tables_ptr.add(c_idx * R) };
                    let c_off = c * num_guesses + chunk_start;
                    let c_ptr = unsafe { data_cg_ptr.add(c_off) };
                    let mut i = 0;
                    while i + 7 < chunk_len {
                        let r0 = unsafe { (*c_ptr.add(i)).to_usize() };
                        let r1 = unsafe { (*c_ptr.add(i + 1)).to_usize() };
                        let r2 = unsafe { (*c_ptr.add(i + 2)).to_usize() };
                        let r3 = unsafe { (*c_ptr.add(i + 3)).to_usize() };
                        let r4 = unsafe { (*c_ptr.add(i + 4)).to_usize() };
                        let r5 = unsafe { (*c_ptr.add(i + 5)).to_usize() };
                        let r6 = unsafe { (*c_ptr.add(i + 6)).to_usize() };
                        let r7 = unsafe { (*c_ptr.add(i + 7)).to_usize() };
                        unsafe {
                            *projs_ptr.add(i) ^= *c_table_ptr.add(r0);
                            *projs_ptr.add(i + 1) ^= *c_table_ptr.add(r1);
                            *projs_ptr.add(i + 2) ^= *c_table_ptr.add(r2);
                            *projs_ptr.add(i + 3) ^= *c_table_ptr.add(r3);
                            *projs_ptr.add(i + 4) ^= *c_table_ptr.add(r4);
                            *projs_ptr.add(i + 5) ^= *c_table_ptr.add(r5);
                            *projs_ptr.add(i + 6) ^= *c_table_ptr.add(r6);
                            *projs_ptr.add(i + 7) ^= *c_table_ptr.add(r7);
                        }
                        i += 8;
                    }
                    while i < chunk_len {
                        let r = unsafe { (*c_ptr.add(i)).to_usize() };
                        unsafe {
                            *projs_ptr.add(i) ^= *c_table_ptr.add(r);
                        }
                        i += 1;
                    }
                }
            }

            for r in 0..R {
                let mut p = 0u64;
                for c_idx in 0..c_len {
                    let c_table_ptr = unsafe { z_tables_ptr.add(c_idx * R) };
                    p ^= unsafe { *c_table_ptr.add(r) };
                }
                if p == 0 {
                    p = 1;
                }
                let proj = p;
                let mut idx = (proj.wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                loop {
                    let slot = unsafe { *table_ptr.add(idx) };
                    if slot == 0 {
                        unsafe {
                            *table_ptr.add(idx) = proj;
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

            let bits_ptr = bits.as_mut_ptr();
            for (block_idx, &block) in parent_active_guesses.iter().enumerate() {
                if block == 0 {
                    continue;
                }
                let mut b = block;
                let mut kept_mask = 0u64;
                while b != 0 {
                    let tz = b.trailing_zeros();
                    let g = block_idx * 64 + tz as usize;
                    b &= b - 1;

                    let proj = unsafe { *projs_raw_ptr.add(g) };
                    let mut idx = (proj.wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                    loop {
                        let slot = unsafe { *table_ptr.add(idx) };
                        if slot == 0 {
                            unsafe {
                                *table_ptr.add(idx) = proj;
                            }
                            added_indices.push(idx);
                            kept_mask |= 1 << tz;
                            break;
                        }
                        if slot == proj {
                            break;
                        }
                        idx = (idx + 1) & mask;
                    }
                }
                unsafe {
                    *bits_ptr.add(block_idx) = kept_mask;
                }
            }

            let added_len = added_indices.len();
            let added_ptr = added_indices.as_ptr();
            for i in 0..added_len {
                unsafe {
                    let idx = *added_ptr.add(i);
                    *table_ptr.add(idx) = 0;
                }
            }

            let rc_new = if should_cache_equiv {
                let mut cache_mut = self.equiv_cache[shard_idx].write().unwrap();
                if let Some(cached) = cache_mut.get(&set_hash) {
                    self.metrics
                        .equiv_cache_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Some(std::sync::Arc::clone(cached))
                } else {
                    let rc = std::sync::Arc::new(bits.clone());
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
                        - bits.iter().map(|b| b.count_ones() as usize).sum::<usize>(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                None
            };

            if let Some(rc_val) = &rc_new {
                slice_ptr = rc_val.as_slice() as *const _;
            } else {
                slice_ptr = bits.as_slice() as *const _;
            }
            active_bits_scratch = Some(bits);
            rc_new
        };

        let active_guesses_slice = unsafe { &*slice_ptr };
        if let Some(bits) = active_bits_scratch {
            self.scratch.as_mut().unwrap().scratch_active_bits[depth] = bits;
        }

        let mut local_lb = parent_lb.max(cached_lower_bound);
        let counts_ptr = self
            .scratch
            .as_mut()
            .unwrap()
            .scratch_state_counts
            .as_mut_ptr();
        let mut non_empty_indices = std::mem::MaybeUninit::<[u16; R]>::uninit();
        let non_empty_indices_ptr = non_empty_indices.as_mut_ptr() as *mut u16;

        let parent_cap_bounds_ptr = parent_cap_bounds.as_ptr();
        let matrix_data_ptr = self.matrix.data_ptr::<Elem>();

        let mut phase1_tuples =
            std::mem::take(&mut self.scratch.as_mut().unwrap().scratch_phase1_tuples[depth]);
        phase1_tuples.clear();
        let mut phase2_tuples =
            std::mem::take(&mut self.scratch.as_mut().unwrap().scratch_phase2_tuples[depth]);
        phase2_tuples.clear();

        let mut local_max_k = 0;
        let mut valid_phase1_k = 0;
        let mut min_val_found = u32::MAX;
        let k_to_beat_best = (3 * c_len as u32).saturating_sub(best_val) as usize;
        let mut min_k_needed = (local_max_k + 1).min(k_to_beat_best);
        let mut max_allowed_duplicates = c_len.saturating_sub(min_k_needed);

        let cand_to_guess_ptr = self.dict.candidate_to_guess.as_ptr();
        let active_guesses_ptr = active_guesses_slice.as_ptr();
        let set_ptr = set.as_ptr();
        for (cand_idx, &cand) in set.iter().enumerate() {
            let g = unsafe { *cand_to_guess_ptr.add(cand) };
            if (unsafe { *active_guesses_ptr.add(g >> 6) } & (1u64 << (g & 63))) == 0 {
                continue;
            }
            let mut num_non_empty = 0;
            let mut duplicates = 0;
            let mut aborted = false;
            let g_off = g * self.matrix.num_candidates;
            let g_ptr = unsafe { matrix_data_ptr.add(g_off) };
            for i in 0..cand_idx {
                let c = unsafe { *set_ptr.add(i) };
                let r = unsafe { (*g_ptr.add(c)).to_usize() };
                unsafe {
                    let cnt_ptr = counts_ptr.add(r);
                    let cnt = *cnt_ptr;
                    if cnt == 0 {
                        *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                        num_non_empty += 1;
                    } else {
                        duplicates += 1;
                        if duplicates > max_allowed_duplicates {
                            for j in 0..num_non_empty {
                                let r_idx = *non_empty_indices_ptr.add(j) as usize;
                                *counts_ptr.add(r_idx) = 0;
                            }
                            aborted = true;
                            break;
                        }
                    }
                    *cnt_ptr = cnt + 1;
                }
            }
            if !aborted {
                for i in (cand_idx + 1)..c_len {
                    let c = unsafe { *set_ptr.add(i) };
                    let r = unsafe { (*g_ptr.add(c)).to_usize() };
                    unsafe {
                        let cnt_ptr = counts_ptr.add(r);
                        let cnt = *cnt_ptr;
                        if cnt == 0 {
                            *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                            num_non_empty += 1;
                        } else {
                            duplicates += 1;
                            if duplicates > max_allowed_duplicates {
                                for j in 0..num_non_empty {
                                    let r_idx = *non_empty_indices_ptr.add(j) as usize;
                                    *counts_ptr.add(r_idx) = 0;
                                }
                                aborted = true;
                                break;
                            }
                        }
                        *cnt_ptr = cnt + 1;
                    }
                }
            }
            if aborted {
                min_val_found = min_val_found.min(best_val);
                continue;
            }
            let total_non_empty = num_non_empty + 1;
            if total_non_empty > local_max_k {
                local_max_k = total_non_empty;
                min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
            }
            let (expected_rem, lb_cost) = if num_non_empty == 1 {
                let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                unsafe {
                    *counts_ptr.add(r0) = 0;
                }
                let rem_c = (c_len as u32) - 1;
                (
                    1 + rem_c * rem_c,
                    (c_len as u32) + unsafe { *parent_cap_bounds_ptr.add(rem_c as usize) },
                )
            } else if num_non_empty == 2 {
                let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                let count0 = unsafe { *counts_ptr.add(r0) } as u32;
                let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                unsafe {
                    *counts_ptr.add(r0) = 0;
                    *counts_ptr.add(r1) = 0;
                }
                let count1 = (c_len as u32 - 1) - count0;
                (
                    1 + count0 * count0 + count1 * count1,
                    (c_len as u32)
                        + unsafe { *parent_cap_bounds_ptr.add(count0 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count1 as usize) },
                )
            } else if num_non_empty == 3 {
                let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                let count0 = unsafe { *counts_ptr.add(r0) } as u32;
                let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                let count1 = unsafe { *counts_ptr.add(r1) } as u32;
                let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                unsafe {
                    *counts_ptr.add(r0) = 0;
                    *counts_ptr.add(r1) = 0;
                    *counts_ptr.add(r2) = 0;
                }
                let count2 = (c_len as u32 - 1) - count0 - count1;
                (
                    1 + count0 * count0 + count1 * count1 + count2 * count2,
                    (c_len as u32)
                        + unsafe { *parent_cap_bounds_ptr.add(count0 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count1 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count2 as usize) },
                )
            } else if num_non_empty == 4 {
                let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                let count0 = unsafe { *counts_ptr.add(r0) } as u32;
                let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                let count1 = unsafe { *counts_ptr.add(r1) } as u32;
                let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                let count2 = unsafe { *counts_ptr.add(r2) } as u32;
                let r3 = unsafe { *non_empty_indices_ptr.add(3) as usize };
                unsafe {
                    *counts_ptr.add(r0) = 0;
                    *counts_ptr.add(r1) = 0;
                    *counts_ptr.add(r2) = 0;
                    *counts_ptr.add(r3) = 0;
                }
                let count3 = (c_len as u32 - 1) - count0 - count1 - count2;
                (
                    1 + count0 * count0 + count1 * count1 + count2 * count2 + count3 * count3,
                    (c_len as u32)
                        + unsafe { *parent_cap_bounds_ptr.add(count0 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count1 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count2 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count3 as usize) },
                )
            } else if num_non_empty == 5 {
                let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                let count0 = unsafe { *counts_ptr.add(r0) } as u32;
                let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                let count1 = unsafe { *counts_ptr.add(r1) } as u32;
                let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                let count2 = unsafe { *counts_ptr.add(r2) } as u32;
                let r3 = unsafe { *non_empty_indices_ptr.add(3) as usize };
                let count3 = unsafe { *counts_ptr.add(r3) } as u32;
                let r4 = unsafe { *non_empty_indices_ptr.add(4) as usize };
                unsafe {
                    *counts_ptr.add(r0) = 0;
                    *counts_ptr.add(r1) = 0;
                    *counts_ptr.add(r2) = 0;
                    *counts_ptr.add(r3) = 0;
                    *counts_ptr.add(r4) = 0;
                }
                let count4 = (c_len as u32 - 1) - count0 - count1 - count2 - count3;
                (
                    1 + count0 * count0
                        + count1 * count1
                        + count2 * count2
                        + count3 * count3
                        + count4 * count4,
                    (c_len as u32)
                        + unsafe { *parent_cap_bounds_ptr.add(count0 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count1 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count2 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count3 as usize) }
                        + unsafe { *parent_cap_bounds_ptr.add(count4 as usize) },
                )
            } else {
                let mut exp = 1u32;
                let mut lb = c_len as u32;
                for i in 0..num_non_empty {
                    let r_idx = unsafe { *non_empty_indices_ptr.add(i) as usize };
                    let count = unsafe { *counts_ptr.add(r_idx) };
                    unsafe {
                        *counts_ptr.add(r_idx) = 0;
                    }
                    exp += (count as u32) * (count as u32);
                    lb += unsafe { *parent_cap_bounds_ptr.add(count as usize) };
                }
                (exp, lb)
            };
            if lb_cost < beta {
                if total_non_empty > valid_phase1_k {
                    valid_phase1_k = total_non_empty;
                }
                phase1_tuples.push((g, expected_rem, lb_cost, total_non_empty));
            } else {
                min_val_found = min_val_found.min(lb_cost);
            }
            if min_k_needed > c_len {
                break;
            }
        }
        if phase1_tuples.len() == 2 {
            let t0 = phase1_tuples[0];
            let t1 = phase1_tuples[1];
            let k0 = (t0.2, t0.1, t0.0);
            let k1 = (t1.2, t1.1, t1.0);
            if k0 > k1 {
                phase1_tuples[0] = t1;
                phase1_tuples[1] = t0;
            }
        } else if phase1_tuples.len() == 3 {
            let mut t0 = phase1_tuples[0];
            let mut t1 = phase1_tuples[1];
            let mut t2 = phase1_tuples[2];
            let mut k0 = (t0.2, t0.1, t0.0);
            let mut k1 = (t1.2, t1.1, t1.0);
            let mut k2 = (t2.2, t2.1, t2.0);
            if k0 > k1 {
                std::mem::swap(&mut t0, &mut t1);
                std::mem::swap(&mut k0, &mut k1);
            }
            if k1 > k2 {
                std::mem::swap(&mut t1, &mut t2);
                std::mem::swap(&mut k1, &mut k2);
            }
            if k0 > k1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            phase1_tuples[0] = t0;
            phase1_tuples[1] = t1;
            phase1_tuples[2] = t2;
        } else if phase1_tuples.len() == 4 {
            let mut t0 = phase1_tuples[0];
            let mut t1 = phase1_tuples[1];
            let mut t2 = phase1_tuples[2];
            let mut t3 = phase1_tuples[3];
            let mut k0 = (t0.2, t0.1, t0.0);
            let mut k1 = (t1.2, t1.1, t1.0);
            let mut k2 = (t2.2, t2.1, t2.0);
            let mut k3 = (t3.2, t3.1, t3.0);
            if k0 > k1 {
                std::mem::swap(&mut t0, &mut t1);
                std::mem::swap(&mut k0, &mut k1);
            }
            if k2 > k3 {
                std::mem::swap(&mut t2, &mut t3);
                std::mem::swap(&mut k2, &mut k3);
            }
            if k0 > k2 {
                std::mem::swap(&mut t0, &mut t2);
                std::mem::swap(&mut k0, &mut k2);
            }
            if k1 > k3 {
                std::mem::swap(&mut t1, &mut t3);
                std::mem::swap(&mut k1, &mut k3);
            }
            if k1 > k2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            phase1_tuples[0] = t0;
            phase1_tuples[1] = t1;
            phase1_tuples[2] = t2;
            phase1_tuples[3] = t3;
        } else if phase1_tuples.len() > 4 && phase1_tuples.len() <= 32 {
            let p1_ptr = phase1_tuples.as_mut_ptr();
            let p1_len = phase1_tuples.len();
            for j in 1..p1_len {
                let key = unsafe { *p1_ptr.add(j) };
                let mut k = j;
                while k > 0 {
                    let prev = unsafe { *p1_ptr.add(k - 1) };
                    let prev_greater = prev.2 > key.2
                        || (prev.2 == key.2
                            && (prev.1 > key.1 || (prev.1 == key.1 && prev.0 > key.0)));
                    if prev_greater {
                        unsafe {
                            *p1_ptr.add(k) = prev;
                        }
                        k -= 1;
                    } else {
                        break;
                    }
                }
                unsafe {
                    *p1_ptr.add(k) = key;
                }
            }
        } else if phase1_tuples.len() > 32 {
            phase1_tuples.sort_unstable_by_key(|a| (a.2, a.1, a.0));
        }

        for (idx, &(_g, _, g_lb, _non_empty)) in phase1_tuples.iter().enumerate() {
            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            if self.current_cost_so_far + local_lb >= current_global_beta {
                cut_by_global = true;
                break;
            }
            if self.current_cost_so_far + g_lb >= current_global_beta {
                cut_by_global = true;
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
            if self.current_cost_so_far + val >= self.global_beta.load(Ordering::Relaxed) {
                cut_by_global = true;
            }
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
            let mut valid_phase2_k = 0;
            let k_to_beat_best =
                (3 * c_len as u32).saturating_sub(best_val.saturating_sub(1)) as usize;
            let mut min_k_needed = (local_max_k + 1).min(k_to_beat_best);
            if c_len >= min_k_needed {
                let mut max_allowed_duplicates = c_len - min_k_needed;
                let mut is_in_set =
                    std::mem::take(&mut self.scratch.as_mut().unwrap().scratch_is_in_set[depth]);
                let is_in_set_ptr = is_in_set.as_mut_ptr();
                let cand_to_guess_ptr = self.dict.candidate_to_guess.as_ptr();
                let cand_masks_ptr = self.matrix.candidate_masks.as_ptr();
                let set_ptr = set.as_ptr();
                let mut c_mask = 0u32;
                let mut multi_c_mask = 0u32;
                for i in 0..c_len {
                    let c = unsafe { *set_ptr.add(i) };
                    let g = unsafe { *cand_to_guess_ptr.add(c) };
                    let m = unsafe { *cand_masks_ptr.add(c) };
                    multi_c_mask |= c_mask & m;
                    c_mask |= m;
                    unsafe {
                        *is_in_set_ptr.add(g / 64) |= 1 << (g % 64);
                    }
                }
                let guess_masks_ptr = self.matrix.guess_masks.as_ptr();
                let num_guesses = self.dict.guesses.len();
                let data_c_g_ptr = self.matrix.data_cg_ptr::<Elem>();
                let mut stack_c_ptrs = std::mem::MaybeUninit::<[*const Elem; 256]>::uninit();
                let stack_c_ptrs_ptr = stack_c_ptrs.as_mut_ptr() as *mut *const Elem;
                let heap_c_ptrs = if c_len > 256 {
                    let mut v = Vec::with_capacity(c_len);
                    let set_ptr = set.as_ptr();
                    for i in 0..c_len {
                        let c = unsafe { *set_ptr.add(i) };
                        v.push(unsafe { data_c_g_ptr.add(c * num_guesses) });
                    }
                    Some(v)
                } else {
                    None
                };
                let c_ptrs: &[*const Elem] = if c_len <= 256 {
                    let set_ptr = set.as_ptr();
                    for i in 0..c_len {
                        let c = unsafe { *set_ptr.add(i) };
                        unsafe {
                            *stack_c_ptrs_ptr.add(i) = data_c_g_ptr.add(c * num_guesses);
                        }
                    }
                    unsafe { std::slice::from_raw_parts(stack_c_ptrs_ptr, c_len) }
                } else {
                    heap_c_ptrs.as_ref().unwrap()
                };

                let c_ptrs_ptr = c_ptrs.as_ptr();
                'guess_loop: for (block_idx, &block) in active_guesses_slice.iter().enumerate() {
                    if block == 0 {
                        continue;
                    }
                    let mut b = block & !unsafe { *is_in_set_ptr.add(block_idx) };
                    while b != 0 {
                        let tz = b.trailing_zeros();
                        let g = block_idx * 64 + tz as usize;
                        b &= b - 1;

                        let g_mask = unsafe { *guess_masks_ptr.add(g) };
                        let shared = g_mask & c_mask;
                        if (shared & shared.wrapping_sub(1)) == 0 && (shared & multi_c_mask) == 0 {
                            continue;
                        }

                        let mut num_non_empty = 0;
                        let mut duplicates = 0;
                        let mut aborted = false;
                        for c_idx in 0..c_len {
                            let c_base = unsafe { *c_ptrs_ptr.add(c_idx) };
                            let r = unsafe { (*c_base.add(g)).to_usize() };
                            unsafe {
                                let cnt_ptr = counts_ptr.add(r);
                                let cnt = *cnt_ptr;
                                if cnt == 0 {
                                    *non_empty_indices_ptr.add(num_non_empty) = r as u16;
                                    num_non_empty += 1;
                                } else {
                                    duplicates += 1;
                                    if duplicates > max_allowed_duplicates {
                                        for i in 0..num_non_empty {
                                            let r_idx = *non_empty_indices_ptr.add(i) as usize;
                                            *counts_ptr.add(r_idx) = 0;
                                        }
                                        aborted = true;
                                        break;
                                    }
                                }
                                *cnt_ptr = cnt + 1;
                            }
                        }
                        if aborted {
                            min_val_found = min_val_found.min(best_val);
                            continue;
                        }
                        if num_non_empty == 1 {
                            unsafe {
                                let r_idx = *non_empty_indices_ptr.add(0) as usize;
                                *counts_ptr.add(r_idx) = 0;
                            }
                            continue;
                        }
                        if num_non_empty == 2 {
                            let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                            let count0 = unsafe { *counts_ptr.add(r0) };
                            let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                            unsafe {
                                *counts_ptr.add(r0) = 0;
                                *counts_ptr.add(r1) = 0;
                            }
                            if count0 == 1 || count0 == (c_len as u16 - 1) {
                                continue;
                            }
                            let c0 = count0 as u32;
                            let c1 = (c_len as u32) - c0;
                            let expected_rem = c0 * c0 + c1 * c1;
                            let lb_cost = (c_len as u32)
                                + unsafe { *parent_cap_bounds_ptr.add(c0 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c1 as usize) };
                            if 2 > local_max_k {
                                local_max_k = 2;
                                min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                                max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
                            }
                            if lb_cost < best_val {
                                if 2 > valid_phase2_k {
                                    valid_phase2_k = 2;
                                }
                                phase2_tuples.push((g, expected_rem, lb_cost, 2));
                            } else {
                                min_val_found = min_val_found.min(lb_cost);
                            }
                            if min_k_needed > c_len {
                                break 'guess_loop;
                            }
                            continue;
                        }
                        if num_non_empty == 3 {
                            let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                            let count0 = unsafe { *counts_ptr.add(r0) };
                            let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                            let count1 = unsafe { *counts_ptr.add(r1) };
                            let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                            unsafe {
                                *counts_ptr.add(r0) = 0;
                                *counts_ptr.add(r1) = 0;
                                *counts_ptr.add(r2) = 0;
                            }
                            let c0 = count0 as u32;
                            let c1 = count1 as u32;
                            let c2 = (c_len as u32) - c0 - c1;
                            let expected_rem = c0 * c0 + c1 * c1 + c2 * c2;
                            let lb_cost = (c_len as u32)
                                + unsafe { *parent_cap_bounds_ptr.add(c0 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c1 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c2 as usize) };
                            if 3 > local_max_k {
                                local_max_k = 3;
                                min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                                max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
                            }
                            if lb_cost < best_val {
                                if 3 > valid_phase2_k {
                                    valid_phase2_k = 3;
                                }
                                phase2_tuples.push((g, expected_rem, lb_cost, 3));
                            } else {
                                min_val_found = min_val_found.min(lb_cost);
                            }
                            if min_k_needed > c_len {
                                break 'guess_loop;
                            }
                            continue;
                        }
                        if num_non_empty == 4 {
                            let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                            let count0 = unsafe { *counts_ptr.add(r0) };
                            let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                            let count1 = unsafe { *counts_ptr.add(r1) };
                            let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                            let count2 = unsafe { *counts_ptr.add(r2) };
                            let r3 = unsafe { *non_empty_indices_ptr.add(3) as usize };
                            unsafe {
                                *counts_ptr.add(r0) = 0;
                                *counts_ptr.add(r1) = 0;
                                *counts_ptr.add(r2) = 0;
                                *counts_ptr.add(r3) = 0;
                            }
                            let c0 = count0 as u32;
                            let c1 = count1 as u32;
                            let c2 = count2 as u32;
                            let c3 = (c_len as u32) - c0 - c1 - c2;
                            let expected_rem = c0 * c0 + c1 * c1 + c2 * c2 + c3 * c3;
                            let lb_cost = (c_len as u32)
                                + unsafe { *parent_cap_bounds_ptr.add(c0 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c1 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c2 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c3 as usize) };
                            if 4 > local_max_k {
                                local_max_k = 4;
                                min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                                max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
                            }
                            if lb_cost < best_val {
                                if 4 > valid_phase2_k {
                                    valid_phase2_k = 4;
                                }
                                phase2_tuples.push((g, expected_rem, lb_cost, 4));
                            } else {
                                min_val_found = min_val_found.min(lb_cost);
                            }
                            if min_k_needed > c_len {
                                break 'guess_loop;
                            }
                            continue;
                        }
                        if num_non_empty == 5 {
                            let r0 = unsafe { *non_empty_indices_ptr.add(0) as usize };
                            let count0 = unsafe { *counts_ptr.add(r0) };
                            let r1 = unsafe { *non_empty_indices_ptr.add(1) as usize };
                            let count1 = unsafe { *counts_ptr.add(r1) };
                            let r2 = unsafe { *non_empty_indices_ptr.add(2) as usize };
                            let count2 = unsafe { *counts_ptr.add(r2) };
                            let r3 = unsafe { *non_empty_indices_ptr.add(3) as usize };
                            let count3 = unsafe { *counts_ptr.add(r3) };
                            let r4 = unsafe { *non_empty_indices_ptr.add(4) as usize };
                            unsafe {
                                *counts_ptr.add(r0) = 0;
                                *counts_ptr.add(r1) = 0;
                                *counts_ptr.add(r2) = 0;
                                *counts_ptr.add(r3) = 0;
                                *counts_ptr.add(r4) = 0;
                            }
                            let c0 = count0 as u32;
                            let c1 = count1 as u32;
                            let c2 = count2 as u32;
                            let c3 = count3 as u32;
                            let c4 = (c_len as u32) - c0 - c1 - c2 - c3;
                            let expected_rem = c0 * c0 + c1 * c1 + c2 * c2 + c3 * c3 + c4 * c4;
                            let lb_cost = (c_len as u32)
                                + unsafe { *parent_cap_bounds_ptr.add(c0 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c1 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c2 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c3 as usize) }
                                + unsafe { *parent_cap_bounds_ptr.add(c4 as usize) };
                            if 5 > local_max_k {
                                local_max_k = 5;
                                min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                                max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
                            }
                            if lb_cost < best_val {
                                if 5 > valid_phase2_k {
                                    valid_phase2_k = 5;
                                }
                                phase2_tuples.push((g, expected_rem, lb_cost, 5));
                            } else {
                                min_val_found = min_val_found.min(lb_cost);
                            }
                            if min_k_needed > c_len {
                                break 'guess_loop;
                            }
                            continue;
                        }
                        let mut expected_rem = 0u32;
                        let mut lb_cost = c_len as u32;
                        for i in 0..num_non_empty {
                            let r_idx = unsafe { *non_empty_indices_ptr.add(i) as usize };
                            let count = unsafe { *counts_ptr.add(r_idx) };
                            unsafe {
                                *counts_ptr.add(r_idx) = 0;
                            }
                            expected_rem += (count as u32) * (count as u32);
                            debug_assert_ne!(r_idx, WIN);
                            lb_cost += unsafe { *parent_cap_bounds_ptr.add(count as usize) };
                        }
                        if num_non_empty > local_max_k {
                            local_max_k = num_non_empty;
                            min_k_needed = (local_max_k + 1).min(k_to_beat_best);
                            max_allowed_duplicates = c_len.saturating_sub(min_k_needed);
                        }
                        if lb_cost < best_val {
                            if num_non_empty > valid_phase2_k {
                                valid_phase2_k = num_non_empty;
                            }
                            phase2_tuples.push((g, expected_rem, lb_cost, num_non_empty));
                        } else {
                            min_val_found = min_val_found.min(lb_cost);
                        }
                        if min_k_needed > c_len {
                            break 'guess_loop;
                        }
                    }
                }
                let set_ptr = set.as_ptr();
                for i in 0..c_len {
                    let c = unsafe { *set_ptr.add(i) };
                    let g = unsafe { *cand_to_guess_ptr.add(c) };
                    unsafe {
                        *is_in_set_ptr.add(g / 64) = 0;
                    }
                }
                self.scratch.as_mut().unwrap().scratch_is_in_set[depth] = is_in_set;
            } else {
                min_val_found = min_val_found.min(best_val);
            }

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
            if local_lb < best_val && self.current_cost_so_far + local_lb >= current_global_beta {
                cut_by_global = true;
            }
            if local_lb < best_val && self.current_cost_so_far + local_lb < current_global_beta {
                if phase2_tuples.len() == 2 {
                    let t0 = phase2_tuples[0];
                    let t1 = phase2_tuples[1];
                    let k0 = (t0.2, t0.1, t0.0);
                    let k1 = (t1.2, t1.1, t1.0);
                    if k0 > k1 {
                        phase2_tuples[0] = t1;
                        phase2_tuples[1] = t0;
                    }
                } else if phase2_tuples.len() == 3 {
                    let mut t0 = phase2_tuples[0];
                    let mut t1 = phase2_tuples[1];
                    let mut t2 = phase2_tuples[2];
                    let mut k0 = (t0.2, t0.1, t0.0);
                    let mut k1 = (t1.2, t1.1, t1.0);
                    let mut k2 = (t2.2, t2.1, t2.0);
                    if k0 > k1 {
                        std::mem::swap(&mut t0, &mut t1);
                        std::mem::swap(&mut k0, &mut k1);
                    }
                    if k1 > k2 {
                        std::mem::swap(&mut t1, &mut t2);
                        std::mem::swap(&mut k1, &mut k2);
                    }
                    if k0 > k1 {
                        std::mem::swap(&mut t0, &mut t1);
                    }
                    phase2_tuples[0] = t0;
                    phase2_tuples[1] = t1;
                    phase2_tuples[2] = t2;
                } else if phase2_tuples.len() == 4 {
                    let mut t0 = phase2_tuples[0];
                    let mut t1 = phase2_tuples[1];
                    let mut t2 = phase2_tuples[2];
                    let mut t3 = phase2_tuples[3];
                    let mut k0 = (t0.2, t0.1, t0.0);
                    let mut k1 = (t1.2, t1.1, t1.0);
                    let mut k2 = (t2.2, t2.1, t2.0);
                    let mut k3 = (t3.2, t3.1, t3.0);
                    if k0 > k1 {
                        std::mem::swap(&mut t0, &mut t1);
                        std::mem::swap(&mut k0, &mut k1);
                    }
                    if k2 > k3 {
                        std::mem::swap(&mut t2, &mut t3);
                        std::mem::swap(&mut k2, &mut k3);
                    }
                    if k0 > k2 {
                        std::mem::swap(&mut t0, &mut t2);
                        std::mem::swap(&mut k0, &mut k2);
                    }
                    if k1 > k3 {
                        std::mem::swap(&mut t1, &mut t3);
                        std::mem::swap(&mut k1, &mut k3);
                    }
                    if k1 > k2 {
                        std::mem::swap(&mut t1, &mut t2);
                    }
                    phase2_tuples[0] = t0;
                    phase2_tuples[1] = t1;
                    phase2_tuples[2] = t2;
                    phase2_tuples[3] = t3;
                } else if phase2_tuples.len() > 4 && phase2_tuples.len() <= 32 {
                    let p2_ptr = phase2_tuples.as_mut_ptr();
                    let p2_len = phase2_tuples.len();
                    for j in 1..p2_len {
                        let key = unsafe { *p2_ptr.add(j) };
                        let mut k = j;
                        while k > 0 {
                            let prev = unsafe { *p2_ptr.add(k - 1) };
                            let prev_greater = prev.2 > key.2
                                || (prev.2 == key.2
                                    && (prev.1 > key.1 || (prev.1 == key.1 && prev.0 > key.0)));
                            if prev_greater {
                                unsafe {
                                    *p2_ptr.add(k) = prev;
                                }
                                k -= 1;
                            } else {
                                break;
                            }
                        }
                        unsafe {
                            *p2_ptr.add(k) = key;
                        }
                    }
                } else if phase2_tuples.len() > 32 {
                    phase2_tuples.sort_unstable_by_key(|a| (a.2, a.1, a.0));
                }

                for (idx, &(_g, _, g_lb, _non_empty)) in phase2_tuples.iter().enumerate() {
                    let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                    if self.current_cost_so_far + local_lb >= current_global_beta {
                        cut_by_global = true;
                        break;
                    }
                    if self.current_cost_so_far + g_lb >= current_global_beta {
                        cut_by_global = true;
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
                    if self.current_cost_so_far + val >= self.global_beta.load(Ordering::Relaxed) {
                        cut_by_global = true;
                    }
                    if val < best_val {
                        best_val = val;
                        if best_val <= local_lb {
                            break;
                        }
                    }
                }
            }
        } else {
            if best_val > phase2_effective_lb {
                cut_by_global = true;
            }
            min_val_found = min_val_found.min(phase2_min_lb);
        }

        self.scratch.as_mut().unwrap().scratch_phase1_tuples[depth] = phase1_tuples;
        self.scratch.as_mut().unwrap().scratch_phase2_tuples[depth] = phase2_tuples;

        let is_exact = best_val < beta;
        if is_exact {
            if !cut_by_global {
                self.cache.insert_set(hash, set, best_val, true);
            }
            crate::certlog::record_tagged(10, set, best_val, true, cut_by_global);
            best_val
        } else {
            let mut final_lb = beta.max(local_lb);
            if best_val > local_lb && min_val_found != u32::MAX {
                final_lb = final_lb.max(min_val_found);
            }
            if !cut_by_global {
                self.cache.insert_set(hash, set, final_lb, false);
            }
            crate::certlog::record_tagged(11, set, final_lb, false, cut_by_global);
            final_lb
        }
    }

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
        let g_ptr = unsafe { self.matrix.data_ptr::<Elem>().add(g_offset) };

        if c_len <= 16 {
            let mut local_counts = std::mem::MaybeUninit::<[(u16, u16); 16]>::uninit();
            let local_counts_ptr = local_counts.as_mut_ptr() as *mut (u16, u16);
            let mut c_responses = std::mem::MaybeUninit::<[u16; 16]>::uninit();
            let c_responses_ptr = c_responses.as_mut_ptr() as *mut u16;
            let set_ptr = set.as_ptr();
            let c0 = unsafe { *set_ptr };
            let r0 = unsafe { (*g_ptr.add(c0)).to_usize() as u16 };
            unsafe {
                *local_counts_ptr = (r0, 1);
                *c_responses_ptr = r0;
            }
            let mut num_non_empty = 1;
            let mut prev_idx = 0usize;
            for cand_idx in 1..c_len {
                let c = unsafe { *set_ptr.add(cand_idx) };
                let r = unsafe { (*g_ptr.add(c)).to_usize() as u16 };
                unsafe {
                    *c_responses_ptr.add(cand_idx) = r;
                }
                let prev_entry = unsafe { &mut *local_counts_ptr.add(prev_idx) };
                if prev_entry.0 == r {
                    prev_entry.1 += 1;
                    continue;
                }
                let mut found = false;
                for i in 0..num_non_empty {
                    let entry = unsafe { &mut *local_counts_ptr.add(i) };
                    if entry.0 == r {
                        entry.1 += 1;
                        prev_idx = i;
                        found = true;
                        break;
                    }
                }
                if !found {
                    prev_idx = num_non_empty;
                    unsafe {
                        *local_counts_ptr.add(num_non_empty) = (r, 1);
                    }
                    num_non_empty += 1;
                }
            }

            if num_non_empty == 1 {
                return beta;
            }

            let cutoff = beta.min(current_global_beta.saturating_sub(self.current_cost_so_far));
            let mut cost = c_len as u32;
            if cost >= cutoff {
                self.metrics
                    .pruned_by_bounds
                    .fetch_add(1, Ordering::Relaxed);
                return cost;
            }

            let mut max_p_len = 0;
            let mut num_eval = 0;
            let mut eval_entries = [(0u16, 0u16); 5];

            for i in 0..num_non_empty {
                let entry = unsafe { *local_counts_ptr.add(i) };
                let r_idx = entry.0 as usize;
                if r_idx == WIN {
                    continue;
                }
                let p_len = entry.1 as usize;
                if p_len > max_p_len {
                    max_p_len = p_len;
                }
                let lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                cost += lb;
                if cost >= cutoff {
                    self.metrics
                        .pruned_by_bounds
                        .fetch_add(1, Ordering::Relaxed);
                    return cost;
                }
                if p_len > 2 {
                    eval_entries[num_eval] = entry;
                    num_eval += 1;
                }
            }

            if max_p_len <= 2 {
                return cost;
            }

            if num_eval == 2 {
                if eval_entries[0].1 < eval_entries[1].1 {
                    eval_entries.swap(0, 1);
                }
            } else if num_eval == 3 {
                if eval_entries[0].1 < eval_entries[1].1 {
                    eval_entries.swap(0, 1);
                }
                if eval_entries[1].1 < eval_entries[2].1 {
                    eval_entries.swap(1, 2);
                }
                if eval_entries[0].1 < eval_entries[1].1 {
                    eval_entries.swap(0, 1);
                }
            } else if num_eval == 4 {
                if eval_entries[0].1 < eval_entries[1].1 {
                    eval_entries.swap(0, 1);
                }
                if eval_entries[2].1 < eval_entries[3].1 {
                    eval_entries.swap(2, 3);
                }
                if eval_entries[0].1 < eval_entries[2].1 {
                    eval_entries.swap(0, 2);
                }
                if eval_entries[1].1 < eval_entries[3].1 {
                    eval_entries.swap(1, 3);
                }
                if eval_entries[1].1 < eval_entries[2].1 {
                    eval_entries.swap(1, 2);
                }
            } else if num_eval == 5 {
                for j in 1..5 {
                    let key = eval_entries[j];
                    let mut k = j;
                    while k > 0 && eval_entries[k - 1].1 < key.1 {
                        eval_entries[k] = eval_entries[k - 1];
                        k -= 1;
                    }
                    eval_entries[k] = key;
                }
            }

            let zobrist_ptr = self.matrix.zobrist.as_ptr();
            let mut eval_hashes = [0u64; 5];
            for cand_idx in 0..c_len {
                let r = unsafe { *c_responses_ptr.add(cand_idx) };
                for j in 0..num_eval {
                    if eval_entries[j].0 == r {
                        let c = unsafe { *set_ptr.add(cand_idx) };
                        eval_hashes[j] ^= unsafe { *zobrist_ptr.add(c) };
                        break;
                    }
                }
            }

            let mut small_p = [[0usize; 16]; 5];
            let mut small_p_len = [0u8; 5];
            let mut small_p_hash = [0u64; 5];
            let mut small_p_extracted = [false; 5];
            let mut small_used_val = [0u32; 5];
            let mut small_is_exact = [false; 5];
            let mut all_exact = true;

            for i in 0..num_eval {
                let entry = eval_entries[i];
                let p_len = entry.1 as usize;
                let p_hash = eval_hashes[i];
                small_p_len[i] = p_len as u8;
                small_p_hash[i] = p_hash;
                let initial_lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                let (used_val, is_exact) =
                    if let Some((cached_val, is_exact)) = self.cache.get(p_hash) {
                        self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                        (cached_val.max(initial_lb), is_exact)
                    } else {
                        let r_target = entry.0;
                        let mut p_idx = 0;
                        let set_ptr = set.as_ptr();
                        for cand_idx in 0..c_len {
                            let r = unsafe { *c_responses_ptr.add(cand_idx) };
                            if r == r_target {
                                let c = unsafe { *set_ptr.add(cand_idx) };
                                small_p[i][p_idx] = c;
                                p_idx += 1;
                                if p_idx == p_len {
                                    break;
                                }
                            }
                        }
                        small_p_extracted[i] = true;
                        let p_slice = &small_p[i][..p_len];
                        let (quick_val, is_exact) =
                            Self::quick_bucket_eval_specialized(self.matrix, self.dict, p_slice);
                        let used_val = quick_val.max(initial_lb);
                        self.cache.insert_set(p_hash, p_slice, used_val, is_exact);
                        cert_record!(4, p_slice, used_val, is_exact);
                        (used_val, is_exact)
                    };

                small_used_val[i] = used_val;
                small_is_exact[i] = is_exact;
                if !is_exact {
                    all_exact = false;
                }

                cost += used_val - initial_lb;

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                    self.metrics
                        .pruned_by_bounds
                        .fetch_add(1, Ordering::Relaxed);
                    return cost;
                }
            }

            if all_exact {
                return cost;
            }

            for i in 0..num_eval {
                if small_is_exact[i] {
                    continue;
                }
                let p_len = small_p_len[i] as usize;
                if !small_p_extracted[i] {
                    let entry = eval_entries[i];
                    let r_target = entry.0;
                    let mut p_idx = 0;
                    let set_ptr = set.as_ptr();
                    for cand_idx in 0..c_len {
                        let r = unsafe { *c_responses_ptr.add(cand_idx) };
                        if r == r_target {
                            let c = unsafe { *set_ptr.add(cand_idx) };
                            small_p[i][p_idx] = c;
                            p_idx += 1;
                            if p_idx == p_len {
                                break;
                            }
                        }
                    }
                    small_p_extracted[i] = true;
                }
                let p_slice = &small_p[i][..p_len];
                let p_hash = small_p_hash[i];
                let used_val = small_used_val[i];

                let b = cost - used_val;
                let new_beta = beta - b;
                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                let effective_beta =
                    new_beta.min(current_global_beta.saturating_sub(self.current_cost_so_far + b));

                if effective_beta == 0 {
                    return cost;
                }

                self.current_cost_so_far += b;
                let val = self.min_state_val_with_hash(
                    p_slice,
                    p_hash,
                    active_guesses_slice,
                    effective_beta,
                    depth + 1,
                    p_len.min(parent_max_k).max(2),
                    used_val,
                );
                self.current_cost_so_far -= b;

                if b + val >= beta
                    || self.current_cost_so_far + b + val
                        >= self.global_beta.load(Ordering::Relaxed)
                {
                    return b + val;
                }
                cost = b + val;
            }
            cost
        } else {
            let (counts_ptr, hashes_ptr, p_lbs_ptr, child_max_ks_ptr) = {
                let s = self.scratch.as_mut().unwrap();
                (
                    s.scratch_guess_counts[depth].as_mut_ptr(),
                    s.scratch_guess_hashes[depth].as_mut_ptr(),
                    s.scratch_guess_p_lbs[depth].as_mut_ptr(),
                    s.scratch_guess_child_max_ks[depth].as_mut_ptr(),
                )
            };

            let mut eval_indices = std::mem::MaybeUninit::<[u16; R]>::uninit();
            let eval_indices_ptr = eval_indices.as_mut_ptr() as *mut u16;
            let mut num_eval = 0;

            struct GuessCleanup {
                counts_ptr: *mut u16,
                hashes_ptr: *mut u64,
                r_len: usize,
                has_hashes: bool,
            }

            impl Drop for GuessCleanup {
                #[inline(always)]
                fn drop(&mut self) {
                    unsafe {
                        std::ptr::write_bytes(self.counts_ptr, 0, self.r_len);
                        if self.has_hashes {
                            std::ptr::write_bytes(self.hashes_ptr, 0, self.r_len);
                        }
                    }
                }
            }

            let mut cleanup = GuessCleanup {
                counts_ptr,
                hashes_ptr,
                r_len: R,
                has_hashes: false,
            };

            let set_ptr = set.as_ptr();
            let mut i = 0;
            while i + 3 < c_len {
                let c0 = unsafe { *set_ptr.add(i) };
                let c1 = unsafe { *set_ptr.add(i + 1) };
                let c2 = unsafe { *set_ptr.add(i + 2) };
                let c3 = unsafe { *set_ptr.add(i + 3) };
                let r0 = unsafe { (*g_ptr.add(c0)).to_usize() };
                let r1 = unsafe { (*g_ptr.add(c1)).to_usize() };
                let r2 = unsafe { (*g_ptr.add(c2)).to_usize() };
                let r3 = unsafe { (*g_ptr.add(c3)).to_usize() };

                unsafe {
                    *counts_ptr.add(r0) += 1;
                    *counts_ptr.add(r1) += 1;
                    *counts_ptr.add(r2) += 1;
                    *counts_ptr.add(r3) += 1;
                }
                i += 4;
            }
            while i < c_len {
                let c = unsafe { *set_ptr.add(i) };
                let r = unsafe { (*g_ptr.add(c)).to_usize() };
                unsafe {
                    *counts_ptr.add(r) += 1;
                }
                i += 1;
            }

            let current_global_beta = self.global_beta.load(Ordering::Relaxed);
            let cutoff = beta.min(current_global_beta.saturating_sub(self.current_cost_so_far));

            let mut cost = set.len() as u32;
            let mut max_p_len = 0;
            let mut num_buckets = 0;

            for r in 0..R {
                let p_len = unsafe { *counts_ptr.add(r) as usize };
                if p_len == 0 {
                    continue;
                }
                num_buckets += 1;
                if r == WIN {
                    continue;
                }
                if p_len > max_p_len {
                    max_p_len = p_len;
                }
                cost += unsafe { *parent_cap_bounds_ptr.add(p_len) };
                if cost >= cutoff {
                    self.metrics
                        .pruned_by_bounds
                        .fetch_add(1, Ordering::Relaxed);
                    return cost;
                }
                if p_len > 2 {
                    unsafe {
                        *eval_indices_ptr.add(num_eval) = r as u16;
                    }
                    num_eval += 1;
                }
            }

            if num_buckets <= 1 {
                return beta;
            }

            if max_p_len <= 2 {
                return cost;
            }

            let split_idx = if num_eval == 1 {
                let r0 = unsafe { *eval_indices_ptr as usize };
                if unsafe { *counts_ptr.add(r0) > 15 } {
                    1
                } else {
                    0
                }
            } else if num_eval == 2 {
                let mut r0 = unsafe { *eval_indices_ptr };
                let mut r1 = unsafe { *eval_indices_ptr.add(1) };
                let mut c0 = unsafe { *counts_ptr.add(r0 as usize) };
                let mut c1 = unsafe { *counts_ptr.add(r1 as usize) };
                if c0 < c1 || (c0 == c1 && r0 > r1) {
                    std::mem::swap(&mut r0, &mut r1);
                    std::mem::swap(&mut c0, &mut c1);
                    unsafe {
                        *eval_indices_ptr = r0;
                        *eval_indices_ptr.add(1) = r1;
                    }
                }
                if c0 > 15 {
                    if c1 > 15 { 2 } else { 1 }
                } else {
                    0
                }
            } else if num_eval == 3 {
                let mut r0 = unsafe { *eval_indices_ptr };
                let mut r1 = unsafe { *eval_indices_ptr.add(1) };
                let mut r2 = unsafe { *eval_indices_ptr.add(2) };
                let mut c0 = unsafe { *counts_ptr.add(r0 as usize) };
                let mut c1 = unsafe { *counts_ptr.add(r1 as usize) };
                let mut c2 = unsafe { *counts_ptr.add(r2 as usize) };
                if c0 < c1 || (c0 == c1 && r0 > r1) {
                    std::mem::swap(&mut r0, &mut r1);
                    std::mem::swap(&mut c0, &mut c1);
                }
                if c1 < c2 || (c1 == c2 && r1 > r2) {
                    std::mem::swap(&mut r1, &mut r2);
                    std::mem::swap(&mut c1, &mut c2);
                }
                if c0 < c1 || (c0 == c1 && r0 > r1) {
                    std::mem::swap(&mut r0, &mut r1);
                    std::mem::swap(&mut c0, &mut c1);
                }
                unsafe {
                    *eval_indices_ptr = r0;
                    *eval_indices_ptr.add(1) = r1;
                    *eval_indices_ptr.add(2) = r2;
                }
                if c0 > 15 {
                    if c1 > 15 {
                        if c2 > 15 { 3 } else { 2 }
                    } else {
                        1
                    }
                } else {
                    0
                }
            } else if num_eval == 4 {
                let mut r0 = unsafe { *eval_indices_ptr };
                let mut r1 = unsafe { *eval_indices_ptr.add(1) };
                let mut r2 = unsafe { *eval_indices_ptr.add(2) };
                let mut r3 = unsafe { *eval_indices_ptr.add(3) };
                let mut c0 = unsafe { *counts_ptr.add(r0 as usize) };
                let mut c1 = unsafe { *counts_ptr.add(r1 as usize) };
                let mut c2 = unsafe { *counts_ptr.add(r2 as usize) };
                let mut c3 = unsafe { *counts_ptr.add(r3 as usize) };
                if c0 < c1 || (c0 == c1 && r0 > r1) {
                    std::mem::swap(&mut r0, &mut r1);
                    std::mem::swap(&mut c0, &mut c1);
                }
                if c2 < c3 || (c2 == c3 && r2 > r3) {
                    std::mem::swap(&mut r2, &mut r3);
                    std::mem::swap(&mut c2, &mut c3);
                }
                if c0 < c2 || (c0 == c2 && r0 > r2) {
                    std::mem::swap(&mut r0, &mut r2);
                    std::mem::swap(&mut c0, &mut c2);
                }
                if c1 < c3 || (c1 == c3 && r1 > r3) {
                    std::mem::swap(&mut r1, &mut r3);
                    std::mem::swap(&mut c1, &mut c3);
                }
                if c1 < c2 || (c1 == c2 && r1 > r2) {
                    std::mem::swap(&mut r1, &mut r2);
                    std::mem::swap(&mut c1, &mut c2);
                }
                unsafe {
                    *eval_indices_ptr = r0;
                    *eval_indices_ptr.add(1) = r1;
                    *eval_indices_ptr.add(2) = r2;
                    *eval_indices_ptr.add(3) = r3;
                }
                if c0 > 15 {
                    if c1 > 15 {
                        if c2 > 15 {
                            if c3 > 15 { 4 } else { 3 }
                        } else {
                            2
                        }
                    } else {
                        1
                    }
                } else {
                    0
                }
            } else {
                for i in 1..num_eval {
                    let r_curr = unsafe { *eval_indices_ptr.add(i) };
                    let c_curr = unsafe { *counts_ptr.add(r_curr as usize) };
                    let mut j = i;
                    while j > 0 {
                        let r_prev = unsafe { *eval_indices_ptr.add(j - 1) };
                        let c_prev = unsafe { *counts_ptr.add(r_prev as usize) };
                        if c_prev < c_curr || (c_prev == c_curr && r_prev > r_curr) {
                            unsafe {
                                *eval_indices_ptr.add(j) = r_prev;
                            }
                            j -= 1;
                        } else {
                            break;
                        }
                    }
                    unsafe {
                        *eval_indices_ptr.add(j) = r_curr;
                    }
                }
                let mut split = 0;
                while split < num_eval
                    && unsafe { *counts_ptr.add(*eval_indices_ptr.add(split) as usize) > 15 }
                {
                    split += 1;
                }
                split
            };

            let mut sorted_set =
                std::mem::take(&mut self.scratch.as_mut().unwrap().scratch_sorted_sets[depth]);
            if sorted_set.len() < set.len() {
                sorted_set.resize(set.len(), 0);
            }
            let sorted_set_ptr = sorted_set.as_mut_ptr();

            let mut current_offsets = std::mem::MaybeUninit::<[u16; R]>::uninit();
            let current_offsets_ptr = current_offsets.as_mut_ptr() as *mut u16;
            let mut curr = 0;
            for r in 0..R {
                unsafe {
                    *current_offsets_ptr.add(r) = curr;
                    curr += *counts_ptr.add(r);
                }
            }

            cleanup.has_hashes = true;
            let zobrist_ptr = self.matrix.zobrist.as_ptr();
            let mut i = 0;
            while i + 1 < c_len {
                let c0 = unsafe { *set_ptr.add(i) };
                let c1 = unsafe { *set_ptr.add(i + 1) };
                let r0 = unsafe { (*g_ptr.add(c0)).to_usize() };
                let r1 = unsafe { (*g_ptr.add(c1)).to_usize() };
                let z0 = unsafe { *zobrist_ptr.add(c0) };
                let z1 = unsafe { *zobrist_ptr.add(c1) };
                unsafe {
                    let pos_ptr0 = current_offsets_ptr.add(r0);
                    let pos0 = *pos_ptr0 as usize;
                    *sorted_set_ptr.add(pos0) = c0;
                    *pos_ptr0 += 1;
                    *hashes_ptr.add(r0) ^= z0;

                    let pos_ptr1 = current_offsets_ptr.add(r1);
                    let pos1 = *pos_ptr1 as usize;
                    *sorted_set_ptr.add(pos1) = c1;
                    *pos_ptr1 += 1;
                    *hashes_ptr.add(r1) ^= z1;
                }
                i += 2;
            }
            if i < c_len {
                let c = unsafe { *set_ptr.add(i) };
                let r_idx = unsafe { (*g_ptr.add(c)).to_usize() };
                let z = unsafe { *zobrist_ptr.add(c) };
                let pos_ptr = unsafe { current_offsets_ptr.add(r_idx) };
                let pos = unsafe { *pos_ptr as usize };
                unsafe {
                    *sorted_set_ptr.add(pos) = c;
                    *pos_ptr += 1;
                    *hashes_ptr.add(r_idx) ^= z;
                }
            }

            let mut exact_mask = [0u64; M];
            let mut cached_mask = [0u64; M];
            let mut num_exact = 0;

            for i in 0..num_eval {
                let r_idx = unsafe { *eval_indices_ptr.add(i) as usize };
                let p_len = unsafe { *counts_ptr.add(r_idx) as usize };
                let cap_lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                let hash = unsafe { *hashes_ptr.add(r_idx) };
                if let Some((cached_val, is_exact)) = self.cache.get(hash) {
                    self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                    let used_val = cached_val.max(cap_lb);
                    cost += used_val - cap_lb;
                    unsafe {
                        *p_lbs_ptr.add(i) = used_val;
                    }
                    let word_idx = i >> 6;
                    let bit = 1u64 << (i & 63);
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
                        self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                        return cost;
                    }
                } else {
                    unsafe {
                        *p_lbs_ptr.add(i) = cap_lb;
                    }
                }
            }

            if num_exact == num_eval {
                self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                return cost;
            }

            for i in split_idx..num_eval {
                let word_idx = i >> 6;
                let bit = 1u64 << (i & 63);
                if (unsafe { *cached_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let r_idx = unsafe { *eval_indices_ptr.add(i) as usize };
                let p_len = unsafe { *counts_ptr.add(r_idx) as usize };
                let start = (unsafe { *current_offsets_ptr.add(r_idx) as usize }) - p_len;
                let p = unsafe { std::slice::from_raw_parts(sorted_set_ptr.add(start), p_len) };
                let hash = unsafe { *hashes_ptr.add(r_idx) };

                let (quick_val, is_exact) =
                    Self::quick_bucket_eval_specialized(self.matrix, self.dict, p);
                let lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                let used_val = quick_val.max(lb);
                cost += used_val - lb;
                unsafe {
                    *p_lbs_ptr.add(i) = used_val;
                }
                if is_exact {
                    unsafe {
                        *exact_mask.get_unchecked_mut(word_idx) |= bit;
                    }
                    num_exact += 1;
                }
                self.cache.insert_set(hash, p, used_val, is_exact);
                cert_record!(5, p, used_val, is_exact);
                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                    self.metrics
                        .pruned_by_bounds
                        .fetch_add(1, Ordering::Relaxed);
                    self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }
            }

            if num_exact == num_eval {
                self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                return cost;
            }

            for i in 0..split_idx {
                unsafe {
                    *child_max_ks_ptr.add(i) = 0;
                }
                let word_idx = i >> 6;
                let bit = 1u64 << (i & 63);
                if (unsafe { *cached_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let r_idx = unsafe { *eval_indices_ptr.add(i) as usize };
                let p_len = unsafe { *counts_ptr.add(r_idx) as usize };
                let start = (unsafe { *current_offsets_ptr.add(r_idx) as usize }) - p_len;
                let p = unsafe { std::slice::from_raw_parts(sorted_set_ptr.add(start), p_len) };
                let hash = unsafe { *hashes_ptr.add(r_idx) };

                let lb = unsafe { *parent_cap_bounds_ptr.add(p_len) };
                let target_k = crate::heuristic::min_k_for_target_bound(p_len, parent_max_k, lb);
                if target_k == 2 {
                    self.cache.insert_set(hash, p, lb, false);
                    cert_record!(6, p, lb, false);
                } else {
                    let k_cand = crate::heuristic::compute_candidate_branching_factor_with_target(
                        self.matrix,
                        p,
                        target_k,
                    );
                    if k_cand >= target_k {
                        self.cache.insert_set(hash, p, lb, false);
                        cert_record!(7, p, lb, false);
                    } else {
                        let k = crate::heuristic::compute_max_branching_factor_seq_with_target(
                            self.matrix,
                            p,
                            k_cand,
                            target_k,
                        );
                        if k >= target_k {
                            self.cache.insert_set(hash, p, lb, false);
                            cert_record!(8, p, lb, false);
                        } else {
                            unsafe {
                                *child_max_ks_ptr.add(i) = k as u16;
                            }
                            let tight_lb =
                                crate::heuristic::tight_capacity_bound(p_len, k, parent_max_k);
                            let used_val = tight_lb.max(lb);
                            cost += used_val - lb;
                            unsafe {
                                *p_lbs_ptr.add(i) = used_val;
                            }
                            self.cache.insert_set(hash, p, used_val, false);
                            cert_record!(9, p, used_val, false);
                        }
                    }
                }

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if cost >= beta || self.current_cost_so_far + cost >= current_global_beta {
                    self.metrics
                        .pruned_by_bounds
                        .fetch_add(1, Ordering::Relaxed);
                    self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }
            }

            for i in 0..num_eval {
                let word_idx = i >> 6;
                let bit = 1u64 << (i & 63);
                if (unsafe { *exact_mask.get_unchecked(word_idx) } & bit) != 0 {
                    continue;
                }
                let r_idx = unsafe { *eval_indices_ptr.add(i) as usize };
                let p_len = unsafe { *counts_ptr.add(r_idx) as usize };

                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if self.current_cost_so_far + cost >= current_global_beta {
                    self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }

                let start = (unsafe { *current_offsets_ptr.add(r_idx) as usize }) - p_len;
                let p = unsafe { std::slice::from_raw_parts(sorted_set_ptr.add(start), p_len) };

                let actual_lb = unsafe { *p_lbs_ptr.add(i) };
                let b = cost - actual_lb;
                let new_beta = beta - b;

                let effective_beta =
                    new_beta.min(current_global_beta.saturating_sub(self.current_cost_so_far + b));

                if effective_beta == 0 {
                    self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                    return cost;
                }

                let child_k = if i < split_idx {
                    let stored_k = unsafe { *child_max_ks_ptr.add(i) as usize };
                    if stored_k != 0 {
                        stored_k
                    } else {
                        p_len.min(parent_max_k)
                    }
                } else {
                    p_len.min(parent_max_k)
                };

                let hash = unsafe { *hashes_ptr.add(r_idx) };
                let known_quick = if i >= split_idx { actual_lb } else { 0 };
                self.current_cost_so_far += b;
                let val = self.min_state_val_with_hash(
                    p,
                    hash,
                    active_guesses_slice,
                    effective_beta,
                    depth + 1,
                    child_k,
                    known_quick,
                );
                self.current_cost_so_far -= b;
                let current_global_beta = self.global_beta.load(Ordering::Relaxed);
                if b + val >= beta || self.current_cost_so_far + b + val >= current_global_beta {
                    self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;
                    return b + val;
                }
                cost = b + val;
            }

            self.scratch.as_mut().unwrap().scratch_sorted_sets[depth] = sorted_set;

            cost
        }
    }
}

macro_rules! dispatch_solver {
    ($word_len:expr, $fn_name:ident ($($arg:expr),*)) => {
        match $word_len {
            1 => SpecializedSolver::<3, 1, 1, u8>::$fn_name($($arg),*),
            2 => SpecializedSolver::<9, 1, 4, u8>::$fn_name($($arg),*),
            3 => SpecializedSolver::<27, 1, 13, u8>::$fn_name($($arg),*),
            4 => SpecializedSolver::<81, 2, 40, u8>::$fn_name($($arg),*),
            5 => SpecializedSolver::<243, 4, 121, u8>::$fn_name($($arg),*),
            6 => SpecializedSolver::<729, 12, 364, Response>::$fn_name($($arg),*),
            7 => SpecializedSolver::<2187, 35, 1093, Response>::$fn_name($($arg),*),
            8 => SpecializedSolver::<6561, 103, 3280, Response>::$fn_name($($arg),*),
            9 => SpecializedSolver::<19683, 308, 9841, Response>::$fn_name($($arg),*),
            10 => SpecializedSolver::<59049, 923, 29524, Response>::$fn_name($($arg),*),
            _ => SpecializedSolver::<243, 4, 121, u8>::$fn_name($($arg),*),
        }
    };
}

macro_rules! new_solver_variant {
    ($word_len:expr, $($arg:expr),*) => {
        match $word_len {
            1 => Solver::L1(SpecializedSolver::<3, 1, 1, u8>::new_with_global_beta($($arg),*)),
            2 => Solver::L2(SpecializedSolver::<9, 1, 4, u8>::new_with_global_beta($($arg),*)),
            3 => Solver::L3(SpecializedSolver::<27, 1, 13, u8>::new_with_global_beta($($arg),*)),
            4 => Solver::L4(SpecializedSolver::<81, 2, 40, u8>::new_with_global_beta($($arg),*)),
            5 => Solver::L5(SpecializedSolver::<243, 4, 121, u8>::new_with_global_beta($($arg),*)),
            6 => Solver::L6(SpecializedSolver::<729, 12, 364, Response>::new_with_global_beta($($arg),*)),
            7 => Solver::L7(SpecializedSolver::<2187, 35, 1093, Response>::new_with_global_beta($($arg),*)),
            8 => Solver::L8(SpecializedSolver::<6561, 103, 3280, Response>::new_with_global_beta($($arg),*)),
            9 => Solver::L9(SpecializedSolver::<19683, 308, 9841, Response>::new_with_global_beta($($arg),*)),
            10 => Solver::L10(SpecializedSolver::<59049, 923, 29524, Response>::new_with_global_beta($($arg),*)),
            _ => Solver::L5(SpecializedSolver::<243, 4, 121, u8>::new_with_global_beta($($arg),*)),
        }
    };
}

macro_rules! match_solver_mut {
    ($self:expr, $inner:ident => $body:expr) => {
        match $self {
            Solver::L1($inner) => $body,
            Solver::L2($inner) => $body,
            Solver::L3($inner) => $body,
            Solver::L4($inner) => $body,
            Solver::L5($inner) => $body,
            Solver::L6($inner) => $body,
            Solver::L7($inner) => $body,
            Solver::L8($inner) => $body,
            Solver::L9($inner) => $body,
            Solver::L10($inner) => $body,
        }
    };
}

/// The optimal Wordle solver using Branch and Bound.
pub enum Solver<'a> {
    L1(SpecializedSolver<'a, 3, 1, 1, u8>),
    L2(SpecializedSolver<'a, 9, 1, 4, u8>),
    L3(SpecializedSolver<'a, 27, 1, 13, u8>),
    L4(SpecializedSolver<'a, 81, 2, 40, u8>),
    L5(SpecializedSolver<'a, 243, 4, 121, u8>),
    L6(SpecializedSolver<'a, 729, 12, 364, Response>),
    L7(SpecializedSolver<'a, 2187, 35, 1093, Response>),
    L8(SpecializedSolver<'a, 6561, 103, 3280, Response>),
    L9(SpecializedSolver<'a, 19683, 308, 9841, Response>),
    L10(SpecializedSolver<'a, 59049, 923, 29524, Response>),
}

impl<'a> Solver<'a> {
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
        new_solver_variant!(
            dict.word_len,
            matrix,
            max_k,
            dict,
            metrics,
            capacity_bounds_2d,
            cache,
            global_beta,
            equiv_cache
        )
    }

    #[inline(always)]
    pub fn quick_bucket_eval(
        matrix: &ResponseMatrix,
        dict: &Dictionary,
        bucket: &[usize],
    ) -> (u32, bool) {
        dispatch_solver!(
            dict.word_len,
            quick_bucket_eval_specialized(matrix, dict, bucket)
        )
    }

    #[inline(always)]
    pub fn min_state_val(
        &mut self,
        set: &[usize],
        parent_active_guesses: &[u64],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        match_solver_mut!(self, s => s.min_state_val(set, parent_active_guesses, beta, depth, parent_max_k))
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn min_state_val_with_hash(
        &mut self,
        set: &[usize],
        hash: u64,
        parent_active_guesses: &[u64],
        beta: u32,
        depth: usize,
        parent_max_k: usize,
        known_quick_lb: u32,
    ) -> u32 {
        match_solver_mut!(self, s => s.min_state_val_with_hash(set, hash, parent_active_guesses, beta, depth, parent_max_k, known_quick_lb))
    }

    #[inline(always)]
    pub fn min_guess_val(
        &mut self,
        set: &[usize],
        active_guesses_slice: &[u64],
        guess: usize,
        beta: u32,
        depth: usize,
        parent_max_k: usize,
    ) -> u32 {
        match_solver_mut!(self, s => s.min_guess_val(set, active_guesses_slice, guess, beta, depth, parent_max_k))
    }

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

        let mut counts = vec![0u16; matrix.num_responses];
        let mut non_empty_indices = vec![0u16; matrix.num_responses];

        let skip_projections = c_mask.count_ones() >= 26;
        let mut seen_projections = if skip_projections {
            rustc_hash::FxHashSet::default()
        } else {
            rustc_hash::FxHashSet::with_capacity_and_hasher(
                dict.guesses.len().min(4096),
                Default::default(),
            )
        };

        if matrix.num_responses <= 256 {
            let matrix_data_ptr = matrix.data_u8.as_ptr();
            for g in 0..dict.guesses.len() {
                if !skip_projections {
                    let chars = &dict.guess_chars[g];
                    let mut proj = 0u64;
                    for i in 0..dict.word_len.min(8) {
                        let li = chars[i] as u64;
                        proj |= ((li + 1) * (((c_mask >> li) & 1) as u64)) << (i * 5);
                    }
                    if !seen_projections.insert(proj) {
                        continue;
                    }
                }

                let g_offset = g * matrix.num_candidates;
                let g_ptr = unsafe { matrix_data_ptr.add(g_offset) };
                let mut num_non_empty = 0;
                for &c in set {
                    let r = unsafe { *g_ptr.add(c) as usize };
                    if counts[r] == 0 {
                        non_empty_indices[num_non_empty] = r as u16;
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
                return u32::MAX;
            }

            let best_g_offset = best_guess * matrix.num_candidates;
            let best_g_ptr = unsafe { matrix_data_ptr.add(best_g_offset) };
            for &c in set {
                let r = unsafe { *best_g_ptr.add(c) as usize };
                counts[r] += 1;
            }

            let mut offsets = vec![0usize; matrix.num_responses + 1];
            for r_idx in 0..matrix.num_responses {
                offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
            }
            let mut sorted_set = vec![0usize; set.len()];
            let mut current_offsets = offsets.clone();
            for &c in set {
                let r = unsafe { *best_g_ptr.add(c) as usize };
                let pos = current_offsets[r];
                sorted_set[pos] = c;
                current_offsets[r] += 1;
            }

            let mut cost = set.len() as u32;
            for r_idx in 0..matrix.num_responses {
                let p_len = counts[r_idx] as usize;
                if p_len == 0 || r_idx == matrix.win_response.0 as usize {
                    continue;
                }
                let start = offsets[r_idx];
                let end = start + p_len;
                cost += Self::greedy_solve(matrix, dict, &sorted_set[start..end]);
            }
            cost
        } else {
            let matrix_data_ptr = matrix.data.as_ptr();
            for g in 0..dict.guesses.len() {
                if !skip_projections {
                    let chars = &dict.guess_chars[g];
                    let mut proj = 0u64;
                    for i in 0..dict.word_len.min(8) {
                        let li = chars[i] as u64;
                        proj |= ((li + 1) * (((c_mask >> li) & 1) as u64)) << (i * 5);
                    }
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
                        non_empty_indices[num_non_empty] = r as u16;
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
                return u32::MAX;
            }

            let best_g_offset = best_guess * matrix.num_candidates;
            let best_g_ptr = unsafe { matrix_data_ptr.add(best_g_offset) };
            for &c in set {
                let r = unsafe { (*best_g_ptr.add(c)).0 as usize };
                counts[r] += 1;
            }

            let mut offsets = vec![0usize; matrix.num_responses + 1];
            for r_idx in 0..matrix.num_responses {
                offsets[r_idx + 1] = offsets[r_idx] + counts[r_idx] as usize;
            }
            let mut sorted_set = vec![0usize; set.len()];
            let mut current_offsets = offsets.clone();
            for &c in set {
                let r = unsafe { (*best_g_ptr.add(c)).0 as usize };
                let pos = current_offsets[r];
                sorted_set[pos] = c;
                current_offsets[r] += 1;
            }

            let mut cost = set.len() as u32;
            for r_idx in 0..matrix.num_responses {
                let p_len = counts[r_idx] as usize;
                if p_len == 0 || r_idx == matrix.win_response.0 as usize {
                    continue;
                }
                let start = offsets[r_idx];
                let end = start + p_len;
                cost += Self::greedy_solve(matrix, dict, &sorted_set[start..end]);
            }
            cost
        }
    }

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
        let cache_size = 64 * 1024 * 1024;
        let global_cache = crate::cache::GlobalCache::new(cache_size);
        let max_k = heuristic::compute_max_branching_factor(matrix, initial_candidates);

        let mut guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        heuristic::sort_guesses_by_expected_remaining(matrix, initial_candidates, &mut guesses);

        let set = initial_candidates;
        let initial_greedy_cost = Self::greedy_solve(matrix, dict, initial_candidates);

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

            let num_resp = matrix.num_responses;
            let mut counts = vec![0u16; num_resp];
            let mut non_empty_indices = vec![0u16; num_resp];
            let mut num_non_empty = 0;

            for &c in set {
                let r_idx = matrix.get(first_g, c).0 as usize;
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
            let mut sorted_set = vec![0usize; set.len()];
            let mut current_offsets = offsets.clone();
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
                if r_idx == matrix.win_response.0 as usize {
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

            let mut root_candidates_tuples: Vec<(usize, u32, u32)> = if matrix.num_responses <= 256
            {
                active_guesses[1..]
                    .par_iter()
                    .copied()
                    .filter_map(|g| {
                        let g_ptr =
                            unsafe { matrix.data_u8.as_ptr().add(g * matrix.num_candidates) };
                        let mut counts = [0u16; 256];
                        for &c in set {
                            let r = unsafe { *g_ptr.add(c) as usize };
                            counts[r] += 1;
                        }
                        let mut lb = set.len() as u32;
                        let win_r = matrix.win_response.0 as usize;
                        let mut expected_rem = 0u32;
                        for (r_idx, &cnt) in counts.iter().enumerate() {
                            if cnt > 0 {
                                expected_rem += (cnt as u32) * (cnt as u32);
                                if r_idx != win_r {
                                    lb += capacity_bounds_2d[max_k][cnt as usize];
                                }
                            }
                        }
                        if lb < greedy_cost_to_beat {
                            Some((g, lb, expected_rem))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                active_guesses[1..]
                    .par_iter()
                    .copied()
                    .filter_map(|g| {
                        let g_ptr = unsafe { matrix.data.as_ptr().add(g * matrix.num_candidates) };
                        let mut counts = vec![0u16; matrix.num_responses];
                        for &c in set {
                            let r = unsafe { (*g_ptr.add(c)).0 as usize };
                            counts[r] += 1;
                        }
                        let mut lb = set.len() as u32;
                        let win_r = matrix.win_response.0 as usize;
                        let mut expected_rem = 0u32;
                        for (r_idx, &cnt) in counts.iter().enumerate() {
                            if cnt > 0 {
                                expected_rem += (cnt as u32) * (cnt as u32);
                                if r_idx != win_r {
                                    lb += capacity_bounds_2d[max_k][cnt as usize];
                                }
                            }
                        }
                        if lb < greedy_cost_to_beat {
                            Some((g, lb, expected_rem))
                        } else {
                            None
                        }
                    })
                    .collect()
            };

            root_candidates_tuples.sort_unstable_by_key(|t| (t.1, t.2));
            let root_candidates: Vec<usize> =
                root_candidates_tuples.into_iter().map(|t| t.0).collect();

            println!(
                "\n*** Root candidates after filter: {} / {} ***",
                root_candidates.len(),
                active_guesses.len() - 1
            );

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
}

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

    /// The optimum of `set` from a search with fresh caches and no global beta.
    fn fresh_optimum(
        matrix: &crate::matrix::ResponseMatrix,
        dict: &crate::dict::Dictionary,
        set: &[usize],
        bits: &[u64],
        capacity_bounds_2d: &[Vec<u32>],
        max_k: usize,
    ) -> u32 {
        let metrics = Metrics::new();
        let cache = crate::cache::GlobalCache::new(1 << 16);
        let equiv_cache: EquivCache =
            std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
        let mut solver = Solver::new(
            matrix,
            max_k,
            dict,
            &metrics,
            capacity_bounds_2d,
            &cache,
            &equiv_cache,
        );
        let greedy = Solver::greedy_solve(matrix, dict, set);
        solver
            .min_state_val(set, bits, greedy + 1, 1, max_k)
            .min(greedy)
    }

    /// A search that the shared global beta cuts short has not proven its
    /// result, so it must leave nothing in the transposition table that a
    /// later, unconstrained search of the same set could trust. Here the global
    /// beta is fixed at or below the optimum, under the search's own beta,
    /// before the search starts. That cuts the search the way a concurrent
    /// tightening does in the parallel scans; a second search sharing the same
    /// table must still find the optimum.
    #[test]
    fn test_global_beta_cut_leaves_no_wrong_cache_entry() {
        let words = crate::dict::Dictionary::read_words(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/words/candidates.txt"
        ))
        .unwrap();
        let dict = crate::dict::Dictionary::from_words(words.clone(), words[..300].to_vec());
        let matrix = crate::matrix::ResponseMatrix::new(&dict);
        let num_u64s = dict.guesses.len().div_ceil(64);
        let mut bits = vec![u64::MAX; num_u64s];
        let rem = dict.guesses.len() % 64;
        if rem != 0 {
            bits[num_u64s - 1] = (1 << rem) - 1;
        }

        let mut checked = 0;
        let mut wrong = Vec::new();
        for (start, len) in [(0, 40), (40, 60), (100, 30), (130, 80), (210, 50)] {
            let set: Vec<usize> = (start..start + len).collect();
            let max_k = heuristic::compute_max_branching_factor(&matrix, &set);
            let mut capacity_bounds_2d = vec![vec![0; dict.candidates.len() + 1]; max_k + 1];
            for k in 2..=max_k {
                for i in 0..=dict.candidates.len() {
                    capacity_bounds_2d[k][i] = heuristic::capacity_bound(i, k);
                }
            }
            let opt = fresh_optimum(&matrix, &dict, &set, &bits, &capacity_bounds_2d, max_k);
            let lb = capacity_bounds_2d[max_k][set.len()];
            let greedy = Solver::greedy_solve(&matrix, &dict, &set);

            for global in (lb + 1)..=opt {
                let metrics = Metrics::new();
                let cache = crate::cache::GlobalCache::new(1 << 16);
                let equiv_cache: EquivCache = std::array::from_fn(|_| {
                    std::sync::RwLock::new(rustc_hash::FxHashMap::default())
                });
                let global_beta = AtomicU32::new(global);
                {
                    let mut cut = Solver::new_with_global_beta(
                        &matrix,
                        max_k,
                        &dict,
                        &metrics,
                        &capacity_bounds_2d,
                        &cache,
                        &global_beta,
                        &equiv_cache,
                    );
                    cut.min_state_val(&set, &bits, greedy + 1, 1, max_k);
                }
                let mut solver = Solver::new(
                    &matrix,
                    max_k,
                    &dict,
                    &metrics,
                    &capacity_bounds_2d,
                    &cache,
                    &equiv_cache,
                );
                let val = solver
                    .min_state_val(&set, &bits, greedy + 1, 1, max_k)
                    .min(greedy);
                if val != opt {
                    wrong.push(format!(
                        "set {start}..{}, global beta {global}: {val} instead of {opt}",
                        start + len
                    ));
                }
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no global beta between the bound and the optimum"
        );
        assert!(
            wrong.is_empty(),
            "after a search cut short by the global beta, a search of the same set \
             with the same cache went wrong in {} of {checked} cases:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
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

        // Test multiple small subsets of candidates (sizes 2..=9)
        for offset in (0..30).step_by(5) {
            for size in 2..=9 {
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
