#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
wasm_target="$repo_root/target/wasm32-unknown-unknown/wasm-release/gaman_wasm.wasm"
pkg_dir="$repo_root/gaman-wasm/pkg"

cargo build \
  --manifest-path "$repo_root/Cargo.toml" \
  -p gaman-wasm \
  --target wasm32-unknown-unknown \
  --profile wasm-release

rm -rf "$pkg_dir"
wasm-bindgen "$wasm_target" \
  --target web \
  --out-dir "$pkg_dir" \
  --out-name gaman_wasm

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt \
    -Oz \
    --enable-bulk-memory \
    --enable-nontrapping-float-to-int \
    "$pkg_dir/gaman_wasm_bg.wasm" \
    -o "$pkg_dir/gaman_wasm_bg.wasm"
fi
