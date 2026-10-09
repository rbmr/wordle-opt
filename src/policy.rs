#![allow(clippy::needless_range_loop)]
//! Policy trees: self-describing storage of a fully determined Wordle
//! strategy, plus the logic to build and validate one.
//!
//! A *policy* maps a state (the set of remaining candidates) to a guess. For a
//! fixed dictionary and a deterministic policy, the whole game is a static
//! decision tree (see `ideas/Wordle.md`, "Storing Policies"):
//!
//! - **nodes are guesses** (the root is the first guess),
//! - **edges are responses**, and
//! - **leaves are the secret word** (a node is terminal exactly when its guess
//!   is correct).
//!
//! The candidate set of a node is *not* stored - it is implied by the path of
//! responses taken to reach it, and can always be recomputed top-down from the
//! root. That is what makes the representation compact, and it is exactly why
//! validation must recompute it: a tree is only valid if, at every node, an
//! edge for a response exists **if and only if** that response is possible for
//! some candidate still reachable at that node, and each edge leads to the
//! subtree for precisely the candidates producing that response.
//!
//! A tree is serialized as **readable** JSON: nested
//! `{"guess": "trace", "children": {"bgybg": ...}}` with the word lists
//! embedded, so the file is fully self-contained (loadable and validatable with
//! no other files, and directly renderable by the GitHub Pages viewer under
//! `site/`). It also carries a dictionary hash, which ties it to the exact word
//! lists it was built from.

use crate::cache::GlobalCache;
use crate::core::Response;
use crate::dict::Dictionary;
use crate::heuristic;
use crate::matrix::ResponseMatrix;
use crate::solver::{EquivCache, Metrics, Solver};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The total number of ternary responses (`3^5`).
pub const NUM_RESPONSES: usize = 243;
/// The index of the all-green ("win") response.
pub const WIN_RESPONSE: u8 = Response::WIN.0;

/// Format tags written into the JSON headers, and checked on load.
pub const FORMAT_READABLE: &str = "wordle-policy-tree";
/// Bumped whenever the on-disk shape changes in a way older readers can't
/// handle. Readers reject a version they don't know rather than guessing.
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Dictionary hash
// ---------------------------------------------------------------------------

/// FNV-1a (64-bit) over a versioned encoding of the *loaded* (sorted,
/// deduplicated) guess and candidate lists.
///
/// This is the value that must accompany any serialized policy tree, per
/// `ideas/Wordle.md`: it ties a tree to the exact word lists it was computed
/// against, so a tree built for one dictionary can never be silently applied
/// to another (which would make response edges meaningless). It is computed
/// from `Dictionary`'s already-normalized lists, so it is independent of the
/// order words appeared in the input files.
pub fn dictionary_hash(dict: &Dictionary) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut h = FNV_OFFSET;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    };
    feed(b"wordle-opt/policy-dictionary/v1\n");
    feed(format!("guesses={}\n", dict.guesses.len()).as_bytes());
    for w in &dict.guesses {
        feed(&w.0);
        feed(b"\n");
    }
    feed(format!("candidates={}\n", dict.candidates.len()).as_bytes());
    for w in &dict.candidates {
        feed(&w.0);
        feed(b"\n");
    }
    h
}

/// Formats a dictionary hash as the `0x`-prefixed hex string used in headers.
pub fn hash_hex(hash: u64) -> String {
    format!("{hash:#018x}")
}

/// Parses a `dictionary_hash` header field (with or without `0x`).
pub fn parse_hash_hex(s: &str) -> Result<u64, PolicyError> {
    let t = s.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u64::from_str_radix(t, 16)
        .map_err(|_| PolicyError::BadHeader(format!("invalid dictionary_hash {s:?}")))
}

// ---------------------------------------------------------------------------
// Response <-> string
// ---------------------------------------------------------------------------

/// Renders a response index as its 5-character `b`/`g`/`y` string (position 0
/// first). `b`=black, `g`=green, `y`=yellow.
pub fn response_to_string(r: u8) -> String {
    let mut v = r;
    let mut s = String::with_capacity(5);
    for _ in 0..5 {
        s.push(match v % 3 {
            0 => 'b',
            1 => 'g',
            _ => 'y',
        });
        v /= 3;
    }
    s
}

/// Inverse of [`response_to_string`].
pub fn response_from_string(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    if bytes.len() != 5 {
        return None;
    }
    let mut r = 0u8;
    let mut mul = 1u8;
    for &b in bytes {
        let d = match b {
            b'b' | b'B' => 0,
            b'g' | b'G' => 1,
            b'y' | b'Y' => 2,
            _ => return None,
        };
        r += d * mul;
        mul *= 3;
    }
    Some(r)
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// The built-in strategies that can be captured into a policy tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The exact cost-minimizing strategy, computed with the branch-and-bound
    /// solver. Expensive: building the full tree requires an exact solve of
    /// every reachable state.
    Optimal,
    /// Greedy heuristic: pick the guess that minimizes the expected number of
    /// remaining candidates, `sum_r |C_{g,r}|^2`.
    MinRemaining,
    /// Greedy heuristic: pick the guess maximizing the summed letter frequency
    /// of its distinct letters over the current candidate set.
    MaxFreq,
}

impl Strategy {
    pub fn parse(s: &str) -> Option<Strategy> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "optimal" => Some(Strategy::Optimal),
            "min-remaining" | "min-guesses-remaining" | "minremaining" => {
                Some(Strategy::MinRemaining)
            }
            "max-freq" | "max-frequency" | "max-letter-freq" | "maxfreq" => Some(Strategy::MaxFreq),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Strategy::Optimal => "optimal",
            Strategy::MinRemaining => "min-remaining",
            Strategy::MaxFreq => "max-freq",
        }
    }

    pub fn all() -> [Strategy; 3] {
        [Strategy::Optimal, Strategy::MinRemaining, Strategy::MaxFreq]
    }
}

/// A candidates policy: given the set of still-possible candidates, returns
/// the guess to make. This is the abstraction the tree builder consumes, so a
/// new strategy only has to implement this one method to be captured.
///
/// The returned index refers to `Dictionary::guesses`. A policy is only
/// required to be *useful* (strictly shrink the candidate set) when more than
/// one candidate remains; the builder special-cases the singleton state (the
/// only winning move is to guess the remaining candidate).
pub trait CandidatesPolicy {
    fn guess(&self, candidates: &[usize]) -> usize;
}

/// Shared helper: the projection used to deduplicate guesses that partition
/// the candidate set identically (same scheme as `Solver::greedy_solve`).
fn guess_projection(dict: &Dictionary, c_mask: u32, guess: usize) -> u32 {
    let chars = &dict.guess_chars[guess];
    let mut proj = 0u32;
    for (i, &l) in chars.iter().enumerate() {
        let l = l as u32;
        proj |= ((l + 1) * ((c_mask >> l) & 1)) << (5 * i);
    }
    proj
}

/// The active (non-useless), projection-deduplicated guesses for `candidates`,
/// sorted by ascending expected-remaining then index (deterministic).
fn active_guesses(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    candidates: &[usize],
) -> Vec<(usize, u32)> {
    let mut c_mask = 0u32;
    for &c in candidates {
        c_mask |= matrix.candidate_masks[c];
    }
    let mut seen = rustc_hash::FxHashSet::default();
    let mut out = Vec::new();
    let mut counts = [0u16; NUM_RESPONSES];
    for g in 0..dict.guesses.len() {
        if !seen.insert(guess_projection(dict, c_mask, g)) {
            continue;
        }
        for v in counts.iter_mut() {
            *v = 0;
        }
        let mut num_non_empty = 0;
        for &c in candidates {
            let r = matrix.get(g, c).0 as usize;
            if counts[r] == 0 {
                num_non_empty += 1;
            }
            counts[r] += 1;
        }
        if num_non_empty <= 1 {
            continue; // useless: does not partition the candidate set
        }
        let mut expected_rem = 0u32;
        for &v in counts.iter() {
            expected_rem += (v as u32) * (v as u32);
        }
        out.push((g, expected_rem));
    }
    out.sort_unstable_by_key(|&(g, e)| (e, g));
    out
}

/// Whether `guess` strictly partitions `candidates` (produces at least two
/// distinct responses). A guess that doesn't is *useless*: it leaves the
/// candidate set unchanged and can never terminate a policy.
fn guess_is_useful(matrix: &ResponseMatrix, candidates: &[usize], guess: usize) -> bool {
    let mut seen = [false; NUM_RESPONSES];
    let mut distinct = 0;
    for &c in candidates {
        let r = matrix.get(guess, c).0 as usize;
        if !seen[r] {
            seen[r] = true;
            distinct += 1;
            if distinct > 1 {
                return true;
            }
        }
    }
    false
}

/// "Min guesses remaining": minimize `sum_r |C_{g,r}|^2`, the expected number
/// of remaining candidates (up to the constant factor `|C|`). Only guesses that
/// partition the candidate set are considered, so the policy always terminates.
pub struct MinRemainingPolicy<'a> {
    matrix: &'a ResponseMatrix,
}

impl<'a> MinRemainingPolicy<'a> {
    pub fn new(matrix: &'a ResponseMatrix) -> Self {
        Self { matrix }
    }
}

impl CandidatesPolicy for MinRemainingPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        let mut best_guess = usize::MAX;
        let mut best_score = u64::MAX;
        let mut counts = [0u32; NUM_RESPONSES];
        for g in 0..self.matrix.num_guesses {
            for v in counts.iter_mut() {
                *v = 0;
            }
            let mut distinct = 0;
            for &c in candidates {
                let r = self.matrix.get(g, c).0 as usize;
                if counts[r] == 0 {
                    distinct += 1;
                }
                counts[r] += 1;
            }
            if distinct <= 1 {
                continue; // useless
            }
            let mut score = 0u64;
            for &v in counts.iter() {
                score += (v as u64) * (v as u64);
            }
            if score < best_score {
                best_score = score;
                best_guess = g;
            }
        }
        best_guess
    }
}

/// "Max letter frequency": maximize the summed frequency, over the candidate
/// set, of the distinct letters appearing in the guess.
///
/// The best-scoring guess that actually partitions the set is used: a
/// top-scoring guess can be useless (for example, one whose letters all yield
/// the same response), and such a guess cannot be part of a terminating policy.
pub struct MaxFreqPolicy<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
}

impl<'a> MaxFreqPolicy<'a> {
    pub fn new(matrix: &'a ResponseMatrix, dict: &'a Dictionary) -> Self {
        Self { matrix, dict }
    }
}

impl CandidatesPolicy for MaxFreqPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        let mut letter_freq = [0u32; 26];
        for &c in candidates {
            let mut mask = 0u32;
            for &b in &self.dict.candidates[c].0 {
                let l = (b - b'a') as usize;
                mask |= 1 << l;
            }
            for l in 0..26 {
                if mask & (1 << l) != 0 {
                    letter_freq[l] += 1;
                }
            }
        }
        let mut scored: Vec<(u32, usize)> = (0..self.dict.guesses.len())
            .map(|g| {
                let mut seen = 0u32;
                let mut score = 0u32;
                for &l in &self.dict.guess_chars[g] {
                    let l = l as usize;
                    if seen & (1 << l) == 0 {
                        seen |= 1 << l;
                        score += letter_freq[l];
                    }
                }
                (score, g)
            })
            .collect();
        scored.sort_unstable_by_key(|&(s, g)| (std::cmp::Reverse(s), g));
        for (_, g) in scored {
            if guess_is_useful(self.matrix, candidates, g) {
                return g;
            }
        }
        usize::MAX
    }
}

/// The exact, cost-minimizing policy.
///
/// For a state `C` it returns `(guess, T*(C))` where `guess` achieves the
/// minimum total cost and `T*` is that cost. It reuses a single persistent
/// `GlobalCache`/`EquivCache` across every state it is asked about, so building
/// a whole tree shares memoization between nodes.
///
/// Cost model: `T*(C) = min_g [ |C| + sum_{r != WIN} T*(C_{g,r}) ]` - the
/// integer reformulation of the expected-guess objective (see `ideas/Wordle.md`).
pub struct OptimalPolicy<'a> {
    matrix: &'a ResponseMatrix,
    dict: &'a Dictionary,
    metrics: &'a Metrics,
    cache: &'a GlobalCache,
    equiv_cache: &'a EquivCache,
    capacity_bounds_2d: &'a [Vec<u32>],
    max_k: usize,
    all_guesses_bits: &'a [u64],
}

impl<'a> OptimalPolicy<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        matrix: &'a ResponseMatrix,
        dict: &'a Dictionary,
        metrics: &'a Metrics,
        cache: &'a GlobalCache,
        equiv_cache: &'a EquivCache,
        capacity_bounds_2d: &'a [Vec<u32>],
        max_k: usize,
        all_guesses_bits: &'a [u64],
    ) -> Self {
        Self {
            matrix,
            dict,
            metrics,
            cache,
            equiv_cache,
            capacity_bounds_2d,
            max_k,
            all_guesses_bits,
        }
    }

    /// The exact optimal guess and its total cost for `candidates`.
    pub fn guess_and_cost(&self, candidates: &[usize]) -> (usize, u32) {
        match candidates.len() {
            0 => (usize::MAX, 0),
            1 => (self.dict.candidate_to_guess[candidates[0]], 1),
            // Any in-set guess is optimal for two candidates (1 + 2 = 3).
            2 => (self.dict.candidate_to_guess[candidates[0]], 3),
            _ => self.scan(candidates),
        }
    }

    fn scan(&self, candidates: &[usize]) -> (usize, u32) {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

        let initial_best_val = Solver::greedy_solve(self.matrix, self.dict, candidates);
        // global_beta tracks best_val + 1 so we get exact values for ties!
        let global_beta = AtomicU32::new(initial_best_val + 1);
        // Pack (cost, index) into u64. We want to minimize cost, and tie-break by index.
        let global_best = AtomicU64::new(((initial_best_val as u64) << 32) | (u32::MAX as u64));
        let active = active_guesses(self.matrix, self.dict, candidates);

        // For very small candidate sets, avoid thread overhead
        if candidates.len() <= 10 {
            let mut solver = Solver::new(
                self.matrix,
                self.max_k,
                self.dict,
                self.metrics,
                self.capacity_bounds_2d,
                self.cache,
                self.equiv_cache,
            );
            let mut best_val = initial_best_val;
            let mut best_guess = usize::MAX;
            for (g, _) in &active {
                let v = solver.min_guess_val(
                    candidates,
                    self.all_guesses_bits,
                    *g,
                    best_val + 1,
                    1,
                    self.max_k,
                );
                if v < best_val {
                    best_val = v;
                    best_guess = *g;
                } else if v == best_val && best_guess == usize::MAX {
                    best_guess = *g;
                }
            }
            return (best_guess, best_val);
        }

        let chunk_size = (active.len() / rayon::current_num_threads()).max(1);

        active
            .par_chunks(chunk_size)
            .enumerate()
            .for_each(|(chunk_idx, chunk)| {
                let mut solver = Solver::new_with_global_beta(
                    self.matrix,
                    self.max_k,
                    self.dict,
                    self.metrics,
                    self.capacity_bounds_2d,
                    self.cache,
                    &global_beta,
                    self.equiv_cache,
                );

                let start_idx = chunk_idx * chunk_size;

                for (i, &(g, _)) in chunk.iter().enumerate() {
                    let idx = start_idx + i;

                    let current_best_val = global_beta.load(Ordering::Relaxed);

                    let v = solver.min_guess_val(
                        candidates,
                        self.all_guesses_bits,
                        g,
                        current_best_val,
                        1,
                        self.max_k,
                    );

                    let packed = ((v as u64) << 32) | (idx as u64);
                    let old = global_best.fetch_min(packed, Ordering::Relaxed);
                    let old_val = (old >> 32) as u32;
                    if v < old_val {
                        global_beta.fetch_min(v + 1, Ordering::Relaxed);
                    }
                }
            });

        let final_best = global_best.load(Ordering::Relaxed);
        let best_val = (final_best >> 32) as u32;
        let best_idx = (final_best & 0xFFFFFFFF) as u32;

        let best_guess = if best_idx == u32::MAX {
            usize::MAX
        } else {
            active[best_idx as usize].0
        };

        (best_guess, best_val)
    }
}

impl CandidatesPolicy for OptimalPolicy<'_> {
    fn guess(&self, candidates: &[usize]) -> usize {
        self.guess_and_cost(candidates).0
    }
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

/// One node of the arena: a guess plus a contiguous range of edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyNode {
    /// Index into `Dictionary::guesses`.
    pub guess: u16,
    /// Index of this node's first edge in `PolicyTree::edges`.
    pub edge_start: u32,
    /// Number of out-edges (possible non-win responses). Zero = leaf (win).
    pub edge_count: u16,
}

/// One edge: a possible non-win response and the child it leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEdge {
    pub response: u8,
    pub child: u32,
}

/// A fully determined policy tree, stored as a flat arena.
#[derive(Debug, Clone)]
pub struct PolicyTree {
    pub strategy: String,
    pub dictionary_hash: u64,
    pub num_guesses: u32,
    pub num_candidates: u32,
    /// Always 0 in the current builder, but stored explicitly so the format
    /// doesn't assume it.
    pub root: u32,
    pub nodes: Vec<PolicyNode>,
    pub edges: Vec<PolicyEdge>,
}

impl PolicyTree {
    fn new(strategy: &str, dictionary_hash: u64, dict: &Dictionary) -> Self {
        Self {
            strategy: strategy.to_string(),
            dictionary_hash,
            num_guesses: dict.guesses.len() as u32,
            num_candidates: dict.candidates.len() as u32,
            root: 0,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Structural bounds check that does not need a dictionary: indices must be
    /// in range and edge ranges must be contiguous and non-overlapping. Full
    /// semantic validation (edge iff possible) is [`PolicyTree::validate`].
    pub fn check_shape(&self) -> Result<(), PolicyError> {
        if self.nodes.is_empty() {
            return Err(PolicyError::Shape("tree has no nodes".into()));
        }
        if self.root as usize >= self.nodes.len() {
            return Err(PolicyError::Shape(format!(
                "root {} out of range",
                self.root
            )));
        }
        let mut expected_start = 0u32;
        for (i, n) in self.nodes.iter().enumerate() {
            if n.edge_start != expected_start {
                return Err(PolicyError::Shape(format!(
                    "node {i}: edge_start {} is not contiguous (expected {expected_start})",
                    n.edge_start
                )));
            }
            let end = n.edge_start as usize + n.edge_count as usize;
            if end > self.edges.len() {
                return Err(PolicyError::Shape(format!(
                    "node {i}: edge range {}..{} exceeds {} edges",
                    n.edge_start,
                    end,
                    self.edges.len()
                )));
            }
            for e in &self.edges[n.edge_start as usize..end] {
                if e.child as usize >= self.nodes.len() {
                    return Err(PolicyError::Shape(format!(
                        "node {i}: child {} out of range",
                        e.child
                    )));
                }
                if e.response == WIN_RESPONSE {
                    return Err(PolicyError::Shape(format!(
                        "node {i}: the win response must be a leaf, never an edge"
                    )));
                }
            }
            expected_start = end as u32;
        }
        if expected_start as usize != self.edges.len() {
            return Err(PolicyError::Shape(format!(
                "{} trailing edges not referenced by any node",
                self.edges.len() - expected_start as usize
            )));
        }
        Ok(())
    }

    /// Expands to the readable (nested, word-based) form. Requires the
    /// dictionary the tree was built against.
    pub fn to_readable(&self, dict: &Dictionary) -> Result<ReadableTreeFile, PolicyError> {
        if self.num_guesses as usize != dict.guesses.len()
            || self.num_candidates as usize != dict.candidates.len()
        {
            return Err(PolicyError::BadHeader(format!(
                "tree is for {}x{}, dictionary is {}x{}",
                self.num_guesses,
                self.num_candidates,
                dict.guesses.len(),
                dict.candidates.len()
            )));
        }
        let root = self.readable_node(self.root, dict, 0)?;
        Ok(ReadableTreeFile {
            format: FORMAT_READABLE.to_string(),
            version: FORMAT_VERSION,
            strategy: self.strategy.clone(),
            dictionary_hash: hash_hex(self.dictionary_hash),
            guesses: dict.guesses.iter().map(|w| w.to_string()).collect(),
            candidates: dict.candidates.iter().map(|w| w.to_string()).collect(),
            root,
        })
    }

    fn readable_node(
        &self,
        idx: u32,
        dict: &Dictionary,
        depth: usize,
    ) -> Result<ReadableNode, PolicyError> {
        if depth > self.num_candidates as usize {
            return Err(PolicyError::Shape("cycle detected while rendering".into()));
        }
        let n = &self.nodes[idx as usize];
        let mut children = BTreeMap::new();
        for e in &self.edges[n.edge_start as usize..(n.edge_start as usize + n.edge_count as usize)]
        {
            children.insert(
                response_to_string(e.response),
                self.readable_node(e.child, dict, depth + 1)?,
            );
        }
        Ok(ReadableNode {
            guess: dict.guesses[n.guess as usize].to_string(),
            children,
        })
    }
}

// ---------------------------------------------------------------------------
// JSON shapes
// ---------------------------------------------------------------------------

/// Readable, self-contained policy tree. Node = guess word, children = map
/// from response string to subtree. A leaf (empty `children`) is a win.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadableTreeFile {
    pub format: String,
    pub version: u32,
    pub strategy: String,
    pub dictionary_hash: String,
    /// The alphabetically sorted allowed-guess list this tree was built with.
    pub guesses: Vec<String>,
    /// The alphabetically sorted initial candidate list.
    pub candidates: Vec<String>,
    pub root: ReadableNode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadableNode {
    pub guess: String,
    #[serde(default)]
    pub children: BTreeMap<String, ReadableNode>,
}

impl ReadableTreeFile {
    pub fn to_json(&self) -> String {
        let mut s =
            serde_json::to_string_pretty(self).expect("readable tree is always serializable");
        s.push('\n');
        s
    }

    pub fn from_json(s: &str) -> Result<Self, PolicyError> {
        let file: ReadableTreeFile = serde_json::from_str(s)?;
        if file.format != FORMAT_READABLE {
            return Err(PolicyError::BadHeader(format!(
                "expected format {FORMAT_READABLE:?}, found {:?}",
                file.format
            )));
        }
        if file.version != FORMAT_VERSION {
            return Err(PolicyError::BadHeader(format!(
                "unsupported version {} (this build understands {})",
                file.version, FORMAT_VERSION
            )));
        }
        Ok(file)
    }

    /// Reconstructs a `Dictionary` from the embedded word lists. The lists are
    /// stored sorted, so the resulting indices are the tree's indices.
    ///
    /// The engine only supports 5-letter Wordle words (`core::Word` is
    /// `[u8; 5]`), so a tree whose words have another length is rejected with a
    /// clear error rather than panicking.
    pub fn embedded_dictionary(&self) -> Result<Dictionary, PolicyError> {
        for w in self.guesses.iter().chain(self.candidates.iter()) {
            if w.len() != 5 {
                return Err(PolicyError::BadHeader(format!(
                    "this build only supports 5-letter dictionaries; tree contains a {}-letter word {:?}",
                    w.len(),
                    w
                )));
            }
        }
        let guesses = self
            .guesses
            .iter()
            .map(|w| crate::core::Word::new(w))
            .collect();
        let candidates = self
            .candidates
            .iter()
            .map(|w| crate::core::Word::new(w))
            .collect();
        Ok(Dictionary::from_words(guesses, candidates))
    }

    /// Flattens back into the arena form. Word indices are looked up in the
    /// embedded lists. Node ids are assigned in the same order the builder
    /// uses (all of a node's children at once, then each child's subtree), so a
    /// build -> readable round-trip reproduces the exact bytes.
    pub fn to_tree(&self) -> Result<PolicyTree, PolicyError> {
        let dict = self.embedded_dictionary()?;
        let mut tree = PolicyTree::new(
            &self.strategy,
            parse_hash_hex(&self.dictionary_hash)?,
            &dict,
        );

        let lookup = |node: &ReadableNode| -> Result<u16, PolicyError> {
            let g = dict
                .guesses
                .binary_search(&crate::core::Word::new(&node.guess))
                .map_err(|_| {
                    PolicyError::Shape(format!(
                        "guess {:?} is not in the embedded guess list",
                        node.guess
                    ))
                })?;
            if g > u16::MAX as usize {
                return Err(PolicyError::TooManyGuesses {
                    count: dict.guesses.len(),
                });
            }
            Ok(g as u16)
        };

        // `children[id]` collects (response, child) pairs, kept in ascending
        // response order because we iterate the BTreeMap in key order.
        let mut children: Vec<Vec<(u8, u32)>> = Vec::new();
        tree.nodes.push(PolicyNode {
            guess: lookup(&self.root)?,
            edge_start: 0,
            edge_count: 0,
        });
        children.push(Vec::new());

        let mut stack: Vec<(u32, &ReadableNode)> = vec![(0, &self.root)];
        while let Some((id, node)) = stack.pop() {
            // The readable form keys children by response *string* (a BTreeMap,
            // so lexicographic); the arena orders by response *index* (the
            // little-endian base-3 value). Re-sort to the numeric order the
            // builder uses so the two round-trip to identical bytes.
            let mut kids: Vec<(u8, &ReadableNode)> = Vec::with_capacity(node.children.len());
            for (k, child) in node.children.iter() {
                let r = response_from_string(k)
                    .ok_or_else(|| PolicyError::Shape(format!("bad response key {k:?}")))?;
                kids.push((r, child));
            }
            kids.sort_unstable_by_key(|&(r, _)| r);

            let mut frames = Vec::with_capacity(kids.len());
            for (r, child) in kids {
                let cid = tree.nodes.len() as u32;
                tree.nodes.push(PolicyNode {
                    guess: lookup(child)?,
                    edge_start: 0,
                    edge_count: 0,
                });
                children.push(Vec::new());
                children[id as usize].push((r, cid));
                frames.push((cid, child));
            }
            for f in frames.into_iter().rev() {
                stack.push(f);
            }
        }

        // Lay out edges contiguously per node, ascending response.
        for (id, kids) in children.iter().enumerate() {
            let start = tree.edges.len() as u32;
            for (r, child) in kids {
                tree.edges.push(PolicyEdge {
                    response: *r,
                    child: *child,
                });
            }
            tree.nodes[id].edge_start = start;
            tree.nodes[id].edge_count = kids.len() as u16;
        }
        tree.check_shape()?;
        Ok(tree)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PolicyError {
    /// The candidate set at some node was empty (should be unreachable).
    EmptyCandidateSet { depth: usize },
    /// The policy returned a guess that does not partition the candidate set,
    /// so the tree would never terminate along that branch.
    UselessGuess {
        guess: usize,
        candidates: usize,
        depth: usize,
    },
    /// The policy returned an out-of-range guess index.
    GuessOutOfRange { guess: usize },
    /// The dictionary has more guesses than the format can index.
    TooManyGuesses { count: usize },
    /// A structural problem with the serialized tree.
    Shape(String),
    /// A header/format problem.
    BadHeader(String),
    /// A JSON parse error.
    Json(serde_json::Error),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::EmptyCandidateSet { depth } => {
                write!(f, "empty candidate set at depth {depth}")
            }
            PolicyError::UselessGuess {
                guess,
                candidates,
                depth,
            } => write!(
                f,
                "strategy is not terminating: guess {guess} does not partition a {candidates}-candidate set (depth {depth})"
            ),
            PolicyError::GuessOutOfRange { guess } => write!(f, "guess index {guess} out of range"),
            PolicyError::TooManyGuesses { count } => write!(
                f,
                "dictionary has {count} guesses, more than the format's {}-index limit",
                u16::MAX
            ),
            PolicyError::Shape(m) => write!(f, "invalid policy tree: {m}"),
            PolicyError::BadHeader(m) => write!(f, "invalid policy tree header: {m}"),
            PolicyError::Json(e) => write!(f, "invalid policy tree JSON: {e}"),
        }
    }
}

impl std::error::Error for PolicyError {}

impl From<serde_json::Error> for PolicyError {
    fn from(e: serde_json::Error) -> Self {
        PolicyError::Json(e)
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Options controlling a tree build.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Collect a cheap periodic progress time series (see [`ProgressSample`])
    /// for the optional stats export. Off by default. Samples are taken at most
    /// once per `sample_interval`, and only every `sample_every_nodes` nodes, so
    /// enabling this cannot meaningfully slow a build down.
    pub collect_samples: bool,
    /// Print periodic progress to stderr.
    pub progress: bool,
    pub progress_interval: Duration,
    /// Minimum wall-clock gap between two [`ProgressSample`]s.
    pub sample_interval: Duration,
    /// Only consult the clock every this many nodes, so sampling adds no
    /// per-node `Instant::now()` cost on the hot path.
    pub sample_every_nodes: u64,
    /// Transposition-table size (entries; must be a power of two).
    pub cache_entries: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            collect_samples: false,
            progress: true,
            progress_interval: Duration::from_secs(2),
            sample_interval: Duration::from_secs(1),
            sample_every_nodes: 8192,
            cache_entries: 1 << 22,
        }
    }
}

/// One cheap periodic sample of build progress. This - not a per-node log - is
/// what the optional Parquet/NDJSON export contains: a small time series
/// suitable for plotting nodes/sec, frontier size, cache behaviour, etc.
#[derive(Debug, Clone)]
pub struct ProgressSample {
    pub elapsed_s: f64,
    pub nodes: u64,
    pub edges: u64,
    pub frontier: u64,
    pub depth: u32,
    pub leaves: u64,
    pub pick_ms_total: f64,
    pub states_evaluated: u64,
    pub guesses_evaluated: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub pruned_by_bounds: u64,
    pub pruned_by_equivalence: u64,
}

/// Final totals for a build.
#[derive(Debug, Clone)]
pub struct BuildSummary {
    pub nodes: u64,
    pub edges: u64,
    /// Structural terminal nodes (no children). A tree has fewer of these than
    /// candidates: a candidate can also be won at an internal node whose guess
    /// is one of several remaining candidates.
    pub leaves: u64,
    /// Number of (node, candidate) wins - one per candidate, so this must equal
    /// the candidate count for a complete tree.
    pub wins: u64,
    pub max_depth: usize,
    pub total_cost: u64,
    pub total_ms: f64,
    pub pick_ms_total: f64,
    pub states_evaluated: u64,
    pub guesses_evaluated: u64,
    pub cache_hits: u64,
    pub pruned_by_bounds: u64,
    pub pruned_by_equivalence: u64,
}

/// Result of a build: the tree plus (if requested) a cheap progress time series.
pub struct BuildStats {
    pub samples: Vec<ProgressSample>,
    pub summary: BuildSummary,
}

/// Builds a complete policy tree for `root_candidates` using `strategy`.
pub fn build_policy_tree(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    strategy: Strategy,
    root_candidates: &[usize],
    opts: &BuildOptions,
) -> Result<(PolicyTree, BuildStats), PolicyError> {
    if dict.guesses.len() > u16::MAX as usize {
        return Err(PolicyError::TooManyGuesses {
            count: dict.guesses.len(),
        });
    }
    let max_k = heuristic::compute_max_branching_factor(matrix, root_candidates);
    let mut capacity_bounds_2d = vec![vec![0u32; root_candidates.len() + 1]; max_k + 1];
    for k in 2..=max_k {
        for i in 0..=root_candidates.len() {
            capacity_bounds_2d[k][i] = heuristic::capacity_bound(i, k);
        }
    }
    let metrics = Metrics::new();
    let global_cache = GlobalCache::new(opts.cache_entries);
    let equiv_cache: EquivCache =
        std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_guesses_bits = vec![u64::MAX; num_u64s];
    let rem = dict.guesses.len() % 64;
    if rem != 0 {
        all_guesses_bits[num_u64s - 1] = (1 << rem) - 1;
    }

    // The optimal policy's per-guess evaluations go through the solver's GPU
    // path, which needs the response matrix uploaded once (exactly as
    // `Solver::solve` does). Without this the GPU branch would read an
    // uninitialized matrix on the compute host.
    #[cfg(cuda_enabled)]
    crate::gpu::init_gpu_once(
        unsafe {
            std::slice::from_raw_parts(matrix.data_c_g.as_ptr() as *const u8, matrix.data_c_g.len())
        },
        &capacity_bounds_2d,
        max_k,
    );

    let optimal;
    let min_remaining;
    let max_freq;
    let policy: &dyn CandidatesPolicy = match strategy {
        Strategy::Optimal => {
            optimal = OptimalPolicy::new(
                matrix,
                dict,
                &metrics,
                &global_cache,
                &equiv_cache,
                &capacity_bounds_2d,
                max_k,
                &all_guesses_bits,
            );
            &optimal
        }
        Strategy::MinRemaining => {
            min_remaining = MinRemainingPolicy::new(matrix);
            &min_remaining
        }
        Strategy::MaxFreq => {
            max_freq = MaxFreqPolicy::new(matrix, dict);
            &max_freq
        }
    };

    let mut tree = PolicyTree::new(strategy.name(), dictionary_hash(dict), dict);
    let stats = build_into(
        &mut tree,
        matrix,
        dict,
        policy,
        root_candidates,
        opts,
        &metrics,
    )?;
    Ok((tree, stats))
}

/// DFS construction of the arena. Uses an explicit stack (not recursion) so a
/// pathologically deep tree can't overflow the stack.
#[allow(clippy::too_many_arguments)]
fn build_into(
    tree: &mut PolicyTree,
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    policy: &dyn CandidatesPolicy,
    root_candidates: &[usize],
    opts: &BuildOptions,
    metrics: &Metrics,
) -> Result<BuildStats, PolicyError> {
    let start = Instant::now();
    let mut stats = BuildStats {
        samples: Vec::new(),
        summary: BuildSummary {
            nodes: 0,
            edges: 0,
            leaves: 0,
            wins: 0,
            max_depth: 0,
            total_cost: 0,
            total_ms: 0.0,
            pick_ms_total: 0.0,
            states_evaluated: 0,
            guesses_evaluated: 0,
            cache_hits: 0,
            pruned_by_bounds: 0,
            pruned_by_equivalence: 0,
        },
    };
    let mut last_report = Instant::now();
    let mut last_sample = Instant::now();
    let mut nodes_since_sample = 0u64;

    let root = tree.nodes.len() as u32;
    tree.nodes.push(PolicyNode {
        guess: 0,
        edge_start: 0,
        edge_count: 0,
    });
    tree.root = root;
    // (node index, depth, response into node, candidates)
    let mut stack: Vec<(u32, usize, i32, Vec<usize>)> =
        vec![(root, 0, -1, root_candidates.to_vec())];
    // Edges are collected as (parent, response, child) and laid out
    // contiguously per node (ascending id, ascending response) at the end, so
    // the arena matches the canonical layout regardless of DFS order.
    let mut pending_edges: Vec<(u32, u8, u32)> = Vec::new();

    while let Some((node_idx, depth, _response, candidates)) = stack.pop() {
        if candidates.is_empty() {
            return Err(PolicyError::EmptyCandidateSet { depth });
        }
        stats.summary.max_depth = stats.summary.max_depth.max(depth);

        // A singleton is always won by guessing the remaining candidate; this
        // bypasses the policy so a heuristic can't loop forever here.
        let pick_start = Instant::now();
        let guess = if candidates.len() == 1 {
            dict.candidate_to_guess[candidates[0]]
        } else {
            policy.guess(&candidates)
        };
        stats.summary.pick_ms_total += pick_start.elapsed().as_secs_f64() * 1000.0;
        if guess >= dict.guesses.len() {
            return Err(PolicyError::GuessOutOfRange { guess });
        }

        // Partition candidates by response (indexed by response, 0..243).
        let mut buckets: Vec<Vec<usize>> = (0..NUM_RESPONSES).map(|_| Vec::new()).collect();
        for &c in &candidates {
            let r = matrix.get(guess, c).0 as usize;
            buckets[r].push(c);
        }

        // Non-win buckets become edges, in ascending response order.
        let mut child_specs: Vec<(u8, Vec<usize>)> = Vec::new();
        for r in 0..NUM_RESPONSES {
            if r == WIN_RESPONSE as usize || buckets[r].is_empty() {
                continue;
            }
            // A non-win bucket equal to the whole set means the guess did not
            // partition - the strategy would never terminate.
            if buckets[r].len() == candidates.len() {
                return Err(PolicyError::UselessGuess {
                    guess,
                    candidates: candidates.len(),
                    depth,
                });
            }
            child_specs.push((r as u8, std::mem::take(&mut buckets[r])));
        }

        let mut child_ids = Vec::with_capacity(child_specs.len());
        for _ in &child_specs {
            child_ids.push(tree.nodes.len() as u32);
            tree.nodes.push(PolicyNode {
                guess: 0,
                edge_start: 0,
                edge_count: 0,
            });
        }
        for (i, (r, _)) in child_specs.iter().enumerate() {
            pending_edges.push((node_idx, *r, child_ids[i]));
        }
        let node = &mut tree.nodes[node_idx as usize];
        node.guess = guess as u16;
        node.edge_count = child_specs.len() as u16;

        if child_specs.is_empty() {
            // A structural leaf must be exactly a win.
            if candidates.len() != 1 || buckets[WIN_RESPONSE as usize].len() != 1 {
                return Err(PolicyError::Shape(format!(
                    "node {node_idx} is a leaf but its guess does not win the {} remaining candidate(s)",
                    candidates.len()
                )));
            }
            stats.summary.leaves += 1;
        }

        // A node wins the candidate equal to its guess, if that candidate is
        // still in the set. This can happen at an internal node (the guess is
        // one of several candidates), so it is counted independently of leaves.
        let wins_here = buckets[WIN_RESPONSE as usize].len() as u64;
        stats.summary.wins += wins_here;
        // The root is guess #1, so a win at tree depth `d` costs `d + 1` guesses.
        stats.summary.total_cost += wins_here * (depth as u64 + 1);

        // Push children in reverse so ascending response order is processed first.
        for (i, (r, bucket)) in child_specs.into_iter().enumerate().rev() {
            stack.push((child_ids[i], depth + 1, r as i32, bucket));
        }

        if opts.progress && last_report.elapsed() >= opts.progress_interval {
            last_report = Instant::now();
            eprintln!(
                "[tree] nodes={} edges={} frontier={} depth={} leaves={} elapsed={:.1}s pick_time={:.1}s",
                tree.nodes.len(),
                pending_edges.len(),
                stack.len(),
                depth,
                stats.summary.leaves,
                start.elapsed().as_secs_f64(),
                stats.summary.pick_ms_total / 1000.0,
            );
        }

        // Cheap periodic sampling: the clock is consulted at most once per
        // `sample_every_nodes` nodes, so this cannot affect the hot path.
        if opts.collect_samples {
            nodes_since_sample += 1;
            if nodes_since_sample >= opts.sample_every_nodes
                && last_sample.elapsed() >= opts.sample_interval
            {
                nodes_since_sample = 0;
                last_sample = Instant::now();
                stats.samples.push(ProgressSample {
                    elapsed_s: start.elapsed().as_secs_f64(),
                    nodes: tree.nodes.len() as u64,
                    edges: pending_edges.len() as u64,
                    frontier: stack.len() as u64,
                    depth: depth as u32,
                    leaves: stats.summary.leaves,
                    pick_ms_total: stats.summary.pick_ms_total,
                    states_evaluated: metrics.states_evaluated.load(Ordering::Relaxed) as u64,
                    guesses_evaluated: metrics.guesses_evaluated.load(Ordering::Relaxed) as u64,
                    cache_hits: metrics.cache_hits.load(Ordering::Relaxed) as u64,
                    cache_misses: metrics.cache_misses.load(Ordering::Relaxed) as u64,
                    pruned_by_bounds: metrics.pruned_by_bounds.load(Ordering::Relaxed) as u64,
                    pruned_by_equivalence: metrics.pruned_by_equivalence.load(Ordering::Relaxed)
                        as u64,
                });
            }
        }
    }

    // Canonical edge layout: contiguous per node, ascending node id then
    // ascending response. Edges for a given parent were pushed in ascending
    // response order, so a stable sort by parent preserves that.
    pending_edges.sort_by_key(|&(p, _, _)| p);
    tree.edges.clear();
    tree.edges.reserve(pending_edges.len());
    for (_, r, child) in &pending_edges {
        tree.edges.push(PolicyEdge {
            response: *r,
            child: *child,
        });
    }
    let mut cursor = 0u32;
    for n in tree.nodes.iter_mut() {
        n.edge_start = cursor;
        cursor += n.edge_count as u32;
    }

    let m = &mut stats.summary;
    m.nodes = tree.nodes.len() as u64;
    m.edges = tree.edges.len() as u64;
    m.total_ms = start.elapsed().as_secs_f64() * 1000.0;
    m.states_evaluated = metrics.states_evaluated.load(Ordering::Relaxed) as u64;
    m.guesses_evaluated = metrics.guesses_evaluated.load(Ordering::Relaxed) as u64;
    m.cache_hits = metrics.cache_hits.load(Ordering::Relaxed) as u64;
    m.pruned_by_bounds = metrics.pruned_by_bounds.load(Ordering::Relaxed) as u64;
    m.pruned_by_equivalence = metrics.pruned_by_equivalence.load(Ordering::Relaxed) as u64;

    // Always record a final sample so even a short build yields a usable
    // series that ends at the completed state.
    if opts.collect_samples {
        let summary = &stats.summary;
        stats.samples.push(ProgressSample {
            elapsed_s: summary.total_ms / 1000.0,
            nodes: summary.nodes,
            edges: summary.edges,
            frontier: 0,
            depth: summary.max_depth as u32,
            leaves: summary.leaves,
            pick_ms_total: summary.pick_ms_total,
            states_evaluated: summary.states_evaluated,
            guesses_evaluated: summary.guesses_evaluated,
            cache_hits: summary.cache_hits,
            cache_misses: 0,
            pruned_by_bounds: summary.pruned_by_bounds,
            pruned_by_equivalence: summary.pruned_by_equivalence,
        });
    }
    Ok(stats)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Summary produced by a successful [`PolicyTree::validate`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub nodes: usize,
    pub edges: usize,
    /// Structural terminal nodes (no children).
    pub leaves: usize,
    /// Candidates won - one per candidate, so this must equal the candidate
    /// count (a win can also occur at an internal node).
    pub wins: u64,
    pub max_depth: usize,
    pub total_cost: u64,
    pub mean_guesses: f64,
    /// Leaves at each depth, indexed by depth.
    pub depth_histogram: Vec<usize>,
}

impl PolicyTree {
    /// Recomputes every node's candidate set from the root and checks the
    /// defining invariant: **at each node, an edge for a response exists if and
    /// only if that response is possible** for some still-reachable candidate,
    /// and each edge leads to exactly the subtree for the candidates that
    /// produce it.
    ///
    /// Also verifies the tree is a tree (every node reachable exactly once),
    /// that leaves are wins, that every candidate terminates, and that the
    /// dictionary hash matches.
    pub fn validate(
        &self,
        matrix: &ResponseMatrix,
        dict: &Dictionary,
    ) -> Result<ValidationReport, PolicyError> {
        self.check_shape()?;
        if self.num_guesses as usize != dict.guesses.len()
            || self.num_candidates as usize != dict.candidates.len()
        {
            return Err(PolicyError::Shape(format!(
                "tree is for {}x{}, dictionary is {}x{}",
                self.num_guesses,
                self.num_candidates,
                dict.guesses.len(),
                dict.candidates.len()
            )));
        }
        let want_hash = dictionary_hash(dict);
        if self.dictionary_hash != want_hash {
            return Err(PolicyError::BadHeader(format!(
                "dictionary hash {} does not match the supplied dictionary ({})",
                hash_hex(self.dictionary_hash),
                hash_hex(want_hash)
            )));
        }

        let n = self.nodes.len();
        let mut visited = vec![false; n];
        let mut report = ValidationReport {
            nodes: n,
            edges: self.edges.len(),
            leaves: 0,
            wins: 0,
            max_depth: 0,
            total_cost: 0,
            mean_guesses: 0.0,
            depth_histogram: Vec::new(),
        };

        let root_candidates: Vec<usize> = (0..dict.candidates.len()).collect();
        let mut stack: Vec<(u32, usize, Vec<usize>)> = vec![(self.root, 0, root_candidates)];

        while let Some((idx, depth, candidates)) = stack.pop() {
            if visited[idx as usize] {
                return Err(PolicyError::Shape(format!(
                    "node {idx} is reachable more than once (not a tree)"
                )));
            }
            visited[idx as usize] = true;
            report.max_depth = report.max_depth.max(depth);

            let node = &self.nodes[idx as usize];
            let guess = node.guess as usize;
            if guess >= dict.guesses.len() {
                return Err(PolicyError::Shape(format!(
                    "node {idx}: guess {guess} out of range"
                )));
            }
            if candidates.is_empty() {
                return Err(PolicyError::Shape(format!(
                    "node {idx}: empty candidate set"
                )));
            }

            // Possible non-win responses, and the candidate set for each.
            let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); NUM_RESPONSES];
            for &c in &candidates {
                buckets[matrix.get(guess, c).0 as usize].push(c);
            }
            let mut possible: Vec<u8> = Vec::new();
            for r in 0..NUM_RESPONSES {
                if r != WIN_RESPONSE as usize && !buckets[r].is_empty() {
                    possible.push(r as u8);
                }
            }

            let actual_edges = &self.edges
                [node.edge_start as usize..(node.edge_start as usize + node.edge_count as usize)];
            let mut actual: Vec<u8> = actual_edges.iter().map(|e| e.response).collect();
            actual.sort_unstable();

            if actual != possible {
                // Produce a precise diagnosis.
                let missing: Vec<String> = possible
                    .iter()
                    .filter(|r| !actual.contains(r))
                    .map(|r| response_to_string(*r))
                    .collect();
                let extra: Vec<String> = actual
                    .iter()
                    .filter(|r| !possible.contains(r))
                    .map(|r| response_to_string(*r))
                    .collect();
                return Err(PolicyError::Shape(format!(
                    "node {idx} (guess {:?}, {} candidates): edge/response mismatch - missing {:?}, impossible/duplicate {:?}",
                    dict.guesses[guess].to_string(),
                    candidates.len(),
                    missing,
                    extra
                )));
            }

            // A node wins the candidate equal to its guess, if still present.
            // This can happen at an internal node, so it is counted separately
            // from structural leaves.
            let wins_here = buckets[WIN_RESPONSE as usize].len() as u64;
            report.wins += wins_here;
            if wins_here > 0 {
                report.total_cost += wins_here * (depth as u64 + 1);
                if report.depth_histogram.len() <= depth {
                    report.depth_histogram.resize(depth + 1, 0);
                }
                report.depth_histogram[depth] += wins_here as usize;
            }

            if actual_edges.is_empty() {
                // Structural leaf: must be exactly the win.
                if candidates.len() != 1 || wins_here != 1 {
                    return Err(PolicyError::Shape(format!(
                        "node {idx} is a leaf but {} candidate(s) remain (not a win)",
                        candidates.len()
                    )));
                }
                report.leaves += 1;
                continue;
            }

            if candidates.len() == 1 {
                return Err(PolicyError::Shape(format!(
                    "node {idx} has edges but only one candidate remains"
                )));
            }

            for e in actual_edges {
                stack.push((
                    e.child,
                    depth + 1,
                    std::mem::take(&mut buckets[e.response as usize]),
                ));
            }
        }

        if visited.iter().any(|v| !v) {
            let orphans = visited.iter().filter(|&&v| !v).count();
            return Err(PolicyError::Shape(format!(
                "{orphans} node(s) are unreachable from the root"
            )));
        }
        if report.wins != dict.candidates.len() as u64 {
            return Err(PolicyError::Shape(format!(
                "{} candidate wins but {} candidates - not every candidate terminates",
                report.wins,
                dict.candidates.len()
            )));
        }
        report.mean_guesses = report.total_cost as f64 / dict.candidates.len() as f64;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict_from(guesses: &[&str], candidates: &[&str]) -> Dictionary {
        Dictionary::from_words(
            guesses.iter().map(|w| crate::core::Word::new(w)).collect(),
            candidates
                .iter()
                .map(|w| crate::core::Word::new(w))
                .collect(),
        )
    }

    #[test]
    fn response_string_roundtrip() {
        for r in 0..NUM_RESPONSES as u8 {
            let s = response_to_string(r);
            assert_eq!(response_from_string(&s), Some(r));
        }
        assert_eq!(response_to_string(Response::WIN.0), "ggggg");
        assert_eq!(response_to_string(0), "bbbbb");
        assert_eq!(
            response_from_string("bgybg"),
            Some(Response::new(0, 1, 2, 0, 1).0)
        );
    }

    #[test]
    fn strategy_parse_roundtrip() {
        for s in Strategy::all() {
            assert_eq!(Strategy::parse(s.name()), Some(s));
        }
        assert_eq!(Strategy::parse("nonsense"), None);
    }

    #[test]
    fn dictionary_hash_is_stable_and_distinguishes() {
        let a = dict_from(&["abcde", "fghij"], &["abcde"]);
        let b = dict_from(&["abcde", "fghij"], &["abcde"]);
        let c = dict_from(&["abcde", "fghij"], &["fghij"]);
        assert_eq!(dictionary_hash(&a), dictionary_hash(&b));
        assert_ne!(dictionary_hash(&a), dictionary_hash(&c));
    }

    /// Build a tiny real dictionary (first N candidates of the shipped list).
    fn real_dict(n: usize) -> Dictionary {
        let full = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let candidates: Vec<crate::core::Word> = full.candidates[..n].to_vec();
        Dictionary::from_words(full.guesses.clone(), candidates)
    }

    #[test]
    fn heuristic_trees_build_and_validate() {
        let dict = real_dict(12);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        for strategy in [Strategy::MinRemaining, Strategy::MaxFreq] {
            let opts = BuildOptions {
                progress: false,
                ..Default::default()
            };
            let (tree, stats) = build_policy_tree(&matrix, &dict, strategy, &root, &opts).unwrap();
            let report = tree.validate(&matrix, &dict).unwrap();
            assert_eq!(report.wins, dict.candidates.len() as u64);
            assert_eq!(report.total_cost, stats.summary.total_cost);
            assert!(report.max_depth >= 1);
        }
    }

    /// Builds the exact optimal tree for the deterministic first-N candidate
    /// subset (the same convention the golden solver tests in `solver.rs`
    /// use), validates it, and returns the report plus what the caller needs
    /// to re-derive the solver's optimum for the same subset.
    fn build_and_validate_optimal_tree(n: usize) -> (Dictionary, ResponseMatrix, ValidationReport) {
        let dict = real_dict(n);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..n).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, stats) =
            build_policy_tree(&matrix, &dict, Strategy::Optimal, &root, &opts).unwrap();
        let report = tree.validate(&matrix, &dict).unwrap();
        assert_eq!(report.wins, n as u64);
        assert_eq!(stats.summary.total_cost, report.total_cost);
        (dict, matrix, report)
    }

    /// The exact correctness criterion for an optimal tree: it must be valid
    /// (every candidate terminates, and an edge exists iff its response is
    /// possible) and its total cost must equal the solver's exact optimal
    /// cost. Together those imply optimality, since no strategy can beat the
    /// solver's optimum. Checked at a non-trivial size as well as a tiny one,
    /// because a depth-3 tree exercises partitioning the tiny case does not.
    #[test]
    fn optimal_tree_cost_matches_solver() {
        for n in [10, 100] {
            let (dict, matrix, report) = build_and_validate_optimal_tree(n);
            let root: Vec<usize> = (0..n).collect();
            let metrics = Metrics::new();
            let equiv: EquivCache =
                std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));
            let solver_cost = Solver::solve(&matrix, &root, &dict, &metrics, &equiv);
            assert_eq!(
                report.total_cost, solver_cost as u64,
                "optimal policy tree total cost must equal the solver's optimal cost at N={n}"
            );
        }
    }

    #[test]
    fn readable_roundtrip_is_deterministic() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (t1, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        let (t2, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        assert_eq!(
            t1.to_readable(&dict).unwrap().to_json(),
            t2.to_readable(&dict).unwrap().to_json()
        );

        let json = t1.to_readable(&dict).unwrap().to_json();
        let parsed = ReadableTreeFile::from_json(&json).unwrap();
        let back = parsed.to_tree().unwrap();
        assert_eq!(back.to_readable(&dict).unwrap().to_json(), json);
        back.validate(&matrix, &dict).unwrap();
    }

    #[test]
    fn readable_roundtrip_validates() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        let readable = tree.to_readable(&dict).unwrap();
        let json = readable.to_json();
        let parsed = ReadableTreeFile::from_json(&json).unwrap();
        let embedded = parsed.embedded_dictionary().unwrap();
        let back = parsed.to_tree().unwrap();
        back.validate(&matrix, &embedded).unwrap();
        assert_eq!(
            back.to_readable(&embedded).unwrap().to_json(),
            tree.to_readable(&dict).unwrap().to_json()
        );
    }

    #[test]
    fn validator_rejects_tampering() {
        let dict = real_dict(8);
        let matrix = ResponseMatrix::new(&dict);
        let root: Vec<usize> = (0..dict.candidates.len()).collect();
        let opts = BuildOptions {
            progress: false,
            ..Default::default()
        };
        let (tree, _) =
            build_policy_tree(&matrix, &dict, Strategy::MinRemaining, &root, &opts).unwrap();
        tree.validate(&matrix, &dict).unwrap();

        // 1. Missing an edge.
        let mut missing = tree.clone();
        let n0 = missing.nodes[missing.root as usize];
        let last = n0.edge_start as usize + n0.edge_count as usize - 1;
        missing.edges.remove(last);
        missing.nodes[missing.root as usize].edge_count -= 1;
        assert!(matches!(
            missing.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 2. An impossible response on an edge.
        let mut impossible = tree.clone();
        let n0 = impossible.nodes[impossible.root as usize];
        let start = n0.edge_start as usize;
        let mut used: Vec<u8> = impossible.edges[start..start + n0.edge_count as usize]
            .iter()
            .map(|e| e.response)
            .collect();
        let spare = (0..NUM_RESPONSES as u8)
            .find(|r| *r != WIN_RESPONSE && !used.contains(r))
            .unwrap();
        used.clear();
        impossible.edges[start].response = spare;
        assert!(matches!(
            impossible.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 3. Orphan node.
        let mut orphan = tree.clone();
        orphan.nodes.push(PolicyNode {
            guess: 0,
            edge_start: 0,
            edge_count: 0,
        });
        assert!(matches!(
            orphan.validate(&matrix, &dict),
            Err(PolicyError::Shape(_))
        ));

        // 4. Wrong dictionary hash.
        let mut wrong_hash = tree.clone();
        wrong_hash.dictionary_hash ^= 1;
        assert!(matches!(
            wrong_hash.validate(&matrix, &dict),
            Err(PolicyError::BadHeader(_))
        ));
    }
}
