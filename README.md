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
  can't produce a different outcome.
- **A lock-free transposition table** (`src/cache.rs::GlobalCache`): a
  fixed-size array of `AtomicU64` slots, each packing a 45-bit Zobrist hash,
  an 18-bit cost value, and an exact/lower-bound flag, shared across threads
  without locking. It's a best-effort cache (a documented, accepted race can
  occasionally lose an update - see issue #4) - correctness of the alpha-beta
  search does not depend on it.
- **Parallelism via `rayon`** at the root: the buckets of the first root
  guess, then the remaining root guesses, are evaluated in parallel with a
  shared atomic beta so threads prune against each other's progress.

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
```

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
