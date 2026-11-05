//! Smoke test of the `wordle-opt` binary: reading word lists from files,
//! `solve` writing a tree, and `validate` reading it back. The solver itself is
//! tested through the library in the other test files.

mod common;

use common::*;
use std::fs;
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wordle-opt"))
        .args(args)
        .output()
        .expect("spawn wordle-opt");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "wordle-opt {args:?} failed:\n{text}");
    text
}

fn total_cost(text: &str) -> u64 {
    text.lines()
        .find_map(|l| l.strip_prefix("Total cost: "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no total cost in:\n{text}"))
}

/// `solve` (default strategy) and `validate` agree with brute force on a small
/// list read from files, including upper case and blank lines in the input,
/// and the tree embeds exactly the normalized lists.
#[test]
fn solve_and_validate_from_files() {
    let mut rng = Rng::new(7);
    let inst = Instance::bundled();
    let guesses = rng.sample(&inst.guesses, 40);
    let candidates = rng.sample(&guesses, 14);
    let inst = Instance::new(guesses, candidates);

    let dir = tempfile::tempdir().expect("temp dir");
    let write = |name: &str, words: &[W]| {
        let path = dir.path().join(name);
        let text: Vec<String> = words.iter().map(|w| show(w).to_uppercase()).collect();
        fs::write(&path, format!("{}\n\n", text.join("\n"))).expect("write list");
        path
    };
    let g = write("guesses.txt", &inst.guesses);
    let c = write("candidates.txt", &inst.candidates);
    let tree = dir.path().join("tree.json");

    let solved = run(&[
        "solve",
        "--guesses",
        g.to_str().unwrap(),
        "--candidates",
        c.to_str().unwrap(),
        "--output",
        tree.to_str().unwrap(),
        "--no-progress",
    ]);
    let validated = run(&["validate", tree.to_str().unwrap()]);

    let optimum = Oracle::optimum(&inst.guesses, &inst.candidates) as u64;
    assert_eq!(total_cost(&solved), optimum);
    assert_eq!(total_cost(&validated), optimum);

    let file = wordle_opt::policy::ReadableTreeFile::from_json(
        &fs::read_to_string(&tree).expect("read tree"),
    )
    .expect("parse tree");
    assert_eq!(file.strategy, "optimal");
    assert_eq!(audit_tree(&file).cost, optimum);
    let listed: Vec<W> = file.candidates.iter().map(|s| word(s)).collect();
    assert_eq!(listed, inst.candidates);
    let listed: Vec<W> = file.guesses.iter().map(|s| word(s)).collect();
    assert_eq!(listed, inst.guesses);
}

#[test]
fn solve_and_validate_non_5_letter_words() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (len, words) in [
        (3, vec!["cat", "car", "bat", "bar", "rat", "tar"]),
        (4, vec!["game", "gate", "gave", "tame", "late", "bake"]),
        (6, vec!["flight", "bright", "slight", "knight", "plight"]),
    ] {
        let file_path = dir.path().join(format!("words_{len}.txt"));
        fs::write(&file_path, words.join("\n")).expect("write words");
        let tree_path = dir.path().join(format!("tree_{len}.json"));

        let solved = run(&[
            "solve",
            "--guesses",
            file_path.to_str().unwrap(),
            "--candidates",
            file_path.to_str().unwrap(),
            "--output",
            tree_path.to_str().unwrap(),
            "--no-progress",
        ]);
        let validated = run(&["validate", tree_path.to_str().unwrap()]);
        assert!(validated.contains("=== POLICY TREE VALID ==="));
        assert_eq!(total_cost(&solved), total_cost(&validated));

        let file = wordle_opt::policy::ReadableTreeFile::from_json(
            &fs::read_to_string(&tree_path).expect("read tree"),
        )
        .expect("parse tree");
        assert_eq!(file.strategy, "optimal");
        assert_eq!(file.candidates.len(), words.len());
        assert_eq!(file.guesses.len(), words.len());
    }
}
