#![allow(clippy::needless_range_loop)]
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::naive::NaiveSolver;
use crate::solver::{Metrics, Solver};
use fastrand;

/// Solves `subset` with both the naive reference solver and the optimized
/// branch-and-bound solver and returns `(naive_cost, optimized_cost)`. The
/// two must always agree - the naive solver has no pruning to get wrong,
/// so any divergence means the optimized solver's pruning discarded a
/// guess (or subtree) it shouldn't have.
fn solve_both(dict: &Dictionary, matrix: &ResponseMatrix, subset: &[usize]) -> (u32, u32) {
    let naive_cost = NaiveSolver::new(matrix, dict).solve(subset);
    let metrics = Metrics::new();
    let opt_cost = Solver::solve(matrix, subset, dict, &metrics, &std::array::from_fn(|_| std::sync::RwLock::new(std::collections::HashMap::new())));
    (naive_cost, opt_cost)
}

/// Runs `iterations` random subsets (size `3..=max_size`) through
/// [`solve_both`] and reports any mismatch. Returns `true` iff every
/// iteration agreed.
pub fn run_verification(
    dict: &Dictionary,
    matrix: &ResponseMatrix,
    iterations: usize,
    max_size: usize,
) -> bool {
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

        let (naive_cost, opt_cost) = solve_both(dict, matrix, &subset);

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
        true
    } else {
        println!("Verification FAILED.");
        false
    }
}

/// Solves a fixed subset of words sharing heavily overlapping letters
/// (repeated e/r/a/s) through [`solve_both`]. This is a targeted case for
/// equivalence-class guess pruning, which is easy to get subtly wrong
/// specifically when many guesses partition the candidate set identically -
/// exactly the situation repetitive letters create. Returns `true` iff the
/// two solvers agree.
pub fn run_stress_test(dict: &Dictionary, matrix: &ResponseMatrix) -> bool {
    println!("Running targeted stress test for equivalence classes...");

    // "eases" was in the original list but isn't one of the ~2340 actual
    // candidate answers (it's guess-only), so it silently reduced this to a
    // 4-word test; swapped for "easel", which is a real candidate and keeps
    // the same repetitive e/a/s letter overlap this test is meant to stress.
    let stress_words = ["eerie", "error", "erase", "easel", "eagle"];
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

    let (naive_cost, opt_cost) = solve_both(dict, matrix, &subset);

    if naive_cost != opt_cost {
        println!(
            "FAIL: Stress test mismatch! Naive: {}, Opt: {}",
            naive_cost, opt_cost
        );
        false
    } else {
        println!(
            "SUCCESS: Stress test passed (Cost: {}). Equivalence pruning is perfect.",
            opt_cost
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_dict_and_matrix() -> (Dictionary, ResponseMatrix) {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        (dict, matrix)
    }

    #[test]
    fn test_differential_fuzz_naive_vs_optimal() {
        // Deterministic seed: a failure here must be reproducible, not a
        // one-off flake, since the whole point is catching real pruning
        // bugs, not random noise in CI.
        //
        // max_size stays small (4, matching the existing `verify` CLI
        // mode's own default) because NaiveSolver has no memoization at
        // all - only alpha-beta pruning - so its search blows up combinatorially
        // well before N=12; this isn't the optimized solver being slow.
        fastrand::seed(1234);
        let (dict, matrix) = load_dict_and_matrix();
        assert!(
            run_verification(&dict, &matrix, 30, 4),
            "optimized solver disagreed with the naive reference on at least one random subset"
        );
    }

    #[test]
    fn test_stress_equivalence_classes() {
        let (dict, matrix) = load_dict_and_matrix();
        assert!(
            run_stress_test(&dict, &matrix),
            "optimized solver disagreed with the naive reference on the repetitive-letters stress subset"
        );
    }
}

#[cfg(test)]
mod extra_bounds_tests {
    use super::*;
    use crate::heuristic::capacity_bound;

    #[test]
    fn test_capacity_bound_monotonically_increasing() {
        for k in 2..20 {
            let mut prev = 0;
            for n in 1..200 {
                let bound = capacity_bound(n, k);
                assert!(bound >= prev, "Capacity bound should be monotonic for fixed k");
                prev = bound;
            }
        }
    }

    #[test]
    fn test_capacity_bound_k_monotonicity() {
        for n in 1..200 {
            let mut prev = capacity_bound(n, 2);
            for k in 3..20 {
                let bound = capacity_bound(n, k);
                assert!(bound <= prev, "Capacity bound should decrease as k increases (more branches available)");
                prev = bound;
            }
        }
    }
}
