//! Word lengths other than five, through the `wordle-opt` binary.
//!
//! The rest of the suite checks five-letter lists against independent
//! helpers. This file has its own feedback function and brute force for any
//! length, and checks that `solve` finds the optimum, that its certificate
//! checks, that a forged certificate does not, and that input the solver
//! cannot handle is refused rather than solved wrongly.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use wordle_opt::certcheck::check;
use wordle_opt::certfile::{self, Claim, Header, RULE_EXPANSION, SetDef};

fn wordle_opt(args: &[&str]) -> (Output, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_wordle-opt"))
        .args(args)
        .output()
        .expect("spawn wordle-opt");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, text)
}

fn run(args: &[&str]) -> String {
    let (out, text) = wordle_opt(args);
    assert!(out.status.success(), "wordle-opt {args:?} failed:\n{text}");
    text
}

fn total_cost(text: &str) -> u64 {
    text.lines()
        .find_map(|l| l.strip_prefix("Total cost: "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no total cost in:\n{text}"))
}

/// Feedback as a base-3 number, position `i` weighted `3^i`: 0 gray,
/// 1 green, 2 yellow. Greens first, then yellows left to right while the
/// answer still has unmatched copies of the letter.
fn feedback(guess: &[u8], answer: &[u8]) -> u32 {
    let mut marks = vec![0u32; guess.len()];
    let mut spare: HashMap<u8, usize> = HashMap::new();
    for i in 0..guess.len() {
        if guess[i] == answer[i] {
            marks[i] = 1;
        } else {
            *spare.entry(answer[i]).or_default() += 1;
        }
    }
    for i in 0..guess.len() {
        if marks[i] == 0
            && let Some(n) = spare.get_mut(&guess[i])
            && *n > 0
        {
            *n -= 1;
            marks[i] = 2;
        }
    }
    marks.iter().rev().fold(0, |acc, &m| acc * 3 + m)
}

fn all_green(len: usize) -> u32 {
    (0..len).fold(0, |acc, _| acc * 3 + 1)
}

/// Optimal total cost by exhaustive search over candidate subsets.
fn optimum(guesses: &[Vec<u8>], candidates: &[Vec<u8>]) -> u64 {
    fn solve(
        mask: u64,
        guesses: &[Vec<u8>],
        candidates: &[Vec<u8>],
        memo: &mut HashMap<u64, u64>,
    ) -> u64 {
        let n = mask.count_ones() as u64;
        if n == 1 {
            return 1;
        }
        if let Some(&v) = memo.get(&mask) {
            return v;
        }
        let win = all_green(candidates[0].len());
        let mut best = u64::MAX;
        for g in guesses {
            let mut buckets: HashMap<u32, u64> = HashMap::new();
            for (c, word) in candidates.iter().enumerate() {
                if mask >> c & 1 == 1 {
                    *buckets.entry(feedback(g, word)).or_default() |= 1 << c;
                }
            }
            if buckets.len() < 2 {
                continue;
            }
            let mut cost = n;
            for (&r, &b) in &buckets {
                if r != win {
                    cost += solve(b, guesses, candidates, memo);
                }
                if cost >= best {
                    break;
                }
            }
            best = best.min(cost);
        }
        memo.insert(mask, best);
        best
    }
    let full = (1u64 << candidates.len()) - 1;
    solve(full, guesses, candidates, &mut HashMap::new())
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn write_list(path: &Path, words: &[Vec<u8>]) {
    let text: Vec<String> = words
        .iter()
        .map(|w| String::from_utf8(w.clone()).unwrap())
        .collect();
    fs::write(path, text.join("\n") + "\n").expect("write list");
}

/// Random lists over small alphabets, so letters repeat within words and
/// feedback has many yellows; every length the solver supports.
#[test]
fn solve_and_certify_match_brute_force_at_every_length() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for len in 1..=10usize {
        for trial in 0..3 {
            let alphabet = &b"abcdefgh"[..2 + rng.below(4)];
            let word = |rng: &mut Rng| -> Vec<u8> {
                (0..len)
                    .map(|_| alphabet[rng.below(alphabet.len())])
                    .collect()
            };
            let mut candidates: Vec<Vec<u8>> = (0..9).map(|_| word(&mut rng)).collect();
            candidates.sort();
            candidates.dedup();
            let mut guesses: Vec<Vec<u8>> = (0..6).map(|_| word(&mut rng)).collect();
            guesses.extend(candidates.iter().cloned());
            guesses.sort();
            guesses.dedup();
            if candidates.len() < 2 {
                continue;
            }

            let g = dir.path().join(format!("g{len}_{trial}.txt"));
            let c = dir.path().join(format!("c{len}_{trial}.txt"));
            let tree = dir.path().join(format!("t{len}_{trial}.json"));
            let cert = dir.path().join(format!("t{len}_{trial}.cert"));
            write_list(&g, &guesses);
            write_list(&c, &candidates);
            let solved = run(&[
                "solve",
                "--guesses",
                g.to_str().unwrap(),
                "--candidates",
                c.to_str().unwrap(),
                "--output",
                tree.to_str().unwrap(),
                "--certify",
                cert.to_str().unwrap(),
                "--no-progress",
            ]);
            let expected = optimum(&guesses, &candidates);
            assert_eq!(
                total_cost(&solved),
                expected,
                "length {len}, trial {trial}: {candidates:?}"
            );
            let checked = run(&["check", tree.to_str().unwrap(), cert.to_str().unwrap()]);
            assert!(
                checked.contains("CERTIFICATE VALID: the tree is optimal"),
                "length {len}, trial {trial}:\n{checked}"
            );
        }
    }
}

/// Words that differ only in their last letter: every guess removes one
/// candidate, so the tree is a chain as deep as the list is long and costs
/// `1 + 2 + ... + n`. This is where long words used to break: the search
/// keeps per-level arrays with `3^L` entries, and guess deduplication looked
/// only at the first eight letters.
#[test]
fn deep_trees_of_long_words() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (len, n) in [(8, 20), (9, 12), (10, 12)] {
        let words: Vec<Vec<u8>> = (0..n)
            .map(|i| {
                let mut w = vec![b'a'; len - 1];
                w.push(b'b' + i as u8);
                w
            })
            .collect();
        let path = dir.path().join(format!("chain{len}.txt"));
        write_list(&path, &words);
        let solved = run(&[
            "solve",
            "--guesses",
            path.to_str().unwrap(),
            "--candidates",
            path.to_str().unwrap(),
            "--output",
            dir.path().join("chain.json").to_str().unwrap(),
            "--no-progress",
        ]);
        assert_eq!(
            total_cost(&solved),
            (n * (n + 1) / 2) as u64,
            "length {len}"
        );
    }
}

/// A certificate claiming that every set costs far more than it does: the
/// root one more than the optimal tree's cost, every other set a million.
/// The all-green response differs between lengths, so a checker that used
/// the five-letter one would let the singleton claims through and accept it.
#[test]
fn forged_certificates_are_rejected_at_other_lengths() {
    let dir = tempfile::tempdir().expect("temp dir");
    for words in [
        vec![
            "cat", "car", "bat", "bar", "rat", "tar", "mat", "hat", "far", "jar",
        ],
        vec!["abcdex", "abcdey", "abcdez", "abcdew", "qbcdex", "rbcdey"],
    ] {
        let mut list: Vec<Vec<u8>> = words.iter().map(|w| w.as_bytes().to_vec()).collect();
        list.sort();
        let path = dir.path().join("words.txt");
        let tree = dir.path().join("tree.json");
        write_list(&path, &list);
        let solved = run(&[
            "solve",
            "--guesses",
            path.to_str().unwrap(),
            "--candidates",
            path.to_str().unwrap(),
            "--output",
            tree.to_str().unwrap(),
            "--no-progress",
        ]);
        let cost = total_cost(&solved);
        assert_eq!(cost, optimum(&list, &list), "{words:?}");

        let win = all_green(list[0].len());
        let mut claims = vec![Claim {
            def: SetDef::Root,
            bound: cost as u32 + 1,
            rule: RULE_EXPANSION,
        }];
        fn forge(parent: u32, set: &[usize], list: &[Vec<u8>], win: u32, claims: &mut Vec<Claim>) {
            for (g, guess) in list.iter().enumerate() {
                let mut buckets: Vec<(u32, Vec<usize>)> = Vec::new();
                for &c in set {
                    let r = feedback(guess, &list[c]);
                    match buckets.iter_mut().find(|(b, _)| *b == r) {
                        Some((_, members)) => members.push(c),
                        None => buckets.push((r, vec![c])),
                    }
                }
                if buckets.len() < 2 {
                    continue;
                }
                for (r, members) in buckets {
                    if r == win {
                        continue;
                    }
                    claims.push(Claim {
                        def: SetDef::Child {
                            parent,
                            guess: g as u32,
                            response: r as u16,
                        },
                        bound: 1_000_000,
                        rule: RULE_EXPANSION,
                    });
                    let me = claims.len() as u32 - 1;
                    forge(me, &members, list, win, claims);
                }
            }
        }
        let all: Vec<usize> = (0..list.len()).collect();
        forge(0, &all, &list, win, &mut claims);
        let header = Header {
            digest: certfile::digest(&list, &list),
            guesses: list.len() as u32,
            candidates: list.len() as u32,
        };
        let json = fs::read_to_string(&tree).expect("read tree");
        let report = check(&json, &certfile::encode(&header, &claims)).expect("check runs");
        assert!(
            !report.proves_optimal(),
            "{words:?}: forged certificate accepted"
        );
    }
}

/// Input the solver cannot represent is refused with an error, not solved.
#[test]
fn unsupported_input_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        fs::write(&path, bytes).expect("write list");
        path.to_str().unwrap().to_string()
    };
    let out = dir.path().join("out.json");
    let out = out.to_str().unwrap();
    let refuses = |args: &[&str], message: &str| {
        let (status, text) = wordle_opt(args);
        assert!(!status.status.success(), "{args:?} succeeded:\n{text}");
        assert!(text.contains(message), "{args:?}:\n{text}");
    };

    // Characters of several bytes would count as several letters.
    let cyrillic = write("cyrillic.txt", "а\nб\nр\nс\n".as_bytes());
    refuses(
        &[
            "solve",
            "--guesses",
            &cyrillic,
            "--candidates",
            &cyrillic,
            "--output",
            out,
        ],
        "must be ASCII",
    );
    // Two lists of different lengths, including the bundled default guesses.
    let four = write("four.txt", b"abcd\nabce\nabcf\n");
    let six = write("six.txt", b"abcdef\nabcdeg\n");
    refuses(
        &["solve", "--candidates", &four, "--output", out],
        "mismatched word lengths",
    );
    refuses(
        &[
            "solve",
            "--guesses",
            &four,
            "--candidates",
            &six,
            "--output",
            out,
        ],
        "mismatched word lengths",
    );
    // A byte-order mark is not part of the first word.
    let bom = write("bom.txt", b"\xef\xbb\xbfapple\nberry\n");
    let solved = run(&[
        "solve",
        "--guesses",
        &bom,
        "--candidates",
        &bom,
        "--output",
        out,
        "--no-progress",
    ]);
    assert_eq!(total_cost(&solved), 3);
}
