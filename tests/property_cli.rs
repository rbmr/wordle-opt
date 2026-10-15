//! CLI-level property test: seeded random word lists in, independently verified
//! optimal tree out.
//!
//! For every case in the corpus below the test:
//!
//!   1. generates a guess list and a candidate list from a fixed seed, with its
//!      own PRNG so cases cannot interfere through shared RNG state;
//!   2. runs the real `wordle-opt` binary via `solve --strategy optimal`, which
//!      exercises the whole pipeline: file loading, the search, tree
//!      serialization and the solve summary;
//!   3. runs `validate` on the written tree. That re-derives the cost by
//!      *traversing the JSON* instead of trusting any in-process figure, and
//!      checks the defining invariant (an edge exists iff that response is
//!      possible for some remaining candidate);
//!   4. compares that traversed cost with an independent brute-force optimum
//!      from `NaiveSolver`, which has no bounds, no equivalence classes and no
//!      capacity pruning, so there is nothing in it to get subtly wrong.
//!
//! Three costs must therefore agree: what `solve` claimed, what `validate`
//! recomputed from the artifact on disk, and what brute force says is optimal.
//!
//! # Why the oracle is cached
//!
//! Brute force is exponential, so recomputing it on every `cargo test` would
//! dominate the suite. Results are cached in `tests/property_oracle.json`, keyed
//! by a hash of the generated input (so a change to the generator changes the
//! key and cannot silently reuse a stale answer). A missing key **fails** the
//! test rather than passing quietly; regenerate deliberately with:
//!
//! ```text
//! WORDLE_OPT_ORACLE_UPDATE=1 cargo test --release --test property_cli
//! ```
//!
//! # Size coverage
//!
//! The solver switches on candidate-set size in several places, so the corpus
//! crosses every such threshold it can reach:
//!
//! | threshold | where | corpus case |
//! |---|---|---|
//! | `c_len == 0/1/2` | `solver.rs`, `naive.rs` base cases | 1, 2 |
//! | `c_len <= 2` | early return, `solver.rs` | 1, 2, 3 |
//! | `c_len <= 8` | `solver.rs` | 8 and 9 |
//! | `c_len <= 10` | `policy.rs` | 10 and 11 |
//! | `10 <= c_len < 40` | equivalence caching, `solver.rs` | 20 |
//! | `c_len <= 15` | quick bucket evaluation, `solver.rs` | 15 and 16 |
//! | `c_len >= 40` | keep-alive, `solver.rs` | 40 |
//! | `c_len >= 48` | parallel vs sequential, `policy.rs` | 48 |
//!
//! The remaining size branch is `c_len <= 256` (stack vs heap pointer array in
//! `solver.rs`). It is crossed by `large_candidate_set_stays_valid` below, which
//! requires the tree to validate and the two independently computed costs to
//! agree. That is weaker than the oracle-backed cases: it proves the branch runs
//! and produces a valid tree, not that the tree is optimal.
//!
//! That weakness is a measured limitation, not an oversight. Reaching 257
//! candidates with the oracle needs a small alphabet to keep the response space
//! tractable, and `NaiveSolver` then takes **626 s** (200 candidates: 58 s;
//! 100: 0.8 s; 48: 30 ms). Paying ten minutes to regenerate the cache is not
//! worth it, so this threshold is covered structurally instead.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::naive::NaiveSolver;

const CACHE: &str = "tests/property_oracle.json";

// ---------------------------------------------------------------------------
// Deterministic generation
// ---------------------------------------------------------------------------

/// SplitMix64. Deliberately local rather than `fastrand`'s global state: cases
/// must be reproducible from their own seed regardless of what else runs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA076_1D64_78BD_642F)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0) has no valid result");
        (self.next_u64() % n as u64) as usize
    }

    /// `k` distinct indices in `0..n`, in draw order.
    fn pick(&mut self, n: usize, k: usize) -> Vec<usize> {
        assert!(k <= n, "cannot draw {k} of {n}");
        let mut idx: Vec<usize> = (0..n).collect();
        let mut out = Vec::with_capacity(k);
        for i in 0..k {
            let j = i + self.below(n - i);
            idx.swap(i, j);
            out.push(idx[i]);
        }
        out
    }
}

#[derive(Clone, Copy)]
enum Words {
    /// Real words from the bundled guess list, so letter overlap, bucket shapes
    /// and equivalence classes are realistic.
    Bundled,
    /// Synthetic words over the first `k` letters. A small alphabet collapses the
    /// response space, which piles many candidates into few buckets and stresses
    /// the equivalence-class logic hard.
    Alphabet(usize),
}

struct Spec {
    seed: u64,
    candidates: usize,
    guesses: usize,
    words: Words,
    why: &'static str,
}

fn corpus() -> Vec<Spec> {
    vec![
        Spec {
            seed: 1,
            candidates: 1,
            guesses: 1,
            words: Words::Bundled,
            why: "base case c_len == 1",
        },
        Spec {
            seed: 2,
            candidates: 2,
            guesses: 2,
            words: Words::Bundled,
            why: "base case c_len == 2",
        },
        Spec {
            seed: 3,
            candidates: 3,
            guesses: 8,
            words: Words::Bundled,
            why: "crosses c_len <= 2; extra guesses that are not candidates",
        },
        Spec {
            seed: 4,
            candidates: 5,
            guesses: 12,
            words: Words::Bundled,
            why: "small set, guesses well beyond candidates",
        },
        Spec {
            seed: 5,
            candidates: 8,
            guesses: 20,
            words: Words::Bundled,
            why: "c_len <= 8 fast path",
        },
        Spec {
            seed: 6,
            candidates: 9,
            guesses: 20,
            words: Words::Bundled,
            why: "crosses c_len <= 8",
        },
        Spec {
            seed: 7,
            candidates: 10,
            guesses: 30,
            words: Words::Bundled,
            why: "c_len <= 10 boundary",
        },
        Spec {
            seed: 8,
            candidates: 11,
            guesses: 30,
            words: Words::Bundled,
            why: "crosses c_len <= 10",
        },
        Spec {
            seed: 9,
            candidates: 12,
            guesses: 12,
            words: Words::Bundled,
            why: "closed set: guesses == candidates",
        },
        Spec {
            seed: 10,
            candidates: 15,
            guesses: 15,
            words: Words::Bundled,
            why: "c_len <= 15 quick bucket evaluation",
        },
        Spec {
            seed: 11,
            candidates: 16,
            guesses: 16,
            words: Words::Bundled,
            why: "crosses c_len <= 15",
        },
        Spec {
            seed: 12,
            candidates: 20,
            guesses: 20,
            words: Words::Bundled,
            why: "inside 10 <= c_len < 40 equivalence caching",
        },
        Spec {
            seed: 13,
            candidates: 8,
            guesses: 8,
            words: Words::Alphabet(3),
            why: "three-letter alphabet: dense equivalence classes",
        },
        Spec {
            seed: 14,
            candidates: 10,
            guesses: 60,
            words: Words::Bundled,
            why: "guess list six times the candidate list",
        },
        Spec {
            seed: 15,
            candidates: 30,
            guesses: 30,
            words: Words::Bundled,
            why: "mid-range, closed set",
        },
        Spec {
            seed: 16,
            candidates: 40,
            guesses: 40,
            words: Words::Bundled,
            why: "crosses c_len >= 40 keep-alive",
        },
        Spec {
            seed: 17,
            candidates: 48,
            guesses: 48,
            words: Words::Bundled,
            why: "crosses c_len >= 48 parallel path",
        },
    ]
}

fn bundled_pool() -> Vec<String> {
    fs::read_to_string("words/guesses.txt")
        .expect("words/guesses.txt must exist; run tests from the crate root")
        .lines()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.len() == 5)
        .collect()
}

fn alphabet_pool(letters: usize) -> Vec<String> {
    assert!((1..=26).contains(&letters));
    let alpha: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().take(letters).collect();
    let mut out = Vec::with_capacity(letters.pow(5));
    let mut idx = [0usize; 5];
    loop {
        out.push(idx.iter().map(|&i| alpha[i]).collect::<String>());
        let mut p = 4;
        loop {
            idx[p] += 1;
            if idx[p] < letters {
                break;
            }
            idx[p] = 0;
            if p == 0 {
                return out;
            }
            p -= 1;
        }
    }
}

struct Case {
    seed: u64,
    why: &'static str,
    candidates: Vec<String>,
    guesses: Vec<String>,
}

fn build_case(spec: &Spec) -> Case {
    let pool = match spec.words {
        Words::Bundled => bundled_pool(),
        Words::Alphabet(k) => alphabet_pool(k),
    };
    let mut rng = Rng::new(spec.seed);

    // Draw the guess list first, then take the candidates out of it: every
    // candidate must also appear in the guess list, which the loader relies on.
    let guesses: Vec<String> = rng
        .pick(pool.len(), spec.guesses)
        .into_iter()
        .map(|i| pool[i].clone())
        .collect();
    let candidates: Vec<String> = rng
        .pick(guesses.len(), spec.candidates)
        .into_iter()
        .map(|i| guesses[i].clone())
        .collect();

    let mut guesses = guesses;
    let mut candidates = candidates;
    guesses.sort();
    candidates.sort();

    Case {
        seed: spec.seed,
        why: spec.why,
        candidates,
        guesses,
    }
}

/// The exact text handed to the solver. Hashing this (rather than the seed)
/// means a change to the generator invalidates the cache instead of silently
/// comparing against an answer for different words.
fn input_text(case: &Case) -> String {
    format!(
        "guesses\n{}\ncandidates\n{}\n",
        case.guesses.join("\n"),
        case.candidates.join("\n")
    )
}

/// FNV-1a. Stable forever, unlike `DefaultHasher`, which is free to change
/// between Rust releases and would churn the committed cache.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

// ---------------------------------------------------------------------------
// The brute-force oracle
// ---------------------------------------------------------------------------

fn oracle_cost(case: &Case) -> u32 {
    let dict = Dictionary::from_strings(
        case.guesses.iter().map(String::as_str),
        case.candidates.iter().map(String::as_str),
    );
    let matrix = ResponseMatrix::new(&dict);
    let set: Vec<usize> = (0..dict.candidates.len()).collect();
    NaiveSolver::new(&matrix, &dict).solve(&set)
}

// ---------------------------------------------------------------------------
// The CLI under test
// ---------------------------------------------------------------------------

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wordle-opt"))
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn the wordle-opt binary");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// Pulls `Total cost: N` out of a CLI summary. Both `solve` and `validate` print
/// this line; `validate` computes it by walking the written tree.
fn parse_total_cost(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix("Total cost: "))
        .and_then(|v| v.trim().parse().ok())
}

fn write_list(dir: &std::path::Path, name: &str, words: &[String]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, format!("{}\n", words.join("\n"))).expect("failed to write a word list");
    p
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn load_cache() -> serde_json::Value {
    match fs::read_to_string(CACHE) {
        Ok(s) => {
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("{CACHE} is not valid JSON: {e}"))
        }
        Err(_) => serde_json::json!({ "cases": {} }),
    }
}

fn save_cache(cache: &serde_json::Value) {
    let text = serde_json::to_string_pretty(cache).expect("cache must serialize");
    fs::write(CACHE, format!("{text}\n")).expect("failed to write the oracle cache");
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

#[test]
fn optimal_tree_matches_brute_force_on_seeded_random_lists() {
    let update = std::env::var("WORDLE_OPT_ORACLE_UPDATE").is_ok_and(|v| v == "1");
    let mut cache = load_cache();
    let dir = tempfile::tempdir().expect("failed to create a temp dir");

    let specs = corpus();
    let mut missing: Vec<String> = Vec::new();
    let mut computed = 0usize;

    for spec in &specs {
        let case = build_case(spec);
        let key = format!("{:016x}", fnv1a(input_text(&case).as_bytes()));

        let cached = cache
            .get("cases")
            .and_then(|c| c.get(&key))
            .and_then(|e| e.get("naive_cost"))
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as u32);

        let expected = match cached {
            Some(v) => v,
            None if update => {
                let v = oracle_cost(&case);
                cache["cases"][&key] = serde_json::json!({
                    "why": case.why,
                    "seed": case.seed,
                    "candidates": case.candidates,
                    "guesses": case.guesses,
                    "naive_cost": v,
                });
                computed += 1;
                v
            }
            None => {
                missing.push(format!(
                    "seed={} candidates={} guesses={} ({})",
                    case.seed,
                    case.candidates.len(),
                    case.guesses.len(),
                    case.why
                ));
                continue;
            }
        };

        let gpath = write_list(dir.path(), "guesses.txt", &case.guesses);
        let cpath = write_list(dir.path(), "candidates.txt", &case.candidates);
        let tree = dir.path().join("tree.json");

        // 1. The CLI must build a tree and claim a cost.
        let (ok, solve_out) = run(&[
            "solve",
            "--strategy",
            "optimal",
            "--guesses",
            gpath.to_str().unwrap(),
            "--candidates",
            cpath.to_str().unwrap(),
            "--output",
            tree.to_str().unwrap(),
            "--no-progress",
        ]);
        assert!(
            ok && solve_out.contains("=== POLICY TREE COMPLETE ==="),
            "solve failed for {} (seed {}, {} candidates, {} guesses)\n{solve_out}",
            case.why,
            case.seed,
            case.candidates.len(),
            case.guesses.len()
        );
        let claimed = parse_total_cost(&solve_out)
            .unwrap_or_else(|| panic!("solve printed no cost for seed {}\n{solve_out}", case.seed));

        // 2. The written artifact must be a valid tree, and its cost must be
        //    re-derived by traversal rather than taken on trust.
        let (vok, validate_out) = run(&["validate", tree.to_str().unwrap()]);
        assert!(
            vok && validate_out.contains("=== POLICY TREE VALID ==="),
            "the tree solve wrote failed validation (seed {}, {})\n{validate_out}",
            case.seed,
            case.why
        );
        let traversed = parse_total_cost(&validate_out).unwrap_or_else(|| {
            panic!(
                "validate printed no cost for seed {}\n{validate_out}",
                case.seed
            )
        });

        // 3. The tree must describe the input it was asked about, not the
        //    bundled dictionary.
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&tree).expect("the tree must be readable"))
                .expect("the tree must be valid JSON");
        assert_eq!(
            json["candidates"],
            serde_json::json!(case.candidates),
            "tree embedded a different candidate list (seed {})",
            case.seed
        );
        assert_eq!(
            json["guesses"],
            serde_json::json!(case.guesses),
            "tree embedded a different guess list (seed {})",
            case.seed
        );

        // 4. All three costs must agree.
        assert_eq!(
            claimed, expected,
            "solve claimed a cost that is not optimal (seed {}, {})\n{solve_out}",
            case.seed, case.why
        );
        assert_eq!(
            traversed, expected,
            "the tree's own cost is not optimal (seed {}, {})\n{validate_out}",
            case.seed, case.why
        );

        println!(
            "ok  seed={:2} candidates={:2} guesses={:2} cost={:3}  {}",
            case.seed,
            case.candidates.len(),
            case.guesses.len(),
            expected,
            case.why
        );
    }

    if update && computed > 0 {
        save_cache(&cache);
        println!("oracle cache updated: {computed} new case(s) -> {CACHE}");
    }

    assert!(
        missing.is_empty(),
        "no cached brute-force optimum for {} case(s):\n  {}\n\
         The oracle is not recomputed on every test run, so a missing entry is a \
         failure rather than a silent pass. Regenerate with:\n  \
         WORDLE_OPT_ORACLE_UPDATE=1 cargo test --release --test property_cli",
        missing.len(),
        missing.join("\n  ")
    );
}

/// The `c_len <= 256` stack-vs-heap branch in `solver.rs` needs a candidate set
/// no brute-force oracle can reach, so it gets weaker treatment: cross the
/// threshold, require the tree to validate, and require the two independently
/// computed costs to agree. This proves the branch runs and yields a valid tree.
/// Optimality at this size rests on the oracle-backed cases above and on the
/// pinned golden tests, not on this test.
#[test]
fn large_candidate_set_stays_valid() {
    // Real words, so the shape of the problem stays realistic: the optimized
    // solver handles a few hundred candidates routinely, and nothing here needs
    // an oracle.
    let spec = Spec {
        seed: 9001,
        candidates: 300,
        guesses: 300,
        words: Words::Bundled,
        why: "crosses c_len <= 256",
    };
    let case = build_case(&spec);
    assert_eq!(case.candidates.len(), 300);

    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let gpath = write_list(dir.path(), "guesses.txt", &case.guesses);
    let cpath = write_list(dir.path(), "candidates.txt", &case.candidates);
    let tree = dir.path().join("tree.json");

    let (ok, solve_out) = run(&[
        "solve",
        "--strategy",
        "optimal",
        "--guesses",
        gpath.to_str().unwrap(),
        "--candidates",
        cpath.to_str().unwrap(),
        "--output",
        tree.to_str().unwrap(),
        "--no-progress",
    ]);
    assert!(ok, "solve failed at 300 candidates\n{solve_out}");

    let (vok, validate_out) = run(&["validate", tree.to_str().unwrap()]);
    assert!(
        vok && validate_out.contains("=== POLICY TREE VALID ==="),
        "the 300-candidate tree failed validation\n{validate_out}"
    );

    let traversed = parse_total_cost(&validate_out).expect("validate must print a cost");
    assert_eq!(
        parse_total_cost(&solve_out),
        Some(traversed),
        "solve's claimed cost disagrees with the cost re-derived from the tree"
    );
    println!("300 candidates: cost={traversed} (valid tree; not oracle-backed at this size)");
}
