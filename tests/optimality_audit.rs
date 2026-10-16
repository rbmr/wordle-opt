//! Optimality audits that scale past the brute-force oracle.
//!
//! `property_cli.rs` proves the solver right against brute force, but brute
//! force stops at a few dozen candidates. Everything the solver does only at
//! large sizes (the parallel root scan, the root pre-filter, the transposition
//! table under pressure, the phase-1/phase-2 capacity bounds) was until now
//! only checked by "the full tree still costs 8001". These tests attack
//! optimality from directions that need no oracle for the root problem:
//!
//!   * **Brute force on every small subtree.** Total cost is additive, so every
//!     subtree of an optimal tree is itself optimal for its candidate set (a
//!     cheaper subtree could be swapped in). Each subtree small enough for the
//!     oracle is therefore checked exactly - including those of the full
//!     2,340-candidate tree. Checking the maximal small subtrees covers every
//!     node below them by the same argument.
//!   * **Metamorphic relations.** Renaming letters, permuting positions or
//!     dropping guesses the tree never uses cannot change the optimum; adding
//!     guesses cannot raise it; removing a guess cannot lower it; removing a
//!     candidate `x` lowers it by at least `x`'s depth in the tree.
//!   * **Re-solving subtrees with the solver itself.** Every node's subtree
//!     must cost exactly what a fresh solve of that node's candidate set
//!     returns. This catches state leaking between contexts (caches, bounds).
//!   * **Alternative-guess fuzzing.** At a node, the cost of any other guess is
//!     `|S| + sum of optimal bucket costs`; if that beats the chosen subtree,
//!     the solver missed a better guess. Bucket optima come from the oracle
//!     when small and from fresh solves otherwise.
//!   * **Planted optima.** Instances built so the optimum is known exactly at
//!     any size (see [`planted_expected`]), up to ~238 candidates.
//!
//! None of this reuses the solver's feedback function or tree validator: the
//! file has its own [`feedback`], its own tree walker and its own oracle, so a
//! shared bug cannot make both sides agree.
//!
//! The expensive full-scale variant, which rebuilds the 2,340-candidate tree
//! with the current solver, is `#[ignore]`d:
//!
//! ```text
//! cargo test --release --test optimality_audit -- --ignored --nocapture
//! ```

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use wordle_opt::core::Word;
use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::policy::{
    BuildOptions, ReadableNode, ReadableTreeFile, Strategy, build_policy_tree,
};

type W = [u8; 5];

/// Response code of the all-green response (base 3, green = 1 everywhere).
const WIN: u8 = 121;

/// Largest candidate set handed to the brute-force oracle. Brute force on the
/// full tree takes ~3 s at 12, ~30 s at 20 and ~5 min at 30 (16 cores);
/// `WORDLE_OPT_AUDIT_ORACLE_MAX` raises it for a deeper audit.
fn oracle_max() -> usize {
    std::env::var("WORDLE_OPT_AUDIT_ORACLE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14)
}

/// Known optimum of the bundled dictionary (2,340 candidates, 14,855 guesses).
const FULL_OPTIMUM: u64 = 8001;

// ---------------------------------------------------------------------------
// Independent primitives
// ---------------------------------------------------------------------------

/// Wordle feedback, written independently of `core::Response::compute`
/// (letter counting rather than a used-position scan). Per position: 0 black,
/// 1 green, 2 yellow; encoded base 3 with position 0 least significant, which
/// is the encoding the policy JSON's `b`/`g`/`y` keys use.
fn feedback(guess: &W, secret: &W) -> u8 {
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

fn code_key(code: u8) -> String {
    let mut v = code;
    (0..5)
        .map(|_| {
            let c = b"bgy"[(v % 3) as usize] as char;
            v /= 3;
            c
        })
        .collect()
}

fn word(s: &str) -> W {
    s.as_bytes()
        .try_into()
        .unwrap_or_else(|_| panic!("{s:?} is not a 5-byte word"))
}

fn show(w: &W) -> String {
    String::from_utf8_lossy(w).into_owned()
}

/// SplitMix64, local so every case is reproducible from its own seed.
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
        (self.next_u64() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }

    fn sample<T: Clone>(&mut self, v: &[T], k: usize) -> Vec<T> {
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
struct Instance {
    guesses: Vec<W>,
    candidates: Vec<W>,
}

impl Instance {
    /// Same normalization as `Dictionary::from_words`: sorted, deduplicated,
    /// and every candidate is also a guess.
    fn new(mut guesses: Vec<W>, mut candidates: Vec<W>) -> Self {
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

    fn bundled() -> Self {
        let d = Dictionary::load_default();
        Self::new(
            d.guesses.iter().map(|w| w.0).collect(),
            d.candidates.iter().map(|w| w.0).collect(),
        )
    }

    fn with_candidates(&self, candidates: Vec<W>) -> Self {
        Self::new(self.guesses.clone(), candidates)
    }

    fn map_words(&self, f: impl Fn(&W) -> W) -> Self {
        Self::new(
            self.guesses.iter().map(&f).collect(),
            self.candidates.iter().map(&f).collect(),
        )
    }
}

struct Solved {
    tree: ReadableTreeFile,
    audit: TreeAudit,
}

/// Builds the optimal tree with the real solver, then re-derives its cost
/// with this file's own walker and requires it to match the claimed cost.
fn solve(inst: &Instance) -> Solved {
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

fn solve_cost(inst: &Instance) -> u64 {
    solve(inst).audit.cost
}

// ---------------------------------------------------------------------------
// Independent tree walker
// ---------------------------------------------------------------------------

struct AuditNode {
    set: Vec<W>,
    guess: W,
    /// Total cost of this node's subtree: the guesses spent on `set` from here.
    cost: u64,
    depth: u32,
    parent_len: usize,
    path: String,
}

struct TreeAudit {
    cost: u64,
    nodes: Vec<AuditNode>,
    /// Number of guesses the tree spends on each candidate.
    win_depth: HashMap<W, u32>,
}

/// Walks a readable tree from its embedded word lists, checking that every
/// node's guess is allowed, that its children are exactly its possible
/// non-win responses, and that every candidate is won exactly once.
fn audit_tree(file: &ReadableTreeFile) -> TreeAudit {
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

fn walk(
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
struct Oracle {
    rows: Vec<Vec<u8>>,
    memo: HashMap<u64, u32>,
    full: u64,
}

impl Oracle {
    fn new(guesses: &[W], set: &[W]) -> Self {
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

    fn optimum(guesses: &[W], set: &[W]) -> u32 {
        let mut o = Self::new(guesses, set);
        o.value(o.full)
    }

    fn value(&mut self, mask: u64) -> u32 {
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
fn check_small_subtrees(guesses: &[W], audit: &TreeAudit, label: &str) -> usize {
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
fn bucket_optimum(inst: &Instance, bucket: &[W]) -> u64 {
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
fn check_resolved_subtrees(
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
fn partition(g: &W, set: &[W]) -> Vec<Vec<W>> {
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
fn alternatives(
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
fn guess_cost(inst: &Instance, set: &[W], g: &W) -> u64 {
    set.len() as u64
        + partition(g, set)
            .iter()
            .map(|b| bucket_optimum(inst, b))
            .sum::<u64>()
}

/// Fuzzes `nodes`: no alternative guess may beat the subtree the solver chose.
/// Returns the number of alternatives evaluated.
fn fuzz_alternatives(
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

fn relabel_letters(inst: &Instance, alphabet: &[u8; 26], rng: &mut Rng) -> Instance {
    let mut image = *alphabet;
    rng.shuffle(&mut image);
    inst.map_words(|w| w.map(|b| image[(b - b'a') as usize]))
}

fn permute_positions(inst: &Instance, rng: &mut Rng) -> Instance {
    let mut p = [0usize, 1, 2, 3, 4];
    while p == [0, 1, 2, 3, 4] {
        rng.shuffle(&mut p);
    }
    inst.map_words(|w| [w[p[0]], w[p[1]], w[p[2]], w[p[3]], w[p[4]]])
}

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

// ---------------------------------------------------------------------------
// Planted optima
// ---------------------------------------------------------------------------

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

fn random_word(alphabet: &[u8], rng: &mut Rng) -> W {
    std::array::from_fn(|_| alphabet[rng.below(alphabet.len())])
}

// ---------------------------------------------------------------------------
// Base instances
// ---------------------------------------------------------------------------

/// A seeded sub-instance of the bundled dictionary. `guesses == 0` keeps the
/// whole 14,855-word guess list.
fn bundled_subset(seed: u64, candidates: usize, guesses: usize) -> Instance {
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
fn small_alphabet(seed: u64, k: usize, candidates: usize, guesses: usize) -> Instance {
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
// Tests
// ---------------------------------------------------------------------------

fn load_committed_full_tree() -> ReadableTreeFile {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/site/examples/optimal.json");
    let json = std::fs::read_to_string(path).expect("read site/examples/optimal.json");
    ReadableTreeFile::from_json(&json).expect("parse optimal.json")
}

/// The full 2,340-candidate optimal tree committed for the viewer (byte
/// identical to the latest full run): every subtree small enough for brute
/// force must be exactly optimal.
#[test]
fn full_tree_small_subtrees_are_brute_force_optimal() {
    let file = load_committed_full_tree();
    let audit = audit_tree(&file);
    assert_eq!(audit.cost, FULL_OPTIMUM);
    let guesses: Vec<W> = file.guesses.iter().map(|s| word(s)).collect();
    let t = Instant::now();
    let checked = check_small_subtrees(&guesses, &audit, "full tree");
    let covered: usize = audit
        .nodes
        .iter()
        .filter(|n| n.set.len() <= oracle_max())
        .count();
    eprintln!(
        "full tree: {checked} maximal subtrees (<= {} candidates) brute-forced, \
         covering {covered}/{} nodes, in {:.1?}",
        oracle_max(),
        audit.nodes.len(),
        t.elapsed()
    );
    assert!(checked > 100);
}

/// Bundled and synthetic instances solved by the current solver: brute force
/// on small subtrees, fresh re-solves of larger ones, and alternative-guess
/// fuzzing at every node.
#[test]
fn subtrees_resolve_and_no_alternative_guess_is_better() {
    let cases = [
        ("bundled n=80 full guesses", bundled_subset(11, 80, 0)),
        ("bundled n=150 g=1500", bundled_subset(12, 150, 1500)),
        ("bundled n=300 g=600", bundled_subset(13, 300, 600)),
        ("alphabet6 n=120 g=300", small_alphabet(14, 6, 120, 300)),
    ];
    cases.par_iter().enumerate().for_each(|(i, (label, inst))| {
        let t = Instant::now();
        let base = solve(inst);
        let small = check_small_subtrees(&inst.guesses, &base.audit, label);
        let resolved = check_resolved_subtrees(inst, &base.audit, usize::MAX, label);
        let internal: Vec<&AuditNode> =
            base.audit.nodes.iter().filter(|n| n.set.len() >= 3).collect();
        let alts = fuzz_alternatives(inst, &internal, 4, 2, 100 + i as u64, label);
        eprintln!(
            "{label}: cost {}, {small} brute-forced, {resolved} re-solved, {alts} alternatives, {:.1?}",
            base.audit.cost,
            t.elapsed()
        );
    });
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

/// The full-scale audit against the *current* solver: rebuilds the 2,340
/// candidate tree, brute-forces its small subtrees, re-solves every larger
/// subtree, fuzzes alternative guesses at the root and depth 1, and checks
/// that renaming letters and permuting positions reproduce the optimum.
#[test]
#[ignore = "full-scale: several full solves; run with --ignored"]
fn full_scale_current_solver_audit() {
    let inst = Instance::bundled();
    let t = Instant::now();
    let base = solve(&inst);
    eprintln!(
        "full solve: cost {} in {:.1?}",
        base.audit.cost,
        t.elapsed()
    );
    assert_eq!(base.audit.cost, FULL_OPTIMUM);
    // A different tree of equal cost is not wrong (optima are not unique), so
    // this is reported rather than asserted.
    let same = base.tree.to_json() == load_committed_full_tree().to_json();
    eprintln!("identical to site/examples/optimal.json: {same}");

    let t = Instant::now();
    let small = check_small_subtrees(&inst.guesses, &base.audit, "full");
    eprintln!("{small} small subtrees brute-forced in {:.1?}", t.elapsed());

    let t = Instant::now();
    let resolved = check_resolved_subtrees(&inst, &base.audit, usize::MAX, "full");
    eprintln!(
        "{resolved} larger subtrees re-solved in {:.1?}",
        t.elapsed()
    );

    let t = Instant::now();
    let depth1: Vec<&AuditNode> = base
        .audit
        .nodes
        .iter()
        .filter(|n| n.depth == 1 && n.set.len() >= 3)
        .collect();
    let alts = fuzz_alternatives(&inst, &depth1, 3, 1, 900, "full depth 1");
    eprintln!(
        "{alts} depth-1 alternatives evaluated in {:.1?}",
        t.elapsed()
    );

    let t = Instant::now();
    let root: Vec<&AuditNode> = base.audit.nodes.iter().filter(|n| n.depth == 0).collect();
    let alts = fuzz_alternatives(&inst, &root, 2, 1, 901, "full root");
    eprintln!("{alts} root alternatives evaluated in {:.1?}", t.elapsed());

    let mut rng = Rng::new(902);
    let t = Instant::now();
    let lower: [u8; 26] = *b"abcdefghijklmnopqrstuvwxyz";
    assert_eq!(
        solve_cost(&relabel_letters(&inst, &lower, &mut rng)),
        FULL_OPTIMUM
    );
    assert_eq!(
        solve_cost(&permute_positions(&inst, &mut rng)),
        FULL_OPTIMUM
    );
    eprintln!("relabel + permutation full solves in {:.1?}", t.elapsed());
}
