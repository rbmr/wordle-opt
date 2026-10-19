//! The certificate checker (`certcheck`) against brute force.
//!
//! For small instances a brute-force oracle knows the optimum of every
//! candidate subset, so this file writes complete certificates from the
//! oracle's values alone (no solver involved) and requires the checker to
//! accept them, then corrupts them and requires it to reject every
//! corruption: a bound raised by one, a certificate computed as if a guess
//! were missing, a capacity claim above what the rule allows, a certificate
//! for other word lists. Each rejected corruption is a false statement the
//! checker refused, which is the property the checker exists for.

mod common;

use common::*;
use std::collections::HashMap;
use wordle_opt::certcheck::check;
use wordle_opt::certfile::{self, RULE_ASSUMED, RULE_BASE, RULE_CAPACITY, RULE_EXPANSION, l3};

fn tree_json(inst: &Instance) -> String {
    solve(inst).tree.to_json()
}

/// Exhaustive optimum of every subset (bitmask) over the guesses in `allowed`;
/// unlike `common::Oracle` it can leave guesses out.
struct GuessOracle {
    /// rows[g][c] = feedback of guess g on candidate c
    rows: Vec<Vec<u16>>,
    allowed: Vec<bool>,
    memo: HashMap<u64, u32>,
}

impl GuessOracle {
    fn new(inst: &Instance, allowed: Vec<bool>) -> Self {
        assert!(inst.candidates.len() <= 64);
        let rows = inst
            .guesses
            .iter()
            .map(|g| inst.candidates.iter().map(|c| feedback(g, c)).collect())
            .collect();
        Self {
            rows,
            allowed,
            memo: HashMap::new(),
        }
    }

    fn buckets(&self, g: usize, mask: u64) -> Vec<(u16, u64)> {
        let mut by: Vec<(u16, u64)> = Vec::new();
        for c in 0..64 {
            if mask >> c & 1 == 1 {
                let r = self.rows[g][c];
                match by.iter_mut().find(|(rr, _)| *rr == r) {
                    Some(e) => e.1 |= 1 << c,
                    None => by.push((r, 1 << c)),
                }
            }
        }
        by
    }

    fn value(&mut self, mask: u64) -> u32 {
        let n = mask.count_ones();
        if n <= 2 {
            return [0, 1, 3][n as usize];
        }
        if let Some(&v) = self.memo.get(&mask) {
            return v;
        }
        let mut best = u32::MAX;
        for g in 0..self.rows.len() {
            if !self.allowed[g] {
                continue;
            }
            let b = self.buckets(g, mask);
            if b.len() == 1 && b[0].0 != WIN {
                continue;
            }
            let mut cost = n;
            for (r, m) in b {
                if r != WIN {
                    cost += self.value(m);
                    if cost >= best {
                        break;
                    }
                }
            }
            best = best.min(cost);
        }
        self.memo.insert(mask, best);
        best
    }

    fn max_branching(&self, mask: u64) -> usize {
        (0..self.rows.len())
            .map(|g| self.buckets(g, mask).len())
            .max()
            .unwrap()
    }
}

#[derive(Clone)]
struct Claim {
    /// None = root, else (parent, guess, response)
    def: Option<(u32, u32, u8)>,
    bound: u32,
    rule: u8,
}

/// A complete certificate built from `oracle` values: every claim at the
/// set's optimum over the oracle's allowed guesses.
fn oracle_certificate(oracle: &mut GuessOracle, n: usize) -> Vec<Claim> {
    let full = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
    let mut claims: Vec<Claim> = Vec::new();
    let mut index: HashMap<u64, usize> = HashMap::new();
    let mut queue = vec![(full, None)];
    while let Some((mask, def)) = queue.pop() {
        if index.contains_key(&mask) {
            continue;
        }
        let id = claims.len();
        index.insert(mask, id);
        let size = mask.count_ones() as usize;
        let v = oracle.value(mask);
        let k = oracle.max_branching(mask);
        let rule = if size <= 2 {
            RULE_BASE
        } else if v as u64 <= l3(size, k) {
            RULE_CAPACITY
        } else {
            RULE_EXPANSION
        };
        claims.push(Claim {
            def,
            bound: v,
            rule,
        });
        if rule != RULE_EXPANSION {
            continue;
        }
        for g in 0..oracle.rows.len() {
            for (r, m) in oracle.buckets(g, mask) {
                let bs = m.count_ones() as usize;
                if r == WIN || m == mask || bs <= 2 {
                    continue;
                }
                if oracle.value(m) as u64 > l3(bs, k) && !index.contains_key(&m) {
                    queue.insert(0, (m, Some((id as u32, g as u32, r as u8))));
                }
            }
        }
    }
    claims
}

fn encode(inst: &Instance, claims: &[Claim]) -> Vec<u8> {
    let mut out = certfile::MAGIC.to_vec();
    out.extend_from_slice(&certfile::digest(&inst.guesses, &inst.candidates).to_le_bytes());
    out.extend_from_slice(&(inst.guesses.len() as u32).to_le_bytes());
    out.extend_from_slice(&(inst.candidates.len() as u32).to_le_bytes());
    out.extend_from_slice(&(claims.len() as u32).to_le_bytes());
    for c in claims {
        match c.def {
            None => out.push(0),
            Some((p, g, r)) => {
                out.push(1);
                out.extend_from_slice(&p.to_le_bytes());
                out.extend_from_slice(&g.to_le_bytes());
                out.push(r);
            }
        }
        out.extend_from_slice(&c.bound.to_le_bytes());
        out.push(c.rule);
    }
    out
}

fn instances() -> Vec<(String, Instance)> {
    let mut v = Vec::new();
    for s in 0..6 {
        v.push((
            format!("bundled seed {s}"),
            bundled_subset(100 + s, 10 + s as usize, 300),
        ));
    }
    for s in 0..6 {
        v.push((
            format!("small alphabet seed {s}"),
            small_alphabet(200 + s, 3 + (s % 3) as usize, 10 + s as usize, 40),
        ));
    }
    v
}

#[test]
fn oracle_certificates_check_and_match_the_solver() {
    for (label, inst) in instances() {
        let n = inst.candidates.len();
        let mut oracle = GuessOracle::new(&inst, vec![true; inst.guesses.len()]);
        let claims = oracle_certificate(&mut oracle, n);
        let json = tree_json(&inst);
        let report = check(&json, &encode(&inst, &claims)).expect(&label);
        assert!(
            report.proves_optimal(),
            "{label}: checker rejected a true certificate: {:?}",
            report.failures
        );
        assert_eq!(
            report.tree_cost, claims[0].bound as u64,
            "{label}: solver tree cost differs from the brute-force optimum"
        );
    }
}

#[test]
fn l3_never_exceeds_the_brute_force_optimum() {
    for (label, inst) in instances() {
        let mut oracle = GuessOracle::new(&inst, vec![true; inst.guesses.len()]);
        let n = inst.candidates.len();
        oracle.value((1u64 << n) - 1);
        let masks: Vec<u64> = oracle.memo.keys().copied().collect();
        for m in masks {
            let v = oracle.value(m) as u64;
            let k = oracle.max_branching(m);
            assert!(
                l3(m.count_ones() as usize, k) <= v,
                "{label}: l3 above the optimum for mask {m:#x}"
            );
        }
    }
}

#[test]
fn raised_bounds_are_rejected() {
    for (label, inst) in instances() {
        let n = inst.candidates.len();
        let mut oracle = GuessOracle::new(&inst, vec![true; inst.guesses.len()]);
        let claims = oracle_certificate(&mut oracle, n);
        let json = tree_json(&inst);
        // Every claim is exactly at its set's optimum, so one more is false.
        for i in 0..claims.len() {
            let mut bad = claims.clone();
            bad[i].bound += 1;
            let r = check(&json, &encode(&inst, &bad)).unwrap();
            assert!(
                !r.failures.is_empty(),
                "{label}: accepted claim {i} raised above the optimum"
            );
        }
        // The same false claim is still rejected if cited under any rule.
        for rule in [RULE_BASE, RULE_CAPACITY, RULE_EXPANSION] {
            let mut bad = claims.clone();
            bad[0].bound += 1;
            bad[0].rule = rule;
            let r = check(&json, &encode(&inst, &bad)).unwrap();
            assert!(!r.failures.is_empty(), "{label}: rule {rule} accepted");
        }
        // Assumed claims are reported, never counted as a proof.
        let mut assumed = claims.clone();
        assumed[0].rule = RULE_ASSUMED;
        let r = check(&json, &encode(&inst, &assumed)).unwrap();
        assert!(
            !r.proves_optimal(),
            "{label}: an assumed root proved optimality"
        );
    }
}

#[test]
fn certificate_ignoring_a_guess_is_rejected() {
    let mut tested = 0;
    // Tiny guess lists, so that some single guess is indispensable.
    let bundled = Instance::bundled();
    let few = (0..200u64).map(|s| {
        let n = 8 + s as usize % 5;
        let inst = match s % 3 {
            0 => Instance::new(Vec::new(), Rng::new(300 + s).sample(&bundled.candidates, n)),
            1 => small_alphabet(400 + s, 3, n, 0),
            _ => small_alphabet(500 + s, 4, n, 2),
        };
        (format!("few guesses seed {s}"), inst)
    });
    for (label, inst) in few {
        if tested == 6 {
            break;
        }
        let n = inst.candidates.len();
        let full = (1u64 << n) - 1;
        let opt = GuessOracle::new(&inst, vec![true; inst.guesses.len()]).value(full);
        // A guess whose removal raises the optimum: a prover that forgot it
        // would claim a bound that is false for the real guess list.
        let found = (0..inst.guesses.len()).find_map(|g| {
            let mut allowed = vec![true; inst.guesses.len()];
            allowed[g] = false;
            let mut o = GuessOracle::new(&inst, allowed);
            (o.value(full) > opt).then_some(o)
        });
        let Some(mut without) = found else { continue };
        tested += 1;
        let claims = oracle_certificate(&mut without, n);
        let r = check(&tree_json(&inst), &encode(&inst, &claims)).unwrap();
        assert!(
            !r.failures.is_empty(),
            "{label}: accepted a certificate that ignores a useful guess"
        );
    }
    assert!(tested >= 3, "too few instances with an indispensable guess");
}

#[test]
fn malformed_certificates_are_rejected() {
    let (_, inst) = instances().remove(0);
    let n = inst.candidates.len();
    let mut oracle = GuessOracle::new(&inst, vec![true; inst.guesses.len()]);
    let claims = oracle_certificate(&mut oracle, n);
    let json = tree_json(&inst);
    let good = encode(&inst, &claims);
    assert!(check(&json, &good[..good.len() - 1]).is_err(), "truncated");
    let mut extra = good.clone();
    extra.push(0);
    assert!(check(&json, &extra).is_err(), "trailing bytes");
    let mut wrong_lists = good.clone();
    wrong_lists[8] ^= 1;
    assert!(check(&json, &wrong_lists).is_err(), "other word lists");
    // A child defined through a later claim is not a valid definition.
    if claims.len() > 1 {
        let mut fwd = claims.clone();
        if let Some((_, g, r)) = fwd[1].def {
            fwd[1].def = Some((5, g, r));
            assert!(
                check(&json, &encode(&inst, &fwd)).is_err(),
                "forward parent"
            );
        }
    }
    // The first claim must cover every candidate.
    let mut no_root = claims.clone();
    no_root.remove(0);
    let r = check(&json, &encode(&inst, &no_root));
    assert!(
        r.is_err() || !r.unwrap().failures.is_empty(),
        "missing root claim"
    );
}

/// End to end with the real generator: refutation pass, assembly, check.
#[test]
fn generated_certificates_check() {
    use wordle_opt::cert;
    use wordle_opt::matrix::ResponseMatrix;
    for (label, inst) in instances() {
        let json = tree_json(&inst);
        let file = wordle_opt::policy::ReadableTreeFile::from_json(&json).unwrap();
        let dict = file.embedded_dictionary().unwrap();
        let matrix = ResponseMatrix::new(&dict);
        let tree = file.to_tree().unwrap();
        let nodes = cert::tree_nodes(&tree, &matrix);
        wordle_opt::certlog::set_enabled(true);
        let pass = cert::refutation_pass(&nodes, &matrix, &dict, usize::MAX, 1 << 16);
        wordle_opt::certlog::set_enabled(false);
        assert!(pass.failures.is_empty(), "{label}: {:?}", pass.failures);
        let facts = wordle_opt::certlog::take_all();
        let a = cert::assemble(&nodes, &matrix, &dict, &facts.bound, u32::MAX);
        assert!(
            a.stats.failures.is_empty(),
            "{label}: {:?}",
            a.stats.failures
        );
        let r = check(&json, &a.to_bytes(&dict)).unwrap();
        assert!(r.proves_optimal(), "{label}: {:?}", r.failures);
    }
}

/// Soundness under random corruption: whatever a corrupted certificate says,
/// if the checker accepts it outright then every one of its claims is true by
/// brute force. Mutations raise or lower bounds, swap rules, and redirect set
/// definitions to other parents, guesses and responses.
#[test]
fn accepted_mutants_only_make_true_claims() {
    let mut rng = Rng::new(7);
    let mut accepted = 0;
    let mut rejected = 0;
    for (label, inst) in instances() {
        let n = inst.candidates.len();
        let mut oracle = GuessOracle::new(&inst, vec![true; inst.guesses.len()]);
        let claims = oracle_certificate(&mut oracle, n);
        let json = tree_json(&inst);
        for _ in 0..60 {
            let mut bad = claims.clone();
            for _ in 0..1 + rng.below(3) {
                let i = rng.below(bad.len());
                match rng.below(4) {
                    0 => bad[i].bound += 1 + rng.below(3) as u32,
                    1 => bad[i].bound = bad[i].bound.saturating_sub(1 + rng.below(3) as u32),
                    2 => bad[i].rule = [RULE_BASE, RULE_CAPACITY, RULE_EXPANSION][rng.below(3)],
                    _ => {
                        if i > 0 {
                            let parent = rng.below(i) as u32;
                            let guess = rng.below(inst.guesses.len()) as u32;
                            let response = rng.below(243) as u8;
                            bad[i].def = Some((parent, guess, response));
                        }
                    }
                }
            }
            let Ok(report) = check(&json, &encode(&inst, &bad)) else {
                rejected += 1;
                continue;
            };
            if !report.proves_optimal() {
                rejected += 1;
                continue;
            }
            accepted += 1;
            // Rebuild every claim's set and require its bound to be true.
            let full = (1u64 << n) - 1;
            let mut sets: Vec<u64> = Vec::new();
            for (i, c) in bad.iter().enumerate() {
                let set = match c.def {
                    None => full,
                    Some((p, g, r)) => oracle
                        .buckets(g as usize, sets[p as usize])
                        .into_iter()
                        .find(|&(rr, _)| rr == r as u16)
                        .map_or(0, |(_, m)| m),
                };
                assert_ne!(set, 0, "{label}: accepted an empty set in claim {i}");
                assert!(
                    c.bound <= oracle.value(set),
                    "{label}: accepted claim {i} with bound {} above the optimum {}",
                    c.bound,
                    oracle.value(set)
                );
                sets.push(set);
            }
        }
    }
    assert!(
        accepted > 0 && rejected > 0,
        "mutations exercised only one outcome"
    );
}
