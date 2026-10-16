//! Transformations of the input whose effect on the optimum is known without
//! solving it: renaming letters or permuting positions keeps it, dropping
//! guesses the tree never plays keeps it, adding guesses cannot raise it,
//! dropping a guess cannot lower it, and removing a candidate lowers it by at
//! least that candidate's depth in the tree.

mod common;

use common::*;
use rayon::prelude::*;
use std::collections::HashSet;
use std::time::Instant;

/// Runs every metamorphic relation against `inst`, whose optimal tree is
/// `base`. Each relation either must preserve the optimum exactly or bounds it
/// in a known direction.
fn check_metamorphic(inst: &Instance, base: &Solved, seed: u64, label: &str) {
    let c = base.audit.cost;
    let mut rng = Rng::new(seed);
    let lower: [u8; 26] = *b"abcdefghijklmnopqrstuvwxyz";

    // Renaming the letters is an isomorphism of the instance.
    for _ in 0..2 {
        let got = solve_cost(&relabel_letters(inst, &lower, &mut rng));
        assert_eq!(got, c, "{label}: renaming letters changed the optimum");
    }

    // Permuting positions consistently preserves every partition (which
    // occurrence of a repeated letter turns yellow may move, but two secrets
    // collide under a guess before the permutation iff they do after it).
    for _ in 0..2 {
        let got = solve_cost(&permute_positions(inst, &mut rng));
        assert_eq!(got, c, "{label}: permuting positions changed the optimum");
    }

    // Guesses the tree never plays can be dropped: the tree stays valid (so the
    // optimum cannot rise) and fewer guesses cannot lower it.
    let used: HashSet<W> = base.audit.nodes.iter().map(|n| n.guess).collect();
    let cand: HashSet<W> = inst.candidates.iter().copied().collect();
    let unused: Vec<W> = inst
        .guesses
        .iter()
        .filter(|g| !used.contains(*g) && !cand.contains(*g))
        .copied()
        .collect();
    if !unused.is_empty() {
        let drop: HashSet<W> = rng
            .sample(&unused, unused.len().div_ceil(2))
            .into_iter()
            .collect();
        let kept: Vec<W> = inst
            .guesses
            .iter()
            .filter(|g| !drop.contains(*g))
            .copied()
            .collect();
        let got = solve_cost(&Instance::new(kept, inst.candidates.clone()));
        assert_eq!(
            got,
            c,
            "{label}: dropping {} unused guesses changed the optimum",
            drop.len()
        );
    }

    // Dropping a guess the tree does play (a non-candidate, since candidates
    // are always guessable) cannot lower the optimum.
    if let Some(g) = base
        .audit
        .nodes
        .iter()
        .map(|n| n.guess)
        .find(|g| !cand.contains(g))
    {
        let kept: Vec<W> = inst.guesses.iter().filter(|&x| *x != g).copied().collect();
        let got = solve_cost(&Instance::new(kept, inst.candidates.clone()));
        assert!(
            got >= c,
            "{label}: dropping used guess {} lowered the optimum {c} -> {got}",
            show(&g)
        );
    }

    // Extra guesses cannot raise the optimum.
    let bundled = Instance::bundled();
    let extra = rng.sample(&bundled.guesses, 400);
    let mut more = inst.guesses.clone();
    more.extend(extra.iter().map(|w| w.map(|b| lower[(b - b'a') as usize])));
    let got = solve_cost(&Instance::new(more, inst.candidates.clone()));
    assert!(
        got <= c,
        "{label}: adding guesses raised the optimum {c} -> {got}"
    );

    // Removing candidate x: the tree with x's path removed is still valid and
    // saves exactly x's depth, so the optimum drops by at least that much.
    for x in rng.sample(&inst.candidates, 3) {
        if inst.candidates.len() < 2 {
            break;
        }
        let rest: Vec<W> = inst
            .candidates
            .iter()
            .filter(|&w| *w != x)
            .copied()
            .collect();
        let got = solve_cost(&inst.with_candidates(rest));
        let bound = c - base.audit.win_depth[&x] as u64;
        assert!(
            got <= bound,
            "{label}: removing {} (depth {}) gave {got}, above {bound}",
            show(&x),
            base.audit.win_depth[&x]
        );
    }
}

/// Optimum-preserving and optimum-bounding transformations of the input.
#[test]
fn metamorphic_relations_hold() {
    let cases = [
        ("bundled n=49 full guesses", bundled_subset(21, 49, 0)),
        ("bundled n=120 g=2000", bundled_subset(22, 120, 2000)),
        ("bundled n=400 g=800", bundled_subset(23, 400, 800)),
        ("alphabet6 n=150 g=200", small_alphabet(24, 6, 150, 200)),
    ];
    cases.par_iter().enumerate().for_each(|(i, (label, inst))| {
        let t = Instant::now();
        let base = solve(inst);
        check_metamorphic(inst, &base, 200 + i as u64, label);
        eprintln!(
            "{label}: cost {}, relations hold, {:.1?}",
            base.audit.cost,
            t.elapsed()
        );
    });
}

/// Arbitrary input means arbitrary symbols, not just `a`-`z`: renaming the
/// letters onto digits and punctuation is still an isomorphism.
#[test]
fn metamorphic_non_latin_alphabet() {
    let symbols: [u8; 26] = *b"0123456789!#$%&*+-=?@^_~;:";
    let cases = [
        ("bundled n=60 g=1000", bundled_subset(31, 60, 1000)),
        ("bundled n=200 g=1000", bundled_subset(32, 200, 1000)),
    ];
    for (i, (label, inst)) in cases.iter().enumerate() {
        let c = solve_cost(inst);
        let mut rng = Rng::new(300 + i as u64);
        for _ in 0..2 {
            let got = solve_cost(&relabel_letters(inst, &symbols, &mut rng));
            assert_eq!(
                got, c,
                "{label}: renaming letters onto symbols changed the optimum"
            );
        }
        // Keep a-m and rename n-z onto the symbols whose byte value is the
        // same letter index mod 26 ('4' = 52 = 0 mod 26, like 'a'), so a
        // letter table that folds non-letters with `% 26` merges every pair.
        let colliding = inst.map_words(|w| {
            w.map(|b| match b {
                b'a'..=b'm' => b,
                _ => b"456789:;<=>?@"[(b - b'n') as usize],
            })
        });
        let got = solve_cost(&colliding);
        assert_eq!(
            got, c,
            "{label}: letters and symbols that share an index mod 26 changed the optimum"
        );
    }
}

/// The same collision on a six-symbol alphabet, where every word mixes
/// letters with their colliding symbols: `4`, `5`, `6` share `a`, `b`, `c`'s
/// index mod 26.
#[test]
fn metamorphic_colliding_symbols_small_alphabet() {
    for seed in 0..8u64 {
        let inst = small_alphabet(500 + seed, 6, 60 + 10 * seed as usize, 150);
        let c = solve_cost(&inst);
        let colliding = inst.map_words(|w| {
            w.map(|b| match b {
                b'd' => b'4',
                b'e' => b'5',
                b'f' => b'6',
                _ => b,
            })
        });
        let got = solve_cost(&colliding);
        assert_eq!(got, c, "seed {seed}: colliding symbols changed the optimum");
    }
}

/// A guess list longer than the bundled 14,855 words, solved in the same
/// process (and so on the same pooled per-thread scratch) as smaller ones.
/// Extra guesses cannot raise the optimum, and dropping the extras the tree
/// never plays must reproduce it exactly.
#[test]
fn guess_list_larger_than_bundled() {
    let mut rng = Rng::new(41);
    let alphabet: Vec<u8> = (b'a'..=b'z').collect();
    let base = bundled_subset(42, 500, 0);
    let mut guesses = base.guesses.clone();
    while guesses.len() < base.guesses.len() + 5000 {
        guesses.push(random_word(&alphabet, &mut rng));
    }
    let big = Instance::new(guesses, base.candidates.clone());
    assert!(big.guesses.len() > 14_855 + 4_000);

    let small_cost = solve_cost(&bundled_subset(43, 40, 300));
    let base_cost = solve_cost(&base);
    let solved = solve(&big);
    assert!(solved.audit.cost <= base_cost);
    let used: HashSet<W> = solved.audit.nodes.iter().map(|n| n.guess).collect();
    let kept: Vec<W> = big
        .guesses
        .iter()
        .filter(|g| base.guesses.contains(g) || used.contains(*g))
        .copied()
        .collect();
    let back = solve_cost(&Instance::new(kept, big.candidates.clone()));
    assert_eq!(back, solved.audit.cost);
    assert_eq!(solve_cost(&bundled_subset(43, 40, 300)), small_cost);
}
