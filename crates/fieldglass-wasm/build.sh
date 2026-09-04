#!/usr/bin/env bash
# Build the browser bundle: wasm32 under the `wasm-release` profile, then
# wasm-bindgen's JS glue and `.d.ts`.
#
# `wasm-bindgen` the CLI must be the same version as the `wasm-bindgen` crate
# the build resolved, or the generated glue does not match the module's ABI.
# This script checks that rather than letting the mismatch surface as an
# "invalid schema version" at import time.
#
#     ./build.sh [web|nodejs]        (default: web)
#
# Output lands in `pkg/`, which is gitignored: it is a build product, and
# publishing it to npm is #466.
set -euo pipefail

TARGET_KIND="${1:-web}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"
OUT_DIR="$CRATE_DIR/pkg/$TARGET_KIND"

want="$(grep -A1 '^name = "wasm-bindgen"$' "$REPO_ROOT/Cargo.lock" \
        | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n1)"
have="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen CLI is $have but the build resolves the crate at $want." >&2
  echo "Install the matching one:  cargo install wasm-bindgen-cli --version $want" >&2
  exit 1
fi

cargo build --manifest-path "$REPO_ROOT/Cargo.toml" \
  -p fieldglass-wasm \
  --profile wasm-release \
  --target wasm32-unknown-unknown

wasm-bindgen \
  --target "$TARGET_KIND" \
  --out-dir "$OUT_DIR" \
  --out-name fieldglass_wasm \
  "$REPO_ROOT/target/wasm32-unknown-unknown/wasm-release/fieldglass_wasm.wasm"

echo "built $OUT_DIR ($(du -h "$OUT_DIR/fieldglass_wasm_bg.wasm" | cut -f1))"
