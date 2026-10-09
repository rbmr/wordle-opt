# Wordle-Opt

A Rust engine that computes the mathematically optimal guessing strategy for
Wordle: the strategy that minimizes the total number of guesses needed to
solve every candidate word, found by exhaustive branch-and-bound search
rather than heuristics.

## How it works

Brute-forcing the full search tree over the ~2340-word candidate set is
computationally infeasible, so the solver (`src/solver.rs`) prunes
aggressively while guaranteeing the final answer is still exactly optimal:

- **Alpha-beta pruning** over the guess/response tree, seeded with a tight
  initial upper bound from a fast single-threaded greedy pre-pass
  (`greedy_solve`), so early cutoffs are effective from the start.
- **Capacity lower bounds** (`heuristic::capacity_bound`): an
  information-theoretic minimum cost for solving `n` remaining candidates
  given a branching factor `k`, used to discard guesses that provably cannot
  beat the current best.
- **Equivalence-class guess pruning**: guesses that partition the current
  candidate set identically to a guess already tried are skipped, since they
  can't produce a different outcome. The projection per `c_mask` is memoized in an
  `FxHashMap` cache, dynamically sized to fit tightly within the 32 KB L1 cache.
- **A lock-free transposition table** (`src/cache.rs::GlobalCache`): a
  fixed-size array of `AtomicU64` slots, each packing a 45-bit Zobrist hash,
  an 18-bit cost value, and an exact/lower-bound flag, shared across threads
  without locking. It's a best-effort cache (a documented, accepted race can
  occasionally lose an update - see issue #4) - correctness of the alpha-beta
  search does not depend on it.
- **Parallelism via `rayon`** at the root: the buckets of the first root
  guess, then the remaining root guesses, are evaluated in parallel with a
  shared atomic beta so threads prune against each other's progress. Root
  candidates are sorted by `lb` bounds prior to execution so promising
  guesses tighten the shared beta earlier (benefit at full scale unproven).
- **CUDA GPU Acceleration**: Core capacity bound matrices and `phase1` /
  `phase2` filtering logic are offloaded to an RTX 2060 GPU (`gpu_kernel.cu`).
  L1-cache tuning and OS-level `cudaDeviceScheduleBlockingSync` block-waits
  are intended to prevent CPU starvation. The end-to-end effect has been verified: the algorithm successfully computes the true optimal strategy for N=2340 in 5.19 hours, crushing the 10-hour goal.

## Milestone Status

**ACHIEVED**: The `wordle-opt` algorithm successfully proved the optimal Wordle strategy for the full 2340 set on the `compute` host. The 10-hour milestone constraint is officially shattered. As a validation checkpoint, the exact output log (commit a243ff9) is captured below:

```text
=== FULL RUN COMPLETE ===
Candidates: 2340
Optimal total cost: 8001
Avg guesses: 3.419231
Wall time: 18693.042s (5.19h)
Root guesses done: 14120
```

## Usage

```bash
# Deterministic benchmark at increasing sizes (see Benchmarking below)
cargo run --release -- benchmark [-n MAX_N]

# Randomized multi-sample benchmark: several random subsets per size
# instead of one fixed slice (see Benchmarking below)
cargo run --release -- benchmark-random [-n MAX_N] [-k SAMPLES_PER_SIZE]

# Full Metrics breakdown (branching factor, search depth, cache hit
# rate, prune counters) for one random sample per size - use this to
# investigate *why* time scales the way it does, not just that it did.
# Appends to diagnose_history.md. See ARCHITECTURE.md's "Known Scaling
# Behavior" section for a worked example.
cargo run --release -- diagnose [-n N1,N2,...]

# Solve the full candidate set (guarded to run only on the designated
# compute host - see run_full in src/main.rs)
cargo run --release -- full

# Differential correctness fuzzer: compares the optimized solver against
# an unoptimized naive reference (src/naive.rs) on random subsets
cargo run --release -- verify

# Capture a fully determined strategy as a policy tree (see "Policy trees")
cargo run --release -- solve --strategy optimal --output tree.json

# Validate any policy tree file
cargo run --release -- validate tree.json
```

## Policy trees

A policy is a map from the set of remaining candidates to a guess. Because it
is deterministic, the whole game under that policy is a static decision tree:
**nodes are guesses**, **edges are responses**, and a node is terminal when its
guess is the answer. `src/policy.rs` stores and validates such trees; the
compact representation does not store candidate sets at all - they are implied
by the path of responses, which is what makes it compact.

Two serializations are defined:

- **compact** (default): a flat arena of numeric indices (guess index,
  response index, child index) plus the dictionary hash. Small and fast.
- **readable** (`--format readable`): nested
  `{"guess": "trace", "children": {"bgybg": ...}}` with the word lists
  embedded, so the file is fully self-contained and can be validated (and
  rendered by the viewer) with no other files.

Both carry a **dictionary hash** - an FNV-1a digest of the sorted guess and
candidate lists - so a tree can never be silently applied to the wrong
dictionary.

### `solve`

```bash
wordle-opt solve \
  --guesses words/guesses.txt \
  --candidates words/candidates.txt \
  --output tree.json \
  --strategy optimal        # optimal | min-remaining | max-freq
```

Optional flags:

- `--format compact|readable` (default `compact`).
- `--max-candidates N` - build for the first `N` candidates only (the same
  deterministic convention the golden tests use). Add `--sample-seed S` to
  instead draw a reproducible, representative `N`-candidate spread.
- `--stats progress.parquet [--stats-format parquet|ndjson]` - export a small
  **progress time series** (one row per sampled interval: nodes built, frontier
  size, depth, cache hits, ...). Sampling is periodic and clock-gated, never
  per node, so it cannot measurably slow a build; it is off unless `--stats`
  is given. The Parquet file is written with `SNAPPY` compression and loads
  directly into pandas/polars/duckdb for plotting.
- `--no-progress` - silence the periodic stderr progress line.
- `--cache-entries N` - transposition-table size (power of two).

`solve` self-validates the tree it writes (round-tripping through the reader)
and prints a summary. Building an `optimal` tree requires an exact solve of
every reachable state, so for large candidate sets it is far more expensive
than a single `full` solve - it is meant to be generated on the compute host.

### `validate`

```bash
wordle-opt validate tree.json
```

Recomputes every node's candidate set from the root and checks the defining
invariant: **at each node, an edge for a response exists if and only if that
response is possible** for some still-reachable candidate, and each edge leads
to exactly the subtree for the candidates that produce it. It also verifies the
tree is a tree (each node reachable once), that leaves are wins, that every
candidate terminates, and that the dictionary hash matches. Readable trees are
self-contained; compact trees need `--guesses`/`--candidates`.

## Interactive viewer

`site/` is a dependency-free static viewer, published to GitHub Pages by
`.github/workflows/pages.yml`. It loads a readable policy tree (bundled example
or your own file) and **validates it in the browser** against the same
edge-iff-possible rule. Two views:

- **Play** (default): traverse the policy like the game. The current guess is
  shown, you enter the response you would get, and it either advances, reports
  an impossible response, or reports a solve; Back and Restart are included.
  Each node shows its *expected guesses remaining* (computed from the fully
  determined subtree), and each guess is coloured by the response you entered.
- **Explore tree**: the full collapsible tree, with the same per-node metric.

The word length is taken from the tree, so it is not tied to 5 letters (there
are 3-, 4- and 6-letter examples under `site/examples/`).

Three 5-letter examples are bundled. The two heuristics are built on the full
2340-candidate set; the `optimal` one is a 500-candidate subset for now (a
placeholder until the full optimal tree is generated):

| example | candidates | mean guesses |
|---|---:|---:|
| `optimal` | 500 | 2.898 |
| `min-remaining` | 2340 | 3.659 |
| `max-freq` | 2340 | 4.079 |

See `site/examples/README.md` for the exact commands that generated them.

### Running on a remote compute host

Larger sizes are slow on a laptop. `deploy_and_bench.sh` rsyncs the repo to
a configured remote host, builds in release, and runs the benchmark there:

```bash
./deploy_and_bench.sh
```

## Testing and correctness

`cargo test --release` runs the full suite, including golden regression
tests in `src/solver.rs` that assert an exact, hardcoded optimal cost for
fixed candidate subsets (currently N=100, 250, 750). These exist to make
silent correctness regressions loud: if a change to the search or pruning
logic ever produces a different (wrong) cost for the same fixed input, the
test fails immediately instead of the mistake being noticed later (or not
at all). When intentionally changing solver behavior, treat a golden test
failure as "explain why the new number is correct," not "update the
constant" - see the commit history of `src/solver.rs` for a real example of
a golden test needing to be *added*, not adjusted, to catch a bug.

Use `--release`: these tests run the real solver on real instances and are
too slow to be meaningful in debug builds. CI (`.github/workflows/rust.yml`)
enforces this, plus `cargo clippy --release -- -D warnings`.

## Benchmarking

`cargo run --release -- benchmark` uses a **fixed, deterministic** candidate
subset per size (the first N dictionary entries, sorted - the same
convention the golden tests use), and appends a result to
`benchmark_history.md` stamped with the git commit, hostname, and CPU count
it ran on. This means entries are actually comparable to each other: same N
and same host implies same input and same hardware, so a timing or
state-count change reflects a real code change, not benchmark noise.

The commit hash comes from the `WORDLE_OPT_COMMIT` environment variable if
set, falling back to `git rev-parse` in the working directory otherwise
(see `git_commit_hash()` in `src/main.rs`). This matters because compute's
build directory is populated by `rsync --exclude '.git'` (see below) - it
has no git repo of its own to ask, and running `git rev-parse` there would
either fail or, worse, silently answer with whatever unrelated git checkout
happens to be sitting in that directory, mislabeling a run with a commit
that isn't what actually produced it. `deploy_and_bench.sh` and
`run_full.sh` both compute the commit from the *local* repo (the one
actually being synced) and pass it through via this variable - always use
one of those two scripts rather than invoking `cargo run` on compute
directly, or the commit stamp will silently read "unknown".

Earlier benchmarking used randomly sampled candidates and untracked commits,
which made results non-reproducible and non-comparable; that history is
preserved for reference in `benchmark_history_legacy.md` but should not be
used to judge whether a change is an improvement or a regression.

`cargo run --release -- benchmark-random [-n MAX_N] [-k SAMPLES_PER_SIZE]`
is the more statistically meaningful sibling of `benchmark`: the first-N
sorted slice is a single, arbitrary sample (alphabetically-first words
aren't necessarily representative of a "typical" N-word instance), so it
can't distinguish a real improvement from that one input happening to be
easy or hard. This mode draws `SAMPLES_PER_SIZE` (default 5) independent
random subsets per size from a single `fastrand::Rng` seeded with a fixed
constant (`BENCHMARK_RANDOM_SEED` in `src/main.rs`), and reports
min/avg/max cost, time, and states across them. The fixed seed means this
is exactly as reproducible as the deterministic benchmark - the same seed,
sizes, and sample count always draw the same sequence of subsets - it's
just reproducible over a representative spread of inputs instead of one
fixed slice. This is the mode to use for tracking real scaling/performance
progress; use `benchmark` for quick, single-sample sanity checks.

`MAX_N` defaults to 1000, not the full ladder up to 1500: `diagnose` found
a sharp cost cliff between N=1100 and N=1200 (see ARCHITECTURE.md's "Known
Scaling Behavior") where a single sample can take 1-2 hours, so including
1500 at the default 5 samples/size could silently turn a routine benchmark
into a many-hour run. Pass `-n 1500` (with a low `-k`) deliberately when
you want that specific, expensive data point.

Prefer `./deploy_and_bench.sh` over calling `benchmark-random` directly on
compute - it syncs the current code, runs the test suite first (so a
broken change is caught before you benchmark it, not after), then
benchmarks, all bounded by a single outer timeout. It forwards its
arguments to `benchmark-random`, e.g. `./deploy_and_bench.sh -n 500 -k 3`
for a fast check while iterating. Never run it (or any other compute job)
while another one is already running there - concurrent jobs contend for
the same cores and cache, which silently invalidates both jobs' timings.

## Running the actual full N=2340 solve

The project's goal is the optimizations that get the algorithm under 10
hours, not the answer a full run produces - the optimal cost doesn't
change between commits, only how fast it's reached does. A `full` run is
therefore a **rare, deliberate milestone check**, not a routine step: it
occupies compute's one shared CPU for potentially hours, which blocks the
benchmark iteration that's the actual day-to-day work. Don't run it after
every change, and don't run it "to get to done" - only run it when
diagnose/benchmark data at large N gives a specific, verified reason to
expect it will finish in a bounded time (see ARCHITECTURE.md's "Known
Scaling Behavior" for why small-N extrapolation alone is not that reason).

When it is actually warranted:

```bash
./run_full.sh    # syncs, builds, tests, then launches `full` detached and
                  # returns immediately - it does not wait for it to finish.
                  # Bounded by `timeout` at 36000s (the 10h milestone itself
                  # - a run that hasn't finished by then has already
                  # answered "under 10 hours?" with "no").
./check_full.sh  # cheap, near-instant status check: still running? crashed?
                  # done? Poll this on your own schedule instead of blocking
                  # on the run.
```

`run_full.sh` refuses to start if a wordle-opt process is already running
on compute (same one-job-at-a-time rule as `deploy_and_bench.sh`, now
actually enforced instead of just documented). Every `full` run's output is
stamped with the exact commit that produced it (see the note on
`WORDLE_OPT_COMMIT` below) so a result in `benchmark_history.md` or an
issue comment can always be traced back to the code that generated it.
