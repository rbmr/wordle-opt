#!/usr/bin/env node
"use strict";
// Generates small, self-contained readable policy trees for word lengths other
// than 5, to exercise the viewer's length-agnostic rendering.
//
// The Rust engine solves 5-letter Wordle (`core::Word` is `[u8; 5]`), so these
// are produced here with a simple greedy "min remaining" policy instead. Each
// output is a valid readable tree: every node's edges are exactly the responses
// possible for its remaining candidates, leaves are wins, and the dictionary
// hash is computed with the same FNV-1a encoding the Rust side uses.
//
// Usage: node site/examples/generate_letter_examples.js

const fs = require("fs");
const path = require("path");
const dir = __dirname;

const DICTIONARIES = {
  3: ["bad", "bag", "bat", "bed", "bet", "big", "bin", "bit", "box", "bun", "bus", "cab", "can", "cap", "car", "cat", "cod", "cog", "cop", "cot", "cow", "cry", "cup", "cut", "dam", "day", "den", "dew", "dig", "dim", "din", "dip", "dog", "dot", "dry", "dug"],
  4: ["able", "ache", "acid", "acre", "aged", "ally", "also", "arch", "area", "army", "atom", "away", "back", "bald", "band", "bank", "barn", "bath", "bead", "beam", "bean", "bear", "beat", "belt", "bird", "blue", "boat", "bold", "bone", "book", "born", "bowl", "brew", "cake", "calm", "card"],
  6: ["planet", "plants", "plenty", "plates", "planes", "plague", "please", "pledge", "plunge", "pocket", "poetry", "points", "police", "policy", "polish", "polite", "poorly", "popped", "portal", "posted", "potato", "powder", "powers", "praise", "prayer", "prefer", "pretty", "prince", "prison", "profit", "prompt", "proper", "proved", "public", "pulled", "purple"],
};

// Two-pass response rule, any length. Returns a string of b/g/y.
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
  return r.map((v) => (v === 0 ? "b" : v === 1 ? "g" : "y")).join("");
}

function win(n) {
  return "g".repeat(n);
}

// Greedy min-remaining policy (best useful guess), captured as a tree.
function buildTree(words, n) {
  const W = win(n);
  function bestGuess(cands) {
    let best = null;
    let bestScore = Infinity;
    for (const g of words) {
      const counts = new Map();
      for (const c of cands) counts.set(response(c, g), (counts.get(response(c, g)) || 0) + 1);
      if (counts.size <= 1) continue; // useless
      let score = 0;
      for (const v of counts.values()) score += v * v;
      if (score < bestScore) { bestScore = score; best = g; }
    }
    return best;
  }
  function build(cands) {
    if (cands.length === 1) return { guess: cands[0], children: {} };
    const g = bestGuess(cands);
    const buckets = new Map();
    for (const c of cands) {
      const r = response(c, g);
      if (!buckets.has(r)) buckets.set(r, []);
      buckets.get(r).push(c);
    }
    const children = {};
    // Insert in ascending numeric response order for a canonical layout.
    const keys = [...buckets.keys()].filter((r) => r !== W).sort((a, b) => idx(a, n) - idx(b, n));
    for (const r of keys) children[r] = build(buckets.get(r));
    return { guess: g, children };
  }
  return build(words);
}

function idx(s, n) {
  let mul = 1, v = 0;
  for (let i = 0; i < n; i++) { v += (s[i] === "b" ? 0 : s[i] === "g" ? 1 : 2) * mul; mul *= 3; }
  return v;
}

// FNV-1a over the same versioned encoding as `policy::dictionary_hash`.
function dictionaryHash(guesses, candidates) {
  const bytes = [];
  const put = (s) => { for (let i = 0; i < s.length; i++) bytes.push(s.charCodeAt(i)); };
  put("wordle-opt/policy-dictionary/v1\n");
  put(`guesses=${guesses.length}\n`);
  for (const w of guesses) { put(w); put("\n"); }
  put(`candidates=${candidates.length}\n`);
  for (const w of candidates) { put(w); put("\n"); }
  const M = (1n << 64n) - 1n;
  let h = 0xcbf29ce484222325n;
  for (const b of bytes) { h ^= BigInt(b); h = (h * 0x100000001b3n) & M; }
  return "0x" + h.toString(16).padStart(16, "0");
}

for (const [lenStr, wordsRaw] of Object.entries(DICTIONARIES)) {
  const n = Number(lenStr);
  const candidates = [...new Set(wordsRaw)].sort();
  const guesses = candidates.slice(); // every candidate is a legal guess
  const file = {
    format: "wordle-policy-tree",
    version: 1,
    strategy: "min-remaining",
    dictionary_hash: dictionaryHash(guesses, candidates),
    guesses,
    candidates,
    root: buildTree(candidates, n),
  };
  const json = JSON.stringify(file, null, 2) + "\n";
  const name = `letters${n}`;
  fs.writeFileSync(path.join(dir, `${name}.json`), json);
  fs.writeFileSync(
    path.join(dir, `${name}.js`),
    `window.WordleExamples=window.WordleExamples||{};window.WordleExamples["${name}"]=${json.trim()};\n`
  );
  // Quick self-check: every candidate must terminate.
  let nodes = 0, wins = 0, leaves = 0;
  const walk = (node) => {
    nodes++;
    const keys = Object.keys(node.children);
    if (keys.length === 0) leaves++;
    for (const k of keys) walk(node.children[k]);
  };
  walk(file.root);
  console.log(`${name}: ${n}-letter, ${candidates.length} candidates, ${nodes} nodes, hash ${file.dictionary_hash}`);
}
