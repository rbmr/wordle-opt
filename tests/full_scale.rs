//! Rebuilds the bundled 2,340-candidate tree with the current solver and runs
//! every check on it: brute force on small subtrees, fresh re-solves of larger
//! ones, alternative guesses at the root and at depth 1, and full solves of a
//! letter-renamed and a position-permuted copy. Takes a few minutes on 16
//! cores, so it is ignored by default:
//!
//! ```text
//! cargo test --release --test full_scale -- --ignored --nocapture
//! ```

mod common;

use common::*;
use std::time::Instant;

/// The full-scale audit against the *current* solver: rebuilds the 2,340
/// candidate tree, brute-forces its small subtrees, re-solves every larger
/// subtree, fuzzes alternative guesses at the root and depth 1, and checks
/// that renaming letters and permuting positions reproduce the optimum.
#[test]
#[ignore = "full-scale: several full solves; run with --ignored"]
fn full_scale_current_solver_audit() {
    let inst = Instance::bundled();
    let t = Instant::now();
    let base = solve(&inst);
    eprintln!(
        "full solve: cost {} in {:.1?}",
        base.audit.cost,
        t.elapsed()
    );
    assert_eq!(base.audit.cost, FULL_OPTIMUM);
    // A different tree of equal cost is not wrong (optima are not unique), so
    // this is reported rather than asserted.
    let same = base.tree.to_json() == load_committed_full_tree().to_json();
    eprintln!("identical to site/examples/optimal.json: {same}");

    let t = Instant::now();
    let small = check_small_subtrees(&inst.guesses, &base.audit, "full");
    eprintln!("{small} small subtrees brute-forced in {:.1?}", t.elapsed());

    let t = Instant::now();
    let resolved = check_resolved_subtrees(&inst, &base.audit, usize::MAX, "full");
    eprintln!(
        "{resolved} larger subtrees re-solved in {:.1?}",
        t.elapsed()
    );

    let t = Instant::now();
    let depth1: Vec<&AuditNode> = base
        .audit
        .nodes
        .iter()
        .filter(|n| n.depth == 1 && n.set.len() >= 3)
        .collect();
    let alts = fuzz_alternatives(&inst, &depth1, 3, 1, 900, "full depth 1");
    eprintln!(
        "{alts} depth-1 alternatives evaluated in {:.1?}",
        t.elapsed()
    );

    let t = Instant::now();
    let root: Vec<&AuditNode> = base.audit.nodes.iter().filter(|n| n.depth == 0).collect();
    let alts = fuzz_alternatives(&inst, &root, 2, 1, 901, "full root");
    eprintln!("{alts} root alternatives evaluated in {:.1?}", t.elapsed());

    let mut rng = Rng::new(902);
    let t = Instant::now();
    let lower: [u8; 26] = *b"abcdefghijklmnopqrstuvwxyz";
    assert_eq!(
        solve_cost(&relabel_letters(&inst, &lower, &mut rng)),
        FULL_OPTIMUM
    );
    assert_eq!(
        solve_cost(&permute_positions(&inst, &mut rng)),
        FULL_OPTIMUM
    );
    eprintln!("relabel + permutation full solves in {:.1?}", t.elapsed());
}
