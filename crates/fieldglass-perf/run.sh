#!/usr/bin/env bash
# The performance harness, in one command (#743).
#
#     crates/fieldglass-perf/run.sh                 # gate: every deterministic tier
#     crates/fieldglass-perf/run.sh --write         # gate, then re-record docs/performance.md
#     crates/fieldglass-perf/run.sh --report        # gate, then the timed and real-data report
#     crates/fieldglass-perf/run.sh --only io,heap  # a subset of the gated tiers
#     crates/fieldglass-perf/run.sh --clean         # empty the real-data cache and the outputs
#
#     --prebuilt-wasm   measure the wasm bundles already in crates/fieldglass-wasm/pkg
#                       rather than building them (for a CI job that just did)
#
# The corpus is generated into a temporary directory and removed on exit, so
# nothing this writes lands in the repository; the measurements go to this
# crate's `target/perf/`, which is ignored.
#
# Every prerequisite is checked up front and a missing one is a failure, never a
# skipped tier: a gate that passes because Valgrind was not installed has
# measured nothing. docs/performance.md lists what to install.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"
OUT="$CRATE_DIR/target/perf"

ONLY="io,heap,instructions,wasm"
WRITE=0
REPORT=0
BUILD_WASM=1
while [ "$#" -gt 0 ]; do
  case "$1" in
    --write) WRITE=1 ;;
    --report) REPORT=1 ;;
    --prebuilt-wasm) BUILD_WASM=0 ;;
    --only)
      [ "$#" -ge 2 ] || { echo "--only wants a comma-separated list of tiers" >&2; exit 2; }
      ONLY="$2"
      shift
      ;;
    --clean)
      python3 "$REPO_ROOT/tools/fetch_perf_data.py" --clean
      rm -rf "$OUT" "$CRATE_DIR/target/gungraun"
      echo "removed $OUT and the Gungraun summaries"
      exit 0
      ;;
    *) sed -n '2,20p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
  esac
  shift
done

has_tier() { case ",$ONLY," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

fail() { echo "error: $*" >&2; exit 1; }

# ── Prerequisites ─────────────────────────────────────────────────────────────
command -v python3 >/dev/null || fail "python3 is required (the corpus generator)"
if has_tier instructions; then
  command -v valgrind >/dev/null \
    || fail "the instructions tier needs Valgrind (apt install valgrind); it runs on Linux only"
  want="$(grep -A1 '^name = "gungraun"$' "$CRATE_DIR/Cargo.lock" | sed -n 's/^version = "\(.*\)"$/\1/p')"
  have="$(gungraun-runner --version 2>/dev/null | awk '{print $2}' || true)"
  [ "$want" = "$have" ] \
    || fail "the instructions tier needs gungraun-runner $want (have: ${have:-none}): cargo install gungraun-runner --version $want --locked"
fi
if has_tier wasm; then
  command -v node >/dev/null || fail "the wasm tier needs Node.js"
  if [ "$BUILD_WASM" -eq 1 ]; then
    "$REPO_ROOT/crates/fieldglass-wasm/build.sh" nodejs
    "$REPO_ROOT/crates/fieldglass-wasm/build.sh" nodejs --simd
  fi
  for pkg in nodejs nodejs-simd; do
    [ -f "$REPO_ROOT/crates/fieldglass-wasm/pkg/$pkg/fieldglass_wasm_bg.wasm" ] \
      || fail "no wasm bundle at crates/fieldglass-wasm/pkg/$pkg; build it (or drop --prebuilt-wasm)"
  done
fi

CORPUS="$(mktemp -d "${TMPDIR:-/tmp}/fieldglass-perf.XXXXXX")"
trap 'rm -rf "$CORPUS"' EXIT
mkdir -p "$OUT"

echo "── corpus"
python3 "$CRATE_DIR/corpus/generate.py" "$CORPUS/inputs"
export FIELDGLASS_PERF_CORPUS="$CORPUS/inputs"

echo "── io + heap"
cargo run --manifest-path "$CRATE_DIR/Cargo.toml" --locked --release -q --bin measure >"$OUT/measured.json"

gate=(python3 "$REPO_ROOT/tools/check_perf_gate.py" --measured "$OUT/measured.json")

if has_tier instructions; then
  echo "── instructions (Callgrind)"
  # Stale summaries from an older catalogue would be read as this run's.
  rm -rf "$CRATE_DIR/target/gungraun"
  cargo bench --manifest-path "$CRATE_DIR/Cargo.toml" --locked -q --bench instructions -- \
    --save-summary=json >"$OUT/instructions.log"
  gate+=(--instructions "$CRATE_DIR/target/gungraun")
fi

if has_tier wasm; then
  echo "── wasm memory"
  node "$CRATE_DIR/wasm/measure.mjs" --corpus "$CORPUS/inputs" \
    --pkg crates/fieldglass-wasm/pkg/nodejs --label baseline >"$OUT/wasm.json"
  node "$CRATE_DIR/wasm/measure.mjs" --corpus "$CORPUS/inputs" \
    --pkg crates/fieldglass-wasm/pkg/nodejs-simd --label +simd128 >"$OUT/wasm-simd.json"
  gate+=(--wasm "baseline=$OUT/wasm.json" --wasm "+simd128=$OUT/wasm-simd.json")
fi

echo "── gate"
if [ "$WRITE" -eq 1 ]; then
  [ "$ONLY" = "io,heap,instructions,wasm" ] || fail "--write re-records every tier; drop --only"
  "${gate[@]}" --write
fi
"${gate[@]}" --only "$ONLY"

if [ "$REPORT" -eq 1 ]; then
  has_tier wasm || fail "--report prints wasm beside native, so it needs the wasm tier"
  echo "── report: real data"
  MANIFEST="$CRATE_DIR/manifests/era5.json"
  CACHE="$(python3 "$REPO_ROOT/tools/fetch_perf_data.py" --print-dir)"
  # Fetch what is missing, then prove the cache is whole without the network:
  # the report reads only what the second call verified.
  python3 "$REPO_ROOT/tools/fetch_perf_data.py" --manifest "$MANIFEST" --cache "$CACHE"
  python3 "$REPO_ROOT/tools/fetch_perf_data.py" --manifest "$MANIFEST" --cache "$CACHE" --offline
  echo "── report: wall time"
  cargo run --manifest-path "$CRATE_DIR/Cargo.toml" --locked --release -q --bin report -- \
    --manifest "$MANIFEST" --cache "$CACHE" >"$OUT/native.json"
  python3 "$CRATE_DIR/report.py" --corpus "$CORPUS/inputs" --native "$OUT/native.json" \
    --wasm "$OUT/wasm.json" --wasm-simd "$OUT/wasm-simd.json" \
    --manifest "$MANIFEST" --cache "$CACHE" | tee "$OUT/report.md"
fi
