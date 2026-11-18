#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo test --locked --workspace --all-features
cargo check --locked -p gaman -p gaman-wasm --no-default-features \
    --features gaman/offline --target wasm32-unknown-unknown
