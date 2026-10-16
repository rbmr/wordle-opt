//! At every node, playing any other guess first and then playing optimally
//! costs `|S|` plus the optimal cost of each bucket. If that beats the subtree
//! the solver chose, the solver missed a better guess. Alternatives are the
//! guesses ranked best by the expected-remaining heuristic plus a few random
//! ones. Bucket optima come from brute force when small and from a fresh
//! solve otherwise.

mod common;

use common::*;
use rayon::prelude::*;
use std::time::Instant;

/// No alternative guess beats the tree's choice at any node with three or
/// more candidates.
#[test]
fn no_alternative_guess_is_better() {
    let cases = [
        ("bundled n=80 full guesses", bundled_subset(11, 80, 0)),
        ("bundled n=150 g=1500", bundled_subset(12, 150, 1500)),
        ("bundled n=300 g=600", bundled_subset(13, 300, 600)),
        ("alphabet6 n=120 g=300", small_alphabet(14, 6, 120, 300)),
    ];
    cases.par_iter().enumerate().for_each(|(i, (label, inst))| {
        let t = Instant::now();
        let base = solve(inst);
        let internal: Vec<&AuditNode> = base
            .audit
            .nodes
            .iter()
            .filter(|n| n.set.len() >= 3)
            .collect();
        let alts = fuzz_alternatives(inst, &internal, 4, 2, 100 + i as u64, label);
        eprintln!(
            "{label}: cost {}, {alts} alternatives evaluated, {:.1?}",
            base.audit.cost,
            t.elapsed()
        );
    });
}
