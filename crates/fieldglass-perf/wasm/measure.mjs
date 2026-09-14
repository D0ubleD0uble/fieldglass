// wasm linear memory per scenario (gated) and wall time per scenario (reported).
//
//   node crates/fieldglass-perf/wasm/measure.mjs --corpus <dir> \
//       [--pkg crates/fieldglass-wasm/pkg/nodejs] [--label baseline] > wasm.json
//
// Linear memory never shrinks, so `memory.buffer.byteLength` read after an
// operation is the most memory that operation made the module hold — a
// high-water mark with no profiler involved. It is only a *per-operation* number
// if nothing else ran in that instance first, so every scenario gets a fresh
// Node process: one instance, one prepare, one operation, one reading.
//
// The shipped glue does not export the module's memory, and adding an export to
// the published bundle for a benchmark would cost every browser download the
// bytes. So the instance is caught as the glue constructs it instead, by wrapping
// `WebAssembly.Instance` before the glue is loaded. Nothing in the package
// changes.
//
// The wasm host opens GRIB and NetCDF from bytes and has no store entry point,
// so Zarr scenarios have no wasm row; `docs/performance.md` says so.

import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = fileURLToPath(import.meta.url);
const repoRoot = join(dirname(here), '..', '..', '..');

// Kept in step with `src/lib.rs`: the plane a slice decodes, the frames a
// scrub decodes, and the contour levels.
const SLICE_PLANE = 3;
const SCRUB_FRAMES = 8;
const CONTOUR_LEVELS = new Float64Array([270, 275, 280, 285, 290, 295]);
const RENDER_INPUTS = ['grib2-5.0'];

// Median of this many timed runs, after the one run the memory reading is taken
// from. The same count as `bench.mjs`.
const ITERATIONS = 5;

function die(message) {
  console.error(message);
  process.exit(1);
}

function argValue(flag, fallback = null) {
  const i = process.argv.indexOf(flag);
  if (i === -1) return fallback;
  const value = process.argv[i + 1];
  if (value === undefined || value.startsWith('--')) die(`${flag} wants a value`);
  return value;
}

const corpusDir = argValue('--corpus') ?? die('--corpus <dir> is required');
const pkgDir = argValue('--pkg', 'crates/fieldglass-wasm/pkg/nodejs');
const label = argValue('--label', 'baseline');
const child = argValue('--child');

const glue = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
if (!existsSync(glue)) die(`missing: ${glue}\n  build it first: crates/fieldglass-wasm/build.sh nodejs`);
const corpus = JSON.parse(readFileSync(join(corpusDir, 'corpus.json'), 'utf8'));

/** Every scenario the wasm host can run, in the native catalogue's naming. */
function scenarios() {
  const out = [];
  for (const name of Object.keys(corpus.inputs).sort()) {
    const input = corpus.inputs[name];
    const family = name.slice(0, name.lastIndexOf('-'));
    let ops;
    if (input.format === 'grib1' || input.format === 'grib2') ops = ['open', 'decode'];
    else if (input.format === 'netcdf') ops = ['open', 'variables', 'slice', 'scrub'];
    else continue;
    if (RENDER_INPUTS.includes(family)) ops.push('warp', 'palette', 'render', 'contours');
    for (const op of ops) out.push(`${name}/${op}`);
  }
  return out;
}

function median(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

/** Run one scenario in this process and print its JSON line. */
function runChild(id) {
  let instance = null;
  const Original = WebAssembly.Instance;
  WebAssembly.Instance = function Instance(module, imports) {
    instance = new Original(module, imports);
    return instance;
  };
  const wasm = require(glue);
  WebAssembly.Instance = Original;
  if (!instance) die('the glue did not construct a WebAssembly.Instance; has wasm-bindgen changed how it loads?');
  const memory = () => instance.exports.memory.buffer.byteLength;

  const [name, op] = id.split('/');
  const input = corpus.inputs[name];
  const bytes = new Uint8Array(readFileSync(join(corpusDir, input.file)));

  // Prepare: everything the native scenario does before its profiler starts.
  let handle = op === 'open' ? null : wasm.open(bytes);
  let variable = 0;
  if (input.format === 'netcdf' && handle) {
    variable = handle.variables().find((v) => v.name === input.variable).index;
  }
  let field = null;
  if (['warp', 'palette', 'render', 'contours'].includes(op)) field = handle.decode(0, {});

  const operation = () => {
    switch (op) {
      case 'open': {
        const h = wasm.open(bytes);
        h.count();
        return () => h.free();
      }
      case 'decode': {
        const f = handle.decode(0, {});
        return () => f.free();
      }
      case 'variables':
        handle.variables();
        handle.dimensions();
        return () => {};
      case 'slice': {
        const f = handle.decodeSlice(variable, 1, 2, new Uint32Array([SLICE_PLANE, 0, 0]), {});
        return () => f.free();
      }
      case 'scrub':
        for (let frame = 0; frame < SCRUB_FRAMES; frame++) {
          handle.decodeSlice(variable, 1, 2, new Uint32Array([frame, 0, 0]), {}).free();
        }
        return () => {};
      case 'warp':
        handle.warp(field, { bilinear: true, bounds: null, width: null, height: null });
        return () => {};
      case 'palette':
        handle.palette(field, {});
        return () => {};
      case 'render':
        handle.render(field, {}, false);
        return () => {};
      case 'contours':
        handle.contours(field, CONTOUR_LEVELS);
        return () => {};
      default:
        die(`no wasm operation ${op}`);
    }
  };

  const before = memory();
  const release = operation();
  const after = memory();
  release();

  const timings = [];
  for (let i = 0; i < ITERATIONS; i++) {
    const start = performance.now();
    const again = operation();
    timings.push(performance.now() - start);
    again();
  }
  if (field) field.free();
  if (handle) handle.free();
  console.log(JSON.stringify({ id, before, after, ms: median(timings) }));
}

if (child) {
  runChild(child);
} else {
  const rows = {};
  const ids = scenarios();
  if (ids.length === 0) die('FAIL: the corpus has no input the wasm host opens');
  for (const id of ids) {
    const result = spawnSync(process.execPath, [here, '--corpus', corpusDir, '--pkg', pkgDir, '--child', id], {
      encoding: 'utf8',
    });
    if (result.status !== 0) die(`FAIL ${id} (${label}):\n${result.stderr}`);
    const row = JSON.parse(result.stdout.trim().split('\n').pop());
    rows[id] = { before: row.before, after: row.after, ms: row.ms };
    console.error(`${label.padEnd(9)} ${id.padEnd(34)} ${String(row.after).padStart(10)} bytes  ${row.ms.toFixed(2)} ms`);
  }
  console.log(JSON.stringify({ label, digest: corpus.digest, scenarios: rows }, null, 1));
}
