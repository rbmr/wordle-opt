#![allow(clippy::needless_range_loop)]
//! The exact branch-and-bound solver behind `wordle-opt`, the strategies it
//! can capture into a policy tree, and the optimality certificates it can
//! write. The shared modules from `wordle_common` are re-exported, so
//! `wordle_solver::dict` and `wordle_common::dict` are the same module.

/// Records a lower-bound fact for an optimality certificate (see `certlog`).
macro_rules! cert_record {
    ($site:expr, $set:expr, $value:expr, $exact:expr) => {
        crate::certlog::record($site, $set, $value, $exact);
    };
}

pub use wordle_common::{core, dict, matrix, tree};

pub mod cache;
pub mod cert;
pub mod certlog;
pub mod heuristic;
pub mod naive;
pub mod parallel_depth;
pub mod policy;
pub mod solver;
pub mod verify;
