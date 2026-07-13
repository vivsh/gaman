#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

: "${POSTGRES_DATABASE_URL:?POSTGRES_DATABASE_URL is required to refresh accepted evidence}"
: "${MYSQL_DATABASE_URL:?MYSQL_DATABASE_URL is required to refresh accepted evidence}"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/gaman-evidence.XXXXXX")"
PUBLISHING=0

restore_bundle() {
  [[ -d "$STAGE/accepted" ]] || return 0
  cp "$STAGE/accepted/parser-results.yaml" results/parser-results.yaml
  cp "$STAGE/accepted/offline-results.yaml" results/offline-results.yaml
  cp "$STAGE/accepted/online-results.yaml" results/online-results.yaml
  cp "$STAGE/accepted/README.md" README.md
  cp "$STAGE/accepted/support-evidence.md" docs/support-evidence.md
}

cleanup() {
  local status=$?
  if [[ $status -ne 0 && $PUBLISHING -eq 1 ]]; then
    restore_bundle
  fi
  rm -rf "$STAGE"
  exit "$status"
}
trap cleanup EXIT

export GAMAN_EVIDENCE_GENERATION="$(date -u +%Y%m%dT%H%M%SZ)-$$"
export GAMAN_PARSER_RESULTS="$STAGE/parser-results.yaml"
export GAMAN_OFFLINE_RESULTS="$STAGE/offline-results.yaml"
export GAMAN_ONLINE_RESULTS="$STAGE/online-results.yaml"
export GAMAN_README_PATH="$STAGE/README.md"
export GAMAN_EVIDENCE_DOC="$STAGE/support-evidence.md"

cp README.md "$GAMAN_README_PATH"
cp docs/support-evidence.md "$GAMAN_EVIDENCE_DOC"

cargo test -p gaman --all-features --test parser -- --record "$GAMAN_PARSER_RESULTS" \
  --failure-output "$STAGE/parser-failures.yaml"
cargo test -p gaman --all-features --test offline -- --record "$GAMAN_OFFLINE_RESULTS" \
  --failure-output "$STAGE/offline-failures.yaml"
cargo test -p gaman --all-features --test online -- --record "$GAMAN_ONLINE_RESULTS" \
  --failure-output "$STAGE/online-failures.yaml"

cargo run -p gaman --all-features --bin gaman-support-matrix -- --update-readme
cargo run -p gaman --all-features --bin gaman-evidence-doc -- --update-doc
cargo run -p gaman --all-features --bin gaman-evidence-doc -- --check
cargo test -p gaman --test offline_coverage

publish() {
  local source="$1"
  local destination="$2"
  local temporary="${destination}.publish.$$"
  cp "$source" "$temporary"
  mv "$temporary" "$destination"
}

mkdir -p "$STAGE/accepted"
cp results/parser-results.yaml "$STAGE/accepted/parser-results.yaml"
cp results/offline-results.yaml "$STAGE/accepted/offline-results.yaml"
cp results/online-results.yaml "$STAGE/accepted/online-results.yaml"
cp README.md "$STAGE/accepted/README.md"
cp docs/support-evidence.md "$STAGE/accepted/support-evidence.md"
PUBLISHING=1
publish "$GAMAN_PARSER_RESULTS" results/parser-results.yaml
publish "$GAMAN_OFFLINE_RESULTS" results/offline-results.yaml
publish "$GAMAN_ONLINE_RESULTS" results/online-results.yaml
publish "$GAMAN_README_PATH" README.md
publish "$GAMAN_EVIDENCE_DOC" docs/support-evidence.md
PUBLISHING=0

unset GAMAN_PARSER_RESULTS GAMAN_OFFLINE_RESULTS GAMAN_ONLINE_RESULTS
unset GAMAN_README_PATH GAMAN_EVIDENCE_DOC
cargo test -p gaman --test offline_coverage
