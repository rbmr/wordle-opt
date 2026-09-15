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

echo "Syncing to $HOST..."
rsync -aP --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Building on compute..."
ssh "$HOST" "cd wordle-opt && ~/.cargo/bin/cargo build --release"

echo "Running correctness tests on compute (catches a broken change before benchmarking it)..."
ssh "$HOST" "cd wordle-opt && timeout 600 ~/.cargo/bin/cargo test --release -- --test-threads=1"

echo "Running benchmark suite on compute (bounded by timeout 36000)..."
ssh "$HOST" "cd wordle-opt && timeout 36000 ~/.cargo/bin/cargo run --release -- benchmark-random $@"
