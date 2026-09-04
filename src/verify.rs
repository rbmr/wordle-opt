#![allow(clippy::needless_range_loop)]
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::naive::NaiveSolver;
use crate::solver::{Metrics, Solver};
use fastrand;

pub fn run_verification(
    dict: &Dictionary,
    matrix: &ResponseMatrix,
    iterations: usize,
    max_size: usize,
) {
    println!("Starting rigorous correctness verification...");
    println!("Comparing optimized solver against naive reference.");

    let candidates = &dict.candidates;
    let mut failures = 0;

    for i in 0..iterations {
        let size = fastrand::usize(3..=max_size);
        let mut subset = Vec::with_capacity(size);

        while subset.len() < size {
            let idx = fastrand::usize(0..candidates.len());
            if !subset.contains(&idx) {
                subset.push(idx);
            }
        }

        subset.sort_unstable();

        // Run Naive
        let naive = NaiveSolver::new(matrix, dict);
        let naive_cost = naive.solve(&subset);

        // Run Optimized
        let metrics = Metrics::new();
        let opt_cost = Solver::solve(matrix, &subset, dict, &metrics);

        if naive_cost != opt_cost {
            println!(
                "FAIL: Subset size {} - Naive cost: {}, Opt cost: {}",
                size, naive_cost, opt_cost
            );
            println!("Subset indices: {:?}", subset);
            let words: Vec<String> = subset
                .iter()
                .map(|&idx| dict.candidates[idx].to_string())
                .collect();
            println!("Words: {:?}", words);
            failures += 1;
            break;
        }

        if (i + 1) % 5 == 0 {
            println!("Passed {}/{} fuzz iterations...", i + 1, iterations);
        }
    }

    if failures == 0 {
        println!(
            "SUCCESS: 100% correctness verified across {} randomly generated subsets.",
            iterations
        );
    } else {
        println!("Verification FAILED.");
        std::process::exit(1);
    }
}

pub fn run_stress_test(dict: &Dictionary, matrix: &ResponseMatrix) {
    println!("Running targeted stress test for equivalence classes...");

    // Construct a subset with highly repetitive letters
    // "eerie", "error", "erase", "eases", "eagle"
    let stress_words = ["eerie", "error", "erase", "eases", "eagle"];
    let mut subset = Vec::new();

    for word in stress_words {
        for (i, c) in dict.candidates.iter().enumerate() {
            if c.to_string() == word {
                subset.push(i);
                break;
            }
        }
    }

    if subset.len() != stress_words.len() {
        println!(
            "Warning: not all stress words found in dict. Found: {}",
            subset.len()
        );
    }

    let naive = NaiveSolver::new(matrix, dict);
    let naive_cost = naive.solve(&subset);

    let metrics = Metrics::new();
    let opt_cost = Solver::solve(matrix, &subset, dict, &metrics);

    if naive_cost != opt_cost {
        println!(
            "FAIL: Stress test mismatch! Naive: {}, Opt: {}",
            naive_cost, opt_cost
        );
        std::process::exit(1);
    } else {
        println!(
            "SUCCESS: Stress test passed (Cost: {}). Equivalence pruning is perfect.",
            opt_cost
        );
    }
}
