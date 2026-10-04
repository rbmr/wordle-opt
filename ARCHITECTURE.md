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

## 3a. Root Pre-Filter and Sorting (`src/solver.rs::solve`)

After the first root guess is evaluated exactly (tightening beta), the
remaining root candidates are filtered by their depth-1 capacity-bound
lower bound before spawning parallel tasks. Any root guess `g` with
`lb(g, full_set) >= beta` cannot possibly improve on the current best
and its entire subtree is skipped. `active_guesses` (the full allowed-
guess list passed to sub-problems) is left unchanged; only the list of
root tasks to launch is filtered.

The surviving root candidates are then sorted by their lower bound and
expected-remaining score, so the most promising guesses are dispatched to
`rayon` first and tighten the shared `beta` earlier. NOTE: the intended
benefit (later root tasks pruned quickly once `beta` is tight) is a
hypothesis, not a measured result. A full N=2340 run at this ordering
(maintainer, commit 6dfb74a) solved only 673/14,472 root guesses in 11
minutes, so any speedup it gives at full scale is unproven. An earlier
revision of this section claimed a 69s full run; that claim was false (no
run completed) and has been retracted - see issue #1.

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

## Known Scaling Behavior: depth transitions, not a smooth curve

`benchmark-random` (see README's Benchmarking section) showed avg
cost/candidate scaling smoothly from N=1000 to N=1500 (~3.13 -> ~3.26
guesses/candidate) while wall-clock time jumped ~25x. Investigated with
`diagnose` (see `diagnose_history.md`): the cause is `max_depth` - the
deepest guess-sequence the search has to explore to *prove* optimality -
stepping from 4 to 5 somewhere between N=1100 and N=1200. That single
step, not smooth growth in N, dominates: wall time jumped 5.2x for only
a 1.09x increase in N right at that transition, and the transposition
table's cache-hit rate dropped from ~61-63% to ~46-56% at the same
point.

Why a depth transition costs so much more than sample-count growth
alone would suggest: transposition-table reuse depends on different
guess orderings converging on the *same* candidate subset, and the
number of distinct subsets reachable grows combinatorially with depth -
so reuse gets rarer each level deeper. Cost-per-candidate barely moves
because only a minority of candidates in a given sample actually need
the extra guess, but the search still has to exhaustively rule out a
shallower solution for the *whole* set before it can conclude a deeper
one is needed - and that exhaustive ruling-out is what balloons.

This matters for the N=2340/10-hour goal: expect similar step-function
jumps, not smooth extrapolation, at every depth threshold the full-scale
search crosses (it will almost certainly need depth 5, likely 6, for the
hardest real Wordle answers). Improving cache/transposition-table reuse
at depth, or reducing how often the search re-proves a shallower
solution impossible before searching deeper, looks like a higher-leverage
target than per-node micro-optimization - though this is one investigation
at one point in the search space, not a proven optimization strategy;
treat it as a lead to chase, not a conclusion to build on unverified.

## Verifying claims in this document

Every algorithmic claim here should be checkable against the code it cites
and against `cargo test --release` (the golden regression tests in
`src/solver.rs` pin down exact expected costs for fixed inputs) or
`benchmark_history.md` (deterministic, commit-stamped timing and search-
statistics data - see README.md's Benchmarking section). If a future change
makes a section here inaccurate, fix the section rather than leaving it as
aspirational documentation of what used to be true.

## Equivalence Caching and Mathematical Bounds (Added 2026-09)

To eliminate the `O(G log G)` sorting overhead of finding equivalence classes at every node, `wordle-opt` implements a lock-striped globally shared `EquivCache`. 
- **The Key (`c_mask`)**: The cache uses a 26-bit integer `c_mask` representing the union of all characters present in the remaining candidates.
- **The Projection (`proj`)**: It maps `c_mask` to a deduplicated list of allowed guesses. The deduplication works by filtering out characters in a guess that do not appear in `c_mask`. If two guesses have identical characters at the identical positions for all characters present in `c_mask`, they are guaranteed to produce the exact same response against the current candidate set. This is a mathematically exact mapping.

Furthermore, the solver splits evaluation into **Phase 1** (candidate guesses) and **Phase 2** (non-candidate guesses):
- `valid_max_k` (the maximum branching factor among Phase 1 guesses) restricts the capacity bounds of trees rooted in a Phase 1 guess.
- `local_max_k` (the maximum branching factor across all guesses) bounds Phase 2 guesses. 
- A rigorous `phase2_capacity_bound` is implemented for Phase 2 guesses (since they cannot result in a WIN at depth 1, all candidates are pushed to depth 2 or deeper). The global heuristic lower bound `tight_lb` is perfectly constrained to `min(phase1_capacity_bound, phase2_capacity_bound)`.


## 8. Depth-2 Parallel Alpha-Beta (`src/parallel_depth.rs`)

To mitigate the straggler problem at extreme depths (e.g. `N=2340`) where a single root guess can take over 17 hours to evaluate, the solver pushes work-stealing parallelism (`rayon`) down to the `depth=2` layer. Rather than exclusively evaluating root guesses in parallel, `parallel_depth.rs` distributes the top-level subtrees across cores while sharing a unified global beta limit to maximize hardware utilization and prevent individual hard subtrees from stalling the cluster wall clock.

## 9. GPU Acceleration (CUDA)

For moderately sized nodes (`set.len() > 3`), the CPU's scalar execution of 14,855 guesses takes ~150 microseconds. To drastically reduce this, `wordle-opt` offloads the node histogram counting and capacity bound calculations to the RTX 2060 GPU using a custom CUDA kernel (`src/gpu_kernel.cu`).
- **L1 Cache Tuning**: The CUDA block size is specifically tuned to `128` threads so that the `243 * sizeof(uint16_t)` local `counts` array perfectly fits inside the RTX 2060's 64 KB L1 cache per Streaming Multiprocessor. The intent is to avoid register spilling and VRAM thrashing (not benchmarked in isolation).
- **Blocking Sync**: The driver is configured with `cudaSetDeviceFlags(cudaDeviceScheduleBlockingSync)` so that the CPU threads yield execution back to the OS while waiting for `cudaStreamSynchronize`. The intent is to avoid CPU spin-wait starvation of the `rayon` pool (effect on end-to-end time not verified at full scale). 
