//! Optimality-certificate generation: an investigation prototype.
//!
//! Turns a solved policy tree into a certificate that `crate::certcheck`
//! verifies without trusting the solver. Two sources of lower-bound facts:
//!
//! * a **refutation pass** ([`refutation_pass`]): the tree is taken as given,
//!   and at every node every other guess is searched with beta fixed at the
//!   node's known exact cost, so only refutation work is redone;
//! * the **solve itself**, logging every bound it proves (`solve --certify`).
//!
//! Facts come from `crate::certlog`.
//! [`assemble`] then builds the smallest proof it can from them, top down from
//! the root, the same way the checker will verify it: expand a claim over
//! every allowed guess, and request a claim on a bucket only where the
//! default bound is not enough. Nothing here is trusted; a wrong fact makes
//! the assembly fail (or the checker reject).

use crate::certfile::{
    self, Claim, Header, RULE_ASSUMED, RULE_BASE, RULE_CAPACITY, RULE_EXPANSION, SetDef,
};
use crate::core::Response;
use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::policy::PolicyTree;
use crate::profile::SetProfile;
use crate::solver::{Metrics, Solver};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub type FactMap = FxHashMap<Box<[u16]>, u32>;

/// A claim the facts could not justify: (set, target, guesses left unrefuted).
pub type Gap = (Box<[u16]>, u32, Vec<u32>);

/// One node of a solved tree with its candidate set and exact subtree cost.
pub struct TreeNode {
    pub set: Vec<usize>,
    pub guess: usize,
    pub cost: u32,
    pub depth: usize,
}

/// Every node of `tree`, parents before children.
pub fn tree_nodes(tree: &PolicyTree, matrix: &ResponseMatrix) -> Vec<TreeNode> {
    let mut out: Vec<TreeNode> = Vec::new();
    let mut parent_of: Vec<Option<usize>> = Vec::new();
    let mut stack = vec![(
        tree.root,
        (0..matrix.num_candidates).collect::<Vec<_>>(),
        0,
        None,
    )];
    while let Some((idx, set, depth, parent)) = stack.pop() {
        let node = tree.nodes[idx as usize];
        let g = node.guess as usize;
        let me = out.len();
        for e in &tree.edges[node.edge_start as usize..][..node.edge_count as usize] {
            let child: Vec<usize> = set
                .iter()
                .copied()
                .filter(|&c| matrix.get(g, c).0 == e.response)
                .collect();
            stack.push((e.child, child, depth + 1, Some(me)));
        }
        out.push(TreeNode {
            cost: set.len() as u32,
            set,
            guess: g,
            depth,
        });
        parent_of.push(parent);
    }
    // Children come after their parent, so a reverse sweep sums subtree costs.
    for i in (0..out.len()).rev() {
        if let Some(p) = parent_of[i] {
            out[p].cost += out[i].cost;
        }
    }
    out
}

fn capacity_table(matrix: &ResponseMatrix, n: usize) -> (usize, Vec<Vec<u32>>) {
    let root: Vec<usize> = (0..n).collect();
    let max_k = crate::heuristic::compute_max_branching_factor(matrix, &root);
    (max_k, capacity_rows(max_k, n))
}

fn capacity_rows(max_k: usize, n: usize) -> Vec<Vec<u32>> {
    let mut caps = vec![vec![0u32; n + 1]; max_k + 1];
    for k in 2..=max_k {
        for i in 0..=n {
            caps[k][i] = crate::heuristic::capacity_bound(i, k);
        }
    }
    caps
}

#[derive(Debug, Default)]
pub struct PassStats {
    pub nodes: usize,
    pub tasks: usize,
    pub skipped_by_capacity: usize,
    pub states_evaluated: usize,
    pub guesses_evaluated: usize,
    /// Task time summed over threads, by tree depth of the node (3 = 3 and deeper).
    pub task_secs_by_depth: [f64; 4],
    pub prefilter_secs: f64,
    pub failures: Vec<String>,
}

/// Refutes, at every tree node of depth `<= max_depth`, every alternative
/// guess against the node's exact cost, with the solver's own search. A
/// guess searched with beta = cost must come back `>= cost`; anything lower
/// means the tree is not optimal (or the solver is wrong).
pub fn refutation_pass(
    nodes: &[TreeNode],
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    max_depth: usize,
    cache_entries: usize,
) -> PassStats {
    let n = matrix.num_candidates;
    let (max_k, caps) = capacity_table(matrix, n);
    let cache = crate::cache::GlobalCache::new(cache_entries);
    let equiv: crate::solver::EquivCache =
        std::array::from_fn(|_| std::sync::RwLock::new(FxHashMap::default()));
    let metrics = Metrics::new();
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_bits = vec![u64::MAX; num_u64s];
    if !dict.guesses.len().is_multiple_of(64) {
        all_bits[num_u64s - 1] = (1 << (dict.guesses.len() % 64)) - 1;
    }

    let work: Vec<usize> = (0..nodes.len())
        .filter(|&i| nodes[i].depth <= max_depth && nodes[i].set.len() >= 3)
        .collect();
    let prefilter_start = Instant::now();
    let per_node: Vec<(usize, Vec<usize>, usize)> = work
        .par_iter()
        .map(|&i| {
            let node = &nodes[i];
            let active = crate::policy::active_guesses(matrix, dict, &node.set, &caps, max_k);
            let total = active.len();
            let todo: Vec<usize> = active
                .into_iter()
                .filter(|&(g, _, lb)| g != node.guess && lb < node.cost)
                .map(|(g, _, _)| g)
                .collect();
            (i, todo, total)
        })
        .collect();
    let mut stats = PassStats {
        nodes: work.len(),
        prefilter_secs: prefilter_start.elapsed().as_secs_f64(),
        ..Default::default()
    };
    let by_depth: [std::sync::atomic::AtomicU64; 4] = Default::default();
    let mut tasks: Vec<(usize, usize)> = Vec::new();
    for (i, todo, total) in per_node {
        stats.skipped_by_capacity += total - todo.len();
        tasks.extend(todo.into_iter().map(|g| (i, g)));
    }
    stats.tasks = tasks.len();
    tasks.sort_by_key(|&(i, g)| (std::cmp::Reverse(nodes[i].set.len()), i, g));

    stats.failures = tasks
        .par_iter()
        .with_max_len(1)
        .filter_map(|&(i, g)| {
            let node = &nodes[i];
            let t = Instant::now();
            let mut solver = Solver::new(matrix, max_k, dict, &metrics, &caps, &cache, &equiv);
            let v = solver.min_guess_val(&node.set, &all_bits, g, node.cost, 1, max_k);
            by_depth[node.depth.min(3)].fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            (v < node.cost).then(|| {
                format!(
                    "node {i} (|S| = {}, cost {}): guess {} reached {v}",
                    node.set.len(),
                    node.cost,
                    dict.guesses[g]
                )
            })
        })
        .collect();
    for d in 0..4 {
        stats.task_secs_by_depth[d] = by_depth[d].load(Ordering::Relaxed) as f64 * 1e-9;
    }
    stats.states_evaluated = metrics.states_evaluated.load(Ordering::Relaxed);
    stats.guesses_evaluated = metrics.guesses_evaluated.load(Ordering::Relaxed);
    stats
}

struct Pending {
    target: u32,
    depth: u32,
    def: SetDef,
}

struct ChildReq {
    set: Box<[u16]>,
    target: u32,
    guess: u32,
    response: u8,
}

#[derive(Debug, Default)]
pub struct AssemblyStats {
    pub claims: usize,
    pub by_rule: [usize; 4],
    pub tree_node_claims: usize,
    pub max_depth_reached: u32,
    pub assumed_max_size: usize,
    /// Assumed claims on sets of at most 30 candidates: within reach of the
    /// brute-force audit.
    pub assumed_small: usize,
    pub failures: Vec<String>,
    /// (set, target, guesses the known facts could not refute)
    pub failed: Vec<Gap>,
    /// Response reads a straightforward checker needs: `|G| |S|` per capacity
    /// claim (computing `k`), twice that per expansion (`k`, then every guess).
    pub check_lookups: u64,
}

pub struct Assembly {
    claims: Vec<Claim>,
    pub stats: AssemblyStats,
}

/// Whether no guess gives more than `limit` responses on `set`, reading only
/// guesses whose live positions (letters that occur in `set`) allow more.
fn branching_at_most(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    set: &[u16],
    limit: usize,
) -> bool {
    if set.len() <= limit {
        return true;
    }
    let profile = SetProfile::new(set.iter().map(|&c| &dict.candidates[c as usize]));
    let ng = matrix.num_guesses;
    let risky: Vec<usize> = (0..ng)
        .filter(|&g| profile.max_responses(&dict.guesses[g]) > limit)
        .collect();
    let mut seen = [[0u64; 36]; 64];
    for chunk in risky.chunks(64) {
        let seen = &mut seen[..chunk.len()];
        seen.fill([0; 36]);
        for &c in set {
            for (s, &g) in seen.iter_mut().zip(chunk) {
                let r = matrix.get_cg(c as usize, g).0;
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
fn max_branching(matrix: &ResponseMatrix, set: &[u16], guesses: &[usize]) -> usize {
    let _ng = matrix.num_guesses;
    let mut k = 0;
    let mut seen = [[0u64; 36]; 64];
    for chunk in guesses.chunks(64) {
        let seen = &mut seen[..chunk.len()];
        seen.fill([0; 36]);
        for &c in set {
            for (s, &g) in seen.iter_mut().zip(chunk) {
                let r = matrix.get_cg(c as usize, g).0;
                s[(r >> 6) as usize] |= 1 << (r & 63);
            }
        }
        for s in seen.iter() {
            k = k.max(s.iter().map(|w| w.count_ones() as usize).sum());
        }
    }
    k
}

/// Picks the rule for `LB(set) >= target` and, for an expansion, the bucket
/// claims it needs.
fn justify(
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    set: &[u16],
    target: u32,
    expand: bool,
    fact: &dyn Fn(&[u16]) -> u32,
) -> Result<(u8, Vec<ChildReq>), (String, Vec<u32>)> {
    let n = set.len();
    if n <= 2 {
        return if target as u64 <= certfile::l3(n, 0) {
            Ok((RULE_BASE, Vec::new()))
        } else {
            Err(("base set claimed above its cost".into(), Vec::new()))
        };
    }
    // Capacity holds if target <= 2n - 1, or if no guess gives more than
    // 3n - 1 - target responses; only then is the exact `k` needed.
    let limit = (3 * n as u64).saturating_sub(1 + target as u64) as usize;
    if (target as u64) < 2 * n as u64 || branching_at_most(matrix, dict, set, limit) {
        return Ok((RULE_CAPACITY, Vec::new()));
    }
    if !expand {
        return Ok((RULE_ASSUMED, Vec::new()));
    }
    let win = Response::WIN.0;
    let _nc = matrix.num_candidates;
    let ng = matrix.num_guesses;
    // Only guesses whose smallest possible default sum (at most `caps[g]`
    // buckets, k at most the largest cap) falls short of the target are read
    // for their sums; the checker skips the same ones.
    let profile = SetProfile::new(set.iter().map(|&c| &dict.candidates[c as usize]));
    let caps: Vec<usize> = (0..ng)
        .map(|g| profile.max_responses(&dict.guesses[g]))
        .collect();
    let k_up = caps.iter().copied().max().unwrap_or(0) as u64;
    let mut in_set = vec![false; ng];
    for &c in set {
        in_set[dict.candidate_to_guess[c as usize]] = true;
    }
    let n64 = n as u64;
    let summed: Vec<usize> = (0..ng)
        .filter(|&g| {
            let (w, m) = (in_set[g] as u64, caps[g] as u64);
            let low = (3 * n64 - 2 * w).saturating_sub(m) + (n64 - w).saturating_sub(m * k_up);
            low < target as u64
        })
        .collect();
    // One pass over the candidate-major table in chunks of those guesses finds
    // their default sums: a bucket of size b defaults to (2b - 1) + max(0, b - k),
    // and only buckets larger than the guess's own bucket count can exceed k,
    // so those are kept until `k` is known (the checker does the same). Only
    // guesses left short are looked at again.
    const CHUNK: usize = 256;
    let nr = matrix.num_responses;
    let mut counts = vec![0u16; CHUNK * nr];
    let mut used: Vec<Vec<u16>> = (0..CHUNK).map(|_| Vec::new()).collect();
    // Guesses whose base sum n + sum(2b - 1) falls short of the bound: only they
    // can need the max(0, b - k) terms, so only their large buckets are kept.
    let mut pending: Vec<(usize, u64, Vec<u16>)> = Vec::new();
    let mut k = 0;
    for chunk in summed.chunks(CHUNK) {
        for &c in set {
            for (j, &g) in chunk.iter().enumerate() {
                let r = matrix.get_cg(c as usize, g);
                let slot = &mut counts[j * nr + r.0 as usize];
                if *slot == 0 {
                    used[j].push(r.0);
                }
                *slot += 1;
            }
        }
        for (j, u) in used[..chunk.len()].iter_mut().enumerate() {
            let m = u.len();
            k = k.max(m);
            let splits = m > 1 || u[0] == win;
            let mut sum = n as u64;
            let mut big: Vec<u16> = Vec::new();
            for &r in u.iter() {
                let slot = &mut counts[j * nr + r as usize];
                let b = *slot as usize;
                if r != win {
                    sum += 2 * b as u64 - 1;
                    if b > m {
                        big.push(b as u16);
                    }
                }
                *slot = 0;
            }
            u.clear();
            if splits && sum < target as u64 {
                pending.push((chunk[j], sum, big));
            }
        }
    }
    // The exact k also needs every unread guess that could beat it.
    let unread: Vec<usize> = (0..ng)
        .filter(|&g| caps[g] > k)
        .filter(|g| summed.binary_search(g).is_err())
        .collect();
    k = k.max(max_branching(matrix, set, &unread));
    let short: Vec<usize> = pending
        .into_iter()
        .filter(|(_, sum, big)| {
            let extra: u64 = big
                .iter()
                .map(|&b| (b as u64).saturating_sub(k as u64))
                .sum();
            sum + extra < target as u64
        })
        .map(|(g, _, _)| g)
        .collect();
    let win = win as usize;
    let mut reqs: FxHashMap<Box<[u16]>, ChildReq> = FxHashMap::default();
    let mut seen_rows: FxHashSet<Vec<u16>> = FxHashSet::default();
    let mut unrefuted: Vec<u32> = Vec::new();
    let mut first_msg = String::new();
    for g in short {
        let key: Vec<u16> = set.iter().map(|&c| matrix.get(g, c as usize).0).collect();
        if !seen_rows.insert(key) {
            continue;
        }
        let mut buckets: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
        for &c in set {
            let r = matrix.get(g, c as usize).0;
            if r as usize != win {
                buckets.entry(r).or_default().push(c);
            }
        }
        // (surplus over default, fact, bucket, response)
        let mut better: Vec<(u64, u64, Vec<u16>, u8)> = Vec::new();
        let mut sum = n as u64;
        let mut detail = String::new();
        for (r, b) in buckets {
            let d = certfile::l3(b.len(), k);
            let f = fact(&b) as u64;
            if b.len() > 2 {
                detail += &format!(" [{}: d{d} f{f}]", b.len());
            }
            if f > d {
                sum += f;
                better.push((f - d, f, b, r as u8));
            } else {
                sum += d;
            }
        }
        if sum < target as u64 {
            if unrefuted.is_empty() {
                first_msg = format!(
                    "guess #{g} reaches only {sum} with the known facts (k = {k}):{detail}"
                );
            }
            unrefuted.push(g as u32);
            continue;
        }
        // Drop the smallest surpluses the slack covers, then lower one more.
        let mut slack = sum - target as u64;
        better.sort_by_key(|t| t.0);
        for (surplus, f, b, r) in better {
            if surplus <= slack {
                slack -= surplus;
                continue;
            }
            let need = (f - slack) as u32;
            slack = 0;
            let set: Box<[u16]> = b.into();
            let e = reqs.entry(set.clone()).or_insert(ChildReq {
                set,
                target: 0,
                guess: g as u32,
                response: r,
            });
            e.target = e.target.max(need);
        }
    }
    if !unrefuted.is_empty() {
        let msg = format!("{first_msg} ({} guesses unrefuted)", unrefuted.len());
        return Err((msg, unrefuted));
    }
    Ok((RULE_EXPANSION, reqs.into_values().collect()))
}

/// Builds the proof `LB(all candidates) >= cost(tree)` from `facts`, expanding
/// claims down to depth `max_depth` and leaving deeper ones assumed.
pub fn assemble(
    nodes: &[TreeNode],
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    facts: &FactMap,
    max_depth: u32,
) -> Assembly {
    let tree_facts: FactMap = nodes
        .iter()
        .filter(|t| t.set.len() > 2)
        .map(|t| (t.set.iter().map(|&c| c as u16).collect(), t.cost))
        .collect();
    let fact = |s: &[u16]| -> u32 {
        facts
            .get(s)
            .copied()
            .unwrap_or(0)
            .max(tree_facts.get(s).copied().unwrap_or(0))
    };

    let mut stats = AssemblyStats::default();
    let mut claims: Vec<Claim> = Vec::new();
    let mut pending: FxHashMap<Box<[u16]>, Pending> = FxHashMap::default();
    let mut by_size: BTreeMap<std::cmp::Reverse<usize>, Vec<Box<[u16]>>> = BTreeMap::new();
    let root: Box<[u16]> = (0..matrix.num_candidates as u16).collect();
    pending.insert(
        root.clone(),
        Pending {
            target: nodes[0].cost,
            depth: 0,
            def: SetDef::Root,
        },
    );
    by_size
        .entry(std::cmp::Reverse(root.len()))
        .or_default()
        .push(root);

    while let Some((_, mut wave)) = by_size.pop_first() {
        wave.sort_unstable();
        let wave: Vec<(Box<[u16]>, Pending)> = wave
            .into_iter()
            .map(|s| {
                let p = pending.remove(&s).unwrap();
                (s, p)
            })
            .collect();
        #[allow(clippy::type_complexity)]
        let results: Vec<Result<(u8, Vec<ChildReq>), (String, Vec<u32>)>> = wave
            .par_iter()
            .map(|(s, p)| justify(matrix, dict, s, p.target, p.depth <= max_depth, &fact))
            .collect();
        for ((set, p), res) in wave.into_iter().zip(results) {
            let id = claims.len() as u32;
            stats.max_depth_reached = stats.max_depth_reached.max(p.depth);
            let (rule, reqs) = match res {
                Ok(v) => v,
                Err((e, guesses)) => {
                    stats.failures.push(format!(
                        "|S| = {}, target {}, depth {}: {e}",
                        set.len(),
                        p.target,
                        p.depth
                    ));
                    stats.failed.push((set.clone(), p.target, guesses));
                    (RULE_ASSUMED, Vec::new())
                }
            };
            let g = matrix.num_guesses as u64;
            stats.check_lookups += match rule {
                RULE_CAPACITY => g * set.len() as u64,
                RULE_EXPANSION => 2 * g * set.len() as u64,
                _ => 0,
            };
            if rule == RULE_ASSUMED {
                stats.assumed_max_size = stats.assumed_max_size.max(set.len());
                stats.assumed_small += (set.len() <= 30) as usize;
            }
            stats.by_rule[rule as usize] += 1;
            stats.tree_node_claims += tree_facts.contains_key(&set) as usize;
            claims.push(Claim {
                def: p.def,
                bound: p.target,
                rule,
            });
            for r in reqs {
                let def = SetDef::Child {
                    parent: id,
                    guess: r.guess,
                    response: r.response,
                };
                match pending.get_mut(&r.set) {
                    Some(q) => {
                        q.target = q.target.max(r.target);
                        q.depth = q.depth.min(p.depth + 1);
                    }
                    None => {
                        by_size
                            .entry(std::cmp::Reverse(r.set.len()))
                            .or_default()
                            .push(r.set.clone());
                        pending.insert(
                            r.set,
                            Pending {
                                target: r.target,
                                depth: p.depth + 1,
                                def,
                            },
                        );
                    }
                }
            }
        }
    }
    stats.claims = claims.len();
    Assembly { claims, stats }
}

impl Assembly {
    /// The certificate in the format of [`certfile`].
    pub fn to_bytes(&self, dict: &Dictionary) -> Vec<u8> {
        let header = Header {
            digest: certfile::digest(dict.guesses.iter(), dict.candidates.iter()),
            guesses: dict.guesses.len() as u32,
            candidates: dict.candidates.len() as u32,
        };
        certfile::encode(&header, &self.claims)
    }
}

pub fn print_assembly(a: &Assembly, elapsed: f64) {
    let s = &a.stats;
    println!("Assembly time: {elapsed:.3}s");
    println!(
        "Proof claims: {} (base {}, capacity {}, expansion {}, assumed {}); tree-node claims {}; max depth {}",
        s.claims,
        s.by_rule[0],
        s.by_rule[1],
        s.by_rule[2],
        s.by_rule[3],
        s.tree_node_claims,
        s.max_depth_reached
    );
    if s.by_rule[RULE_ASSUMED as usize] > 0 {
        println!(
            "Assumed claims: {} (largest |S| = {}, {} with |S| <= 30)",
            s.by_rule[3], s.assumed_max_size, s.assumed_small
        );
    }
    println!(
        "Check volume: {} response reads ({:.2e})",
        s.check_lookups, s.check_lookups as f64
    );
    println!("Assembly failures: {}", s.failures.len());
    for f in s.failures.iter().take(20) {
        println!("ASSEMBLY FAIL: {f}");
    }
}

pub fn print_facts(f: &crate::certlog::Facts) {
    println!(
        "Facts proved: {} records ({} exact, {} lower-bound only), {} distinct sets, {} MB of member data",
        f.records,
        f.exact_records,
        f.records - f.exact_records,
        f.bound.len(),
        f.member_bytes / (1 << 20)
    );
}

/// `wordle-opt certify <tree.json> [--cert out] [--max-depth D]`
pub fn run_certify_cli(args: &[String]) {
    let mut path = None;
    let mut cert_out = None;
    let mut max_depth = u32::MAX;
    let mut cache_entries = 1usize << 22;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cert" => cert_out = it.next().cloned(),
            "--max-depth" => max_depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(u32::MAX),
            "--cache-entries" => {
                cache_entries = it.next().and_then(|v| v.parse().ok()).unwrap_or(1 << 22)
            }
            _ => path = Some(a.clone()),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: wordle-opt certify <tree.json> [--cert <out>] [--max-depth D]");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("certify: cannot read {path}: {e}");
        std::process::exit(1);
    });
    let file = crate::policy::ReadableTreeFile::from_json(&text).unwrap_or_else(|e| {
        eprintln!("certify: {e}");
        std::process::exit(1);
    });
    let dict = file.embedded_dictionary().expect("embedded dictionary");
    let tree = file.to_tree().expect("tree");
    let matrix = ResponseMatrix::new(&dict);
    let nodes = tree_nodes(&tree, &matrix);
    println!(
        "certify: candidates={} guesses={} tree cost={} nodes={} max-depth={}",
        dict.candidates.len(),
        dict.guesses.len(),
        nodes[0].cost,
        nodes.len(),
        if max_depth == u32::MAX {
            "all".to_string()
        } else {
            max_depth.to_string()
        }
    );

    crate::certlog::set_enabled(cert_out.is_some());
    let start = Instant::now();
    let pass = refutation_pass(&nodes, &matrix, &dict, max_depth as usize, cache_entries);
    let pass_time = start.elapsed().as_secs_f64();
    crate::certlog::set_enabled(false);
    println!(
        "Refutation pass: {} nodes, {} guesses searched, {} refuted by capacity alone, states {} guesses_evaluated {}",
        pass.nodes,
        pass.tasks,
        pass.skipped_by_capacity,
        pass.states_evaluated,
        pass.guesses_evaluated
    );
    println!("Refutation pass time: {pass_time:.3}s");
    let t = pass.task_secs_by_depth;
    println!(
        "Refutation pass detail: prefilter {:.3}s wall; task time summed over threads by node depth: d0 {:.2}s, d1 {:.2}s, d2 {:.2}s, d3+ {:.2}s",
        pass.prefilter_secs, t[0], t[1], t[2], t[3]
    );
    for f in pass.failures.iter().take(20) {
        println!("REFUTATION FAIL: {f}");
    }
    if !pass.failures.is_empty() {
        println!("Refutation failures: {}", pass.failures.len());
        std::process::exit(1);
    }

    let Some(cert_out) = cert_out else { return };
    {
        let facts = crate::certlog::take_all();
        print_facts(&facts);
        write_certificate(&nodes, &matrix, &dict, facts.bound, max_depth, &cert_out);
    }
}

/// Refutes, with recording on, the guesses an assembly could not refute from
/// the logged facts: the always-output log's gaps (see `write_certificate`).
fn fill_gaps(matrix: &ResponseMatrix, dict: &Dictionary, failed: &[Gap]) -> (usize, usize) {
    let n = matrix.num_candidates;
    let root: Vec<usize> = (0..n).collect();
    let max_k = crate::heuristic::compute_max_branching_factor(matrix, &root);
    let caps = capacity_rows(max_k, n);
    let cache = crate::cache::GlobalCache::new(1 << 20);
    let equiv: crate::solver::EquivCache =
        std::array::from_fn(|_| std::sync::RwLock::new(FxHashMap::default()));
    let metrics = Metrics::new();
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_bits = vec![u64::MAX; num_u64s];
    if !dict.guesses.len().is_multiple_of(64) {
        all_bits[num_u64s - 1] = (1 << (dict.guesses.len() % 64)) - 1;
    }
    let tasks: Vec<(Vec<usize>, u32, usize)> = failed
        .iter()
        .flat_map(|(s, t, gs)| {
            let set: Vec<usize> = s.iter().map(|&c| c as usize).collect();
            gs.iter().map(move |&g| (set.clone(), *t, g as usize))
        })
        .collect();
    crate::certlog::set_enabled(true);
    let still_short = tasks
        .par_iter()
        .with_max_len(1)
        .filter(|(set, t, g)| {
            let mut solver = Solver::new(matrix, max_k, dict, &metrics, &caps, &cache, &equiv);
            solver.min_guess_val(set, &all_bits, *g, *t, 1, max_k) < *t
        })
        .count();
    crate::certlog::set_enabled(false);
    (tasks.len(), still_short)
}

/// Assembles a certificate from `facts` and writes it to `out`.
///
/// An assembly that cannot refute some guess from
/// the facts gets one targeted refutation of exactly that guess (recorded),
/// and is redone, at most a few rounds. The solve's facts have such gaps where
/// its search was cut short by the shared global beta: what it returns there is
/// not all proven, so it is not logged (see `Solver::min_state_val`).
pub fn write_certificate(
    nodes: &[TreeNode],
    matrix: &ResponseMatrix,
    dict: &Dictionary,
    facts: FactMap,
    max_depth: u32,
    out: &str,
) {
    let mut facts = facts;
    {
        let cut = crate::certlog::take_cut();
        println!(
            "Facts set aside (search cut short by the global beta): {} ({} claimed exact)",
            cut.len(),
            cut.iter().filter(|c| c.2).count()
        );
    }
    let assemble_timed = |facts: &FactMap| {
        let start = Instant::now();
        let a = assemble(nodes, matrix, dict, facts, max_depth);
        print_assembly(&a, start.elapsed().as_secs_f64());
        a
    };
    let a = assemble_timed(&facts);
    let a = {
        let mut a = a;
        let mut round = 1;
        while !a.stats.failed.is_empty() && round <= 4 {
            let start = Instant::now();
            let (searches, short) = fill_gaps(matrix, dict, &a.stats.failed);
            let added = crate::certlog::take_all();
            let new_facts = added.bound.len();
            for (s, v) in added.bound {
                let e = facts.entry(s).or_insert(0);
                *e = (*e).max(v);
            }
            println!(
                "Gap fill round {round}: {searches} targeted refutations ({short} not refuted), {new_facts} facts, {:.3}s",
                start.elapsed().as_secs_f64()
            );
            a = assemble_timed(&facts);
            round += 1;
        }
        a
    };
    let bytes = a.to_bytes(dict);
    if let Err(e) = std::fs::write(out, &bytes) {
        eprintln!("certify: cannot write {out}: {e}");
        std::process::exit(1);
    }
    println!("Wrote certificate: {out} ({} bytes)", bytes.len());
}
