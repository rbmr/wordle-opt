//! The solver and the certificate checker share the word lists, the response
//! table and the tree format (`core`, `dict`, `matrix` and `tree`). A bug there would reach both,
//! so this file checks each against code written independently of it, in
//! `tests/common/`.

mod common;

use common::*;
use wordle_opt::core::{Response, Word};
use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::policy::{BuildOptions, ReadableTreeFile, Strategy, build_policy_tree};

/// Every word of length 5 over `letters`.
fn all_words(letters: &[u8]) -> Vec<W> {
    let k = letters.len();
    (0..k.pow(5))
        .map(|mut i| {
            std::array::from_fn(|_| {
                let l = letters[i % k];
                i /= k;
                l
            })
        })
        .collect()
}

/// Requires the table, both of its layouts, and `Response::compute` to agree
/// with the independent feedback function on every pair of `words`.
fn check_table(guesses: Vec<W>, candidates: Vec<W>) {
    let dict = Dictionary::from_words(
        guesses.into_iter().map(Word::from).collect(),
        candidates.into_iter().map(Word::from).collect(),
    );
    let m = ResponseMatrix::new(&dict);
    assert_eq!(m.num_guesses, dict.guesses.len());
    assert_eq!(m.num_candidates, dict.candidates.len());
    for (g, guess) in dict.guesses.iter().enumerate() {
        for (c, cand) in dict.candidates.iter().enumerate() {
            let want = feedback(
                guess.as_bytes().try_into().unwrap(),
                cand.as_bytes().try_into().unwrap(),
            );
            let row = m.data[g * m.num_candidates + c].0;
            let col = m.data_c_g[c * m.num_guesses + g].0;
            let direct = Response::compute(cand, guess).0;
            assert!(
                row == want && col == want && direct == want,
                "{} on {}: table {row}, transposed {col}, compute {direct}, expected {want}",
                guess,
                cand
            );
        }
    }
    for (c, &g) in dict.candidate_to_guess.iter().enumerate() {
        assert_eq!(dict.guesses[g], dict.candidates[c]);
    }
}

#[test]
fn response_table_matches_on_every_pair_over_small_alphabets() {
    // Few letters means many repeated letters, where the yellow counting rules
    // matter most.
    for letters in [&b"ab"[..], b"abc", b"abcd"] {
        let words = all_words(letters);
        check_table(words.clone(), words);
    }
}

#[test]
fn response_table_matches_on_random_words() {
    let mut rng = Rng::new(7);
    let lower: Vec<u8> = (b'a'..=b'z').collect();
    // More than 31 distinct symbols: the table's letter masks are disabled
    // there, which must not change any response.
    let wide: Vec<u8> = (b'!'..=b'~').filter(|b| !b.is_ascii_uppercase()).collect();
    for alphabet in [&lower, &wide] {
        let word =
            |rng: &mut Rng| -> W { std::array::from_fn(|_| alphabet[rng.below(alphabet.len())]) };
        let guesses: Vec<W> = (0..600).map(|_| word(&mut rng)).collect();
        let candidates: Vec<W> = (0..400).map(|_| word(&mut rng)).collect();
        check_table(guesses, candidates);
    }
}

#[test]
fn trees_survive_the_readable_format_unchanged() {
    for seed in 0..6u64 {
        let mut rng = Rng::new(100 + seed);
        let base = Instance::bundled();
        let guesses = rng.sample(&base.guesses, 60);
        let candidates = rng.sample(&guesses, 25);
        let inst = Instance::new(guesses, candidates);
        let dict = Dictionary::from_words(
            inst.guesses.iter().map(|&w| Word::from(w)).collect(),
            inst.candidates.iter().map(|&w| Word::from(w)).collect(),
        );
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        for strategy in Strategy::all() {
            let (tree, stats) =
                build_policy_tree(&matrix, &dict, strategy, &root, &opts).expect("build");
            let json = tree.to_readable(&dict).expect("readable").to_json();
            let file = ReadableTreeFile::from_json(&json).expect("parse");
            let back = file.to_tree().expect("to tree");
            assert_eq!(back.nodes, tree.nodes, "seed {seed} {}", strategy.name());
            assert_eq!(back.edges, tree.edges, "seed {seed} {}", strategy.name());
            let again = file.embedded_dictionary().expect("dictionary");
            assert_eq!(again.guesses, dict.guesses);
            assert_eq!(again.candidates, dict.candidates);
            let report = back.validate(&matrix, &dict).expect("valid");
            assert_eq!(report.total_cost, stats.summary.total_cost);
            // The independent walker reaches the same cost from the JSON.
            assert_eq!(audit_tree(&file).cost, stats.summary.total_cost);
        }
    }
}
