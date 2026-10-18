#!/bin/bash
set -e

# Solves the full bundled word lists on a remote machine in the background and
# returns at once. Check on the run with scripts/check_full.sh.
#
# It copies, builds and tests the code first, the same way as
# scripts/deploy_and_bench.sh. It refuses to start if the working tree has
# uncommitted changes, because the run is labelled with the commit, or if a
# wordle-opt process is already running on the remote machine.
#
# Each run keeps its own files on the remote machine, named by the UTC date
# and the commit:
#   ~/full-runs/<YYYY-MM-DD>-<commit>.json              the policy tree
#   ~/full-runs/<YYYY-MM-DD>-<commit>.log               the output, with the summary
cd "$(dirname "$0")/.."
HOST="${WORDLE_OPT_COMPUTE_HOST:?set WORDLE_OPT_COMPUTE_HOST to the remote machine, for example user@host}"
RUN_DIR="full-runs"
PIDFILE="$RUN_DIR/current.pid"
STAMPFILE="$RUN_DIR/CURRENT"
TIMEOUT_SECS="${WORDLE_OPT_FULL_TIMEOUT:-36000}"
# 1 << 27 cache units of 16 bytes = 2 GB.
CACHE_ENTRIES=134217728

if ! git rev-parse --short HEAD >/dev/null 2>&1; then
	echo "Not starting: no git repository here, so the run cannot be labelled with a commit." >&2
	exit 1
fi
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
	echo "Not starting: the working tree has uncommitted changes. Commit them first." >&2
	exit 1
fi
COMMIT="$(git rev-parse --short HEAD)"
STAMP="$(date -u +%Y-%m-%d)-$COMMIT"

if ssh "$HOST" "pgrep -f '[t]arget/release/wordle-opt' >/dev/null 2>&1"; then
	echo "Not starting: a wordle-opt process is already running on $HOST." >&2
	echo "Check it with scripts/check_full.sh." >&2
	exit 1
fi

echo "Syncing to $HOST (commit $COMMIT)..."
rsync -aP --delete --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on $HOST..."
ssh "$HOST" "cd wordle-opt && env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo build --release"

echo "Running the test suite on $HOST..."
ssh "$HOST" "cd wordle-opt && timeout 1800 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo test --release"

echo "Starting the full solve on $HOST in the background (time limit ${TIMEOUT_SECS} s)."
echo "Run: $STAMP (files in ~/$RUN_DIR/ on $HOST)"
# The binary is started directly rather than through `cargo run`, so the saved
# PID is the solver's own and check_full.sh can tell whether it is still alive.
ssh "$HOST" "mkdir -p \$HOME/$RUN_DIR; cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT nohup timeout ${TIMEOUT_SECS} ./target/release/wordle-opt solve --strategy optimal --output \$HOME/$RUN_DIR/$STAMP.json --cache-entries $CACHE_ENTRIES > \$HOME/$RUN_DIR/$STAMP.log 2>&1 < /dev/null & echo \$! > \$HOME/$PIDFILE; echo $STAMP > \$HOME/$STAMPFILE"

echo "Started. Check on it with: scripts/check_full.sh"
