#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo test -p gaman-core
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo clippy -p gaman-core --all-targets
cargo clippy -p gaman --features sqlite --all-targets
git diff --check
