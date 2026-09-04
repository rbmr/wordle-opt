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

fn run_benchmark(matrix: &ResponseMatrix, dict: &Dictionary, sizes: &[usize]) {
    println!(
        "{:<6} | {:<6} | {:<12} | {:<14} | {:<16} | {:<14} | {:<12} | {:<10} | {:<10}",
        "Size",
        "Cost",
        "Time(s)",
        "States Eval",
        "Guesses Eval",
        "Bounds Pruned",
        "Equiv Pruned",
        "Cache Hits",
        "Max Depth"
    );
    println!(
        "{:-<6}-+-{:-<6}-+-{:-<12}-+-{:-<14}-+-{:-<16}-+-{:-<14}-+-{:-<12}-+-{:-<10}-+-{:-<10}",
        "", "", "", "", "", "", "", "", ""
    );

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("benchmark_history.md")
        .expect("Cannot open benchmark_history.md");

    writeln!(
        file,
        "## Benchmark Run: {:?}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
    .unwrap();
    writeln!(
        file,
        "| Size | Cost | Time(s) | States | Guesses | B-Pruned | E-Pruned | Cache Hits |"
    )
    .unwrap();
    writeln!(
        file,
        "|------|------|---------|--------|---------|----------|----------|------------|"
    )
    .unwrap();

    for &s in sizes {
        let size = s.min(dict.candidates.len());
        let initial_candidates: Vec<usize> = (0..size).collect();
        let metrics = Metrics::new();

        let start = Instant::now();
        let cost = Solver::solve(matrix, &initial_candidates, dict, &metrics);
        let duration = start.elapsed();
        let secs = duration.as_secs_f64();

        let states = metrics
            .states_evaluated
            .load(std::sync::atomic::Ordering::Relaxed);
        let guesses = metrics
            .guesses_evaluated
            .load(std::sync::atomic::Ordering::Relaxed);
        let bounds = metrics
            .pruned_by_bounds
            .load(std::sync::atomic::Ordering::Relaxed);
        let equiv = metrics
            .pruned_by_equivalence
            .load(std::sync::atomic::Ordering::Relaxed);
        let chits = metrics
            .cache_hits
            .load(std::sync::atomic::Ordering::Relaxed);

        println!(
            "{:<6} | {:<6} | {:<12.4} | {:<14} | {:<16} | {:<14} | {:<12} | {:<10} | {:<10}",
            size,
            cost,
            secs,
            states,
            guesses,
            bounds,
            equiv,
            chits,
            metrics.max_depth.load(std::sync::atomic::Ordering::Relaxed)
        );

        writeln!(
            file,
            "| {} | {} | {:.3} | {} | {} | {} | {} | {} |",
            size, cost, secs, states, guesses, bounds, equiv, chits
        )
        .unwrap();
    }
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
        
        let is_compute = std::fs::read_to_string("/etc/hostname").map(|s| s.trim() == "ubuntu-main" || s.trim() == "compute").unwrap_or(false);
        if !is_compute && max_n > 500 {
            eprintln!("HARD GUARD: Cannot run heavy benchmarks on local VM. Use deploy_and_bench.sh");
            std::process::exit(1);
        }

        let sizes: Vec<usize> = vec![100, 250, 500, 750, 1000, 1500, 2340].into_iter().filter(|&x| x <= max_n).collect();
        run_benchmark(&matrix, &dict, &sizes);
    } else if args.len() > 1 && args[1] == "verify" {
        verify::run_verification(&dict, &matrix, 50, 4);
        verify::run_stress_test(&dict, &matrix);
    } else {
        println!("Please specify 'benchmark' or 'verify' as an argument.");
    }
}
pub mod cache;
