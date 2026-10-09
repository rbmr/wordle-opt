"use strict";
// Pure policy-tree logic for the viewer: response computation, candidate
// filtering, validation, and per-node stats. Kept free of DOM access so it can
// be exercised under Node (`node site/policy-core.test.js`); the browser loads
// it with a plain <script> before app.js and reads it as `PolicyCore`.
//
// The win rule is the subtle part. A node wins for the candidate equal to its
// own guess, and only while that candidate is still reachable at the node:
// children hold the *non-win* response buckets, so the all-green response is
// never an edge, and a node wins iff the all-green response is possible for
// some remaining candidate, i.e. iff the node's guess is in the node's
// recomputed candidate set. "The guess is in the *initial* candidate list" is
// not the same test - a guess can have been eliminated along the path - and
// using it draws phantom wins (the test file pins this down).
//
// Candidate sets are not stored in the tree at all: they are recomputed
// top-down by filtering the parent's set with `response(candidate, guess)`.
// That is the same rule the Rust validator enforces (src/policy.rs,
// src/core.rs), which is why a tree the viewer accepts is one `wordle-opt
// validate` accepts too.
(function (root) {
  // The response/win string length is derived from the tree's words, so the
  // viewer is not tied to 5 letters. `n` is the word length.
  function winString(n) {
    return "g".repeat(n);
  }

  // --- response computation (same two-pass rule as Rust `Response::compute`,
  // generalised to any word length) ---
  function response(secret, guess) {
    const n = secret.length;
    const r = new Array(n).fill(0);
    const used = new Array(n).fill(false);
    for (let i = 0; i < n; i++) if (guess[i] === secret[i]) { r[i] = 1; used[i] = true; }
    for (let i = 0; i < n; i++) {
      if (r[i] === 1) continue;
      for (let j = 0; j < n; j++) {
        if (guess[i] === secret[j] && !used[j]) { r[i] = 2; used[j] = true; break; }
      }
    }
    let s = "";
    for (let i = 0; i < n; i++) s += r[i] === 0 ? "b" : r[i] === 1 ? "g" : "y";
    return s;
  }

  // Numeric ordering of a response string (little-endian base 3), matching the
  // tree's edge order. Length-agnostic.
  function responseIndex(s) {
    let mul = 1, v = 0;
    for (let i = 0; i < s.length; i++) {
      v += (s[i] === "b" ? 0 : s[i] === "g" ? 1 : 2) * mul;
      mul *= 3;
    }
    return v;
  }

  function wordLength(file) {
    const sample = (file.candidates && file.candidates[0]) || (file.guesses && file.guesses[0]) || file.root.guess;
    return sample.length;
  }

  function candidatesMap(file, node, candidates) {
    const m = new Map();
    for (const c of candidates) {
      const r = response(file.candidates[c], node.guess);
      if (!m.has(r)) m.set(r, []);
      m.get(r).push(c);
    }
    return m;
  }

  // Expected guesses remaining from each node, computed from the fully
  // determined subtree: T(C) = |C| + sum over non-win children T(C_r), so the
  // expected number of further guesses starting at a node is T(C)/|C| (this
  // includes the node's own guess, and correctly credits the candidate the guess
  // itself wins). This is the meaningful per-node metric - the raw number of
  // out-edges understates it whenever the guess is also a candidate.
  function computeNodeStats(file) {
    const stats = new Map();
    const WIN = winString(wordLength(file));
    function visit(node, candidates) {
      const map = candidatesMap(file, node, candidates);
      let total = candidates.length; // the node's own guess, once per candidate
      for (const [r, cs] of map) {
        if (r === WIN) continue;
        const child = node.children[r];
        if (child) total += visit(child, cs);
      }
      // `hasWin` is true when the node's own guess is one of its candidates, i.e.
      // the all-green response is possible here.
      stats.set(node, { n: candidates.length, total, exp: total / candidates.length, hasWin: map.has(WIN) });
      return total;
    }
    visit(file.root, file.candidates.map((_, i) => i));
    return stats;
  }

  // --- validation (shared by both views; a tree is rejected on load if invalid) ---
  function validateTree(file) {
    const errors = [];
    const WIN = winString(wordLength(file));
    const guesses = new Set(file.guesses);
    const nCandidates = file.candidates.length;
    let nodes = 0, edges = 0, leaves = 0, maxDepth = 0, wins = 0, totalCost = 0;
    const depthHistogram = [];
    const stack = [[file.root, 0, file.candidates.map((_, i) => i)]];

    while (stack.length) {
      const [node, depth, candidates] = stack.pop();
      nodes++;
      edges += Object.keys(node.children).length;
      maxDepth = Math.max(maxDepth, depth);
      if (!guesses.has(node.guess)) { errors.push(`depth ${depth}: guess "${node.guess}" is not a valid guess`); continue; }
      if (candidates.length === 0) { errors.push(`depth ${depth}: empty candidate set`); continue; }

      const buckets = new Map();
      for (const c of candidates) {
        const r = response(file.candidates[c], node.guess);
        if (!buckets.has(r)) buckets.set(r, []);
        buckets.get(r).push(c);
      }
      const possible = [...buckets.keys()].filter((r) => r !== WIN).sort();
      const actual = Object.keys(node.children).sort();
      if (possible.length !== actual.length || possible.some((r, i) => r !== actual[i])) {
        const missing = possible.filter((r) => !actual.includes(r));
        const extra = actual.filter((r) => !possible.includes(r));
        errors.push(`depth ${depth} (guess "${node.guess}", ${candidates.length} candidates): missing [${missing}], impossible/duplicate [${extra}]`);
        if (errors.length > 25) break;
      }

      const winsHere = (buckets.get(WIN) || []).length;
      wins += winsHere;
      if (winsHere > 0) {
        totalCost += winsHere * (depth + 1);
        depthHistogram[depth] = (depthHistogram[depth] || 0) + winsHere;
      }
      const childKeys = Object.keys(node.children);
      if (childKeys.length === 0) {
        leaves++;
        if (candidates.length !== 1 || winsHere !== 1) errors.push(`depth ${depth}: leaf is not a win`);
        continue;
      }
      if (candidates.length === 1) { errors.push(`depth ${depth}: node has edges but one candidate remains`); continue; }
      for (const r of childKeys) stack.push([node.children[r], depth + 1, buckets.get(r) || []]);
    }
    if (wins !== nCandidates) errors.push(`${wins} wins but ${nCandidates} candidates - not every candidate terminates`);

    return {
      ok: errors.length === 0,
      errors,
      stats: {
        nodes, edges, leaves, maxDepth, wins, totalCost,
        meanGuesses: nCandidates ? totalCost / nCandidates : 0,
        candidates: nCandidates, guesses: file.guesses.length, depthHistogram,
      },
    };
  }

  root.PolicyCore = {
    winString,
    response,
    responseIndex,
    wordLength,
    candidatesMap,
    computeNodeStats,
    validateTree,
  };
})(typeof window !== "undefined" ? window : globalThis);
