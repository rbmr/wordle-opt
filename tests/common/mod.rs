//! Shared helpers for the optimality tests.
//!
//! Nothing here reuses the solver's own feedback function or tree validator:
//! [`feedback`], [`audit_tree`] and [`Oracle`] are written independently, so a
//! bug in the solver cannot make both sides of a check agree.

#![allow(dead_code)]

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use wordle_opt::core::Word;
use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::policy::{
    BuildOptions, ReadableNode, ReadableTreeFile, Strategy, build_policy_tree,
};

pub type W = [u8; 5];

/// Response code of the all-green response (base 3, green = 1 everywhere).
pub const WIN: u8 = 121;

/// Largest candidate set handed to the brute-force oracle. Brute force on the
/// full tree takes ~3 s at 12, ~30 s at 20 and ~5 min at 30 (16 cores);
/// `WORDLE_OPT_AUDIT_ORACLE_MAX` raises it for a deeper audit.
pub fn oracle_max() -> usize {
    std::env::var("WORDLE_OPT_AUDIT_ORACLE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14)
}

/// Known optimum of the bundled dictionary (2,340 candidates, 14,855 guesses).
pub const FULL_OPTIMUM: u64 = 8001;

// ---------------------------------------------------------------------------
// Independent primitives
// ---------------------------------------------------------------------------

/// Wordle feedback, written independently of `core::Response::compute`
/// (letter counting rather than a used-position scan). Per position: 0 black,
/// 1 green, 2 yellow; encoded base 3 with position 0 least significant, which
/// is the encoding the policy JSON's `b`/`g`/`y` keys use.
pub fn feedback(guess: &W, secret: &W) -> u8 {
    let mut marks = [0u8; 5];
    let mut unmatched = [0u8; 256];
    for i in 0..5 {
        if guess[i] == secret[i] {
            marks[i] = 1;
        } else {
            unmatched[secret[i] as usize] += 1;
        }
    }
    for i in 0..5 {
        let l = guess[i] as usize;
        if marks[i] == 0 && unmatched[l] > 0 {
            marks[i] = 2;
            unmatched[l] -= 1;
        }
    }
    marks.iter().rev().fold(0, |acc, &m| acc * 3 + m)
}

pub fn code_key(code: u8) -> String {
    let mut v = code;
    (0..5)
        .map(|_| {
            let c = b"bgy"[(v % 3) as usize] as char;
            v /= 3;
            c
        })
        .collect()
}

pub fn word(s: &str) -> W {
    s.as_bytes()
        .try_into()
        .unwrap_or_else(|_| panic!("{s:?} is not a 5-byte word"))
}

pub fn show(w: &W) -> String {
    String::from_utf8_lossy(w).into_owned()
}

/// SplitMix64, local so every case is reproducible from its own seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA076_1D64_78BD_642F)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }

    pub fn sample<T: Clone>(&mut self, v: &[T], k: usize) -> Vec<T> {
        let mut v = v.to_vec();
        self.shuffle(&mut v);
        v.truncate(k);
        v
    }
}

// ---------------------------------------------------------------------------
// Instances and the solver under test
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Instance {
    pub guesses: Vec<W>,
    pub candidates: Vec<W>,
}

impl Instance {
    /// Same normalization as `Dictionary::from_words`: sorted, deduplicated,
    /// and every candidate is also a guess.
    pub fn new(mut guesses: Vec<W>, mut candidates: Vec<W>) -> Self {
        candidates.sort_unstable();
        candidates.dedup();
        guesses.extend_from_slice(&candidates);
        guesses.sort_unstable();
        guesses.dedup();
        Self {
            guesses,
            candidates,
        }
    }

    pub fn bundled() -> Self {
        let d = Dictionary::load_default();
        Self::new(
            d.guesses.iter().map(|w| w.0).collect(),
            d.candidates.iter().map(|w| w.0).collect(),
        )
    }

    pub fn with_candidates(&self, candidates: Vec<W>) -> Self {
        Self::new(self.guesses.clone(), candidates)
    }

    pub fn map_words(&self, f: impl Fn(&W) -> W) -> Self {
        Self::new(
            self.guesses.iter().map(&f).collect(),
            self.candidates.iter().map(&f).collect(),
        )
    }
}

pub struct Solved {
    pub tree: ReadableTreeFile,
    pub audit: TreeAudit,
}

/// Builds the optimal tree with the real solver, then re-derives its cost
/// with this file's own walker and requires it to match the claimed cost.
pub fn solve(inst: &Instance) -> Solved {
    let dict = Dictionary::from_words(
        inst.guesses.iter().map(|&w| Word(w)).collect(),
        inst.candidates.iter().map(|&w| Word(w)).collect(),
    );
    let matrix = ResponseMatrix::new(&dict);
    let root: Vec<usize> = (0..dict.candidates.len()).collect();
    let opts = BuildOptions {
        progress: false,
        ..Default::default()
    };
    let (tree, stats) =
        build_policy_tree(&matrix, &dict, Strategy::Optimal, &root, &opts).expect("tree build");
    let tree = tree.to_readable(&dict).expect("readable tree");
    let audit = audit_tree(&tree);
    assert_eq!(
        audit.cost, stats.summary.total_cost,
        "solver claimed cost {} but its tree costs {}",
        stats.summary.total_cost, audit.cost
    );
    Solved { tree, audit }
}

pub fn solve_cost(inst: &Instance) -> u64 {
    solve(inst).audit.cost
}

// ---------------------------------------------------------------------------
// Independent tree walker
// ---------------------------------------------------------------------------

pub struct AuditNode {
    pub set: Vec<W>,
    pub guess: W,
    /// Total cost of this node's subtree: the guesses spent on `set` from here.
    pub cost: u64,
    pub depth: u32,
    pub parent_len: usize,
    pub path: String,
}

pub struct TreeAudit {
    pub cost: u64,
    pub nodes: Vec<AuditNode>,
    /// Number of guesses the tree spends on each candidate.
    pub win_depth: HashMap<W, u32>,
}

/// Walks a readable tree from its embedded word lists, checking that every
/// node's guess is allowed, that its children are exactly its possible
/// non-win responses, and that every candidate is won exactly once.
pub fn audit_tree(file: &ReadableTreeFile) -> TreeAudit {
    let guesses: HashSet<W> = file.guesses.iter().map(|s| word(s)).collect();
    let candidates: Vec<W> = file.candidates.iter().map(|s| word(s)).collect();
    let mut audit = TreeAudit {
        cost: 0,
        nodes: Vec::new(),
        win_depth: HashMap::new(),
    };
    let n = candidates.len();
    audit.cost = walk(
        &file.root,
        candidates,
        0,
        n,
        String::new(),
        &guesses,
        &mut audit,
    );
    assert_eq!(audit.win_depth.len(), n, "not every candidate is won");
    audit
}

pub fn walk(
    node: &ReadableNode,
    set: Vec<W>,
    depth: u32,
    parent_len: usize,
    path: String,
    guesses: &HashSet<W>,
    audit: &mut TreeAudit,
) -> u64 {
    let guess = word(&node.guess);
    assert!(
        guesses.contains(&guess),
        "{path}: {} is not a guess",
        node.guess
    );
    assert!(!set.is_empty(), "{path}: node with no candidates");

    let mut buckets: HashMap<u8, Vec<W>> = HashMap::new();
    for s in &set {
        buckets.entry(feedback(&guess, s)).or_default().push(*s);
    }
    if let Some(won) = buckets.remove(&WIN) {
        let fresh = audit.win_depth.insert(won[0], depth + 1).is_none();
        assert!(fresh, "{path}: {} won twice", show(&won[0]));
    }
    assert!(
        !(buckets.len() == 1 && buckets.values().next().unwrap().len() == set.len()),
        "{path}: guess {} does not split its set",
        node.guess
    );
    let mut want: Vec<String> = buckets.keys().map(|&c| code_key(c)).collect();
    want.sort_unstable();
    let have: Vec<String> = node.children.keys().cloned().collect();
    assert_eq!(
        have, want,
        "{path}: children are not the possible responses"
    );

    let idx = audit.nodes.len();
    audit.nodes.push(AuditNode {
        set: set.clone(),
        guess,
        cost: 0,
        depth,
        parent_len,
        path: path.clone(),
    });
    let mut cost = set.len() as u64;
    for (code, bucket) in buckets {
        let key = code_key(code);
        let child_path = format!("{path}{}:{key} ", node.guess);
        cost += walk(
            &node.children[&key],
            bucket,
            depth + 1,
            set.len(),
            child_path,
            guesses,
            audit,
        );
    }
    audit.nodes[idx].cost = cost;
    cost
}

// ---------------------------------------------------------------------------
// Brute-force oracle
// ---------------------------------------------------------------------------

/// Exact optimum for one small candidate set by exhaustive search over every
/// guess, memoized on the subset bitmask. The only pruning is arithmetic: a
/// guess's bucket sum is abandoned once it reaches the best total so far
/// (bucket optima are non-negative), and guesses whose response row over the
/// set is identical to an earlier guess's are skipped (identical rows induce
/// identical partitions of every subset).
pub struct Oracle {
    pub rows: Vec<Vec<u8>>,
    pub memo: HashMap<u64, u32>,
    pub full: u64,
}

impl Oracle {
    pub fn new(guesses: &[W], set: &[W]) -> Self {
        assert!(set.len() <= 64);
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for g in guesses.iter().chain(set) {
            let row: Vec<u8> = set.iter().map(|s| feedback(g, s)).collect();
            if seen.insert(row.clone()) {
                rows.push(row);
            }
        }
        let full = if set.len() == 64 {
            u64::MAX
        } else {
            (1u64 << set.len()) - 1
        };
        Self {
            rows,
            memo: HashMap::new(),
            full,
        }
    }

    pub fn optimum(guesses: &[W], set: &[W]) -> u32 {
        let mut o = Self::new(guesses, set);
        o.value(o.full)
    }

    pub fn value(&mut self, mask: u64) -> u32 {
        let n = mask.count_ones();
        match n {
            0 => return 0,
            1 => return 1,
            2 => return 3,
            _ => {}
        }
        if let Some(&v) = self.memo.get(&mask) {
            return v;
        }
        let mut best = u32::MAX;
        let mut tried: HashSet<Vec<u64>> = HashSet::new();
        for r in 0..self.rows.len() {
            let mut buckets: Vec<(u8, u64)> = Vec::new();
            let mut m = mask;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                m &= m - 1;
                let code = self.rows[r][i];
                match buckets.iter_mut().find(|b| b.0 == code) {
                    Some(b) => b.1 |= 1 << i,
                    None => buckets.push((code, 1 << i)),
                }
            }
            // A guess that leaves the set whole learns nothing (with n >= 3
            // it cannot be the win), so it is never part of an optimal tree.
            if buckets.len() == 1 {
                continue;
            }
            let win = buckets.iter().find(|b| b.0 == WIN).map_or(0, |b| b.1);
            let mut key: Vec<u64> = buckets.iter().filter(|b| b.0 != WIN).map(|b| b.1).collect();
            key.sort_unstable();
            key.push(win);
            if !tried.insert(key.clone()) {
                continue;
            }
            let mut cost = n;
            for &child in &key[..key.len() - 1] {
                if cost >= best {
                    break;
                }
                cost += self.value(child);
            }
            best = best.min(cost);
        }
        self.memo.insert(mask, best);
        best
    }
}

/// Checks every maximal subtree with at most `oracle_max()` candidates against
/// brute force. Returns how many subtrees were checked.
pub fn check_small_subtrees(guesses: &[W], audit: &TreeAudit, label: &str) -> usize {
    let small: Vec<&AuditNode> = audit
        .nodes
        .iter()
        .filter(|n| n.set.len() >= 3 && n.set.len() <= oracle_max() && n.parent_len > oracle_max())
        .collect();
    let bad: Vec<String> = small
        .par_iter()
        .filter_map(|n| {
            let opt = Oracle::optimum(guesses, &n.set) as u64;
            (opt != n.cost).then(|| {
                format!(
                    "{label}: subtree at [{}] ({} candidates, guess {}) costs {} but brute force finds {}",
                    n.path,
                    n.set.len(),
                    show(&n.guess),
                    n.cost,
                    opt
                )
            })
        })
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    small.len()
}

/// Optimal cost of one bucket: brute force when small, a fresh solve otherwise.
pub fn bucket_optimum(inst: &Instance, bucket: &[W]) -> u64 {
    match bucket.len() {
        0 => 0,
        1 => 1,
        2 => 3,
        n if n <= oracle_max() => Oracle::optimum(&inst.guesses, bucket) as u64,
        _ => solve_cost(&inst.with_candidates(bucket.to_vec())),
    }
}

// ---------------------------------------------------------------------------
// Re-solve subtrees and alternative-guess fuzzing
// ---------------------------------------------------------------------------

/// Every node with more than `oracle_max()` candidates (the root excluded, and
/// optionally only up to `max_len`) must cost exactly what a fresh solve of
/// its candidate set returns.
pub fn check_resolved_subtrees(
    inst: &Instance,
    audit: &TreeAudit,
    max_len: usize,
    label: &str,
) -> usize {
    let nodes: Vec<&AuditNode> = audit
        .nodes
        .iter()
        .filter(|n| n.depth > 0 && n.set.len() > oracle_max() && n.set.len() <= max_len)
        .collect();
    for n in &nodes {
        let fresh = solve_cost(&inst.with_candidates(n.set.clone()));
        assert_eq!(
            fresh,
            n.cost,
            "{label}: subtree at [{}] ({} candidates) costs {} in the tree but {} when solved alone",
            n.path,
            n.set.len(),
            n.cost,
            fresh
        );
    }
    nodes.len()
}

/// Partition of `set` under `g`: non-win buckets, sorted for comparison.
pub fn partition(g: &W, set: &[W]) -> Vec<Vec<W>> {
    let mut buckets: HashMap<u8, Vec<W>> = HashMap::new();
    for s in set {
        let r = feedback(g, s);
        if r != WIN {
            buckets.entry(r).or_default().push(*s);
        }
    }
    let mut v: Vec<Vec<W>> = buckets.into_values().collect();
    v.sort_unstable();
    v
}

/// Up to `top` alternative guesses ranked by expected remaining candidates
/// (`sum |bucket|^2`, the solver's own move-ordering heuristic, so these are
/// the guesses most likely to be close calls), plus `random` arbitrary useful
/// ones. Guesses partitioning `set` exactly like `chosen` are skipped.
pub fn alternatives(
    inst: &Instance,
    set: &[W],
    chosen: &W,
    top: usize,
    random: usize,
    rng: &mut Rng,
) -> Vec<W> {
    let chosen_key = (partition(chosen, set), set.contains(chosen));
    let mut seen = HashSet::new();
    seen.insert(chosen_key);
    let mut ranked: Vec<(u64, W)> = Vec::new();
    for g in &inst.guesses {
        let parts = partition(g, set);
        let wins = set.contains(g);
        if !wins && parts.len() == 1 {
            continue; // useless
        }
        let score = parts
            .iter()
            .map(|b| (b.len() * b.len()) as u64)
            .sum::<u64>()
            + wins as u64;
        if seen.insert((parts, wins)) {
            ranked.push((score, *g));
        }
    }
    ranked.sort_unstable();
    let mut out: Vec<W> = ranked.iter().take(top).map(|&(_, g)| g).collect();
    let rest: Vec<W> = ranked.iter().skip(top).map(|&(_, g)| g).collect();
    out.extend(rng.sample(&rest, random));
    out
}

/// The exact cost of playing `g` first at `set` and optimally afterwards.
pub fn guess_cost(inst: &Instance, set: &[W], g: &W) -> u64 {
    set.len() as u64
        + partition(g, set)
            .iter()
            .map(|b| bucket_optimum(inst, b))
            .sum::<u64>()
}

/// Fuzzes `nodes`: no alternative guess may beat the subtree the solver chose.
/// Returns the number of alternatives evaluated.
pub fn fuzz_alternatives(
    inst: &Instance,
    nodes: &[&AuditNode],
    top: usize,
    random: usize,
    seed: u64,
    label: &str,
) -> usize {
    let mut rng = Rng::new(seed);
    let mut evaluated = 0;
    for n in nodes {
        for g in alternatives(inst, &n.set, &n.guess, top, random, &mut rng) {
            let c = guess_cost(inst, &n.set, &g);
            evaluated += 1;
            assert!(
                c >= n.cost,
                "{label}: at [{}] ({} candidates) the tree's guess {} costs {} but {} costs {}",
                n.path,
                n.set.len(),
                show(&n.guess),
                n.cost,
                show(&g),
                c
            );
        }
    }
    evaluated
}

// ---------------------------------------------------------------------------
// Metamorphic relations
// ---------------------------------------------------------------------------

pub fn relabel_letters(inst: &Instance, alphabet: &[u8; 26], rng: &mut Rng) -> Instance {
    let mut image = *alphabet;
    rng.shuffle(&mut image);
    inst.map_words(|w| w.map(|b| image[(b - b'a') as usize]))
}

pub fn permute_positions(inst: &Instance, rng: &mut Rng) -> Instance {
    let mut p = [0usize, 1, 2, 3, 4];
    while p == [0, 1, 2, 3, 4] {
        rng.shuffle(&mut p);
    }
    inst.map_words(|w| [w[p[0]], w[p[1]], w[p[2]], w[p[3]], w[p[4]]])
}

pub fn random_word(alphabet: &[u8], rng: &mut Rng) -> W {
    std::array::from_fn(|_| alphabet[rng.below(alphabet.len())])
}

// ---------------------------------------------------------------------------
// Base instances
// ---------------------------------------------------------------------------

/// A seeded sub-instance of the bundled dictionary. `guesses == 0` keeps the
/// whole 14,855-word guess list.
pub fn bundled_subset(seed: u64, candidates: usize, guesses: usize) -> Instance {
    let b = Instance::bundled();
    let mut rng = Rng::new(seed);
    let c = rng.sample(&b.candidates, candidates);
    let g = if guesses == 0 {
        b.guesses.clone()
    } else {
        rng.sample(&b.guesses, guesses)
    };
    Instance::new(g, c)
}

/// Synthetic words over the first `k` letters: few responses, big buckets.
pub fn small_alphabet(seed: u64, k: usize, candidates: usize, guesses: usize) -> Instance {
    let alphabet: Vec<u8> = (b'a'..b'a' + k as u8).collect();
    let mut rng = Rng::new(seed);
    let mut c = HashSet::new();
    while c.len() < candidates {
        c.insert(random_word(&alphabet, &mut rng));
    }
    let g = (0..guesses)
        .map(|_| random_word(&alphabet, &mut rng))
        .collect();
    Instance::new(g, c.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Committed and published word lists
// ---------------------------------------------------------------------------

/// The full optimal tree committed for the viewer.
pub fn load_committed_full_tree() -> ReadableTreeFile {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/site/examples/optimal.json");
    let json = std::fs::read_to_string(path).expect("read site/examples/optimal.json");
    ReadableTreeFile::from_json(&json).expect("parse optimal.json")
}

/// One word per line from a file in the repository.
pub fn read_list(rel: &str) -> Vec<W> {
    let path = format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .map(|l| word(&l))
        .collect()
}
