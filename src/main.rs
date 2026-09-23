#![allow(clippy::needless_range_loop)]

pub mod ida;
pub mod core;
pub mod dict;
pub mod heuristic;
pub mod matrix;
pub mod naive;
pub mod solver;
pub mod verify;

use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::solver::{Metrics, Solver};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

/// Short git commit hash of the working tree, or "unknown" if it can't be
/// determined. Every benchmark run is stamped with this so a number in
/// `benchmark_history.md` can always be traced back to the exact code that
/// produced it - a claim about performance is only as good as being able to
/// check it against the commit it came from.
///
/// Prefers `WORDLE_OPT_COMMIT` (set by `deploy_and_bench.sh` from the
/// *local* repo before rsyncing) over running `git rev-parse` here: the
/// remote build directory deliberately has no `.git` (rsync excludes it), so
/// `git rev-parse` there either fails or - worse - silently reports the HEAD
/// of some unrelated, stale git checkout that happens to sit in the same
/// directory, mislabeling a run with a commit hash that isn't what actually
/// ran. Only a git repo that is *actually the source of the code running
/// right now* can answer this correctly, which on the remote is never true.
fn git_commit_hash() -> String {
    if let Ok(commit) = std::env::var("WORDLE_OPT_COMMIT")
        && !commit.trim().is_empty()
    {
        return commit.trim().to_string();
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Whether this process is running on the designated high-memory/high-core
/// compute host, as opposed to a laptop/dev machine. Gates both the
/// transposition table size (`Solver::solve`) and which CLI operations are
/// allowed to run locally (`run_full`, `benchmark`'s size guard) - kept as a
/// single check so the two decisions can't silently drift apart if the
/// compute host's name ever changes.
pub fn is_compute_host() -> bool {
    let host = hostname();
    host == "ubuntu-main" || host == "compute"
}

/// Runs the solver on fixed, deterministic candidate subsets (the first `n`
/// dictionary entries, sorted - identical to the golden regression tests in
/// `solver.rs`) and appends a self-contained, traceable record to
/// `benchmark_history.md`.
///
/// Determinism is the whole point: it's what makes a number in the history
/// file *comparable* across runs and across commits. Earlier versions of
/// this function sampled a random subset of candidates at each size, which
/// meant two "N=750" entries could legitimately report different costs,
/// states, and timings for reasons that had nothing to do with the code
/// changing - making the history file useless for judging whether a change
/// actually helped. Fixed candidate sets remove that confound; only real
/// machine/thread-scheduling jitter remains, which is why we still repeat
/// small sizes and report min/avg/max wall time.
fn run_benchmark(matrix: &ResponseMatrix, dict: &Dictionary, sizes: &[usize]) {
    let commit = git_commit_hash();
    let host = hostname();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    println!(
        "Benchmark run: commit={} host={} cpus={} unix_time={}",
        commit, host, cpus, timestamp
    );
    println!(
        "{:<6} | {:<6} | {:<22} | {:<14} | {:<16} | {:<14} | {:<12} | {:<10} | {:<10}",
        "Size",
        "Cost",
        "Time [Min/Avg/Max]",
        "States Eval",
        "Guesses Eval",
        "Bounds Pruned",
        "Equiv Pruned",
        "Cache Hits",
        "Max Depth"
    );
    println!(
        "{:-<6}-+-{:-<6}-+-{:-<22}-+-{:-<14}-+-{:-<16}-+-{:-<14}-+-{:-<12}-+-{:-<10}-+-{:-<10}",
        "", "", "", "", "", "", "", "", ""
    );

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("benchmark_history.md")
        .expect("Cannot open benchmark_history.md");

    writeln!(
        file,
        "## Benchmark Run: commit={} host={} cpus={} unix_time={}",
        commit, host, cpus, timestamp
    )
    .unwrap();
    writeln!(
        file,
        "Candidates are deterministic: the first N entries of the dictionary, sorted (same convention as the golden tests in `solver.rs`)."
    )
    .unwrap();
    writeln!(
        file,
        "| Size | Cost | Time(s) [Min/Avg/Max] | States | Guesses | B-Pruned | E-Pruned | Cache Hits |"
    )
    .unwrap();
    writeln!(
        file,
        "|------|------|------------------------|--------|---------|----------|----------|------------|"
    )
    .unwrap();

    for &s in sizes {
        let size = s.min(dict.candidates.len());
        // Fixed, deterministic subset - not a random sample. See doc comment above.
        let candidates: Vec<usize> = (0..size).collect();
        let iterations = if size >= 750 { 1 } else { 3 };

        let mut sum_secs = 0.0;
        let mut min_secs = f64::MAX;
        let mut max_secs = f64::MIN;

        let mut last_cost = None;
        let mut last_states = 0;
        let mut last_guesses = 0;
        let mut last_bounds = 0;
        let mut last_equiv = 0;
        let mut last_chits = 0;
        let mut last_depth = 0;

        for _ in 0..iterations {
            let metrics = Metrics::new();
            let start = Instant::now();
            let equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
                std::sync::RwLock::new(
                    std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new(),
                )
            });
            let cost = Solver::solve(matrix, &candidates, dict, &metrics, &equiv_cache_arr);
            let secs = start.elapsed().as_secs_f64();

            sum_secs += secs;
            min_secs = min_secs.min(secs);
            max_secs = max_secs.max(secs);

            // Same input, same code, same machine: cost must be identical
            // across iterations. If it isn't, the solver has a
            // nondeterminism bug (e.g. thread-scheduling-dependent
            // reduction order) - surface that loudly instead of silently
            // reporting whichever iteration ran last.
            if let Some(prev) = last_cost {
                assert_eq!(
                    cost, prev,
                    "nondeterministic solve at N={}: cost varied across repeated runs on identical input",
                    size
                );
            }
            last_cost = Some(cost);
            last_states = metrics
                .states_evaluated
                .load(std::sync::atomic::Ordering::Relaxed);
            last_guesses = metrics
                .guesses_evaluated
                .load(std::sync::atomic::Ordering::Relaxed);
            last_bounds = metrics
                .pruned_by_bounds
                .load(std::sync::atomic::Ordering::Relaxed);
            last_equiv = metrics
                .pruned_by_equivalence
                .load(std::sync::atomic::Ordering::Relaxed);
            last_chits = metrics
                .cache_hits
                .load(std::sync::atomic::Ordering::Relaxed);
            last_depth = metrics.max_depth.load(std::sync::atomic::Ordering::Relaxed);
        }

        let last_cost = last_cost.unwrap();
        let avg_secs = sum_secs / (iterations as f64);
        let time_str = if iterations > 1 {
            format!("{:.2}/{:.2}/{:.2}", min_secs, avg_secs, max_secs)
        } else {
            format!("{:.4}", avg_secs)
        };

        println!(
            "{:<6} | {:<6} | {:<22} | {:<14} | {:<16} | {:<14} | {:<12} | {:<10} | {:<10}",
            size,
            last_cost,
            time_str,
            last_states,
            last_guesses,
            last_bounds,
            last_equiv,
            last_chits,
            last_depth
        );

        writeln!(
            file,
            "| {} | {} | {:.2}/{:.2}/{:.2} | {} | {} | {} | {} | {} |",
            size,
            last_cost,
            min_secs,
            avg_secs,
            max_secs,
            last_states,
            last_guesses,
            last_bounds,
            last_equiv,
            last_chits
        )
        .unwrap();
    }
    writeln!(file).unwrap();
}

/// A fixed seed for `run_benchmark_random`'s sampling. Fixed (not time-based)
/// so that re-running this benchmark against the same code and the same
/// dictionary files draws the exact same sequence of random subsets every
/// time - "randomized" here means "not biased toward one arbitrary slice of
/// the dictionary" (see doc comment below), not "different every run".
const BENCHMARK_RANDOM_SEED: u64 = 20260906;

/// Runs the solver on `samples_per_size` independently-drawn random subsets
/// per size in `sizes`, instead of `run_benchmark`'s single fixed first-N
/// slice.
///
/// `run_benchmark`'s deterministic first-N-sorted subset is reproducible,
/// but it's still just one arbitrary sample - the first N candidates in
/// dictionary (alphabetical) order aren't necessarily representative of a
/// "typical" N-word instance, and a single sample can't distinguish a real
/// improvement from that one input happening to be easy or hard. Multiple
/// random samples per size give an actual distribution (reported as
/// min/avg/max cost and time), and because the RNG is seeded with a fixed
/// constant, the exact same set of samples is drawn on every re-run - so
/// this stays exactly as reproducible as the deterministic benchmark, it's
/// just reproducible over a representative spread of inputs instead of one
/// fixed slice.
fn run_benchmark_random(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    sizes: &[usize],
    samples_per_size: usize,
) {
    let commit = git_commit_hash();
    let host = hostname();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    println!(
        "Randomized benchmark run: commit={} host={} cpus={} seed={} samples_per_size={} unix_time={}",
        commit, host, cpus, BENCHMARK_RANDOM_SEED, samples_per_size, timestamp
    );
    println!(
        "{:<6} | {:<8} | {:<24} | {:<24}",
        "Size", "Samples", "Cost [Min/Avg/Max]", "Time(s) [Min/Avg/Max]"
    );
    println!("{:-<6}-+-{:-<8}-+-{:-<24}-+-{:-<24}", "", "", "", "");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("benchmark_history.md")
        .expect("Cannot open benchmark_history.md");

    writeln!(
        file,
        "## Randomized Benchmark Run: commit={} host={} cpus={} seed={} samples_per_size={} unix_time={}",
        commit, host, cpus, BENCHMARK_RANDOM_SEED, samples_per_size, timestamp
    )
    .unwrap();
    writeln!(
        file,
        "Each size draws {} independent random subsets (no replacement within a subset) from a single fastrand::Rng seeded with {} at the start of the run, consumed in size order - so this exact sequence of samples is reproduced by any re-run with the same seed, sizes, and sample count.",
        samples_per_size, BENCHMARK_RANDOM_SEED
    )
    .unwrap();
    writeln!(
        file,
        "| Size | Samples | Cost [Min/Avg/Max] | Time(s) [Min/Avg/Max] | States [Min/Avg/Max] | CacheHits Avg |"
    )
    .unwrap();
    writeln!(
        file,
        "|------|---------|---------------------|------------------------|----------------------|---------------|"
    )
    .unwrap();

    let mut rng = fastrand::Rng::with_seed(BENCHMARK_RANDOM_SEED);
    let n_candidates = dict.candidates.len();

    for &s in sizes {
        let size = s.min(n_candidates);
        let mut costs = Vec::with_capacity(samples_per_size);
        let mut times = Vec::with_capacity(samples_per_size);
        let mut states = Vec::with_capacity(samples_per_size);
        let mut cache_hits_vec = Vec::with_capacity(samples_per_size);

        for sample_idx in 0..samples_per_size {
            let subset = sample_random_subset(&mut rng, n_candidates, size);

            let metrics = Metrics::new();
            let start = Instant::now();
            let _equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
                std::sync::RwLock::new(
                    std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new(),
                )
            });
            let equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
                std::sync::RwLock::new(
                    std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new(),
                )
            });
            let cost = Solver::solve(matrix, &subset, dict, &metrics, &equiv_cache_arr);
            let secs = start.elapsed().as_secs_f64();
            let s_states = metrics
                .states_evaluated
                .load(std::sync::atomic::Ordering::Relaxed);
            let s_chits = metrics
                .cache_hits
                .load(std::sync::atomic::Ordering::Relaxed);

            println!(
                "  size={} sample={}/{} cost={} time={:.3}s states={} cache_hits={} hit_rate={:.1}%",
                size,
                sample_idx + 1,
                samples_per_size,
                cost,
                secs,
                s_states,
                s_chits,
                100.0 * s_chits as f64 / (s_chits + s_states).max(1) as f64
            );

            costs.push(cost);
            times.push(secs);
            states.push(s_states);
            cache_hits_vec.push(s_chits);
        }

        let cost_min = *costs.iter().min().unwrap();
        let cost_max = *costs.iter().max().unwrap();
        let cost_avg = costs.iter().sum::<u32>() as f64 / costs.len() as f64;
        let time_min = times.iter().cloned().fold(f64::MAX, f64::min);
        let time_max = times.iter().cloned().fold(f64::MIN, f64::max);
        let time_avg = times.iter().sum::<f64>() / times.len() as f64;
        let states_min = *states.iter().min().unwrap();
        let states_max = *states.iter().max().unwrap();
        let states_avg = states.iter().sum::<usize>() as f64 / states.len() as f64;
        let chits_avg = cache_hits_vec.iter().sum::<usize>() as f64 / cache_hits_vec.len() as f64;

        println!(
            "{:<6} | {:<8} | {:<24} | {:<24}",
            size,
            samples_per_size,
            format!("{}/{:.1}/{}", cost_min, cost_avg, cost_max),
            format!("{:.2}/{:.2}/{:.2}", time_min, time_avg, time_max)
        );

        writeln!(
            file,
            "| {} | {} | {}/{:.1}/{} | {:.2}/{:.2}/{:.2} | {}/{:.1}/{} | {:.0} |",
            size,
            samples_per_size,
            cost_min,
            cost_avg,
            cost_max,
            time_min,
            time_avg,
            time_max,
            states_min,
            states_avg,
            states_max,
            chits_avg
        )
        .unwrap();
    }
    writeln!(file).unwrap();
}

/// Draws one random subset of `size` distinct candidate indices (no
/// replacement) from `rng`, sorted. Shared by `benchmark-random` and
/// `diagnose` so both draw samples the same way.
fn sample_random_subset(rng: &mut fastrand::Rng, n_candidates: usize, size: usize) -> Vec<usize> {
    let mut subset = Vec::with_capacity(size);
    while subset.len() < size {
        let idx = rng.usize(0..n_candidates);
        if !subset.contains(&idx) {
            subset.push(idx);
        }
    }
    subset.sort_unstable();
    subset
}

/// Separate fixed seed from `BENCHMARK_RANDOM_SEED`, so a `diagnose` run
/// never silently shares (or is confused for) `benchmark-random`'s sample
/// sequence - they draw from independent, but each individually
/// reproducible, RNG streams.
const DIAGNOSE_SEED: u64 = 20260910;

/// Reports the full `Metrics` breakdown (not just states_evaluated, like
/// `benchmark-random` does) plus the max branching factor `k` for one random
/// sample per size in `sizes`. Use this to investigate *why* wall-clock time
/// scales the way it does relative to cost - e.g. `benchmark-random` showed
/// avg cost/candidate scaling smoothly from N=1000 to N=1500 while wall time
/// jumped ~25x, and the aggregate cost metric alone can't say whether that's
/// weaker equivalence pruning, weaker bounds pruning, worse cache locality,
/// more guesses needed per state, or a larger branching factor loosening
/// capacity bounds. This prints the raw counters needed to tell those apart;
/// it does not try to precompute derived ratios and hand you the answer -
/// pruned_by_bounds in particular is incremented from two different call
/// sites in solver.rs (a guess-selection-time prune in min_state_val and a
/// mid-evaluation prune in min_guess_val) that this doesn't disentangle.
fn run_diagnose(matrix: &ResponseMatrix, dict: &Dictionary, sizes: &[usize]) {
    let mut rng = fastrand::Rng::with_seed(DIAGNOSE_SEED);
    let n_candidates = dict.candidates.len();
    let commit = git_commit_hash();
    let host = hostname();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("diagnose_history.md")
        .expect("Cannot open diagnose_history.md");
    writeln!(
        file,
        "## Diagnose Run: commit={} host={} seed={} sizes={:?}",
        commit, host, DIAGNOSE_SEED, sizes
    )
    .unwrap();
    writeln!(
        file,
        "| Size | MaxK | Depth | Cost | Time(s) | States | Guesses | CacheHit | EquivPrn | BndsPrn |"
    )
    .unwrap();
    writeln!(
        file,
        "|------|------|-------|------|---------|--------|---------|----------|----------|---------|"
    )
    .unwrap();

    println!(
        "{:<6} | {:<6} | {:<6} | {:<10} | {:<10} | {:<11} | {:<9} | {:<9} | {:<9} | {:<9}",
        "Size",
        "MaxK",
        "Depth",
        "Cost",
        "Time(s)",
        "States",
        "Guesses",
        "CacheHit",
        "EquivPrn",
        "BndsPrn"
    );
    println!(
        "{:-<6}-+-{:-<6}-+-{:-<6}-+-{:-<10}-+-{:-<10}-+-{:-<11}-+-{:-<9}-+-{:-<9}-+-{:-<9}-+-{:-<9}",
        "", "", "", "", "", "", "", "", "", ""
    );

    for &s in sizes {
        let size = s.min(n_candidates);
        let subset = sample_random_subset(&mut rng, n_candidates, size);

        let max_k = heuristic::compute_max_branching_factor(matrix, &subset);

        let metrics = Metrics::new();
        let start = Instant::now();
        let equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
            std::sync::RwLock::new(
                std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new(),
            )
        });
        let cost = Solver::solve(matrix, &subset, dict, &metrics, &equiv_cache_arr);
        let secs = start.elapsed().as_secs_f64();

        let states = metrics
            .states_evaluated
            .load(std::sync::atomic::Ordering::Relaxed);
        let guesses = metrics
            .guesses_evaluated
            .load(std::sync::atomic::Ordering::Relaxed);
        let cache_hits = metrics
            .cache_hits
            .load(std::sync::atomic::Ordering::Relaxed);
        let bounds_pruned = metrics
            .pruned_by_bounds
            .load(std::sync::atomic::Ordering::Relaxed);
        let equiv_pruned = metrics
            .pruned_by_equivalence
            .load(std::sync::atomic::Ordering::Relaxed);
        let max_depth = metrics.max_depth.load(std::sync::atomic::Ordering::Relaxed);

        println!(
            "{:<6} | {:<6} | {:<6} | {:<10} | {:<10.3} | {:<11} | {:<9} | {:<9} | {:<9} | {:<9}",
            size,
            max_k,
            max_depth,
            cost,
            secs,
            states,
            guesses,
            cache_hits,
            equiv_pruned,
            bounds_pruned
        );
        writeln!(
            file,
            "| {} | {} | {} | {} | {:.3} | {} | {} | {} | {} | {} |",
            size,
            max_k,
            max_depth,
            cost,
            secs,
            states,
            guesses,
            cache_hits,
            equiv_pruned,
            bounds_pruned
        )
        .unwrap();
    }
    writeln!(file).unwrap();
}

fn run_full(matrix: &ResponseMatrix, dict: &Dictionary) {
    // Restored 2026-09-16: this guard was silently commented out in commit
    // ffb81d2 ("Perf: Prune branches using parent's max_k before full
    // capacity check") - a commit whose message describes an unrelated
    // pruning optimization and says nothing about disabling this check.
    // Without it, nothing stops `full` (an exhaustive N=2340 solve) from
    // being launched on the tiny 2-core/4GB `assistant` VM, which is
    // supposed to stay lightweight and available - see AGENTS.md and the
    // task guidance on why that machine must never run heavy computation.
    if !is_compute_host() {
        eprintln!(
            "HARD GUARD: full run must execute on the compute host (hostname 'ubuntu-main' or 'compute'). Use rsync + ssh, or deploy_and_bench.sh."
        );
        std::process::exit(1);
    }

    let all_candidates: Vec<usize> = (0..dict.candidates.len()).collect();
    let n_candidates = all_candidates.len();
    let commit = git_commit_hash();
    let host = hostname();
    println!(
        "Running full N={} optimal solve... commit={} host={}",
        n_candidates, commit, host
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let metrics = std::sync::Arc::new(Metrics::new());
    let start = Instant::now();

    // Progress-reporting thread: prints status every 60 seconds.
    let metrics_clone = std::sync::Arc::clone(&metrics);
    let start_clone = start;
    let progress_thread = std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let elapsed = start_clone.elapsed().as_secs_f64();
            let done = metrics_clone
                .root_guesses_done
                .load(std::sync::atomic::Ordering::Relaxed);
            // We don't have a direct count of total_active_guesses here, so just report done count.
            eprintln!(
                "[progress] elapsed={:.0}s root_guesses_done={} states={} bounds_pruned={}",
                elapsed,
                done,
                metrics_clone
                    .states_evaluated
                    .load(std::sync::atomic::Ordering::Relaxed),
                metrics_clone
                    .pruned_by_bounds
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
        }
    });
    // Thread is intentionally leaked (daemon-like); process exits when solve completes.
    drop(progress_thread);

    let equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
        std::sync::RwLock::new(std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new())
    });
    let cost = Solver::solve(matrix, &all_candidates, dict, &metrics, &equiv_cache_arr);
    let elapsed = start.elapsed();

    println!("\n=== FULL RUN COMPLETE ===");
    println!("Candidates: {}", n_candidates);
    println!("Optimal total cost: {}", cost);
    println!("Avg guesses: {:.6}", cost as f64 / n_candidates as f64);
    println!(
        "Wall time: {:.3}s ({:.2}h)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / 3600.0
    );
    println!(
        "States evaluated: {}",
        metrics
            .states_evaluated
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Guesses evaluated: {}",
        metrics
            .guesses_evaluated
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Root guesses done: {}",
        metrics
            .root_guesses_done
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Bounds pruned: {}",
        metrics
            .pruned_by_bounds
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Equiv pruned: {}",
        metrics
            .pruned_by_equivalence
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Cache hits: {}",
        metrics
            .cache_hits
            .load(std::sync::atomic::Ordering::Relaxed)
    );

    // Append to benchmark history
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("benchmark_history.md")
        .expect("Cannot open benchmark_history.md");
    writeln!(
        file,
        "## FULL RUN N=2340: commit={} host={} unix_time={}",
        commit,
        host,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
    .unwrap();
    writeln!(file, "- Optimal cost: {}", cost).unwrap();
    writeln!(
        file,
        "- Avg guesses: {:.6}",
        cost as f64 / n_candidates as f64
    )
    .unwrap();
    writeln!(file, "- Time: {:.3}s", elapsed.as_secs_f64()).unwrap();
    writeln!(file).unwrap();
}

fn main() {
    let args: Vec<String> = env::args().collect();

    println!("Loading dictionary...");
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    println!(
        "Loaded {} guesses and {} candidates.",
        dict.guesses.len(),
        dict.candidates.len()
    );

    println!("Computing response matrix...");
    let start = Instant::now();
    let matrix = ResponseMatrix::new(&dict);
    let duration = start.elapsed();
    println!(
        "Computed matrix of size {}x{} in {:?}",
        matrix.num_guesses, matrix.num_candidates, duration
    );

    if args.len() > 1 && args[1] == "benchmark" {
        let mut max_n = 2340;
        if args.len() > 3 && args[2] == "-n" {
            max_n = args[3].parse().unwrap();
        }

        if !is_compute_host() && max_n > 500 {
            eprintln!(
                "HARD GUARD: Cannot run heavy benchmarks on local VM. Use deploy_and_bench.sh"
            );
            std::process::exit(1);
        }

        let sizes: Vec<usize> = vec![100, 250, 500, 750, 1000, 1500, 2340]
            .into_iter()
            .filter(|&x| x <= max_n)
            .collect();
        run_benchmark(&matrix, &dict, &sizes);
    } else if args.len() > 1 && args[1] == "benchmark-random" {
        // Default caps at 1000, not 1500: `diagnose` found a sharp cost-cliff
        // between N=1100 and N=1200 (search depth stepping from 4 to 5 - see
        // ARCHITECTURE.md's "Known Scaling Behavior") where a single sample
        // can take 1-2 hours. At the default 5 samples/size, including 1500
        // could turn a routine benchmark into a many-hour run with no
        // warning. Request `-n 1500` explicitly (with a low `-k`) when you
        // actually want that data point.
        let mut max_n = 1000;
        let mut samples = 5;
        let mut i = 2;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "-n" => max_n = args[i + 1].parse().unwrap(),
                "-k" => samples = args[i + 1].parse().unwrap(),
                _ => {}
            }
            i += 2;
        }

        if !is_compute_host() && max_n > 500 {
            eprintln!(
                "HARD GUARD: Cannot run heavy benchmarks on local VM. Use deploy_and_bench.sh"
            );
            std::process::exit(1);
        }

        let sizes: Vec<usize> = vec![100, 250, 500, 750, 1000, 1500]
            .into_iter()
            .filter(|&x| x <= max_n)
            .collect();
        run_benchmark_random(&matrix, &dict, &sizes, samples);
    } else if args.len() > 1 && args[1] == "diagnose" {
        let mut sizes: Vec<usize> = vec![100, 250, 500, 750, 1000, 1500];
        let mut i = 2;
        while i + 1 < args.len() {
            if args[i] == "-n" {
                sizes = args[i + 1].split(',').map(|s| s.parse().unwrap()).collect();
            }
            i += 2;
        }

        if !is_compute_host() && sizes.iter().any(|&x| x > 500) {
            eprintln!(
                "HARD GUARD: Cannot run heavy diagnostics on local VM. Use deploy_and_bench.sh's host."
            );
            std::process::exit(1);
        }

        run_diagnose(&matrix, &dict, &sizes);
    } else if args.len() > 1 && args[1] == "evaluate-root" {
        let root_guess = args[2].parse::<usize>().unwrap();

        let initial_candidates: Vec<usize> = (0..dict.candidates.len()).collect();
        let max_k = heuristic::compute_max_branching_factor(&matrix, &initial_candidates);
        let mut capacity_bounds_2d = vec![vec![0; initial_candidates.len() + 1]; max_k + 1];
        for k in 2..=max_k {
            for i in 0..=initial_candidates.len() {
                capacity_bounds_2d[k][i] = heuristic::capacity_bound(i, k);
            }
        }

        let cache_size = if crate::is_compute_host() {
            512 * 1024 * 1024
        } else {
            64 * 1024 * 1024
        };
        let global_cache = crate::cache::GlobalCache::new(cache_size);
        let metrics = Metrics::new();

        use std::sync::atomic::AtomicU32;
        let global_beta = AtomicU32::new(u32::MAX);

        let equiv_cache_arr: [_; 64] = std::array::from_fn(|_| {
            std::sync::RwLock::new(
                std::collections::HashMap::<u32, std::sync::Arc<Vec<u16>>>::new(),
            )
        });
        let mut solver = Solver::new_with_global_beta(
            &matrix,
            max_k,
            &dict,
            &metrics,
            &capacity_bounds_2d,
            &global_cache,
            &global_beta,
            &equiv_cache_arr,
        );

        let mut allowed_guesses: Vec<usize> = (0..matrix.num_guesses).collect();
        // Compute expected remaining for all allowed guesses
        heuristic::sort_guesses_by_expected_remaining(
            &matrix,
            &initial_candidates,
            &mut allowed_guesses,
        );

        let start = Instant::now();
        let val = solver.min_guess_val(&initial_candidates, root_guess, u32::MAX, 1, max_k);
        println!(
            "Root Guess: {} ({}) -> Cost: {}",
            root_guess,
            std::str::from_utf8(&dict.guesses[root_guess].0).unwrap(),
            val
        );
        println!("Time: {:?}", start.elapsed());
    } else if args.len() > 1 && args[1] == "full" {
        run_full(&matrix, &dict);
    } else if args.len() > 1 && args[1] == "verify" {
        let fuzz_ok = verify::run_verification(&dict, &matrix, 50, 4);
        let stress_ok = verify::run_stress_test(&dict, &matrix);
        if !fuzz_ok || !stress_ok {
            std::process::exit(1);
        }
    } else {
        println!(
            "Usage: wordle-opt <benchmark [-n N] | benchmark-random [-n N] [-k SAMPLES] | diagnose [-n N1,N2,...] | full | verify | evaluate-root <GUESS_ID>>"
        );
    }
}
pub mod cache;
