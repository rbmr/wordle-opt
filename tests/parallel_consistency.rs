//! Multi-threaded solves must reach the same optimum as a single-threaded one.
//!
//! The parallel scans share a global bound between threads, so a search can be
//! cut short at a moment that depends on thread timing. A result from such a
//! search must never be reused as if it were proven; if it is, an occasional
//! run returns a valid but suboptimal tree. That failure is rare and depends on
//! timing, so this test solves one instance many times. It is ignored by
//! default:
//!
//! ```text
//! cargo test --release --test parallel_consistency -- --ignored --nocapture
//! ```

mod common;

use common::*;
use std::process::Command;

/// Solves `--max-candidates 200 --sample-seed 5059` of the bundled answer
/// list, with the answer list as the guess list, through the binary, and
/// returns the cost of the written tree as re-derived by the independent
/// walker in `tests/common/`.
fn solve_once(threads: Option<usize>, tree: &std::path::Path) -> u64 {
    let list = concat!(env!("CARGO_MANIFEST_DIR"), "/words/candidates.txt");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wordle-opt"));
    cmd.args([
        "solve",
        "--guesses",
        list,
        "--candidates",
        list,
        "--max-candidates",
        "200",
        "--sample-seed",
        "5059",
        "--no-progress",
        "--output",
        tree.to_str().unwrap(),
    ]);
    if let Some(n) = threads {
        cmd.env("RAYON_NUM_THREADS", n.to_string());
    }
    let out = cmd.output().expect("spawn wordle-opt");
    assert!(
        out.status.success(),
        "wordle-opt solve failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json = std::fs::read_to_string(tree).expect("read tree");
    let file = wordle_opt::policy::ReadableTreeFile::from_json(&json).expect("parse tree");
    audit_tree(&file).cost
}

/// An instance on which runs whose searches were cut short by another thread
/// used to return 539 instead of the optimum 538 a few times in a hundred.
#[test]
#[ignore = "repeated solves: ~1.5 min; run with --ignored"]
fn multi_threaded_solves_match_single_threaded_optimum() {
    const OPTIMUM: u64 = 538;
    const RUNS: usize = 300;
    let dir = tempfile::tempdir().expect("temp dir");
    let tree = dir.path().join("tree.json");

    assert_eq!(solve_once(Some(1), &tree), OPTIMUM, "single-threaded solve");

    let mut costs = std::collections::BTreeMap::new();
    for _ in 0..RUNS {
        *costs.entry(solve_once(None, &tree)).or_insert(0usize) += 1;
    }
    println!("costs over {RUNS} multi-threaded runs: {costs:?}");
    assert_eq!(
        costs.keys().copied().collect::<Vec<_>>(),
        vec![OPTIMUM],
        "multi-threaded runs must all reach the optimum; costs seen: {costs:?}"
    );
}
