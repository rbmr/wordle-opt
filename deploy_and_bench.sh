#!/bin/bash
set -e

# This is the standard "verify + benchmark on compute" entry point - use it
# instead of hand-rolling rsync/ssh/cargo commands, so every run (human or
# agent) syncs, tests, and benchmarks the same way.
#
# Default host resolves via Tailscale (see `tailscale status`), which works
# regardless of which network this script runs from. `compute` is the
# bare-metal machine `pc01` since the October 2026 migration off Proxmox -
# reachable as `user@pc01` over Tailscale; the old 10.10.10.2 private link
# is retired and the tailnet grants this path in accept mode (no re-auth).
# The machine must be powered on first - `pc01` is a WoL machine now: wake
# it with `ssh wake-pc01` (Wake-on-LAN via the rpi5) if it is off.
#
# Any arguments are passed straight through to the CLI - e.g.
# `./deploy_and_bench.sh solve --strategy optimal --max-candidates 500
# --sample-seed 1 --output /tmp/n500.json` for a reproducible tree run at a
# smaller candidate set. If no arguments are provided, it defaults to
# building a small reproducible optimal tree (--max-candidates 250 --sample-seed 1).
HOST="${WORDLE_OPT_COMPUTE_HOST:-user@pc01}"

# Computed HERE, not on compute: the remote build directory has no .git (see
# the rsync --exclude below), so it structurally cannot answer "what commit
# is this" on its own - only the local repo, which is the actual source of
# the code about to be synced, can. Appends "-dirty" when the working tree
# has uncommitted changes, so a benchmark run stamped from mid-edit is
# visibly distinguishable from one run at a clean commit instead of silently
# claiming the commit's number for code the commit doesn't contain.
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
	COMMIT="${COMMIT}-dirty"
fi

echo "Syncing to $HOST (commit $COMMIT)..."
# --delete makes the remote an exact mirror of this working tree, not just a
# superset of it - without it, any stray file ever created directly on the
# remote (a scratch script, a stale binary, an old git checkout) accumulates
# there forever since rsync only ever adds/updates, never removes.
rsync -aP --delete --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on compute..."
ssh "$HOST" "cd wordle-opt && env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo build --release"

echo "Running correctness tests on compute (catches a broken change before benchmarking it)..."
ssh "$HOST" "cd wordle-opt && timeout 1800 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo test --release -- --test-threads=1"

# This script is the tree-producing iteration entry point, always foreground
# and bounded to 10h - it does NOT have a `full` mode. run_full.sh/check_full.sh
# launch the full N=2340 run detached and let you poll it instead of blocking
# on it. Use those for a `full` run; this script is for tree iteration.
CMD_ARGS="$*"
if [ -z "$CMD_ARGS" ]; then
	CMD_ARGS="solve --strategy optimal --max-candidates 250 --sample-seed 1 --output /tmp/iter-tree-250.json"
fi

echo "Running tree iteration on compute (bounded by timeout 36000): $CMD_ARGS"
ssh "$HOST" "cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT timeout 36000 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo run --release -- $CMD_ARGS"
