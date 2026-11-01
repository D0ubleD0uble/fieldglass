#!/usr/bin/env bash
# Assemble the `@fieldglass/wasm` npm package from two `--target web` builds,
# the default entry and `full/` (#1158), and produce the tarball npm would
# publish (#466).
#
#     ./pack.sh [--no-opt]
#
#     --no-opt   passed through to build.sh, for a clone without binaryen. The
#                tarball is then NOT the one to publish: the shipped module is
#                the `wasm-opt -Oz` output, and the size the README records is
#                measured on that.
#
# Output lands in `pkg/npm/`, which is gitignored, together with the `.tgz`.
# Publishing it is `npm publish` from that directory, which release.yml does on
# a `v*` tag under npm Trusted Publishing.
#
# The version comes from the workspace `Cargo.toml`, never from an argument:
# the extension, the library crates and this package share one version, and a
# package.json that could be given a different one is a way for them to drift.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"
OUT_DIR="$CRATE_DIR/pkg/npm"

# Two entries (#1158): the default one, `@fieldglass/wasm`, and the full one,
# `@fieldglass/wasm/full` in the package's `full/` directory, built with the
# features below. The owner chose a second entry over growing the default one
# (#1081, #1083) because it can later be folded into one bundle, with `full`
# kept as an alias, without breaking anyone.
FULL_FEATURES=fetchplan,zarr
WEB_DIR="$CRATE_DIR/pkg/web"
FULL_DIR="$CRATE_DIR/pkg/web-${FULL_FEATURES//,/-}"

"$CRATE_DIR/build.sh" web "$@"
"$CRATE_DIR/build.sh" web "--features=$FULL_FEATURES" "$@"

version="$(grep -m1 '^version' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
if [ -z "$version" ]; then
  echo "could not read the workspace version from Cargo.toml" >&2
  exit 1
fi

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Exactly the four artifacts `package.json` lists under `files` for each entry,
# plus the two documents. Copied by name rather than with a glob so that a new
# file appearing in a build — a `snippets/` directory, say, which wasm-bindgen
# emits for inline JS — fails here rather than being silently left out of the
# package.
copy_entry() {
  local from="$1" to="$2" artifact
  mkdir -p "$to"
  for artifact in \
    fieldglass_wasm.js \
    fieldglass_wasm.d.ts \
    fieldglass_wasm_bg.wasm \
    fieldglass_wasm_bg.wasm.d.ts
  do
    if [ ! -f "$from/$artifact" ]; then
      echo "missing build artifact: $from/$artifact" >&2
      exit 1
    fi
    cp "$from/$artifact" "$to/"
  done

  if [ -e "$from/snippets" ]; then
    echo "wasm-bindgen emitted $from/snippets/, which package.json does not ship." >&2
    echo "Add it to \`files\` and \`exports\` before publishing." >&2
    exit 1
  fi
}
copy_entry "$WEB_DIR" "$OUT_DIR"
copy_entry "$FULL_DIR" "$OUT_DIR/full"

cp "$CRATE_DIR/npm/NOTICE" "$OUT_DIR/NOTICE"
# The license texts themselves, which NOTICE only names. Both licenses ask for
# their text to ship with the software, and 0.6.0 went out without it (#1545).
cp "$REPO_ROOT/LICENSE-MIT" "$OUT_DIR/LICENSE-MIT"
cp "$REPO_ROOT/LICENSE-APACHE" "$OUT_DIR/LICENSE-APACHE"
cp "$CRATE_DIR/npm/README.md" "$OUT_DIR/README.md"

# The committed manifest carries `0.0.0`; the workspace version goes in here.
# `npm version` would rewrite the committed file and wants a git tree, so the
# substitution is done on the copy.
python3 - "$CRATE_DIR/npm/package.json" "$OUT_DIR/package.json" "$version" <<'PY'
import json
import sys

source, destination, version = sys.argv[1], sys.argv[2], sys.argv[3]
with open(source, encoding="utf-8") as f:
    manifest = json.load(f)
if manifest["version"] != "0.0.0":
    raise SystemExit(
        f"{source} should carry the 0.0.0 placeholder, found {manifest['version']!r}"
    )
manifest["version"] = version
with open(destination, "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY

# `npm pack` prints the file list and the packed size, and its exit status is
# the check that `files` names nothing missing.
cd "$OUT_DIR"
npm pack

echo
echo "packed @fieldglass/wasm ${version} in ${OUT_DIR}"
