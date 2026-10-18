#!/bin/bash
set -e

# Copies this working tree to a remote machine, builds it and runs the test
# suite there, then runs one wordle-opt command on it. Arguments are passed to
# the CLI unchanged, for example:
#
#   WORDLE_OPT_COMPUTE_HOST=user@host scripts/deploy_and_bench.sh \
#     solve --strategy optimal --max-candidates 500 --sample-seed 1 --output /tmp/n500.json
#
# Without arguments it builds an optimal tree for a seeded 250-candidate subset.
#
# Optional: WORDLE_OPT_FEATURES=tt-audit builds, tests and runs with that cargo
# feature; WORDLE_OPT_REPEAT=N runs the command N times in a row, so one
# invocation yields N timings of the same build; WORDLE_OPT_RUSTFLAGS replaces
# the default compiler flags (-C target-cpu=native), for example to time a
# portable build.
#
# The remote copy lives in ~/wordle-opt and is an exact mirror of this working
# tree without .git, so the commit is determined here and passed to the run in
# WORDLE_OPT_COMMIT. It ends in "-dirty" when there are uncommitted changes.
cd "$(dirname "$0")/.."
HOST="${WORDLE_OPT_COMPUTE_HOST:?set WORDLE_OPT_COMPUTE_HOST to the remote machine, for example user@host}"
FEATURES=""
if [ -n "${WORDLE_OPT_FEATURES:-}" ]; then
	FEATURES="--features ${WORDLE_OPT_FEATURES}"
fi
REPEAT="${WORDLE_OPT_REPEAT:-1}"
RUSTFLAGS_REMOTE="${WORDLE_OPT_RUSTFLAGS:--C target-cpu=native}"

COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
	COMMIT="${COMMIT}-dirty"
fi

echo "Syncing to $HOST (commit $COMMIT)..."
rsync -aP --delete --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on $HOST..."
ssh "$HOST" "cd wordle-opt && env RUSTFLAGS=\"$RUSTFLAGS_REMOTE\" ~/.cargo/bin/cargo build --release $FEATURES"

echo "Running the test suite on $HOST..."
ssh "$HOST" "cd wordle-opt && timeout 1800 env RUSTFLAGS=\"$RUSTFLAGS_REMOTE\" ~/.cargo/bin/cargo test --release $FEATURES"

CMD_ARGS="$*"
if [ -z "$CMD_ARGS" ]; then
	CMD_ARGS="solve --strategy optimal --max-candidates 250 --sample-seed 1 --output /tmp/iter-tree-250.json"
fi

for i in $(seq 1 "$REPEAT"); do
	echo "Running on $HOST (run $i/$REPEAT, time limit 10 h): $CMD_ARGS"
	ssh "$HOST" "cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT timeout 36000 env RUSTFLAGS=\"$RUSTFLAGS_REMOTE\" ~/.cargo/bin/cargo run --release $FEATURES -- $CMD_ARGS"
done
