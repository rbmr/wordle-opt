# Wordle-Opt Architecture

This document describes the algorithmic techniques `wordle-opt` uses to make
an exhaustive optimal search over the Wordle candidate set tractable, and
where to look in the code for each one. All of these preserve exactness:
the solver always returns the true optimal cost, never an approximation.

## 1. Transposition Table (`src/cache.rs::GlobalCache`)

A subset of candidates can be reached via multiple distinct guess
sequences; the transposition table memoizes results so an already-solved
subtree isn't re-searched. This is **not** a `DashMap` or any lock-based
structure - it's a fixed-size `Vec<AtomicU64>`, each slot packing a 45-bit
Zobrist hash, an 18-bit cost value, and a 1-bit exact/lower-bound flag, read
and written with relaxed atomics and no locking.

`insert()` has a known, deliberately-accepted TOCTOU race: a concurrent
writer's update can occasionally be lost. This does not affect correctness
- alpha-beta search with a transposition table is correct even when entries
are lost, since a lost entry just costs a redundant recompute, not a wrong
answer - so the code accepts the race rather than paying for a
compare_exchange retry loop under contention. See issue #4 for the original
analysis and `src/cache.rs`'s comment on `insert()`.

## 2. Capacity Bounds (`src/heuristic.rs::capacity_bound`)

Rather than a single global bound, the solver computes a per-subtree lower
bound: the minimum possible total cost to solve `n` remaining candidates
given the maximum observed branching factor `k` at that node (an
information-theoretic packing argument - see the doc comment on
`capacity_bound` and its tests for the exact recurrence). A guess whose
lower bound already exceeds the current best cannot possibly improve on it
and is discarded before its subtree is ever expanded.

## 3. Initial Upper Bound (`src/solver.rs::greedy_solve`)

Alpha-beta pruning is only as effective as its initial bound is tight. A
fast, single-threaded greedy pre-pass (always picking the guess that
minimizes expected remaining candidates) runs first to produce a decent
upper bound before the parallel exhaustive search begins, so early cutoffs
in the main search have something real to prune against from the start.

## 4. Move Ordering (`src/heuristic.rs::sort_guesses_by_expected_remaining`)

Guesses are evaluated in ascending order of their expected-remaining-
candidates score (sum of squared bucket sizes over the guess's response
partition). Trying the guess most likely to be strong first tightens beta
earlier, which prunes more of the guesses evaluated afterward.

## 5. Equivalence-Class Guess Pruning

Guesses that partition the current candidate set identically to one
already tried can't produce a different search outcome and are skipped.
Each guess's partition is summarized as a projection over the active
candidate set's letter inventory; guesses producing a projection already
seen at this node are skipped. See `min_state_val` in `src/solver.rs`.

## 6. Parallelism (`rayon`)

Work-stealing parallelism is applied at the root: the first root guess's
response buckets are evaluated in parallel, then the remaining root
guesses are evaluated in parallel against each other, sharing a single
atomic `global_beta` so one thread's progress prunes the others' search
space. Recursion below the root is single-threaded per branch. Bucket
histogramming in the hot loop uses fixed-size stack arrays (`counts[243]`,
etc., since there are at most 243 distinct Wordle responses) rather than
heap allocation.

## 7. Fail-Hard Transposition Cache Lookups

Most search nodes fail high (their cost reaches `beta` before completing),
producing a lower bound rather than an exact value; `GlobalCache` stores
these with `is_exact = false`. On a cache hit, a stored lower bound is only
usable to prune if it's already `>= beta` at the current node - this keeps
lookups sound (never reusing a bound that doesn't apply to a tighter
current constraint) while still avoiding a full re-expansion when it does
apply.

## Verifying claims in this document

Every algorithmic claim here should be checkable against the code it cites
and against `cargo test --release` (the golden regression tests in
`src/solver.rs` pin down exact expected costs for fixed inputs) or
`benchmark_history.md` (deterministic, commit-stamped timing and search-
statistics data - see README.md's Benchmarking section). If a future change
makes a section here inaccurate, fix the section rather than leaving it as
aspirational documentation of what used to be true.
