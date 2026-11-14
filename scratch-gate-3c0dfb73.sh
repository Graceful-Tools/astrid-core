#!/bin/sh
# Scratch: the core's gate for task 3c0dfb73. Not committed.
cd /c/Users/jonpa/mycode/astrid-dev/astrid-core || exit 1
cargo fmt --all --check || { echo "GATE_EXIT=fmt"; exit 1; }
cargo clippy --workspace --all-targets -- -D warnings || { echo "GATE_EXIT=clippy"; exit 1; }
cargo test --workspace || { echo "GATE_EXIT=test"; exit 1; }
echo "GATE_EXIT=0"
