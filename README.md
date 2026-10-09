<!--
  This file is for humans: people using the CLI, people using the visualizer,
  or people wanting to understand the project. Agent-facing process rules,
  compute-host operations and the daily-run discipline live in AGENTS.md -
  please add them there, not here.
-->

# Wordle-Opt

A Rust engine that computes the mathematically optimal guessing strategy for
Wordle: the strategy that minimizes the total number of guesses needed to solve
every candidate word, found by exhaustive branch-and-bound search rather than
heuristics.

It has solved the full 2340-word candidate set exactly (total cost **8001**,
mean 3.419 guesses per game), and the complete optimal policy tree is bundled
with the interactive viewer - so you can inspect the optimal strategy guess by
guess, or play against it.

## How it works

Brute-forcing the full search tree over the ~2340-word candidate set is
computationally infeasible, so the solver (`src/solver.rs`) prunes aggressively
while guaranteeing the final answer is still exactly optimal:

- **Alpha-beta pruning** over the guess/response tree, seeded with a tight
  initial upper bound from a fast single-threaded greedy pre-pass
  (`greedy_solve`), so early cutoffs are effective from the start.
- **Capacity lower bounds** (`heuristic::capacity_bound`): an
  information-theoretic minimum cost for solving `n` remaining candidates given
  a branching factor `k`, used to discard guesses that provably cannot beat the
  current best.
- **Equivalence-class guess pruning**: guesses that partition the current
  candidate set identically to a guess already tried are skipped, since they
  can't produce a different outcome. The projection per `c_mask` is memoized in
  an `FxHashMap` cache, dynamically sized to fit tightly within the 32 KB L1
  cache.
- **A lock-free transposition table** (`src/cache.rs::GlobalCache`): a
  fixed-size array of `AtomicU64` slots, each packing a 45-bit Zobrist hash, an
  18-bit cost value, and an exact/lower-bound flag, shared across threads
  without locking. It is a best-effort cache (a documented, accepted race can
  occasionally lose an update - see issue #4); the correctness of the
  alpha-beta search does not depend on it.
- **Parallelism via `rayon`**: root guesses are evaluated in parallel against a
  shared atomic beta, so threads prune against each other's progress. Root
  candidates are sorted by their lower bounds first, so promising guesses
  tighten the shared beta earlier.
- **CUDA GPU acceleration**: capacity-bound work and phase filtering are
  offloaded to an RTX 2060 (`gpu_kernel.cu`), with L1-tuned kernels and
  spin-waiting stream syncs.

The reasoning behind each of these, the scaling behaviour at large N, and the
memory layout are in `ARCHITECTURE.md`.

## Status

The engine solved the full 2340-candidate set exactly: total cost **8001**,
mean **3.419** guesses per game. The cost-only run took 5.19 hours; the
complete **optimal policy tree** - 2478 nodes, maximum depth 5 - took 4.28
hours to build, and is what the viewer renders by default.

The optimal strategy is fixed; the remaining work is speed, i.e. reaching the
same exact result in less wall-clock time.

## Usage

```bash
# Build a policy tree (see "Policy trees" below)
cargo run --release -- solve --strategy optimal --output tree.json

# A smaller tree for a quick look: the first 250 candidates
cargo run --release -- solve --strategy optimal --max-candidates 250 \
  --output tree-250.json

# Cheaper strategies: min-remaining | max-freq
cargo run --release -- solve --strategy min-remaining --output heuristic.json

# Validate any policy tree file
cargo run --release -- validate tree.json

# Differential correctness fuzzer: compares the optimized solver against an
# unoptimized naive reference (src/naive.rs) on random subsets
cargo run --release -- verify
```

Building an `optimal` tree for the full candidate set takes hours, so start
with `--max-candidates`. Other subcommands exist for measuring and validating
the engine itself (`benchmark`, `benchmark-random`, `diagnose`, `full`);
AGENTS.md describes what they do and the dedicated machine they are meant to
run on.

### `solve` flags

- `--max-candidates N` - build for the first `N` candidates only (the same
  deterministic convention the golden tests use). Add `--sample-seed S` to
  instead draw a reproducible, representative `N`-candidate spread.
- `--stats progress.parquet [--stats-format parquet|ndjson]` - export a small
  **progress time series** (one row per sampled interval: nodes built, frontier
  size, depth, cache hits, ...). Sampling is periodic and clock-gated, never
  per node, so it cannot measurably slow a build; it is off unless `--stats` is
  given. A periodic progress thread fills the series during long node
  evaluations (the root scan can run for hours), so it covers the whole run
  rather than only the node-boundary phase. The Parquet file is written with
  `SNAPPY` compression and loads directly into pandas/polars/duckdb for
  plotting.
- `--compare` - also run the cost-only solve on the same candidate set, print
  its time and the tree/cost-only ratio, and fail if its exact optimum
  disagrees with the tree's total. Measured on the compute host, the tree build
  is about 1.1-1.5x the cost-only solve at N=500-1000 (2-5x at N<=250, where
  fixed per-node costs dominate), converging toward parity as the root scan
  dominates.
- `--no-progress` - silence the periodic stderr progress line.
- `--cache-entries N` - transposition-table size (power of two).

`solve` self-validates the tree it writes (round-tripping through the reader)
and prints a summary.

### `validate`

```bash
wordle-opt validate tree.json
```

Recomputes every node's candidate set from the root and checks the defining
invariant: **at each node, an edge for a response exists if and only if that
response is possible** for some still-reachable candidate, and each edge leads
to exactly the subtree for the candidates that produce it. It also verifies the
tree is a tree (each node reachable once), that leaves are wins, that every
candidate terminates, and that the dictionary hash matches. The tree is
self-contained, so no other files are needed.

## Policy trees

A policy is a map from the set of remaining candidates to a guess. Because it
is deterministic, the whole game under that policy is a static decision tree:
**nodes are guesses**, **edges are responses**, and a node **wins** for the
candidate equal to its own guess whenever that candidate is still reachable
there. The all-green response is never an edge - it is the implicit win - so a
leaf (no children) is a win with one candidate left. `src/policy.rs` stores and
validates such trees. A node's candidate set is not stored at all, since it is
implied by the path of responses taken to reach it, and consumers (the
validator, the viewer) recompute it; that is what makes the representation
compact. Note that a win therefore depends on the *remaining* candidates, not
the initial list: a guess can be in the initial candidate list and still have
been eliminated along the path, in which case it cannot win there.

A tree is serialized as **readable** JSON: nested
`{"guess": "trace", "children": {"bgybg": ...}}` with the word lists embedded,
so the file is fully self-contained and can be validated (and rendered by the
viewer) with no other files. It also carries a **dictionary hash**, an FNV-1a
digest of the sorted guess and candidate lists, so a tree can never be silently
applied to the wrong dictionary.

## Interactive viewer

`site/` is a dependency-free static viewer, published to GitHub Pages at
**https://rbmr.github.io/wordle-opt/**. It loads a readable policy tree (a
bundled example, or one of your own) and **validates it in the browser**
against the same edge-iff-possible rule, so anything that loads can be assumed
valid. The viewer's pure logic - response computation, candidate filtering,
validation, per-node stats and the win rule - lives in `site/policy-core.js`,
separate from the DOM layer in `site/app.js`, and is tested by
`node site/policy-core.test.js` (also run by CI).

Two views, selected once a policy is loaded:

- **Play** (default): traverse the policy like the game. Each row is one guess;
  click a letter of the current guess to cycle its response (gray -> yellow ->
  green), then Submit. An impossible response is reported as such; the
  all-green response wins and shows a solved line with a confetti button. Back
  returns to the previous guess with its response still filled in, so a typo
  can be fixed without re-entering it. When only one candidate remains, the
  all-green response is pre-filled as a convenience - it is still just a
  response, and Submit is what wins. Each node shows its *expected guesses
  remaining* and how many candidates are still possible, and Options lists
  every possible response for the current guess (including the win, when it is
  available).
- **Explore**: the statistics (a table of the input files and the policy, next
  to a bar plot of the guess-count distribution) and the tree explorer (the
  full collapsible tree, with expand/collapse, expand-to-depth, and find).

The word length is taken from the tree, so it is not tied to 5 letters (there
are 3-, 4- and 6-letter examples under `site/examples/`).

Bundled examples:

| example | strategy | candidates | nodes | max depth | total cost | mean guesses |
|---|---|--:|--:|--:|--:|--:|
| `optimal` | optimal | 2340 | 2478 | 5 | 8001 | 3.419 |
| `min-remaining` | min-remaining | 2340 | 2934 | 4 | 8561 | 3.659 |
| `max-freq` | max-freq | 2340 | 2908 | 8 | 9545 | 4.079 |

See `site/examples/README.md` for how they were generated.

## Development

`cargo test --release` runs the full suite, including golden regression tests
in `src/solver.rs` that assert exact optimal costs for fixed candidate subsets.
They exist to make silent correctness regressions loud: if a change to the
search or pruning logic produces a different cost for the same fixed input, the
test fails immediately instead of the mistake being noticed later. Treat a
golden-test failure as "explain why the new number is correct," not "update the
constant." Use `--release` - these tests run the real solver on real instances.
CI (`.github/workflows/rust.yml`) runs the tests, `cargo clippy --release -- -D
warnings`, `cargo fmt -- --check`, and the viewer's Node tests.
