#![allow(clippy::needless_range_loop)]

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

/// Short git commit hash of the working tree, or "unknown" if git isn't
/// available. Every benchmark run is stamped with this so a number in
/// `benchmark_history.md` can always be traced back to the exact code that
/// produced it - a claim about performance is only as good as being able to
/// check it against the commit it came from.
fn git_commit_hash() -> String {
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
            let cost = Solver::solve(matrix, &candidates, dict, &metrics);
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
            last_states = metrics.states_evaluated.load(std::sync::atomic::Ordering::Relaxed);
            last_guesses = metrics.guesses_evaluated.load(std::sync::atomic::Ordering::Relaxed);
            last_bounds = metrics.pruned_by_bounds.load(std::sync::atomic::Ordering::Relaxed);
            last_equiv = metrics.pruned_by_equivalence.load(std::sync::atomic::Ordering::Relaxed);
            last_chits = metrics.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
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
            size, last_cost, min_secs, avg_secs, max_secs, last_states, last_guesses, last_bounds, last_equiv, last_chits
        )
        .unwrap();
    }
    writeln!(file).unwrap();
}
fn run_full(matrix: &ResponseMatrix, dict: &Dictionary) {
    let is_compute = std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim() == "ubuntu-main" || s.trim() == "compute")
        .unwrap_or(false);
    if !is_compute {
        eprintln!("HARD GUARD: full run must execute on compute node (ubuntu-main). Use rsync + ssh.");
        std::process::exit(1);
    }

    let all_candidates: Vec<usize> = (0..dict.candidates.len()).collect();
    let n_candidates = all_candidates.len();
    println!("Running full N={} optimal solve...", n_candidates);
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
            let done = metrics_clone.root_guesses_done.load(std::sync::atomic::Ordering::Relaxed);
            // We don't have a direct count of total_active_guesses here, so just report done count.
            eprintln!(
                "[progress] elapsed={:.0}s root_guesses_done={} states={} bounds_pruned={}",
                elapsed,
                done,
                metrics_clone.states_evaluated.load(std::sync::atomic::Ordering::Relaxed),
                metrics_clone.pruned_by_bounds.load(std::sync::atomic::Ordering::Relaxed),
            );
        }
    });
    // Thread is intentionally leaked (daemon-like); process exits when solve completes.
    drop(progress_thread);

    let cost = Solver::solve(matrix, &all_candidates, dict, &metrics);
    let elapsed = start.elapsed();

    println!("\n=== FULL RUN COMPLETE ===");
    println!("Candidates: {}", n_candidates);
    println!("Optimal total cost: {}", cost);
    println!("Avg guesses: {:.6}", cost as f64 / n_candidates as f64);
    println!("Wall time: {:.3}s ({:.2}h)", elapsed.as_secs_f64(), elapsed.as_secs_f64() / 3600.0);
    println!("States evaluated: {}", metrics.states_evaluated.load(std::sync::atomic::Ordering::Relaxed));
    println!("Guesses evaluated: {}", metrics.guesses_evaluated.load(std::sync::atomic::Ordering::Relaxed));
    println!("Root guesses done: {}", metrics.root_guesses_done.load(std::sync::atomic::Ordering::Relaxed));
    println!("Bounds pruned: {}", metrics.pruned_by_bounds.load(std::sync::atomic::Ordering::Relaxed));
    println!("Equiv pruned: {}", metrics.pruned_by_equivalence.load(std::sync::atomic::Ordering::Relaxed));
    println!("Cache hits: {}", metrics.cache_hits.load(std::sync::atomic::Ordering::Relaxed));

    // Append to benchmark history
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("benchmark_history.md")
        .expect("Cannot open benchmark_history.md");
    writeln!(file, "## FULL RUN N=2340: {:?}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
    writeln!(file, "- Optimal cost: {}", cost).unwrap();
    writeln!(file, "- Avg guesses: {:.6}", cost as f64 / n_candidates as f64).unwrap();
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

        let is_compute = std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim() == "ubuntu-main" || s.trim() == "compute")
            .unwrap_or(false);

        if !is_compute && max_n > 500 {
            eprintln!("HARD GUARD: Cannot run heavy benchmarks on local VM. Use deploy_and_bench.sh");
            std::process::exit(1);
        }

        let sizes: Vec<usize> = vec![100, 250, 500, 750, 1000, 1500, 2340]
            .into_iter()
            .filter(|&x| x <= max_n)
            .collect();
        run_benchmark(&matrix, &dict, &sizes);
    } else if args.len() > 1 && args[1] == "full" {
        run_full(&matrix, &dict);
    } else if args.len() > 1 && args[1] == "verify" {
        let fuzz_ok = verify::run_verification(&dict, &matrix, 50, 4);
        let stress_ok = verify::run_stress_test(&dict, &matrix);
        if !fuzz_ok || !stress_ok {
            std::process::exit(1);
        }
    } else {
        println!("Usage: wordle-opt <benchmark [-n N] | full | verify>");
    }
}
pub mod cache;

