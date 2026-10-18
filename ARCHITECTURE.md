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

To combat severe cache thrashing (the "depth cliff") at large scales where 
frequent deep-node evaluation overwrites valuable shallow-node results, 
`GlobalCache` implements a **Lock-Free Two-Tier Replacement Policy**:
- **Slot 0 (Always-Replace):** Stores the most recently visited node.
- **Slot 1 (Depth-Preferred):** Preserves the harder/deeper subtrees (those with a higher cost value) and protects them from being evicted by shallow exploration.

`insert()` has a known, deliberately-accepted TOCTOU race: a concurrent
writer's update can occasionally be lost. This does not affect correctness
- alpha-beta search with a transposition table is correct even when entries
are lost, since a lost entry just costs a redundant recompute, not a wrong
answer - so the code accepts the race rather than paying for a
compare_exchange retry loop under contention. See the comment on `insert()`
in `src/cache.rs`.

## 2. Capacity Bounds (`src/heuristic.rs::capacity_bound`)

Rather than a single global bound, the solver computes a per-subtree lower
bound on the total cost of solving `n` remaining candidates when no guess
has more than `k` responses on them. A guess that is one of the candidates
spends one response on the win and has at most `k - 1` subtrees; any other
guess solves nothing at its own depth but can have `k`. Counting how many
nodes, and so how many solved candidates, each depth can hold gives the
bound (see `forest_bound` in `src/heuristic.rs` for the argument).

Counting only trees made of candidate guesses is not enough: from
`n = k^2 - k + 3` on, a first guess outside the set can be cheaper, because
it gives up one candidate at depth 1 for a whole extra subtree at depth 2.
The bound accounts for both kinds of guess, and equals the simpler
all-candidates count wherever that count is still a valid bound. The tests
in `src/heuristic.rs` check it against exact minima computed by brute force.

A guess whose lower bound already exceeds the current best cannot possibly
improve on it and is discarded before its subtree is ever expanded.

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
`rayon` first and tighten the shared `beta` earlier.

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
space. Recursion below the root is single-threaded per branch.

The shared bound must never leave a wrong value in the transposition table.
`global_beta` is the cost of a complete tree some thread has already found
(plus one where ties matter), and it only ever decreases.
`current_cost_so_far` is a lower bound on the rest of the tree the current
node belongs to. Every value a search returns is either *proven* for its set
(below the search's own `beta` it is the exact optimum, at or above it a
lower bound) or *cut*: the search stopped early and
`current_cost_so_far + value >= global_beta` held when it returned. A cut
value can be wrong in either direction. Because `global_beta` never
increases, the condition still holds when the caller reads it, and every
caller checks it straight after the call and then treats its own result as
cut as well. `min_state_val` tracks this in `cut_by_global` and does not
store a cut result. Every other write to the table is a capacity bound or a
small-set evaluation, which depend only on the set. So the table only ever
holds proven values.

At the root, a guess that costs no more than the best tree found so far is
never cut. Along its optimal subtree, `current_cost_so_far` plus the optimum
of the current set never exceeds the guess's cost, which is below
`global_beta`, so no comparison that could cut it succeeds. It is therefore
evaluated exactly. `tests/tt_audit.rs` (behind the `tt-audit` feature)
checks every table write made during repeated multi-threaded solves
against a solve of that set alone.

Bucket
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

Solve time does not grow smoothly with the number of candidates. In
measurements with an earlier version, the average cost per candidate grew
smoothly from N=1000 to N=1500 (about 3.13 to 3.26 guesses), while wall-clock
time grew about 25 times. The cause was `max_depth`, the deepest guess sequence
the search has to explore to prove optimality, which stepped from 4 to 5
between N=1100 and N=1200. At that step, wall time grew 5.2 times for a 1.09
times larger N, and the transposition table's hit rate fell from about 61-63%
to about 46-56%.

A depth step costs more than the growth in N suggests because table reuse
depends on different guess orders reaching the same candidate subset, and the
number of distinct subsets grows quickly with depth. The cost per candidate
barely moves, because only a few candidates need the extra guess, but the
search still has to rule out a shallower solution for the whole set before it
can accept a deeper one.

So timings at small N cannot be extrapolated to large N with a smooth curve.
Expect a jump at each depth the search has to cross.

## Verifying claims in this document

Every algorithmic claim here should be checkable against the code it cites and
against `cargo test --release`, whose golden tests in `src/solver.rs` fix the
exact optimal cost for several inputs. If a change makes a section here
inaccurate, update the section.

## Equivalence Caching and Mathematical Bounds

To eliminate the `O(G log G)` sorting overhead of finding equivalence classes at every node, `wordle-opt` implements a lock-striped globally shared `EquivCache`. 
- **The Key (`set_hash`)**: The cache uses a 64-bit Zobrist hash of the exact candidate set (`set_hash`).
- **The Projection (`proj`)**: It maps `set_hash` to a deduplicated list of allowed guesses. The deduplication works by computing a 64-bit Zobrist hash of the actual responses each guess produces against the current candidate set. Two guesses with the same hash are treated as partitioning the set identically. Guesses that give the same response for every candidate always get the same hash. Two guesses with different responses could in principle share a 64-bit hash, which would wrongly drop one of them, but the chance is negligible.
- **Deep Node Bypass**: Small sets (`c_len < 10`) are extremely fast to compute on the fly (~50µs) and are rarely re-searched. To prevent millions of deep nodes from polluting the cache and evicting highly reusable shallow sets (which are critical for MTDF zero-window re-searches), `EquivCache` completely bypasses insertion for small sets.
- **L1 Cache Sizing**: To minimize memory fetch latency during intense node lookups, the individual `FxHashMap` underlying caches dynamically size their mask, tightly bound to a maximum of 32 KB to perfectly reside in the CPU L1 cache.

Furthermore, the solver splits evaluation into **Phase 1** (candidate guesses) and **Phase 2** (non-candidate guesses):
- `valid_max_k` (the maximum branching factor among Phase 1 guesses) restricts the capacity bounds of trees rooted in a Phase 1 guess.
- `local_max_k` (the maximum branching factor across all guesses) bounds Phase 2 guesses. 
- A rigorous `phase2_capacity_bound` is implemented for Phase 2 guesses (since they cannot result in a WIN at depth 1, all candidates are pushed to depth 2 or deeper). The global heuristic lower bound `tight_lb` is perfectly constrained to `min(phase1_capacity_bound, phase2_capacity_bound)`.
- **Bitwise Phase Classification**: To rapidly separate Phase 1 and Phase 2 guesses inside the hot loop without branching, a thread-local `is_in_set` bitset with one bit per guess in the dictionary. By taking the bitwise AND of the `active_guesses_slice` bitset with `is_in_set[block_idx]` and `!is_in_set[block_idx]`, guesses are perfectly partitioned into `phase1_guesses` and `phase2_guesses` using only bitwise operations and `trailing_zeros()`, completely eliminating the per-guess `if is_in_set[g]` conditional branch.


## 8. Depth-2 Parallel Alpha-Beta (`src/parallel_depth.rs`)

A few root guesses can take much longer to evaluate than the rest, which leaves cores idle at the end of a parallel root scan. To reduce this, the solver also parallelizes at depth 2 with `rayon`: `parallel_depth.rs` distributes the subtrees below the root across cores, sharing one global beta, so a single slow subtree does not hold up the whole solve.

## 9. GPU Acceleration Evaluation and Removal

An RTX 2060 GPU offload path (`gpu_kernel.cu`, `gpu.rs`) was evaluated to accelerate node histogram counting and capacity bound calculations. On 2026-10-15, a direct comparison on a 16-core machine (commit `0ba60c3`) across identical candidate sets showed negligible wall-time differences (CPU was 2.3% faster at N=1000; difference was 2.4% / 0.67s at N=1600 within measurement noise) while the CPU path consumed 40% less memory (163MB vs 269MB).

In policy tree construction, candidate sets after the opener partition rapidly into small buckets (size $\le 50$), well below the threshold where GPU offload kernel launches break even with cache-local CPU execution. Consequently, the GPU path was removed completely in favor of pure CPU SIMD/Rayon parallelism, eliminating external toolchain dependencies (`nvcc`, `g++-13`, `cc`), build scripts, and FFI maintenance overhead.
 
