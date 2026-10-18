//! Independent checker for optimality certificates.
//!
//! A certificate is a list of claims `LB(S) >= b` about candidate sets `S`
//! (format: `wordle_common::certfile`). This crate checks one against a
//! readable policy tree. It shares with the solver only what both must agree
//! on - the word lists, the response table and the tree format, all in
//! `wordle_common` - and depends on nothing else from it: no search, no
//! transposition table, no pruning shortcuts. If every claim checks and the
//! root claim bounds the whole candidate list by the tree's cost, the tree is
//! optimal.
//!
//! Each claim names its set explicitly, as the root (every candidate) or as
//! one bucket of an earlier claim's set (`parent`, `guess`, `response`), and
//! the checker recomputes the members. Claims are looked up by their exact
//! member list, never by a hash alone. Each claim cites one rule:
//!
//! * **base**: `|S| = 1` costs 1 and `|S| = 2` costs 3.
//! * **capacity**: `b <= L3(|S|, k)`, where the checker computes `k`, the most
//!   distinct responses any allowed guess gives on `S`. See
//!   `wordle_common::certfile::l3`.
//! * **expansion**: for *every* allowed guess `g` (the checker enumerates the
//!   whole list itself) that splits `S`, `|S| + sum over non-win buckets B of
//!   LB(B) >= b`, where `LB(B)` is the largest claim on exactly `B`, or the
//!   default `base` / `L3(|B|, k(S))` bound if that is larger. A guess that
//!   does not split `S` returns to the same set one guess later, so it never
//!   helps and is skipped.
//! * **assumed**: not checked. A shallow certificate stops at a depth and
//!   leaves its deepest claims to other evidence; the report says how many.
//!
//! Soundness is induction on `|S|`: the buckets an expansion uses are strict
//! subsets, so every claim it relies on is about a smaller set.

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use wordle_common::certfile::{
    self, RULE_ASSUMED, RULE_BASE, RULE_CAPACITY, RULE_EXPANSION, SetDef, l3,
};
use wordle_common::core::Response;
use wordle_common::dict::Dictionary;
use wordle_common::matrix::ResponseMatrix;
use wordle_common::profile::SetProfile;
use wordle_common::tree::ReadableTreeFile;

/// The all-green response.
const WIN: u8 = Response::WIN.0;

#[derive(Debug, Default)]
pub struct Report {
    pub tree_cost: u64,
    pub claims: usize,
    pub by_rule: [usize; 4],
    pub root_bound: u64,
    pub assumed_max_size: usize,
    /// Response reads the check performed in its main loops: `|G| |S|` per
    /// capacity claim, `2 |G| |S|` per expansion.
    pub lookups: u64,
    /// Every failed check. Empty means the certificate is valid.
    pub failures: Vec<String>,
    /// Wall-clock seconds: reading the tree and building the response
    /// table, decoding the claims, checking capacity claims, checking
    /// expansion claims.
    pub seconds: [f64; 4],
}

impl Report {
    /// Optimal outright: valid, and no claim was left unchecked.
    pub fn proves_optimal(&self) -> bool {
        self.failures.is_empty() && self.by_rule[RULE_ASSUMED as usize] == 0
    }
}

struct Lists {
    dict: Dictionary,
    matrix: ResponseMatrix,
}

impl Lists {
    fn guesses(&self) -> usize {
        self.dict.guesses.len()
    }
    fn candidates(&self) -> usize {
        self.dict.candidates.len()
    }
    /// Responses of guess `g` to every candidate.
    fn row(&self, g: usize) -> &[Response] {
        let n = self.candidates();
        &self.matrix.data[g * n..(g + 1) * n]
    }
    /// Responses of every guess to candidate `c`.
    fn column(&self, c: usize) -> &[Response] {
        let n = self.guesses();
        &self.matrix.data_c_g[c * n..(c + 1) * n]
    }
}

struct ClaimRec {
    set: Vec<u16>,
    bound: u64,
    rule: u8,
}

/// Decodes `cert`, checks that it is for these lists, and recomputes each
/// claim's set from its definition.
fn parse_claims(cert: &[u8], lists: &Lists) -> Result<Vec<ClaimRec>, String> {
    let (header, raw) = certfile::decode(cert)?;
    let digest = certfile::digest(
        lists.dict.guesses.iter().map(|w| &w.0),
        lists.dict.candidates.iter().map(|w| &w.0),
    );
    if header.digest != digest {
        return Err("certificate is for different word lists".into());
    }
    if header.guesses as usize != lists.guesses()
        || header.candidates as usize != lists.candidates()
    {
        return Err("certificate list sizes differ from the tree's".into());
    }
    let mut claims: Vec<ClaimRec> = Vec::with_capacity(raw.len());
    for (i, c) in raw.iter().enumerate() {
        let set = match c.def {
            SetDef::Root => (0..lists.candidates() as u16).collect(),
            SetDef::Child {
                parent,
                guess,
                response,
            } => {
                let (parent, g) = (parent as usize, guess as usize);
                if parent >= i || g >= lists.guesses() || response >= 243 || response == WIN {
                    return Err(format!("claim {i}: bad set definition"));
                }
                let row = lists.row(g);
                let set: Vec<u16> = claims[parent]
                    .set
                    .iter()
                    .copied()
                    .filter(|&c| row[c as usize].0 == response)
                    .collect();
                if set.is_empty() {
                    return Err(format!("claim {i}: empty set"));
                }
                set
            }
        };
        claims.push(ClaimRec {
            set,
            bound: c.bound as u64,
            rule: c.rule,
        });
    }
    Ok(claims)
}

/// Whether no allowed guess gives more than `limit` distinct responses on
/// `set`. Only guesses whose [`SetProfile`] bound exceeds `limit` are read,
/// from the candidate-major table.
fn branching_at_most(set: &[u16], lists: &Lists, limit: usize) -> bool {
    if set.len() <= limit {
        return true;
    }
    let profile = SetProfile::new(set.iter().map(|&c| &lists.dict.candidates[c as usize].0));
    let risky: Vec<usize> = (0..lists.guesses())
        .filter(|&g| profile.max_responses(&lists.dict.guesses[g].0) > limit)
        .collect();
    let mut seen = [[0u64; 4]; 256];
    for chunk in risky.chunks(256) {
        let seen = &mut seen[..chunk.len()];
        seen.fill([0; 4]);
        for &c in set {
            let column = lists.column(c as usize);
            for (s, &g) in seen.iter_mut().zip(chunk) {
                let r = column[g].0;
                s[(r >> 6) as usize] |= 1 << (r & 63);
            }
        }
        if seen
            .iter()
            .any(|s| s.iter().map(|w| w.count_ones() as usize).sum::<usize>() > limit)
        {
            return false;
        }
    }
    true
}

/// The most distinct responses any of `guesses` gives on `set`.
fn max_branching(set: &[u16], lists: &Lists, guesses: &[usize]) -> usize {
    let mut k = 0;
    let mut seen = [[0u64; 4]; 256];
    for chunk in guesses.chunks(256) {
        let seen = &mut seen[..chunk.len()];
        seen.fill([0; 4]);
        for &c in set {
            let column = lists.column(c as usize);
            for (s, &g) in seen.iter_mut().zip(chunk) {
                let r = column[g].0;
                s[(r >> 6) as usize] |= 1 << (r & 63);
            }
        }
        for s in seen.iter() {
            k = k.max(s.iter().map(|w| w.count_ones() as usize).sum());
        }
    }
    k
}

/// Checks one expansion claim; `Err` names the first guess that beats it.
fn check_expansion(
    set: &[u16],
    bound: u64,
    lists: &Lists,
    best: &HashMap<&[u16], u64>,
) -> Result<(), String> {
    let n = set.len();
    let ng = lists.guesses();
    // Which guesses can matter. A bucket of size b >= 1 defaults to
    // L3(b, k) = (2b - 1) + max(0, b - k). A guess with at most m buckets, w of
    // them the win, then has a default sum of at least
    // n + 2(n - w) - m + max(0, n - w - m k) >= that with k replaced by any
    // upper bound on k. If even that reaches `bound`, its buckets are never
    // read for the sum.
    let profile = SetProfile::new(set.iter().map(|&c| &lists.dict.candidates[c as usize].0));
    let caps: Vec<usize> = (0..ng)
        .map(|g| profile.max_responses(&lists.dict.guesses[g].0))
        .collect();
    let k_up = caps.iter().copied().max().unwrap_or(0) as u64;
    let mut in_set = vec![false; ng];
    for &c in set {
        in_set[lists.dict.candidate_to_guess[c as usize]] = true;
    }
    let n64 = n as u64;
    let summed: Vec<usize> = (0..ng)
        .filter(|&g| {
            let (w, m) = (in_set[g] as u64, caps[g] as u64);
            let low = (3 * n64 - 2 * w).saturating_sub(m) + (n64 - w).saturating_sub(m * k_up);
            low < bound
        })
        .collect();
    // One pass over the candidate-major table, in chunks of those guesses,
    // finds their bucket counts and default sums. k is at least the number of
    // buckets m of any guess, so only buckets larger than m can contribute the
    // second term: those are kept until `k` is known.
    const CHUNK: usize = 256;
    let mut counts = vec![0u16; CHUNK * 243];
    let mut used: Vec<Vec<u8>> = (0..CHUNK).map(|_| Vec::new()).collect();
    // Guesses whose base sum n + sum(2b - 1) falls short of the bound: only they
    // can need the max(0, b - k) terms, so only their large buckets are kept.
    let mut pending: Vec<(usize, u64, Vec<u16>)> = Vec::new();
    let mut k = 0;
    for chunk in summed.chunks(CHUNK) {
        for &c in set {
            let column = lists.column(c as usize);
            for (j, &g) in chunk.iter().enumerate() {
                let r = column[g].0;
                let slot = &mut counts[j * 243 + r as usize];
                if *slot == 0 {
                    used[j].push(r);
                }
                *slot += 1;
            }
        }
        for (j, u) in used[..chunk.len()].iter_mut().enumerate() {
            let m = u.len();
            k = k.max(m);
            let splits = m > 1 || u[0] == WIN;
            let mut sum = n as u64;
            let mut big: Vec<u16> = Vec::new();
            for &r in u.iter() {
                let slot = &mut counts[j * 243 + r as usize];
                let b = *slot as usize;
                if r != WIN {
                    sum += 2 * b as u64 - 1;
                    if b > m {
                        big.push(b as u16);
                    }
                }
                *slot = 0;
            }
            u.clear();
            if splits && sum < bound {
                pending.push((chunk[j], sum, big));
            }
        }
    }
    // The exact k also needs every unread guess that could give more
    // responses than the most seen so far.
    let unread: Vec<usize> = (0..ng)
        .filter(|&g| caps[g] > k)
        .filter(|g| summed.binary_search(g).is_err())
        .collect();
    k = k.max(max_branching(set, lists, &unread));
    // A guess that does not split the set is never useful. Only guesses whose
    // default sum falls short of `bound` are looked at again, with the claims
    // on their buckets.
    let short: Vec<usize> = pending
        .into_iter()
        .filter(|(_, sum, big)| {
            let extra: u64 = big
                .iter()
                .map(|&b| (b as u64).saturating_sub(k as u64))
                .sum();
            sum + extra < bound
        })
        .map(|(g, _, _)| g)
        .collect();
    let mut seen_rows: HashSet<Vec<u8>> = HashSet::new();
    for g in short {
        let row = lists.row(g);
        // Defaults are not enough: use the claims on this guess's buckets.
        let key: Vec<u8> = set.iter().map(|&c| row[c as usize].0).collect();
        if !seen_rows.insert(key) {
            continue; // identical partition, already checked
        }
        let mut buckets: HashMap<u8, Vec<u16>> = HashMap::new();
        for &c in set {
            let r = row[c as usize].0;
            if r != WIN {
                buckets.entry(r).or_default().push(c);
            }
        }
        let mut sum = n as u64;
        for b in buckets.values() {
            let d = l3(b.len(), k);
            sum += best.get(b.as_slice()).map_or(d, |&v| v.max(d));
        }
        if sum < bound {
            return Err(format!(
                "guess {} gives only {sum} < {bound}",
                lists.dict.guesses[g]
            ));
        }
    }
    Ok(())
}

/// Checks `cert` against the readable policy tree `tree_json`.
pub fn check(tree_json: &str, cert: &[u8]) -> Result<Report, String> {
    let start = Instant::now();
    let file = ReadableTreeFile::from_json(tree_json).map_err(|e| format!("tree: {e}"))?;
    let dict = file
        .embedded_dictionary()
        .map_err(|e| format!("tree: {e}"))?;
    let n = dict.candidates.len();
    if n == 0 || n > u16::MAX as usize {
        return Err("unsupported candidate count".into());
    }
    let tree = file.to_tree().map_err(|e| format!("tree: {e}"))?;
    let matrix = ResponseMatrix::new(&dict);
    let tree_cost = tree
        .validate(&matrix, &dict)
        .map_err(|e| format!("tree is not a valid strategy: {e}"))?
        .total_cost;
    let lists = Lists { dict, matrix };
    let setup = start.elapsed().as_secs_f64();
    let start = Instant::now();

    let claims = parse_claims(cert, &lists)?;
    let mut report = Report {
        tree_cost,
        claims: claims.len(),
        ..Default::default()
    };
    report.seconds[0] = setup;
    report.seconds[1] = start.elapsed().as_secs_f64();
    let mut best: HashMap<&[u16], u64> = HashMap::new();
    for c in &claims {
        report.by_rule[c.rule as usize] += 1;
        let g = lists.guesses() as u64;
        report.lookups += match c.rule {
            RULE_CAPACITY => g * c.set.len() as u64,
            RULE_EXPANSION => 2 * g * c.set.len() as u64,
            _ => 0,
        };
        let e = best.entry(c.set.as_slice()).or_insert(0);
        *e = (*e).max(c.bound);
        if c.rule == RULE_ASSUMED {
            report.assumed_max_size = report.assumed_max_size.max(c.set.len());
        }
    }

    let check_one = |i: usize, c: &ClaimRec| -> Option<String> {
        let size = c.set.len();
        let res = match c.rule {
            RULE_BASE => match size {
                1 | 2 if c.bound <= l3(size, 0) => Ok(()),
                _ => Err("base rule needs |S| <= 2 and b <= 1 or 3".to_string()),
            },
            RULE_CAPACITY => {
                // b <= L3(n, k) = max(2n - 1, 3n - k - 1): either b <= 2n - 1,
                // or no guess may give more than 3n - 1 - b responses.
                let n = size as u64;
                let limit = (3 * n).saturating_sub(1 + c.bound) as usize;
                if c.bound < 2 * n || branching_at_most(&c.set, &lists, limit) {
                    Ok(())
                } else {
                    Err(format!("some guess gives more than {limit} responses"))
                }
            }
            RULE_EXPANSION => check_expansion(&c.set, c.bound, &lists, &best),
            _ => Ok(()),
        };
        res.err()
            .map(|e| format!("claim {i} (|S| = {size}, b = {}): {e}", c.bound))
    };
    // Expansions after everything else, so each phase can be timed.
    let mut failures = Vec::new();
    for (phase, expansion) in [(2, false), (3, true)] {
        let start = Instant::now();
        failures.extend(
            claims
                .par_iter()
                .enumerate()
                .filter(|(_, c)| (c.rule == RULE_EXPANSION) == expansion)
                .filter_map(|(i, c)| check_one(i, c))
                .collect::<Vec<_>>(),
        );
        report.seconds[phase] = start.elapsed().as_secs_f64();
    }
    failures.sort_by_key(|f| {
        f.strip_prefix("claim ")
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse::<usize>().ok())
    });
    report.failures = failures;

    match claims.first() {
        Some(c) if c.set.len() == n => {
            report.root_bound = c.bound;
            if c.bound < tree_cost {
                report.failures.push(format!(
                    "root claim {} is below the tree's cost {tree_cost}",
                    c.bound
                ));
            }
        }
        _ => report
            .failures
            .push("the first claim must be about every candidate".into()),
    }
    Ok(report)
}

/// `wordle-opt check-cert <tree.json> <cert.bin>`
pub fn run_cli(args: &[String]) {
    let (Some(tree_path), Some(cert_path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: wordle-opt check-cert <tree.json> <certificate>");
        std::process::exit(2);
    };
    let start = std::time::Instant::now();
    let tree = std::fs::read_to_string(tree_path).unwrap_or_else(|e| {
        eprintln!("check-cert: cannot read {tree_path}: {e}");
        std::process::exit(1);
    });
    let cert = std::fs::read(cert_path).unwrap_or_else(|e| {
        eprintln!("check-cert: cannot read {cert_path}: {e}");
        std::process::exit(1);
    });
    match check(&tree, &cert) {
        Ok(r) => {
            println!("Tree cost: {}", r.tree_cost);
            println!("Root claim: LB >= {}", r.root_bound);
            println!(
                "Claims: {} (base {}, capacity {}, expansion {}, assumed {})",
                r.claims, r.by_rule[0], r.by_rule[1], r.by_rule[2], r.by_rule[3]
            );
            println!("Certificate bytes: {}", cert.len());
            println!(
                "Check volume: {} response reads ({:.2e})",
                r.lookups, r.lookups as f64
            );
            println!(
                "Check time: {:.3}s (tree and table {:.3}s, claims {:.3}s, capacity {:.3}s, expansion {:.3}s)",
                start.elapsed().as_secs_f64(),
                r.seconds[0],
                r.seconds[1],
                r.seconds[2],
                r.seconds[3]
            );
            for f in r.failures.iter().take(20) {
                println!("FAIL: {f}");
            }
            if !r.failures.is_empty() {
                println!("CERTIFICATE INVALID ({} failures)", r.failures.len());
                std::process::exit(1);
            }
            if r.proves_optimal() {
                println!("CERTIFICATE VALID: the tree is optimal");
            } else {
                println!(
                    "CERTIFICATE VALID GIVEN {} ASSUMED CLAIMS (largest |S| = {})",
                    r.by_rule[3], r.assumed_max_size
                );
                std::process::exit(3);
            }
        }
        Err(e) => {
            eprintln!("check-cert: {e}");
            std::process::exit(1);
        }
    }
}
