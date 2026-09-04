pub mod core;
pub mod dict;
pub mod heuristic;
pub mod matrix;
pub mod solver;

use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::solver::{Metrics, Solver};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

fn run_benchmark(matrix: &ResponseMatrix, dict: &Dictionary, sizes: &[usize]) {
    println!(
        "{:<6} | {:<6} | {:<12} | {:<14} | {:<16} | {:<14} | {:<10} | {:<10}",
        "Size",
        "Cost",
        "Time(s)",
        "States Eval",
        "Guesses Eval",
        "Bounds Pruned",
        "Equiv Pruned",
        "Cache Hits"
    );
    println!(
        "{:-<6}-+-{:-<6}-+-{:-<12}-+-{:-<14}-+-{:-<16}-+-{:-<14}-+-{:-<10}-+-{:-<10}",
        "", "", "", "", "", "", "", ""
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
            "{:<6} | {:<6} | {:<12.4} | {:<14} | {:<16} | {:<14} | {:<10} | {:<10}",
            size, cost, secs, states, guesses, bounds, equiv, chits
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
    let is_benchmark = args.len() > 1 && args[1] == "benchmark";

    let subset_size = if !is_benchmark {
        if args.len() > 1 {
            args[1].parse::<usize>().unwrap_or(50)
        } else {
            50
        }
    } else {
        0
    };

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

    if is_benchmark {
        let sizes = vec![10, 20, 50, 100, 150, 200, 250, 300, 400, 500, 750, 1000];
        run_benchmark(&matrix, &dict, &sizes);
    } else {
        let size = subset_size.min(dict.candidates.len());
        let initial_candidates: Vec<usize> = (0..size).collect();
        let metrics = Metrics::new();

        println!("Solving for {} candidates...", size);
        let start = Instant::now();
        let cost = Solver::solve(&matrix, &initial_candidates, &dict, &metrics);
        let duration = start.elapsed();

        println!(
            "Total cost: {}, Expected guesses: {:.4}",
            cost,
            cost as f64 / size as f64
        );
        println!("Solved in {:?}", duration);

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

        println!("States evaluated: {}", states);
        println!("Guesses evaluated: {}", guesses);
        println!("Pruned by bounds: {}", bounds);
        println!("Pruned by equivalence: {}", equiv);
        println!("Cache Hits: {}", chits);
        println!(
            "Nodes / sec: {:.0}",
            (states + guesses) as f64 / duration.as_secs_f64()
        );
    }
}
