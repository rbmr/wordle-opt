#!/bin/bash
set -e

echo "Syncing to robert@compute..."
rsync -aP --exclude 'target' --exclude '.git' . robert@compute:~/wordle-opt/

echo "Compiling on compute..."
ssh robert@compute "cd wordle-opt && ~/.cargo/bin/cargo build --release"

echo "Running full benchmark suite on compute..."
ssh robert@compute "cd wordle-opt && ~/.cargo/bin/cargo run --release -- benchmark"
