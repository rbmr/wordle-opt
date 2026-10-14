#!/bin/bash
set -e

# Launches the full N=2340 optimal policy tree build on compute - a deliberate
# checkpoint, not a routine step, and at most one a day. The output is the
# tree: a run is only meaningful now if it produces the policy (the cost-only
# modes are on their way out - see the tree-only task). The project's goal is
# the optimizations that make the tree build fast, not the tree itself; the
# tree does not change between commits, only how fast it is reached.
#
# A run runs the code of exactly one commit and records which: this refuses to
# start on a dirty working tree, so the commit it names is the commit that ran.
# Every run keeps its own artifacts, stamped with the UTC day and the commit,
# so no run can overwrite another's data (compute's home, not the rsync
# target, which the next deploy deletes):
#   ~/full-runs/<YYYY-MM-DD>-<commit>.json             the policy tree
#   ~/full-runs/<YYYY-MM-DD>-<commit>-progress.parquet its progress series
#   ~/full-runs/<YYYY-MM-DD>-<commit>.log              the run log (summary included)
# Add a row for the completed run to run_history.md - that file is the
# record that survives a compute rebuild; the artifacts above do not.
#
# When to run is this repo's own call: a full run once a new commit with a
# performance-relevant change has landed, and no run when nothing
# performance-relevant changed. Nothing runs it on a schedule. Don't run it
# twice in a day, and don't run it reflexively after every change - every hour
# it runs is an hour compute is unavailable for the benchmark iteration that
# is the real day-to-day work. Only launch when large-N data gives a specific,
# verified reason to expect it will finish in a bounded time (see
# ARCHITECTURE.md's "Known Scaling Behavior" for why small-N extrapolation
# alone is not that evidence).
#
# Bounded by `timeout` as a safety cap, not at the milestone threshold: the
# milestone (under 2 hours) is read off the wall time the completed run
# reports, while killing the run early would throw away the tree it spent
# hours building. The cap exists only so a pathological run cannot block
# compute indefinitely.
#
# Like deploy_and_bench.sh this syncs+builds+tests first, but unlike it, it
# does NOT block until the solve finishes - it starts the run detached and
# returns immediately. Use check_full.sh to poll its status later instead of
# leaving a shell blocked on it for hours.
#
# Refuses to start if a wordle-opt process is already running on compute -
# concurrent jobs contend for the same cores/cache and invalidate both jobs'
# timings, and a second `full` run would also race the first's artifacts.
HOST="${WORDLE_OPT_COMPUTE_HOST:-user@pc01}"
RUN_DIR="full-runs"
PIDFILE="$RUN_DIR/current.pid"
STAMPFILE="$RUN_DIR/CURRENT"
TIMEOUT_SECS="${WORDLE_OPT_FULL_TIMEOUT:-36000}" # 10h, the milestone threshold

# The run names the commit it ran, so the tree has to be committed first.
if ! git rev-parse --short HEAD >/dev/null 2>&1; then
	echo "REFUSING TO START: no git repository here, so the run cannot record a commit." >&2
	exit 1
fi
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
	echo "REFUSING TO START: the working tree has uncommitted changes." >&2
	echo "Commit them first - a full run records the commit it ran as, and an" >&2
	echo "uncommitted edit would make that record wrong." >&2
	exit 1
fi
COMMIT="$(git rev-parse --short HEAD)"
STAMP="$(date -u +%Y-%m-%d)-$COMMIT"

if ssh "$HOST" "pgrep -f '[t]arget/release/wordle-opt' >/dev/null 2>&1"; then
	echo "REFUSING TO START: a wordle-opt process is already running on $HOST." >&2
	echo "Check its status with ./check_full.sh before starting another." >&2
	exit 1
fi

echo "Syncing to $HOST (commit $COMMIT)..."
rsync -aP --delete --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on compute..."
ssh "$HOST" "cd wordle-opt && env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo build --release"

echo "Running correctness tests on compute (catches a broken change before committing to a multi-hour run)..."
ssh "$HOST" "cd wordle-opt && timeout 600 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo test --release"

echo "Launching full N=2340 policy tree build on compute, detached, bounded at ${TIMEOUT_SECS}s."
echo "Run stamp: $STAMP (artifacts under ~/$RUN_DIR/ on $HOST)"
# Invoke the built binary directly, not `cargo run` - `cargo run` would make
# $! the PID of the cargo wrapper process, not the actual solver, which
# check_full.sh needs to be able to tell whether the run is still alive.
# WORDLE_OPT_POLICY_OUT/STATS name the tree and its progress series; the stamp
# in both names is what stops one day's run overwriting another's.
ssh "$HOST" "mkdir -p \$HOME/$RUN_DIR; cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT WORDLE_OPT_POLICY_OUT=\$HOME/$RUN_DIR/$STAMP.json WORDLE_OPT_POLICY_STATS=\$HOME/$RUN_DIR/$STAMP-progress.parquet nohup timeout ${TIMEOUT_SECS} ./target/release/wordle-opt full > \$HOME/$RUN_DIR/$STAMP.log 2>&1 < /dev/null & echo \$! > \$HOME/$PIDFILE; echo $STAMP > \$HOME/$STAMPFILE"

echo "Started. Poll progress with: ./check_full.sh"
