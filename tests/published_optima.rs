//! Full-scale word lists whose optimum Alex Selby computed with his own,
//! independent solver (see `words/published/README.md`). Each test is a
//! full-size solve, so they are ignored by default:
//!
//! ```text
//! cargo test --release --test published_optima -- --ignored --nocapture
//! ```

mod common;

use common::*;
use std::time::Instant;

/// Full-scale instances whose optimum was computed independently by Alex
/// Selby's solver (see `words/published/README.md`); the `orig` value was also
/// reproduced by Bertsimas and Paskov. The tree must reach exactly the
/// published total.
#[test]
#[ignore = "full-scale: four ~40 s solves; run with --ignored"]
fn published_optima() {
    let cases = [
        ("orig", "words/published/wordlist_orig_all.txt", 7920),
        (
            "nyt20220215",
            "words/published/wordlist_nyt20220215_all.txt",
            7897,
        ),
        (
            "nyt20220316",
            "words/published/wordlist_nyt20220316_all.txt",
            7897,
        ),
        ("nyt20220830", "words/guesses.txt", 7883),
    ];
    for (name, guesses, published) in cases {
        let candidates = read_list(&format!("words/published/wordlist_{name}_hidden.txt"));
        let inst = Instance::new(read_list(guesses), candidates);
        let t = Instant::now();
        let got = solve_cost(&inst);
        eprintln!(
            "{name}: {} candidates x {} guesses: cost {got} (published {published}) in {:.1?}",
            inst.candidates.len(),
            inst.guesses.len(),
            t.elapsed()
        );
        assert_eq!(
            got, published,
            "{name}: solver disagrees with the published optimum"
        );
    }
}

/// The 3,158-candidate NYT list of July 2023 against the full guess list:
/// larger than any instance the solver is otherwise run on. Published optimum
/// 11,219 (Selby); about 6 minutes on a 16-core machine.
#[test]
#[ignore = "full-scale: one ~6 minute solve on 16 cores; run with --ignored"]
fn published_optimum_3158() {
    let inst = Instance::new(
        read_list("words/guesses.txt"),
        read_list("words/published/wordlist_nyt20230701_hidden.txt"),
    );
    assert_eq!(inst.candidates.len(), 3158);
    let t = Instant::now();
    let got = solve_cost(&inst);
    eprintln!(
        "nyt20230701: cost {got} (published 11219) in {:.1?}",
        t.elapsed()
    );
    assert_eq!(got, 11219, "solver disagrees with the published optimum");
}
