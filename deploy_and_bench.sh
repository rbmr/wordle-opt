#!/bin/bash
set -e

# This is the standard "verify + benchmark on compute" entry point - use it
# instead of hand-rolling rsync/ssh/cargo commands, so every run (human or
# agent) syncs, tests, and benchmarks the same way.
#
# Default host resolves via Tailscale (see `tailscale status`), which works
# regardless of which network this script runs from. If you're running this
# FROM assistant specifically (the wordle-opt agent's own use case), set
# WORDLE_OPT_COMPUTE_HOST=robert@10.10.10.2 - a private link that exists
# only between assistant and compute on their shared Proxmox host, so it
# never depends on the external LAN's DHCP or on Tailscale re-authentication.
# The VM itself must be powered on first - see pve01 (`qm start 100`) if
# neither address connects.
#
# Any arguments are passed through to `benchmark-random` - e.g.
# `./deploy_and_bench.sh -n 500 -k 3` for a quick check while iterating,
# or no arguments for the fuller default sweep once an idea looks promising.
HOST="${WORDLE_OPT_COMPUTE_HOST:-robert@compute}"

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

# This script is for benchmark-random only, always foreground and bounded to
# 10h - it does NOT have a `full` mode. An earlier version briefly added one
# that blocked the calling shell for up to 10 hours waiting on the SSH
# command to return - exactly the synchronous-wait anti-pattern AGENTS.md
# warns against. run_full.sh/check_full.sh supersede that: they launch the
# full run detached and let you poll it instead of blocking on it. Use those
# for a `full` run; this script stays benchmark-random-only.
echo "Running benchmark suite on compute (bounded by timeout 36000)..."
ssh "$HOST" "cd wordle-opt && WORDLE_OPT_COMMIT=$COMMIT timeout 36000 env RUSTFLAGS=\"-C target-cpu=native\" ~/.cargo/bin/cargo run --release -- $*"
