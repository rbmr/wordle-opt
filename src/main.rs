use rand::prelude::SliceRandom;

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
    
    let mut rng = rand::rng();

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

    let all_candidates: Vec<usize> = (0..dict.candidates.len()).collect();

    for &s in sizes {
        let size = s.min(dict.candidates.len());
        let iterations = if size >= 750 { 1 } else { 3 };
        
        let mut sum_secs = 0.0;
        let mut min_secs = f64::MAX;
        let mut max_secs = f64::MIN;
        
        let mut last_cost = 0;
        let mut last_states = 0;
        let mut last_guesses = 0;
        let mut last_bounds = 0;
        let mut last_equiv = 0;
        let mut last_chits = 0;
        let mut last_depth = 0;

        for _ in 0..iterations {
            let mut initial_candidates = all_candidates.clone();
            initial_candidates.shuffle(&mut rng);
            initial_candidates.truncate(size);
            initial_candidates.sort_unstable();

            let metrics = Metrics::new();
            let start = Instant::now();
            let cost = Solver::solve(matrix, &initial_candidates, dict, &metrics);
            let duration = start.elapsed();
            let secs = duration.as_secs_f64();

            sum_secs += secs;
            min_secs = min_secs.min(secs);
            max_secs = max_secs.max(secs);

            last_cost = cost;
            last_states = metrics.states_evaluated.load(std::sync::atomic::Ordering::Relaxed);
            last_guesses = metrics.guesses_evaluated.load(std::sync::atomic::Ordering::Relaxed);
            last_bounds = metrics.pruned_by_bounds.load(std::sync::atomic::Ordering::Relaxed);
            last_equiv = metrics.pruned_by_equivalence.load(std::sync::atomic::Ordering::Relaxed);
            last_chits = metrics.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
            last_depth = metrics.max_depth.load(std::sync::atomic::Ordering::Relaxed);
        }

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
            "| {} | {} | {:.3} | {} | {} | {} | {} | {} |",
            size, last_cost, avg_secs, last_states, last_guesses, last_bounds, last_equiv, last_chits
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
