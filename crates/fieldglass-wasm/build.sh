#!/usr/bin/env bash
# Build the browser bundle: wasm32 under the `wasm-release` profile, then
# wasm-bindgen's JS glue and `.d.ts`, then `wasm-opt -Oz`.
#
# `wasm-bindgen` the CLI must be the same version as the `wasm-bindgen` crate
# the build resolved, or the generated glue does not match the module's ABI.
# This script checks that rather than letting the mismatch surface as an
# "invalid schema version" at import time.
#
#     ./build.sh [web|nodejs] [--no-opt] [--simd]
#
#     --no-opt   skip wasm-opt (for a clone without binaryen; the sizes it
#                prints are then not the shipped ones)
#     --simd     add `-C target-feature=+simd128`, into `pkg/<kind>-simd`
#
# Output lands in `pkg/`, which is gitignored: it is a build product, and
# publishing it to npm is #466.
set -euo pipefail

TARGET_KIND=web
RUN_WASM_OPT=1
SIMD=0
for arg in "$@"; do
  case "$arg" in
    web|nodejs) TARGET_KIND="$arg" ;;
    --no-opt)   RUN_WASM_OPT=0 ;;
    --simd)     SIMD=1 ;;
    *) echo "usage: $0 [web|nodejs] [--no-opt] [--simd]" >&2; exit 2 ;;
  esac
done

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"
OUT_DIR="$CRATE_DIR/pkg/$TARGET_KIND"
[ "$SIMD" -eq 1 ] && OUT_DIR="$OUT_DIR-simd"

want="$(grep -A1 '^name = "wasm-bindgen"$' "$REPO_ROOT/Cargo.lock" \
        | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n1)"
have="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen CLI is $have but the build resolves the crate at $want." >&2
  echo "Install the matching one:  cargo install wasm-bindgen-cli --version $want" >&2
  exit 1
fi

if [ "$RUN_WASM_OPT" -eq 1 ] && ! command -v wasm-opt >/dev/null 2>&1; then
  echo "wasm-opt is not on PATH, and the shipped bundle is the -Oz output." >&2
  echo "Install binaryen (https://github.com/WebAssembly/binaryen/releases)," >&2
  echo "or pass --no-opt to build an unoptimised bundle." >&2
  exit 1
fi

# One list, used twice: once as the flag cargo compiles with, once as the flag
# `rustc --print cfg` is asked under, so the wasm-opt feature set below matches
# what was actually built rather than the bare target's defaults.
TARGET_FEATURE_ARGS=()
if [ "$SIMD" -eq 1 ]; then
  TARGET_FEATURE_ARGS=(-C target-feature=+simd128)
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+simd128"
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

WASM="$OUT_DIR/fieldglass_wasm_bg.wasm"

if [ "$RUN_WASM_OPT" -eq 1 ]; then
  # wasm-opt validates against its *own* default feature set, not the module's,
  # so a plain `-Oz` fails on every `memory.copy` rustc emits ("require bulk
  # memory operations"). Enable exactly what the target turns on, read from the
  # toolchain rather than hardcoded, so a rustc that enables a new proposal is a
  # loud "Unknown option" here instead of a silently skipped optimisation.
  # Three of the names differ between rustc and binaryen.
  features=()
  while read -r feature; do
    case "$feature" in
      nontrapping-fptoint) feature=nontrapping-float-to-int ;;
      simd128)             feature=simd ;;
      atomics)             feature=threads ;;
    esac
    features+=("--enable-$feature")
  done < <(rustc --target wasm32-unknown-unknown "${TARGET_FEATURE_ARGS[@]}" --print cfg \
             | sed -n 's/^target_feature="\(.*\)"$/\1/p')

  wasm-opt -Oz "${features[@]}" "$WASM" -o "$WASM.opt"
  mv "$WASM.opt" "$WASM"
fi

# Raw bytes only. The gzipped figure -- the one the README records and CI gates
# -- comes from `tools/check_wasm_bundle_size.py`, because GNU and BSD `gzip`
# produce different sizes for the same input and a script that disagreed with
# the gate on a maintainer's machine would be worse than not printing it.
opt_note=$([ "$RUN_WASM_OPT" -eq 1 ] && echo "-Oz" || echo "no wasm-opt")
echo "built $OUT_DIR ($opt_note): $(wc -c <"$WASM") bytes"
echo "gzipped, and checked against the README: python3 tools/check_wasm_bundle_size.py --help"
