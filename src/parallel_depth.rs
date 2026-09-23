use crate::solver::{Solver, Metrics};
use crate::matrix::ResponseMatrix;
use crate::dict::Dictionary;
use crate::cache::EquivCache;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub fn solve_parallel_depth2<'a>(
    matrix: &'a ResponseMatrix,
    initial_candidates: &[usize],
    dict: &'a Dictionary,
    metrics: &'a Metrics,
    equiv_cache: &'a EquivCache,
) -> u32 {
    // This function will replace the root parallelism in `solver::solve`
    // and push parallelism to depth 2 using a nested par_iter!
    0
}
