// Does the wasm build decode the same numbers the shipped Node addon does?
//
//   node crates/fieldglass-wasm/tests/node/parity.mjs
//
// Two independent builds of the same decoders — one native, one wasm32 — asked
// for the same message. Any difference is a target-dependent bug (a `usize`
// that is 32 bits on wasm, a float path that reassociated), which is exactly
// the class the Rust suite cannot see because it only ever runs on the host.
//
// This is a **local / manual** check, not a CI gate: it needs the real sample
// files, which are git-ignored (see `samples/README.md`), and the built addon.
// It therefore fails loudly when either is missing rather than skipping — a
// parity check that quietly passes because it compared nothing is worse than
// no parity check.
//
// Prerequisites:
//   tools/fetch_samples.sh gfs hrrr rap        # the sample corpus
//   cd crates/fieldglass-napi && npx napi build --platform   # the addon
//   crates/fieldglass-wasm/build.sh nodejs                   # the wasm build

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const ADDON = join(repoRoot, 'crates/fieldglass-napi/fieldglass.linux-x64-gnu.node');
const WASM = join(repoRoot, 'crates/fieldglass-wasm/pkg/nodejs/fieldglass_wasm.js');
const SAMPLES = ['samples/hrrr.grib2', 'samples/rap.grib2'];

function require_file(path, how) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  build or fetch it first: ${how}`);
    process.exit(1);
  }
  return path;
}

require_file(ADDON, 'cd crates/fieldglass-napi && npx napi build --platform');
require_file(WASM, 'crates/fieldglass-wasm/build.sh nodejs');

const native = require(ADDON);
const wasm = require(WASM);

let failures = 0;
let comparedCells = 0;

for (const rel of SAMPLES) {
  const path = require_file(join(repoRoot, rel), `tools/fetch_samples.sh ${rel.split('/')[1].split('.')[0]}`);
  const bytes = readFileSync(path);

  const nativeHandle = native.Grib2Handle.fromBytes(bytes);
  const nativeGrid = nativeHandle.decodeGrid(0);

  const handle = wasm.open(new Uint8Array(bytes));
  const field = handle.decode(0, { dtype: 'f64' });

  const label = rel;
  const problems = [];

  if (field.ni() !== nativeGrid.width || field.nj() !== nativeGrid.height) {
    problems.push(
      `dimensions: wasm ${field.ni()}x${field.nj()}, native ${nativeGrid.width}x${nativeGrid.height}`,
    );
  }

  const values = field.values();
  const mask = field.mask();
  const n = Math.min(values.length, nativeGrid.values.length);
  if (values.length !== nativeGrid.values.length) {
    problems.push(`value count: wasm ${values.length}, native ${nativeGrid.values.length}`);
  }

  let maskDiff = 0;
  let valueDiff = 0;
  let firstDiff = null;
  for (let k = 0; k < n; k++) {
    const nativePresent = nativeGrid.mask[k] === 1;
    const wasmPresent = mask[k] === 1;
    if (nativePresent !== wasmPresent) {
      // The umbrella also masks a non-finite decoded value, which the addon
      // leaves present with a NaN in it. That is a deliberate difference and
      // not a decode disagreement, so it is reported separately.
      if (nativePresent && !Number.isFinite(nativeGrid.values[k])) continue;
      maskDiff++;
      continue;
    }
    if (!wasmPresent) continue;
    comparedCells++;
    if (values[k] !== nativeGrid.values[k]) {
      valueDiff++;
      if (firstDiff === null) firstDiff = { k, wasm: values[k], native: nativeGrid.values[k] };
    }
  }
  if (maskDiff) problems.push(`${maskDiff} cells disagree about being present`);
  if (valueDiff) {
    problems.push(
      `${valueDiff} of ${n} values differ; first at ${firstDiff.k}: ` +
        `wasm ${firstDiff.wasm}, native ${firstDiff.native}`,
    );
  }

  if (problems.length) {
    failures++;
    console.error(`FAIL ${label}\n  ${problems.join('\n  ')}`);
  } else {
    const grid = field.grid();
    console.log(
      `ok   ${label}: ${field.ni()}x${field.nj()} ${grid.kind}, ` +
        `${n} values identical, dtype ${field.dtype()}`,
    );
  }

  field.free();
  handle.free();
}

if (comparedCells === 0) {
  console.error('FAIL: no cells were compared, so nothing was proved');
  process.exit(1);
}
console.log(`${comparedCells} present cells compared`);
process.exit(failures === 0 ? 0 : 1);
