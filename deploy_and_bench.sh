#!/bin/bash
set -e

# "compute" resolves via Tailscale (see `tailscale status`), which works
# regardless of which network this script runs from; falls back to the LAN
# IP if you're not on Tailscale. The VM itself must be powered on first -
# see pve01 (`qm start 100`) if `ssh compute` doesn't connect.
HOST="${WORDLE_OPT_COMPUTE_HOST:-robert@compute}"

echo "Syncing to $HOST..."
rsync -aP --exclude 'target' --exclude '.git' . "$HOST:~/wordle-opt/"

echo "Compiling on compute..."
ssh "$HOST" "cd wordle-opt && ~/.cargo/bin/cargo build --release"

echo "Running randomized benchmark suite on compute (bounded by timeout 36000)..."
ssh "$HOST" "cd wordle-opt && timeout 36000 ~/.cargo/bin/cargo run --release -- benchmark-random"
