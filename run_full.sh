#!/bin/bash
set -e

# Launches the actual full N=2340 solve on compute - a RARE, DELIBERATE
# milestone check, not a routine step and not the project's actual goal.
# The goal is the optimizations that get the algorithm under 10 hours, not
# the answer a full run produces; the answer doesn't change between
# commits, only how fast it's reached does. Compute has one shared CPU that
# every benchmark also needs, so a `full` run and benchmark iteration are
# mutually exclusive - don't run this reflexively after every change, and
# don't run it just to "get to done." Only run it when diagnose/benchmark
# data at large N gives a specific, verified reason to expect it will
# finish in a bounded time, as a checkpoint on that evidence - see
# ARCHITECTURE.md's "Known Scaling Behavior" and the task guidance on this
# machine for why small-N extrapolation alone is not that evidence.
#
# Bounded by `timeout` at the milestone threshold itself: a run that hasn't
# finished in 10h has already answered "under 10 hours?" with "no", so
# there's nothing to gain by letting it run longer uncapped, and it would
# just block benchmarking indefinitely instead.
#
# Like deploy_and_bench.sh this syncs+builds+tests first, but unlike it,
# it does NOT block until the solve finishes - it starts the run detached
# and returns immediately. Use check_full.sh to poll its status later
# instead of leaving a shell blocked on it for hours.
#
# Refuses to start if a wordle-opt process is already running on compute -
# concurrent jobs contend for the same cores/cache and invalidate both jobs'
# timings, and a second `full` run racing the first would also stomp on the
# same output files.
HOST="${WORDLE_OPT_COMPUTE_HOST:-robert@compute}"
LOG="full_run.log"
PIDFILE="full_run.pid"
TIMEOUT_SECS="${WORDLE_OPT_FULL_TIMEOUT:-36000}" # 10h, the milestone threshold

if ssh "$HOST" "pgrep -f '[t]arget/release/wordle-opt' >/dev/null 2>&1"; then
	echo "REFUSING TO START: a wordle-opt process is already running on $HOST." >&2
	echo "Check its status with ./check_full.sh before starting another." >&2
	exit 1
fi

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
	COMMIT="${COMMIT}-dirty"
fi

echo "Syncing to $HOST (commit $COMMIT)..."
rsync -aP --delete --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on compute..."
ssh "$HOST" "cd wordle-opt && env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo build --release"

echo "Running correctness tests on compute (catches a broken change before committing to a 10-hour run)..."
ssh "$HOST" "cd wordle-opt && timeout 600 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo test --release -- --test-threads=1"

echo "Launching full N=2340 solve on compute, detached, bounded at ${TIMEOUT_SECS}s (log: ~/$LOG)..."
# Invoke the built binary directly, not `cargo run` - `cargo run` would make
# $! the PID of the cargo wrapper process, not the actual solver, which
# check_full.sh needs to be able to tell whether the run is still alive.
ssh "$HOST" "cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT nohup timeout ${TIMEOUT_SECS} ./target/release/wordle-opt full > $LOG 2>&1 & echo \$! > $PIDFILE"

echo "Started. Poll progress with: ./check_full.sh"
