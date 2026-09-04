// How long does a field take to decode under wasm, and is it still the right
// field after `wasm-opt`?
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/bench.mjs [--native native.json] \
//       [--pkg crates/fieldglass-wasm/pkg/nodejs] [--label baseline]
//
// The one load-bearing number nobody had measured (#462). Literature says
// wasm runs numeric code 1.5-3x slower than native; this makes it a figure in
// the repository, on every PR, rather than an estimate.
//
// The corpus is the three committed real-producer GRIB2 fixtures, not
// `samples/`: `samples/` is git-ignored, so a CI run would either skip (a
// benchmark that measures nothing) or fetch a live model run (a different file
// every day, and a network dependency in a lint job). The fixtures are the same
// producers at the same grid sizes and they have eccodes value oracles beside
// them, which is what lets this double as a correctness check.
//
// It is a correctness check on purpose. `wasm-opt -Oz` rewrites the module, and
// the failure mode of a bad rewrite is a fast wrong answer -- which a timing
// harness alone reports as an improvement. Every field is checked against its
// oracle before it is timed, so the size gate and the benchmark cannot both go
// green on a module that decodes garbage.
//
// `--native <file>` is the JSON from
// `cargo run --release -p fieldglass --example bench_decode`; without
// it the native column is omitted and the run says so.

import { createRequire } from 'node:module';
import { readFileSync, existsSync, appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const FIXTURES = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures');

// Median of this many timed decodes after one untimed warm-up -- the same shape
// as the native example, so the two columns are comparable.
const ITERATIONS = 5;

const CORPUS = [
  { file: 'ecmwf_ccsds_latlon.grib2', label: 'ECMWF IFS 0.25 deg, CCSDS (5.42)' },
  { file: 'hrrr_complex_spd_lambert.grib2', label: 'HRRR 3 km Lambert, complex+spd (5.3)' },
  { file: 'rap_jpeg2000_lambert.grib2', label: 'RAP 13 km Lambert, JPEG 2000 (5.40)' },
];

function die(message) {
  console.error(message);
  process.exit(1);
}

// A flag that is present but has nothing after it is a mistake, not a default:
// silently falling back would drop the native column, or benchmark the baseline
// bundle under the `+simd128` heading, and the run would still look fine.
function argValue(flag, fallback = null) {
  const i = process.argv.indexOf(flag);
  if (i === -1) return fallback;
  const value = process.argv[i + 1];
  if (value === undefined || value.startsWith('--')) die(`${flag} wants a value`);
  return value;
}

// `--pkg` so the `+simd128` variant, which `build.sh --simd` writes to
// `pkg/nodejs-simd`, is timed by the same harness rather than a second copy.
// Repository-relative, like every other path here.
const pkgDir = argValue('--pkg', 'crates/fieldglass-wasm/pkg/nodejs');
const variant = argValue('--label', 'baseline');
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');

if (!existsSync(WASM)) {
  die(`missing: ${WASM}\n  build it first: crates/fieldglass-wasm/build.sh nodejs`);
}
const wasm = require(WASM);

const nativePath = argValue('--native');
let native = null;
if (nativePath) {
  if (!existsSync(nativePath)) {
    die(
      `missing: ${nativePath}\n  produce it with: cargo run --release -p fieldglass ` +
        '--example bench_decode',
    );
  }
  const parsed = JSON.parse(readFileSync(nativePath, 'utf8'));
  native = new Map(parsed.fields.map((f) => [f.file, f]));
}

function median(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

// Present values in scan order, which is the list the eccodes oracle holds.
function present(values, mask) {
  const out = [];
  for (let i = 0; i < values.length; i++) if (mask[i] === 1) out.push(values[i]);
  return out;
}

function checkAgainstOracle(label, file, handle) {
  const oracle = JSON.parse(readFileSync(join(FIXTURES, `${file.replace(/\.grib2$/, '')}_expected.json`), 'utf8'));
  const tol = oracle.tolerance_absolute;
  // A degenerate oracle would let every assertion below pass vacuously, which is
  // the one way this check could go quiet without anyone noticing.
  if (!(oracle.count > 0) || !(tol > 0) || Object.keys(oracle.samples ?? {}).length === 0) {
    die(`FAIL ${label}: ${file}'s oracle has no count, tolerance or samples to check against`);
  }

  const field = handle.decode(0, { dtype: 'f64' });
  const values = present(field.values(), field.mask());
  const problems = [];

  if (values.length !== oracle.count) {
    problems.push(`present count ${values.length}, oracle ${oracle.count}`);
  }
  if (values.length > 0) {
    let min = Infinity;
    let max = -Infinity;
    let sum = 0;
    for (const v of values) {
      if (v < min) min = v;
      if (v > max) max = v;
      sum += v;
    }
    const stats = { min, max, mean: sum / values.length };
    for (const key of ['min', 'max', 'mean']) {
      if (Math.abs(stats[key] - oracle[key]) >= tol) {
        problems.push(`${key} ${stats[key]} vs oracle ${oracle[key]} (tolerance ${tol})`);
      }
    }
    for (const [idx, want] of Object.entries(oracle.samples)) {
      const got = values[Number(idx)];
      if (!(Math.abs(got - want) < tol)) {
        problems.push(`values[${idx}] ${got} vs oracle ${want}`);
      }
    }
  }
  field.free();
  if (problems.length) {
    die(`FAIL ${label}: the wasm build does not decode this fixture correctly\n  ${problems.join('\n  ')}`);
  }
}

const rows = [];
for (const { file, label } of CORPUS) {
  const path = join(FIXTURES, file);
  if (!existsSync(path)) die(`missing committed fixture: ${path}`);
  const bytes = readFileSync(path);

  const openStart = performance.now();
  const handle = wasm.open(new Uint8Array(bytes));
  const openMs = performance.now() - openStart;

  // Correctness before speed: see the note at the top.
  checkAgainstOracle(label, file, handle);

  const warm = handle.decode(0, {});
  const points = warm.values().length;
  const dtype = warm.dtype();
  warm.free();

  const timings = [];
  for (let i = 0; i < ITERATIONS; i++) {
    const start = performance.now();
    const field = handle.decode(0, {});
    timings.push(performance.now() - start);
    field.free();
  }
  handle.free();

  rows.push({ file, label, points, dtype, openMs, ms: median(timings) });
}

if (rows.length === 0) die('FAIL: nothing was measured');

const header = native
  ? '| Field | Points | dtype | native ms | wasm ms | wasm / native |'
  : '| Field | Points | dtype | wasm ms |';
const rule = native ? '|---|---:|---|---:|---:|---:|' : '|---|---:|---|---:|';
const lines = [
  `### wasm decode benchmark - ${variant} (median of ${ITERATIONS}, Node ${process.version})`,
  '',
  header,
  rule,
];

for (const row of rows) {
  const points = row.points.toLocaleString('en-US');
  if (native) {
    const n = native.get(row.file);
    if (!n) die(`FAIL: the native run has no entry for ${row.file}, so the columns do not line up`);
    const ratio = (row.ms / n.decodeMs).toFixed(2);
    lines.push(
      `| ${row.label} | ${points} | ${row.dtype} | ${n.decodeMs.toFixed(1)} | ${row.ms.toFixed(1)} | ${ratio}x |`,
    );
  } else {
    lines.push(`| ${row.label} | ${points} | ${row.dtype} | ${row.ms.toFixed(1)} |`);
  }
}
if (!native) {
  lines.push('', '_No native column: pass `--native <file>` from the `bench_decode` example._');
}

const report = lines.join('\n');
console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${report}\n\n`);
}
