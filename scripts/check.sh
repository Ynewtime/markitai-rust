#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p .local/test-home
export MARKITAI_HOME="$PWD/.local/test-home"
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*.py'
