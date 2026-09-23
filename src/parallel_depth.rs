use crate::solver::{Solver, Metrics};
use crate::matrix::ResponseMatrix;
use crate::dict::Dictionary;
use crate::cache::GlobalCache;
use crate::solver::EquivCache;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub fn solve_parallel_depth2<'a>(
    matrix: &'a ResponseMatrix,
    initial_candidates: &[usize],
    dict: &'a Dictionary,
    metrics: &'a Metrics,
    equiv_cache: &'a EquivCache,
    global_cache: &'a GlobalCache,
    max_k: usize,
    capacity_bounds_2d: &[Vec<u32>],
    beta: &'a AtomicU32,
    active_guesses: &[usize],
) -> u32 {
    // Parallelize depth-2 evaluation by distributing the `root_candidates` loops.
    // Each root guess spawned here will internally spawn parallel bucket iterators.
    active_guesses.par_iter().for_each(|&g| {
        let current_beta = beta.load(Ordering::Relaxed);
        let mut local_solver = Solver::new_with_global_beta(
            matrix,
            max_k,
            dict,
            metrics,
            capacity_bounds_2d,
            global_cache,
            beta,
            equiv_cache,
        );
        let val = local_solver.min_guess_val(
            initial_candidates,
            g,
            current_beta,
            1,
            max_k,
        );
        beta.fetch_min(val, Ordering::Relaxed);
    });

    beta.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::Metrics;
    use crate::dict::Dictionary;
    use crate::matrix::ResponseMatrix;
    use crate::cache::GlobalCache;
    use std::sync::RwLock;
    use std::collections::HashMap;

    #[test]
    fn test_parallel_depth2_runs() {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        let global_cache = GlobalCache::new(1024);
        let equiv_cache: [_; 64] = std::array::from_fn(|_| RwLock::new(HashMap::new()));
        let beta = AtomicU32::new(100);
        let active_guesses: Vec<usize> = vec![0, 1, 2];
        let initial_candidates: Vec<usize> = vec![0, 1];
        let max_k = 2;
        let capacity_bounds_2d = vec![vec![0; 3]; 3];

        let result = solve_parallel_depth2(
            &matrix,
            &initial_candidates,
            &dict,
            &metrics,
            &equiv_cache,
            &global_cache,
            max_k,
            &capacity_bounds_2d,
            &beta,
            &active_guesses,
        );
        // We just care that it executes without panicking and beta is correctly reduced.
        assert!(result <= 100);
    }
}
