//! Small word lists, solved by the solver and by two independent brute-force
//! searches: this crate's `NaiveSolver`, which has no bounds, caches or guess
//! deduplication, and the oracle in `common`, which also has its own feedback
//! function. All three must agree, and the solver's tree must be valid and cost
//! what the solver claims (checked in `common::solve`).

mod common;

use common::*;
use rayon::prelude::*;
use wordle_opt::core::Word;
use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::naive::NaiveSolver;

#[derive(Clone, Copy)]
enum Words {
    /// Real words from the bundled guess list.
    Bundled,
    /// Every word over the first `k` letters. A small alphabet piles many
    /// candidates into few responses, which stresses guess deduplication.
    Alphabet(usize),
}

struct Case {
    seed: u64,
    candidates: usize,
    guesses: usize,
    words: Words,
    why: &'static str,
}

/// The solver switches strategy at several candidate-set sizes, so the list
/// crosses each threshold it can reach.
const CASES: &[Case] = &[
    case(1, 1, 1, Words::Bundled, "base case of 1 candidate"),
    case(2, 2, 2, Words::Bundled, "base case of 2 candidates"),
    case(5, 8, 20, Words::Bundled, "fast path for at most 8"),
    case(6, 9, 20, Words::Bundled, "just above 8"),
    case(7, 10, 30, Words::Bundled, "at the 10-candidate boundary"),
    case(8, 11, 30, Words::Bundled, "just above 10"),
    case(9, 12, 12, Words::Bundled, "guesses equal candidates"),
    case(
        10,
        15,
        15,
        Words::Bundled,
        "quick bucket evaluation, at most 15",
    ),
    case(11, 16, 16, Words::Bundled, "just above 15"),
    case(12, 20, 20, Words::Bundled, "equivalence caching, 10 to 39"),
    case(13, 8, 8, Words::Alphabet(3), "three-letter alphabet"),
    case(
        14,
        10,
        60,
        Words::Bundled,
        "six times more guesses than candidates",
    ),
    case(
        15,
        30,
        30,
        Words::Bundled,
        "mid-range, guesses equal candidates",
    ),
    case(16, 40, 40, Words::Bundled, "at the 40-candidate boundary"),
    case(17, 48, 48, Words::Bundled, "parallel root scan, 48 and up"),
];

const fn case(
    seed: u64,
    candidates: usize,
    guesses: usize,
    words: Words,
    why: &'static str,
) -> Case {
    Case {
        seed,
        candidates,
        guesses,
        words,
        why,
    }
}

fn pool(words: Words) -> Vec<W> {
    match words {
        Words::Bundled => Instance::bundled().guesses,
        Words::Alphabet(k) => {
            let letters: Vec<u8> = (b'a'..b'a' + k as u8).collect();
            let mut out = Vec::with_capacity(k.pow(5));
            for i in 0..k.pow(5) {
                let mut w = [0u8; 5];
                let mut x = i;
                for slot in w.iter_mut() {
                    *slot = letters[x % k];
                    x /= k;
                }
                out.push(w);
            }
            out
        }
    }
}

/// Draws the guess list from the pool, then the candidates from the guesses.
fn build(case: &Case) -> Instance {
    let mut rng = Rng::new(case.seed);
    let guesses = rng.sample(&pool(case.words), case.guesses);
    let candidates = rng.sample(&guesses, case.candidates);
    Instance::new(guesses, candidates)
}

fn naive_optimum(inst: &Instance) -> u64 {
    let dict = Dictionary::from_words(
        inst.guesses.iter().map(|&w| Word(w)).collect(),
        inst.candidates.iter().map(|&w| Word(w)).collect(),
    );
    let matrix = ResponseMatrix::new(&dict);
    let all: Vec<usize> = (0..dict.candidates.len()).collect();
    NaiveSolver::new(&matrix, &dict).solve(&all) as u64
}

fn check(label: &str, inst: &Instance) {
    let solver = solve_cost(inst);
    let oracle = Oracle::optimum(&inst.guesses, &inst.candidates) as u64;
    let naive = naive_optimum(inst);
    assert!(
        solver == oracle && oracle == naive,
        "{label} ({} candidates, {} guesses): solver {solver}, oracle {oracle}, naive {naive}",
        inst.candidates.len(),
        inst.guesses.len()
    );
}

/// Hand-picked sizes that cross the solver's internal thresholds.
#[test]
fn threshold_cases_match_brute_force() {
    CASES.par_iter().for_each(|c| {
        let label = format!("seed {} ({})", c.seed, c.why);
        check(&label, &build(c));
    });
}

/// Random sizes, guess-list lengths and alphabets, one seed each.
#[test]
fn random_cases_match_brute_force() {
    (100..400u64).into_par_iter().for_each(|seed| {
        let mut rng = Rng::new(seed);
        let candidates = 3 + rng.below(40);
        let guesses = candidates + rng.below(3 * candidates);
        let words = match rng.below(3) {
            0 => Words::Alphabet(4 + rng.below(3)),
            _ => Words::Bundled,
        };
        let c = Case {
            seed,
            candidates,
            guesses,
            words,
            why: "random",
        };
        check(&format!("seed {seed}"), &build(&c));
    });
}
