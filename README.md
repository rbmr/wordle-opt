# Wordle-Opt

A mathematically optimal, parallelized Rust engine designed to compute the theoretically perfect guessing strategy for Wordle.

## Overview

Wordle-Opt determines the true optimal guessing strategy to minimize the number of expected guesses in a standard Wordle game. Given that an exhaustive search of the full $2340$ candidate set is computationally massive, this engine is aggressively optimized for throughput:

- **L1 Cache Optimizations**: Memory structures (such as `ResponseMatrix` and contiguous bitmasks) are aligned to fit entirely within L1/L2 cache lines to prevent memory-bandwidth bottlenecking during the deep Alpha-Beta pruning recursion.
- **Data Parallelism**: The top-level initializations and heuristic evaluations are fully parallelized across all logical CPU cores using `rayon`.
- **Advanced Pruning**: Incorporates bitwise projection-filtering to aggressively prune symmetrically equivalent guesses, and leverages descending bucket sorting to trigger Alpha-Beta cutoffs almost instantly.
- **SIMD Auto-Vectorization**: The heuristic cost evaluators are written completely branchless, allowing LLVM to auto-vectorize the mathematical summations using AVX2/AVX-512 instructions.

## Usage

### Running Locally
To test the engine locally on small dictionary subsets:
```bash
cargo run --release -- benchmark
```

### Running on Compute Node
For testing larger subsets ($N > 150$), local machines typically thermal throttle. Use the deployment script to execute the benchmark remotely on the primary compute cluster:
```bash
./deploy_and_bench.sh
```

## Benchmarks & Scaling
The engine scales predictably in both time and state evaluations. Refer to `benchmark_history.md` for historical throughput data and asymptotic scaling analysis. At peak performance, the engine reliably evaluates ~6-8 Million pruning bounds per second.

### Correctness Verification
To run the automated differential correctness fuzzer (which compares the optimized solver against an unoptimized naive reference on random subsets):
```bash
cargo run --release -- verify
```
