// The browser host, run through the ADR-0006 conformance suite (#573).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/conformance.mjs
//
// The suite is `crates/fieldglass/conformance/suite.json`: cases and expected
// observations, recorded by `fieldglass`'s own runner. Nothing here is a
// translation of that runner — this file reads the same JSON and drives the
// **built wasm module through its JavaScript surface**, which is the only way
// the browser binding's own work (serde-wasm-bindgen conversion, the typed
// arrays, the error mapping) is under test at all.
//
// Three runners share this one file of expectations:
//
//   fieldglass/tests/conformance.rs        Session, native and wasm32-wasip1
//   fieldglass-napi/src/conformance_host   the napi handles
//   this file                              the browser bundle, from Node
//
// It is a CI gate, not a manual check: unlike `parity.mjs` it needs no sample
// corpus, only the committed fixtures and a built `pkg/nodejs`. It therefore
// fails loudly when the bundle is missing rather than skipping — a conformance
// runner that quietly compared nothing is worse than none.
//
// # One difference from the Rust comparator, stated
//
// The Rust side compares JSON integers exactly and only real-valued leaves
// within the tolerance (ADR-0009: the discrete decisions agree exactly across
// targets, so comparing them loosely gives away coverage). JavaScript has one
// number type and `JSON.parse` cannot tell `1` from `1.0`, so this runner
// applies the tolerance to every number. The distinction costs nothing here:
// the tolerance is `1e-9 + 1e-9 * |expected|` and no count, length or byte in
// the suite exceeds ~10^5, so the slack on a discrete value stays below 10^-4
// where the smallest real difference is 1.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

// `--pkg <dir>` picks a different build — the `+simd128` one, say, which is
// where a float result would most plausibly move.
const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const SUITE = join(repoRoot, 'crates/fieldglass/conformance/suite.json');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [SUITE, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);
const suite = JSON.parse(readFileSync(SUITE, 'utf8'));

// ---------------------------------------------------------------------------
// The observation
// ---------------------------------------------------------------------------

/** `fieldglass::conformance::sample_indices`, to the integer. */
function sampleIndices(len) {
  if (len === 0) return [];
  const out = [0, Math.floor(len / 4), Math.floor(len / 2), Math.floor(len / 4) * 3, len - 1];
  return [...new Set(out)].sort((a, b) => a - b);
}

/** A real-valued leaf, in the spelling `fieldglass::conformance::real` uses:
 *  `null` for absent, the number when it is finite, and the *name*
 *  `"nonFinite"` otherwise. Deliberately not `"NaN"` / `"Infinity"`: JavaScript
 *  and Rust spell those differently, and which non-finite it was is not
 *  something the hosts have to agree on. */
function real(v) {
  if (v === null || v === undefined) return null;
  return Number.isFinite(v) ? v : 'nonFinite';
}

/** `undefined` is how napi and serde-wasm-bindgen both spell Rust's `None`;
 *  the recording spells it `null`. Normalise so a missing key and an absent
 *  value stay distinguishable from each other.
 *
 *  Only plain objects and arrays are walked. A `Map` — what serde-wasm-bindgen
 *  gives a Rust map by default — comes back untouched, and `compare` then
 *  reports every key as missing, which is a loud failure rather than a silent
 *  pass. No DTO holds a map today; if one does, this is where to teach it. */
function nulled(v) {
  if (v === undefined) return null;
  if (Array.isArray(v)) return v.map(nulled);
  if (v && typeof v === 'object' && v.constructor === Object) {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, nulled(x)]));
  }
  return v;
}

/** A `Georef` as the suite records one: everything but `geometry`, which is
 *  `core`'s tagged enum and deliberately outside the host contract. */
function georef(grid) {
  const { geometry, ...rest } = nulled(grid);
  void geometry;
  return rest;
}

function paletteOptions(args) {
  return {
    colormap: args.colormap ?? null,
    reversed: args.reversed ?? false,
    min: args.rangeMin ?? null,
    max: args.rangeMax ?? null,
    scale: args.scale ?? null,
  };
}

function decodeOptions(args) {
  return { dtype: args.dtype ?? 'auto' };
}

/** What a decoded field contributes to an observation —
 *  `fieldglass::conformance::field_value`. Shared by `decode` and `combine`,
 *  which record the same shape because `combine` answers a `Field` too. */
function fieldObservation(field) {
  const values = field.values();
  const mask = field.mask();
  return {
    dtype: field.dtype(),
    len: values.length,
    maskLen: mask.length,
    maskOnes: mask.reduce((n, m) => n + (m === 1 ? 1 : 0), 0),
    ni: field.ni(),
    nj: field.nj(),
    parameter: field.parameter(),
    units: field.units(),
    stats: nulled(field.stats()),
    georef: georef(field.grid()),
    samples: sampleIndices(values.length).map((i) => ({
      i,
      // Read the mask first, as the DTO's own doc says: the buffer holds
      // *something* at a masked slot and it is not data.
      v: mask[i] === 1 ? real(values[i]) : null,
      m: mask[i],
    })),
  };
}

/** What the browser host answers for one case. Failures become the same
 *  `{ error: { code, hasMessage } }` shape the Rust runner records — and the
 *  code is really there, because `throw()` sets it on the JS `Error`. */
function observe(caseSpec) {
  const path = join(repoRoot, 'crates', caseSpec.fixture);
  let bytes = readFileSync(path);
  if (caseSpec.args.truncate !== null && caseSpec.args.truncate !== undefined) {
    bytes = bytes.subarray(0, Math.min(caseSpec.args.truncate, bytes.length));
  }
  try {
    return run(caseSpec, new Uint8Array(bytes));
  } catch (e) {
    return {
      error: {
        code: e && typeof e.code === 'string' ? e.code : null,
        hasMessage: Boolean(e && e.message),
      },
    };
  }
}

function run(caseSpec, bytes) {
  const handle = wasm.open(bytes);
  // The host's own memory contract: linear memory never shrinks, so both the
  // field and the handle are released as soon as the observation is built.
  // 149 leaked sessions over the committed corpus is not much, but a runner
  // that ignores the contract it is checking is not a good example of it.
  try {
    return withHandle(caseSpec, handle);
  } finally {
    handle.free();
  }
}

function withHandle(caseSpec, handle) {
  const { op, args } = caseSpec;

  if (op === 'open') {
    return { format: handle.format(), count: handle.count(), addressing: handle.addressing() };
  }
  if (op === 'message') {
    const info = nulled(handle.message(args.index));
    if (info.grid) info.grid = georef(info.grid);
    return info;
  }
  // The variable addressing mode (#679): a listing, and a slice of a variable
  // rather than a message decoded by index.
  if (op === 'variables') return nulled(handle.variables());
  if (op === 'dimensions') return nulled(handle.dimensions());
  if (op === 'decodeSlice') {
    const slice = handle.decodeSlice(
      args.variable,
      args.yDim,
      args.xDim,
      new Uint32Array(args.sliceIndices ?? []),
      decodeOptions(args),
    );
    try {
      return fieldObservation(slice);
    } finally {
      slice.free();
    }
  }

  // Everything else decodes first. The field is owned by this side, so it is
  // freed as soon as the observation is built.
  const field = handle.decode(args.index, decodeOptions(args));
  try {
    switch (op) {
      case 'decode':
        return fieldObservation(field);
      case 'combine': {
        // Field B is a second decode of the same message: `combine` takes two
        // fields, and every GRIB fixture in the corpus holds one message, so
        // the suite has no second index to point at. Freed here rather than in
        // the `finally` below, which owns field A.
        const b = handle.decode(args.index, decodeOptions(args));
        try {
          const out = handle.combine(field, b, args.combineOp);
          try {
            return fieldObservation(out);
          } finally {
            out.free();
          }
        } finally {
          b.free();
        }
      }
      case 'warp': {
        const out = handle.warp(field, {
          bilinear: args.bilinear ?? true,
          bounds: args.bounds ?? null,
          // The caller's own output raster (#465). Passed through as `null`
          // rather than omitted when the case does not name one, so this
          // adapter states the whole option set the façade takes.
          width: args.width ?? null,
          height: args.height ?? null,
        });
        return {
          width: out.width,
          height: out.height,
          bounds: Array.from(out.bounds),
          len: out.values.length,
          maskOnes: out.mask.reduce((n, m) => n + (m === 1 ? 1 : 0), 0),
          samples: sampleIndices(out.values.length).map((i) => ({
            i,
            v: out.mask[i] === 1 ? real(out.values[i]) : null,
            m: out.mask[i],
          })),
        };
      }
      case 'palette': {
        const p = handle.palette(field, paletteOptions(args));
        return {
          t0: real(p.t0),
          t1: real(p.t1),
          scale: p.scale,
          lutLen: p.lut.length,
          lutSamples: sampleIndices(p.lut.length).map((i) => ({ i, b: p.lut[i] })),
          maskedRgba: Array.from(p.maskedRgba),
        };
      }
      case 'render': {
        const rgba = handle.render(field, paletteOptions(args), args.flipY ?? false);
        let opaque = 0;
        for (let k = 3; k < rgba.length; k += 4) if (rgba[k] === 255) opaque += 1;
        return {
          width: field.ni(),
          height: field.nj(),
          rgbaLen: rgba.length,
          opaque,
          pixels: sampleIndices(rgba.length / 4).map((i) => ({
            i,
            rgba: Array.from(rgba.subarray(i * 4, i * 4 + 4)),
          })),
        };
      }
      case 'probe': {
        const p = handle.probe(field, args.lat ?? 0, args.lon ?? 0);
        return p === undefined ? null : nulled(p);
      }
      case 'contours': {
        const lines = handle.contours(field, new Float64Array(args.levels ?? []));
        return {
          levelCount: lines.length,
          levels: lines.map((l) => real(l.value)),
          segmentCounts: lines.map((l) => l.segments.length),
        };
      }
      default:
        throw new Error(`the suite names an operation this runner does not know: ${op}`);
    }
  } finally {
    field.free();
  }
}

// ---------------------------------------------------------------------------
// The comparison — `fieldglass::conformance::compare`, in JavaScript
// ---------------------------------------------------------------------------

function compare(expected, observed, tol, path = '$', out = []) {
  const kind = (v) => (v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v);
  const ke = kind(expected);
  const ko = kind(observed);
  if (ke !== ko) {
    out.push(`${path}: expected ${ke} ${JSON.stringify(expected)}, got ${ko} ${JSON.stringify(observed)}`);
    return out;
  }
  if (ke === 'object') {
    const a = Object.keys(expected).sort();
    const b = Object.keys(observed).sort();
    const missing = a.filter((k) => !b.includes(k));
    const extra = b.filter((k) => !a.includes(k));
    if (missing.length) out.push(`${path}: missing keys ${JSON.stringify(missing)}`);
    if (extra.length) out.push(`${path}: unexpected keys ${JSON.stringify(extra)}`);
    for (const k of a) if (b.includes(k)) compare(expected[k], observed[k], tol, `${path}.${k}`, out);
    return out;
  }
  if (ke === 'array') {
    if (expected.length !== observed.length) {
      out.push(`${path}: length ${expected.length} != ${observed.length}`);
      return out;
    }
    expected.forEach((v, i) => compare(v, observed[i], tol, `${path}[${i}]`, out));
    return out;
  }
  if (ke === 'number') {
    // Written as a negated `<=`, not as `>`, for one reason: `NaN > x` is
    // false and `NaN <= x` is false, so the `>` spelling accepts a NaN
    // observation against any expectation. The recording can never hold one
    // (`real()` on the Rust side names a non-finite rather than writing it),
    // so a NaN here is always a browser-host result the reference does not
    // share — exactly what this runner exists to catch. The Rust comparator
    // is the same way round.
    if (!(Math.abs(expected - observed) <= tol.absolute + tol.relative * Math.abs(expected))) {
      out.push(`${path}: ${expected} != ${observed}`);
    }
    return out;
  }
  if (expected !== observed) out.push(`${path}: ${JSON.stringify(expected)} != ${JSON.stringify(observed)}`);
  return out;
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

if (!Array.isArray(suite.cases) || suite.cases.length === 0) {
  console.error('the conformance suite holds no cases — it would pass while checking nothing');
  process.exit(1);
}

let failures = 0;
const codesSeen = new Set();
for (const entry of suite.cases) {
  const { expect, ...caseSpec } = entry;
  const observed = observe(caseSpec);
  if (observed && observed.error && typeof observed.error.code === 'string') {
    codesSeen.add(observed.error.code);
  }
  const lines = compare(expect, observed, suite.tolerance);
  if (lines.length) {
    failures += 1;
    console.error(`FAIL ${caseSpec.id}`);
    for (const line of lines.slice(0, 8)) console.error(`       ${line}`);
    if (lines.length > 8) console.error(`       … and ${lines.length - 8} more`);
  }
}

// `Error::code()` is the stable half of the contract and this host is the one
// that actually exposes it (`throw()` sets `e.code`). So the browser runner is
// where "the codes are reachable through a binding" is really proved.
const missingCodes = suite.errorCodes.filter((c) => !codesSeen.has(c));
if (missingCodes.length) {
  failures += 1;
  console.error(`FAIL error codes never reached through this binding: ${missingCodes.join(', ')}`);
}

const label = `${suite.cases.length} cases, ${suite.errorCodes.length} error codes`;
if (failures) {
  console.error(`\nbrowser host (${pkgDir}): ${failures} conformance failure(s) over ${label}`);
  process.exit(1);
}
console.log(`browser host (${pkgDir}): conformance OK (${label})`);
