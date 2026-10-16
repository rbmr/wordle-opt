//! Instances built so that their optimum is known in closed form, at sizes
//! brute force cannot reach. See [`planted_expected`] for the argument.

mod common;

use common::*;
use rayon::prelude::*;
use std::collections::HashMap;

/// Whether `g` sends every member of `set` to a different response.
fn separates(g: &W, set: &[W]) -> bool {
    let mut seen = [false; 243];
    set.iter()
        .all(|s| !std::mem::replace(&mut seen[feedback(g, s) as usize], true))
}

/// The exact optimum whenever some guess separates the whole set.
///
/// Every candidate costs at least one guess, and only the first guess can win
/// with one, so `T >= 2n - 1`. Equality needs the first guess to be a
/// candidate and every other candidate to be won at guess two, i.e. a
/// candidate that separates the set. Without one, either the first guess is
/// not a candidate (all n cost >= 2) or some bucket holds >= 2 candidates, of
/// which at most one is won at guess two; either way `T >= 2n`. Any separating
/// guess achieves `2n`. Returns `None` when no guess separates.
fn planted_expected(inst: &Instance) -> Option<u64> {
    let n = inst.candidates.len() as u64;
    if inst
        .candidates
        .iter()
        .any(|c| separates(c, &inst.candidates))
    {
        Some(2 * n - 1)
    } else if inst.guesses.iter().any(|g| separates(g, &inst.candidates)) {
        Some(2 * n)
    } else {
        None
    }
}

/// One random secret per response of `pivot`, as many responses as sampling
/// reaches (about 238 of the 243 for a pivot with distinct letters). The
/// sampler favours `pivot`'s own letters so rare responses (all-yellow, four
/// greens) still turn up.
fn one_word_per_response(pivot: &W, alphabet: &[u8], rng: &mut Rng) -> Vec<W> {
    let mut by_code: HashMap<u8, W> = HashMap::new();
    for _ in 0..300_000 {
        let mut w = [0u8; 5];
        for (i, slot) in w.iter_mut().enumerate() {
            *slot = match rng.below(20) {
                0..=4 => pivot[i],
                5..=11 => pivot[rng.below(5)],
                _ => alphabet[rng.below(alphabet.len())],
            };
        }
        by_code.entry(feedback(pivot, &w)).or_insert(w);
    }
    let mut v: Vec<W> = by_code.into_values().collect();
    v.sort_unstable();
    v
}

/// Instances whose optimum is known in closed form at sizes brute force cannot
/// reach, crossing the solver's size thresholds (10, 15/16, 48).
#[test]
fn planted_optima() {
    let alphabet: Vec<u8> = (b'a'..=b'z').collect();
    let sizes = [
        3usize, 9, 10, 11, 15, 16, 17, 40, 47, 48, 49, 100, 160, 200, 243,
    ];
    let cases: Vec<(u64, usize, bool)> = sizes
        .iter()
        .enumerate()
        .flat_map(|(i, &n)| [(i as u64, n, true), (1000 + i as u64, n, false)])
        .collect();
    cases.par_iter().for_each(|&(seed, n, pivot_is_candidate)| {
        let mut rng = Rng::new(seed);
        // A pivot with five distinct letters reaches the most responses.
        let mut letters = alphabet.clone();
        rng.shuffle(&mut letters);
        let pivot: W = [letters[0], letters[1], letters[2], letters[3], letters[4]];
        let pool = one_word_per_response(&pivot, &alphabet, &mut rng);
        let mut pool: Vec<W> = pool.into_iter().filter(|w| *w != pivot).collect();
        rng.shuffle(&mut pool);
        let mut candidates: Vec<W> = pool
            .into_iter()
            .take(n.saturating_sub(pivot_is_candidate as usize))
            .collect();
        if pivot_is_candidate {
            candidates.push(pivot);
        }
        let mut guesses: Vec<W> = (0..300).map(|_| random_word(&alphabet, &mut rng)).collect();
        guesses.push(pivot);
        let inst = Instance::new(guesses, candidates);
        let n = inst.candidates.len();
        let expected = planted_expected(&inst).expect("the pivot separates the set");
        let got = solve_cost(&inst);
        assert_eq!(
            got,
            expected,
            "planted n={n} seed={seed} pivot {} ({}candidate): solver {got}, optimum {expected}",
            show(&pivot),
            if pivot_is_candidate { "" } else { "non-" }
        );
    });
}
