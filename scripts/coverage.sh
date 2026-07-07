#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required. Install it with: cargo install cargo-llvm-cov --locked" >&2
  exit 1
fi

cargo llvm-cov clean --workspace
mkdir -p results/coverage/html
cargo llvm-cov --workspace --all-targets --features sqlite --html --output-dir results/coverage/html
cargo llvm-cov --workspace --all-targets --features sqlite --lcov --output-path results/coverage/lcov.info
cargo llvm-cov report --summary-only
awk -F: '/^LH:/ {hit += $2} /^LF:/ {found += $2} END {printf "workspace line coverage from LCOV: %d/%d %.2f%%\n", hit, found, (found ? hit * 100 / found : 0)}' results/coverage/lcov.info
