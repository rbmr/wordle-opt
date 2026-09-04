#!/bin/bash
set -e

echo "Syncing to robert@192.168.1.72..."
rsync -aP --exclude 'target' --exclude '.git' . robert@192.168.1.72:~/wordle-opt/

echo "Compiling on compute..."
ssh robert@192.168.1.72 "cd wordle-opt && ~/.cargo/bin/cargo build --release"

echo "Running full benchmark suite on compute (bounded by timeout 36000)..."
ssh robert@192.168.1.72 "cd wordle-opt && timeout 36000 ~/.cargo/bin/cargo run --release -- benchmark"
