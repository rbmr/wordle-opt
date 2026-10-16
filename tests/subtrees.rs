//! Every subtree of an optimal tree is itself optimal for its candidate set,
//! because total cost is additive: a cheaper subtree could be swapped in.
//! These tests check subtrees small enough for brute force against the
//! oracle, and larger subtrees against a fresh solve of the same set.

mod common;

use common::*;
use rayon::prelude::*;
use std::time::Instant;

/// The full 2,340-candidate optimal tree committed for the viewer (byte
/// identical to the latest full run): every subtree small enough for brute
/// force must be exactly optimal.
#[test]
fn full_tree_small_subtrees_are_brute_force_optimal() {
    let file = load_committed_full_tree();
    let audit = audit_tree(&file);
    assert_eq!(audit.cost, FULL_OPTIMUM);
    let guesses: Vec<W> = file.guesses.iter().map(|s| word(s)).collect();
    let t = Instant::now();
    let checked = check_small_subtrees(&guesses, &audit, "full tree");
    let covered: usize = audit
        .nodes
        .iter()
        .filter(|n| n.set.len() <= oracle_max())
        .count();
    eprintln!(
        "full tree: {checked} maximal subtrees (<= {} candidates) brute-forced, \
         covering {covered}/{} nodes, in {:.1?}",
        oracle_max(),
        audit.nodes.len(),
        t.elapsed()
    );
    assert!(checked > 100);
}

fn solved_instances() -> Vec<(&'static str, Instance)> {
    vec![
        ("bundled n=80 full guesses", bundled_subset(11, 80, 0)),
        ("bundled n=150 g=1500", bundled_subset(12, 150, 1500)),
        ("bundled n=300 g=600", bundled_subset(13, 300, 600)),
        ("alphabet6 n=120 g=300", small_alphabet(14, 6, 120, 300)),
    ]
}

/// Instances solved by the current solver: every small subtree must match
/// brute force.
#[test]
fn small_subtrees_match_brute_force() {
    solved_instances().par_iter().for_each(|(label, inst)| {
        let t = Instant::now();
        let base = solve(inst);
        let checked = check_small_subtrees(&inst.guesses, &base.audit, label);
        eprintln!(
            "{label}: cost {}, {checked} subtrees brute-forced, {:.1?}",
            base.audit.cost,
            t.elapsed()
        );
    });
}

/// Instances solved by the current solver: every subtree too large for brute
/// force must cost exactly what a fresh solve of its candidate set returns.
#[test]
fn large_subtrees_match_a_fresh_solve() {
    solved_instances().par_iter().for_each(|(label, inst)| {
        let t = Instant::now();
        let base = solve(inst);
        let resolved = check_resolved_subtrees(inst, &base.audit, usize::MAX, label);
        eprintln!(
            "{label}: cost {}, {resolved} subtrees re-solved, {:.1?}",
            base.audit.cost,
            t.elapsed()
        );
    });
}
