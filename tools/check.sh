#!/usr/bin/env bash
# Pre-submission check: formatting, lints, release build, tests, the sample
# diff on the release binary, and the independent black-box check.
# Stops at the first failure.
set -euo pipefail

cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --check

step "cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

step "cargo build --release"
cargo build --release

step "cargo test"
cargo test

step "sample diff (release binary)"
target/release/engine sample_input.txt | diff - sample_expected.txt

step "tools/verify.py --streams 100"
python3 tools/verify.py target/release/engine --streams 100

printf '\nOK: all checks passed\n'
