"use strict";
// Tests for site/policy-core.js - the pure viewer logic. Run with:
//
//   node site/policy-core.test.js
//
// Two layers:
//
//   1. `response` fixtures mirroring the Rust tests in src/core.rs, so the JS
//      rule cannot drift from the one the trees were built with.
//   2. End-to-end validation of every bundled example tree: validateTree must
//      accept it, and the win rule must credit exactly one win per candidate.
//      The examples contain nodes whose guess has been eliminated from the
//      initial candidate list, so a "guess is in the initial candidate list"
//      shortcut produces phantom wins here and fails the count - that is the
//      regression this file exists to catch.

const fs = require("fs");
const path = require("path");
require(path.join(__dirname, "policy-core.js"));
const { response, validateTree, computeNodeStats } = globalThis.PolicyCore;

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${name}: ${e.message}`);
  }
}
function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}
function loadExample(name) {
  return JSON.parse(fs.readFileSync(path.join(__dirname, "examples", `${name}.json`), "utf8"));
}

// --- response fixtures (src/core.rs) ---
const FIXTURES = [
  ["crane", "crane", "ggggg"], // win
  ["crane", "stomp", "bbbbb"], // all black
  ["abcde", "eabcd", "yyyyy"], // all yellow
  ["crane", "crate", "gggbg"], // mixed
  ["abcde", "bbfff", "bgbbb"], // duplicate capped by green
  ["mxcde", "xqxrs", "ybbbb"], // duplicate, only first occurrence yellow
];
check("response matches the Rust rule", () => {
  for (const [secret, guess, want] of FIXTURES) {
    const got = response(secret, guess);
    assert(got === want, `${secret}/${guess}: got ${got}, want ${want}`);
  }
});

// --- bundled examples ---
const EXAMPLES = ["optimal", "min-remaining", "max-freq", "letters3", "letters4", "letters6"];
for (const name of EXAMPLES) {
  check(`${name}: validates (edge-iff-possible, leaves are wins, every candidate terminates)`, () => {
    const file = loadExample(name);
    const report = validateTree(file);
    assert(report.ok, report.errors[0] || "validation failed");
    assert(
      report.stats.wins === file.candidates.length,
      `${report.stats.wins} wins for ${file.candidates.length} candidates`
    );
  });

  check(`${name}: the win rule credits exactly one win per candidate`, () => {
    const file = loadExample(name);
    let winningNodes = 0;
    for (const st of computeNodeStats(file).values()) if (st.hasWin) winningNodes++;
    assert(
      winningNodes === file.candidates.length,
      `${winningNodes} winning nodes for ${file.candidates.length} candidates`
    );
  });
}

// The naive "guess is in the initial candidate list" shortcut must be visibly
// wrong on the bundled policy trees, otherwise these fixtures could not catch a
// regression to it. Count the nodes where it would draw a win that cannot
// happen (guess eliminated along the path) and where it would miss a real one.
check("policy examples contain nodes that distinguish the naive shortcut", () => {
  let phantom = 0;
  let missed = 0;
  for (const name of ["optimal", "min-remaining", "max-freq"]) {
    const file = loadExample(name);
    const initial = new Set(file.candidates);
    for (const [node, st] of computeNodeStats(file)) {
      const naive = initial.has(node.guess);
      if (naive && !st.hasWin) phantom++;
      if (!naive && st.hasWin) missed++;
    }
  }
  assert(phantom > 0, "no node where the naive shortcut invents a win");
  console.log(`     (naive shortcut: ${phantom} phantom wins, ${missed} missed wins)`);
});

console.log(`\n${failures === 0 ? "all viewer tests passed" : `${failures} test(s) failed`}`);
process.exit(failures === 0 ? 0 : 1);
