#![allow(clippy::needless_range_loop)]
//! Policy trees: self-describing storage of a fully determined Wordle
//! strategy, and the logic to validate one.
//!
//! A *policy* maps a state (the set of remaining candidates) to a guess. For a
//! fixed dictionary and a deterministic policy, the whole game is a static
//! decision tree (see `ideas/Wordle.md`, "Storing Policies"):
//!
//! - **nodes are guesses** (the root is the first guess),
//! - **edges are responses** (only the *non-win* responses: the all-green
//!   response is never an edge), and
//! - **wins are implicit**: a node wins for the candidate equal to its own
//!   guess, which happens exactly when that candidate is still reachable at the
//!   node (the all-green response is then possible). A structural leaf (no
//!   children) is such a win with one candidate left.
//!
//! Because wins are implicit, a node's win state depends on its *remaining*
//! candidate set, not on the initial one: a guess that is in the initial
//! candidate list may have been eliminated along the path, in which case it
//! cannot win there. Consumers must recompute the candidate set (below) rather
//! than testing the guess against the initial list.
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

use crate::core::Response;
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The total number of ternary responses (`3^5`).
pub const NUM_RESPONSES: usize = 243;
/// The index of the all-green ("win") response.
pub const WIN_RESPONSE: u8 = Response::WIN.0;

/// Format tags written into the JSON headers, and checked on load.
pub const FORMAT_READABLE: &str = "wordle-policy-tree";
/// Bumped whenever the on-disk shape changes in a way older readers can't
/// handle. Readers reject a version they don't know rather than guessing.
pub const FORMAT_VERSION: u32 = 1;

thread_local! {
    static SCRATCH_SORTED_SET: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

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
    /// An empty tree for `dict`, to be filled by a builder.
    pub fn new(strategy: &str, dictionary_hash: u64, dict: &Dictionary) -> Self {
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
/// from response string to subtree. Children hold only the *non-win* responses:
/// a node wins iff the all-green response is possible for a remaining candidate
/// (see the module docs for the exact rule and the trap of testing against the
/// initial candidate list), and a leaf (empty `children`) is such a win with one
/// candidate left.
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

/// One node: the guess word and its *non-win* response children. The all-green
/// response is never a child; see [`ReadableTreeFile`] for the win rule.
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
