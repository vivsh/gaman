#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo test -p gaman --test parser -- --record results/parser-results.yaml
cargo test -p gaman --features sqlite --test offline -- --record results/offline-results.yaml

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

cargo test -p gaman --features sqlite --test online -- --record results/online-results.yaml
cargo run -p gaman --bin gaman-support-matrix -- --update-readme
cargo run -p gaman --bin gaman-evidence-doc -- --update-doc
cargo run -p gaman --bin gaman-evidence-doc -- --check
cargo test -p gaman --test offline_coverage
