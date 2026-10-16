#!/usr/bin/env bash
# Repeat the issue #1087 measurements: fieldglass-napi on napi-rs's WASI targets
# against fieldglass-wasm (wasm-bindgen). Not part of any gate.
#
#     tools/research/1087-napi-wasi/run.sh build     # heavy: run it under the gate lock
#     tools/research/1087-napi-wasi/run.sh measure   # sizes, Node timings, headless Chrome
#
# Needs: rustup targets wasm32-wasip1 and wasm32-wasip1-threads, wasm-bindgen at
# the Cargo.lock version, binaryen 132 (BINARYEN, default ~/.local/opt/binaryen-version_132),
# Node 22, and a Chrome binary (CHROME, default /usr/bin/google-chrome).
# Everything it writes lands in SCRATCH (default target/research-1087), plus
# crates/fieldglass-wasm/pkg, which build.sh owns.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
S="${SCRATCH:-$ROOT/target/research-1087}"
BINARYEN="${BINARYEN:-$HOME/.local/opt/binaryen-version_132}"
export PATH="$BINARYEN/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"

stage() {
  mkdir -p "$S/js/websrc" "$S/web"
  cp "$HERE/package.json" "$S/js/"
  cp "$HERE"/websrc/*.mjs "$S/js/websrc/"
  cp "$HERE"/probe*.mjs "$S/js/"
  cp "$HERE"/web/*.html "$S/web/"
  cp "$HERE"/serve.mjs "$HERE"/bench-node.mjs "$HERE"/bycrate.py "$S/"
  [ -d "$S/js/node_modules" ] || (cd "$S/js" && npm install --no-audit --no-fund)
}

# wasm-opt -Oz with the target's own feature list, as crates/fieldglass-wasm/build.sh does.
oz() {
  local target="$1" in="$2" out="$3" f
  local feats=()
  while read -r f; do
    [ "$f" = crt-static ] && continue
    case "$f" in nontrapping-fptoint) f=nontrapping-float-to-int;; simd128) f=simd;; atomics) f=threads;; esac
    feats+=("--enable-$f")
  done < <(rustc --target "$target" --print cfg | sed -n 's/^target_feature="\(.*\)"$/\1/p')
  [ "$target" = wasm32-wasip1-threads ] && feats+=(--enable-threads)
  wasm-opt -Oz "${feats[@]}" "$in" -o "$out"
}

build() {
  stage
  cd "$ROOT"
  for t in wasm32-wasip1-threads wasm32-wasip1; do
    EMNAPI_LINK_DIR="$S/js/node_modules/emnapi/lib/$t" \
      cargo build -p fieldglass-napi --profile wasm-release --target "$t"
    oz "$t" "target/$t/wasm-release/fieldglass_napi.wasm" "$S/napi-$t.opt.wasm"
  done
  # The CLI's own build: generates the browser/Node loaders and the worker.
  (cd "$S/js" && npx napi build --platform --target wasm32-wasip1-threads --profile wasm-release \
     --manifest-path "$ROOT/crates/fieldglass-napi/Cargo.toml" --package-json-path "$S/js/package.json" \
     --target-dir "$ROOT/target" --output-dir "$S/js/out")
  rm -rf crates/fieldglass-wasm/pkg
  (cd crates/fieldglass-wasm && ./build.sh web)
  cargo build -p fieldglass-napi --release
  cp target/release/libfieldglass_napi.so "$S/fieldglass.native.node"
}

measure() {
  stage
  cd "$S"
  echo "== .wasm after -Oz (raw, gzip -9 via Python, as tools/check_wasm_bundle_size.py)"
  python3 -I -c "
import gzip, sys
for p in sys.argv[1:]:
    d = open(p, 'rb').read(); print(f'{len(d):>9} {len(gzip.compress(d, compresslevel=9, mtime=0)):>9}  {p}')
" "$S/napi-wasm32-wasip1-threads.opt.wasm" "$S/napi-wasm32-wasip1.opt.wasm" \
  "$ROOT/crates/fieldglass-wasm/pkg/web/fieldglass_wasm_bg.wasm"

  echo "== JS each needs at runtime (esbuild --minify, then gzip -9)"
  cp js/out/fieldglass.wasi-browser.js js/out/wasi-worker-browser.mjs js/websrc/
  cp "$ROOT/crates/fieldglass-wasm/pkg/web/fieldglass_wasm.js" js/websrc/
  printf "import { Buffer } from 'buffer'; globalThis.Buffer = Buffer;\n" > js/websrc/bufpoly.mjs
  mkdir -p js/min
  local e
  for e in fieldglass.wasi-browser.js wasi-worker-browser.mjs nothreads.mjs bufpoly.mjs fieldglass_wasm.js; do
    js/node_modules/.bin/esbuild "js/websrc/$e" --bundle --minify --format=esm --target=es2022 \
      --outfile="js/min/$e" --log-level=error
  done
  python3 -I -c "
import gzip, os
for p in sorted(os.listdir('js/min')):
    d = open('js/min/' + p, 'rb').read(); print(f'{len(d):>9} {len(gzip.compress(d, compresslevel=9, mtime=0)):>9}  {p}')
"

  echo "== Node decode timings"
  FIXTURES="$ROOT/crates/fieldglass-grib2/tests/fixtures" \
    node bench-node.mjs "$S/fieldglass.native.node" "$S/js/out/fieldglass.wasi.cjs" \
    "$ROOT/crates/fieldglass-wasm/pkg/web"

  echo "== headless Chrome"
  cp js/out/fieldglass.wasm32-wasi.wasm web/
  cp napi-wasm32-wasip1.opt.wasm web/fieldglass_napi_nothreads.wasm
  cp "$ROOT/crates/fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2" web/
  cp "$ROOT/crates/fieldglass-wasm/pkg/web/fieldglass_wasm.js" "$ROOT/crates/fieldglass-wasm/pkg/web/fieldglass_wasm_bg.wasm" web/
  local b=(--bundle --format=esm --target=es2022 --log-level=warning)
  js/node_modules/.bin/esbuild js/websrc/page.mjs "${b[@]}" --outfile=web/page.js
  js/node_modules/.bin/esbuild js/websrc/wasi-worker-browser.mjs "${b[@]}" --outfile=web/wasi-worker-browser.mjs
  js/node_modules/.bin/esbuild js/websrc/nothreads-page.mjs "${b[@]}" --platform=neutral \
    --main-fields=browser,module,main --external:node:fs --outfile=web/nothreads.js
  node serve.mjs > /dev/null &
  SERVER=$!
  trap 'kill "$SERVER" 2>/dev/null || true' EXIT
  sleep 1
  (cd js && node probe.mjs && node probe-nothreads.mjs)
  kill "$SERVER"
}

case "${1:-}" in
  build) build ;;
  measure) measure ;;
  *) echo "usage: $0 build|measure" >&2; exit 2 ;;
esac
