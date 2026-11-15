//! Checks every value the search writes to its transposition table, not just
//! the final answer.
//!
//! The table is shared by all threads, and the parallel scans also share a
//! global bound that can cut a search short at a moment that depends on
//! thread timing. A value from such a search is not proven for its set, and
//! storing it would let a later search build on a wrong number. Most wrong
//! entries never change the final tree, so checking answers alone misses
//! them; this test records each write (the `tt-audit` feature) during repeated
//! multi-threaded solves and checks it against a solve of that set alone:
//! the brute-force oracle in `tests/common/` for small sets, and a
//! single-threaded search, which has no global bound, for larger ones.
//!
//! ```text
//! cargo test --release --features tt-audit --test tt_audit -- --ignored --nocapture
//! ```
//!
//! `WORDLE_OPT_AUDIT_RUNS` sets the number of solves per thread count
//! (default 10).
#![cfg(feature = "tt-audit")]

mod common;

use common::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use wordle_opt::cache::{GlobalCache, audit};
use wordle_opt::dict::Dictionary;
use wordle_opt::heuristic;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::policy::{BuildOptions, Strategy, build_policy_tree};
use wordle_opt::solver::{EquivCache, Metrics, Solver};

/// Largest set the brute-force oracle checks; larger ones use a fresh
/// single-threaded search.
const ORACLE_MAX: usize = 14;

struct Lists {
    dict: Dictionary,
    matrix: ResponseMatrix,
    all_guesses: Vec<u64>,
    guess_words: Vec<W>,
    candidate_words: Vec<W>,
}

/// The answer list as the guess list, and `size` of its words selected by
/// `--max-candidates size --sample-seed seed` as the candidates.
fn lists(seed: u64, size: usize) -> Lists {
    let list = concat!(env!("CARGO_MANIFEST_DIR"), "/words/candidates.txt");
    let full = Dictionary::try_load(list, list).expect("read answer list");
    let subset = wordle_opt::seeded_subset(seed, size, full.candidates.len());
    let dict = Dictionary::from_words(
        full.guesses.clone(),
        subset.iter().map(|&i| full.candidates[i]).collect(),
    );
    let matrix = ResponseMatrix::new(&dict);
    let n = dict.guesses.len();
    let mut all_guesses = vec![u64::MAX; n.div_ceil(64)];
    if !n.is_multiple_of(64) {
        *all_guesses.last_mut().unwrap() = (1 << (n % 64)) - 1;
    }
    let guess_words = dict
        .guesses
        .iter()
        .map(|w| w.as_bytes().try_into().unwrap())
        .collect();
    let candidate_words = dict
        .candidates
        .iter()
        .map(|w| w.as_bytes().try_into().unwrap())
        .collect();
    Lists {
        dict,
        matrix,
        all_guesses,
        guess_words,
        candidate_words,
    }
}

/// The optimum of `set` from a search of that set alone, with fresh tables
/// and no global bound, so nothing another thread does can affect it.
fn fresh_optimum(l: &Lists, set: &[usize]) -> u32 {
    if set.len() <= ORACLE_MAX {
        let words: Vec<W> = set.iter().map(|&c| l.candidate_words[c]).collect();
        return Oracle::optimum(&l.guess_words, &words);
    }
    let max_k = heuristic::compute_max_branching_factor(&l.matrix, set);
    let mut capacity_bounds_2d = vec![vec![0; set.len() + 1]; max_k + 1];
    for (k, row) in capacity_bounds_2d.iter_mut().enumerate().skip(2) {
        for (i, b) in row.iter_mut().enumerate() {
            *b = heuristic::capacity_bound(i, k);
        }
    }
    let metrics = Metrics::new();
    let cache = GlobalCache::new(1 << 16);
    let equiv_cache: EquivCache =
        std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
    let mut solver = Solver::new(
        &l.matrix,
        max_k,
        &l.dict,
        &metrics,
        &capacity_bounds_2d,
        &cache,
        &equiv_cache,
    );
    let greedy = Solver::greedy_solve(&l.matrix, &l.dict, set);
    solver
        .min_state_val(set, &l.all_guesses, greedy + 1, 1, max_k)
        .min(greedy)
}

/// Solves the whole list on `threads` threads and returns its cost and every
/// table write made during the solve.
fn audited_solve(l: &Lists, threads: usize) -> (u64, Vec<audit::Entry>) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool");
    audit::take();
    let root: Vec<usize> = (0..l.dict.candidates.len()).collect();
    let opts = BuildOptions {
        progress: false,
        ..Default::default()
    };
    let (_, stats) = pool
        .install(|| build_policy_tree(&l.matrix, &l.dict, Strategy::Optimal, &root, &opts))
        .expect("tree build");
    (stats.summary.total_cost, audit::take())
}

#[test]
#[ignore = "repeated audited solves: minutes; run with --features tt-audit --ignored"]
fn every_table_entry_holds_for_its_set() {
    let runs: usize = std::env::var("WORDLE_OPT_AUDIT_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    // The first instance is the one on which multi-threaded runs used to
    // return a suboptimal tree a few times in a hundred.
    let instances = [(5059u64, 200usize), (1, 150), (2, 250)];
    let max_threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let thread_counts: Vec<usize> = [2, 3, 4, 8, max_threads]
        .into_iter()
        .filter(|&t| t <= max_threads)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut failures = Vec::new();
    for (seed, size) in instances {
        let l = lists(seed, size);
        let optimum = fresh_optimum(&l, &(0..size).collect::<Vec<_>>());
        let optima: Mutex<HashMap<Vec<usize>, u32>> = Mutex::new(HashMap::new());
        // Every distinct write seen so far and whether it holds, so a wrong
        // entry that recurs in a later run is reported there too.
        let mut verdicts: HashMap<audit::Entry, Option<String>> = HashMap::new();
        let mut writes = 0usize;
        let mut wrong_runs = 0;
        for &threads in &thread_counts {
            for run in 0..runs {
                let (cost, entries) = audited_solve(&l, threads);
                writes += entries.len();
                let distinct: HashSet<audit::Entry> = entries
                    .into_iter()
                    .map(|(mut set, v, exact)| {
                        set.sort_unstable();
                        (set, v, exact)
                    })
                    .collect();
                let new: Vec<&audit::Entry> = distinct
                    .iter()
                    .filter(|e| !verdicts.contains_key(*e))
                    .collect();
                let judged: Vec<(audit::Entry, Option<String>)> = new
                    .par_iter()
                    .map(|&(set, v, exact)| {
                        let known = optima.lock().unwrap().get(set).copied();
                        let opt = known.unwrap_or_else(|| {
                            let o = fresh_optimum(&l, set);
                            optima.lock().unwrap().insert(set.clone(), o);
                            o
                        });
                        let holds = if *exact { *v == opt } else { *v <= opt };
                        let why = (!holds).then(|| {
                            format!(
                                "{} {v} for a set of {} whose optimum is {opt}",
                                if *exact { "exact" } else { "lower bound" },
                                set.len()
                            )
                        });
                        ((set.clone(), *v, *exact), why)
                    })
                    .collect();
                verdicts.extend(judged);
                let bad: Vec<&String> = distinct
                    .iter()
                    .filter_map(|e| verdicts[e].as_ref())
                    .collect();
                if cost != optimum as u64 || !bad.is_empty() {
                    wrong_runs += 1;
                    failures.push(format!(
                        "seed {seed} size {size}, {threads} threads, run {run}: cost {cost} \
                         (optimum {optimum}), {} wrong entries; first: {:?}",
                        bad.len(),
                        bad.first()
                    ));
                }
            }
        }
        println!(
            "seed {seed} size {size}: {} solves on {thread_counts:?} threads, {writes} writes, \
             {} distinct entries checked, {wrong_runs} runs with a wrong entry or cost",
            runs * thread_counts.len(),
            verdicts.len()
        );
    }
    assert!(
        failures.is_empty(),
        "{} runs wrote a wrong entry or returned a wrong cost:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
