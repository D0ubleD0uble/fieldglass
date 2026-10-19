// Do the two hosts report the same colour range for the same field and
// options (#1156)?
//
//   cd crates/fieldglass-napi && npx napi build --platform   # the addon
//   crates/fieldglass-wasm/build.sh nodejs                   # the wasm build
//   node crates/fieldglass-wasm/tests/node/range_hosts.mjs [--addon <path>] [--pkg <dir>]
//
// The browser module's `render`, `palette`, `renderDisplay` and
// `paletteDisplay`, against the Node addon's `renderGrid` and `renderDisplayGrid`
// in the source projection, where a pixel is a cell and the two count the same
// cells. Field and options in, and the whole `ColourRange` out must be
// identical: the ends, which were given, the center, and the counts. A range
// either refuses must be refused by both, with the same `code` and
// `rangeRefusal`, and the addon's `projectContours` must refuse it in the same
// fields as the browser's `contours`.
//
// Committed fixtures only, so it needs no sample corpus. It needs the built
// addon, which the CI job that builds the browser bundle does not build, so it
// is a local check like `parity.mjs`; the CI gate for the same agreement is the
// conformance suite, whose `render`, `renderDisplay`, `palette` and
// `paletteDisplay` cases record the range and which both hosts' runners
// replay. It fails loudly when the addon or the bundle is missing.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { isDeepStrictEqual } from 'node:util';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i === -1 ? fallback : process.argv[i + 1];
};
const ADDON = join(repoRoot, arg('--addon', 'crates/fieldglass-napi/fieldglass.linux-x64-gnu.node'));
const WASM = join(repoRoot, arg('--pkg', 'crates/fieldglass-wasm/pkg/nodejs'), 'fieldglass_wasm.js');
const G1 = join(repoRoot, 'crates/fieldglass-grib1/tests/fixtures');
const G2 = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures');

for (const [path, how] of [
  [ADDON, 'cd crates/fieldglass-napi && npx napi build --platform'],
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  build it first: ${how}`);
    process.exit(1);
  }
}

const native = require(ADDON);
const wasm = require(WASM);

let failures = 0;
let compared = 0;
function same(name, a, b) {
  compared += 1;
  if (isDeepStrictEqual(a, b)) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}:\n    node    ${JSON.stringify(a)}\n    browser ${JSON.stringify(b)}`);
    failures += 1;
  }
}

/** What a call threw, as the fields a host branches on; `null` when it did not. */
function thrown(call) {
  try {
    call();
    return null;
  } catch (e) {
    return { code: e.code ?? null, rangeRefusal: e.rangeRefusal ?? null, message: e.message };
  }
}

/** One set of range options, as each host spells it. */
function spellings(o) {
  const browser = { colormap: 'viridis', min: o.min ?? null, max: o.max ?? null, center: o.center ?? null, scale: o.scale ?? null };
  // The addon's options object takes a missing key, not `null`, for an
  // option left out.
  const node = Object.fromEntries(
    Object.entries({
      projection: 'source',
      resampling: 'nearest',
      flipY: false,
      colormap: 'viridis',
      rangeMin: o.min,
      rangeMax: o.max,
      rangeCenter: o.center,
      scaleMode: o.scale,
    }).filter(([, v]) => v !== undefined),
  );
  return { browser, node };
}

/** Options around a field's own range: automatic, both ends inside it so cells
 *  clamp at each end, one end alone, a center, log10, and two refusals. */
function optionSets(lo, hi) {
  const span = hi - lo;
  return [
    ['auto', {}],
    ['clamped', { min: lo + span / 4, max: hi - span / 4 }],
    ['min alone', { min: lo + span / 2 }],
    ['max alone', { max: lo + span / 2 }],
    ['centred', { center: lo + span / 3 }],
    ['log10', { scale: 'log10' }],
    ['log10, min given', { scale: 'log10', min: Math.max(lo, 0) + span / 4 }],
    ['refused: inverted', { min: hi, max: lo }, 'inverted'],
    ['refused: min past the data', { min: hi + span }, 'inverted'],
  ];
}

/** Both hosts refused, as `invalid_option`, with `reason` on the refusal. */
function refusedFor(label, ne, be, reason) {
  same(`${label}: the addon refuses as invalid_option`, ne?.code ?? null, 'invalid_option');
  same(`${label}: the browser refuses as invalid_option`, be?.code ?? null, 'invalid_option');
  same(`${label}: for the expected reason`, [ne?.rangeRefusal?.reason ?? null, be?.rangeRefusal?.reason ?? null], [
    reason,
    reason,
  ]);
}

function compareHosts(label, nodeRender, browserRender, browserPalette, lo, hi) {
  for (const [name, o, reason] of optionSets(lo, hi)) {
    const { node, browser } = spellings(o);
    let n = null;
    let b = null;
    const ne = thrown(() => {
      n = nodeRender(node);
    });
    const be = thrown(() => {
      b = browserRender(browser);
    });
    same(`${label} ${name}: refused alike`, ne && { code: ne.code, rangeRefusal: ne.rangeRefusal }, be && {
      code: be.code,
      rangeRefusal: be.rangeRefusal,
    });
    if (reason) {
      // Not vacuous: both must refuse, as invalid_option, for this reason.
      refusedFor(`${label} ${name}`, ne, be, reason);
      continue;
    }
    if (ne || be) continue;
    same(`${label} ${name}: the range result`, n.range, b.range);
    same(`${label} ${name}: usedMin / usedMax echo it`, [n.usedMin, n.usedMax], [b.range.usedMin, b.range.usedMax]);
    same(`${label} ${name}: the palette's range`, n.range, browserPalette(browser).range);
  }
}

// --- whole fields ----------------------------------------------------------
for (const file of [
  join(G2, 'gfs_c255_latlon.grib2'),
  join(G2, 'regular_latlon_surface.grib2'),
  join(G2, 'log_negative_regular_latlon.grib2'),
  join(G2, 'rap_jpeg2000_lambert.grib2'),
  join(G1, 'ecmwf_lfpw_msg0.grib1'),
]) {
  const bytes = readFileSync(file);
  const label = file.split('/').pop();
  const nodeHandle = native.open(bytes);
  const handle = wasm.open(new Uint8Array(bytes));
  const field = handle.decode(0, { dtype: 'f64' });
  const { min: lo, max: hi } = field.stats();
  compareHosts(
    label,
    (o) => nodeHandle.renderGrid(0, o),
    (o) => handle.render(field, o, false),
    (o) => handle.palette(field, o),
    lo,
    hi,
  );

  // Contours refuse what the render refuses, in the same fields (#1156: the
  // addon's used to arrive as prose alone).
  const { node, browser } = spellings({ min: hi, max: lo });
  const ne = thrown(() => nodeHandle.projectContours(0, node));
  const be = thrown(() => handle.contours(field, new Float64Array([]), browser));
  same(`${label} contours: refused alike`, ne && { code: ne.code, rangeRefusal: ne.rangeRefusal }, be && {
    code: be.code,
    rangeRefusal: be.rangeRefusal,
  });
  refusedFor(`${label} contours`, ne, be, 'inverted');
  field.free();
  handle.free();
}

// --- a field decoded to draw (#1082) -----------------------------------------
{
  const bytes = readFileSync(join(G2, 'rap_jpeg2000_lambert.grib2'));
  const nodeHandle = native.open(bytes);
  const handle = wasm.open(new Uint8Array(bytes));
  const full = handle.decode(0, { dtype: 'f64' });
  const stats = full.stats();
  const coarse = handle.decodeDisplay(0, { reduce: 1 });
  const nodeCoarse = nodeHandle.decodeDisplay(0, { reduce: 1 });
  compareHosts(
    'rap reduce 1',
    (o) => nodeHandle.renderDisplayGrid(nodeCoarse, stats, o),
    (o) => handle.renderDisplay(coarse, stats, o, false),
    (o) => handle.paletteDisplay(coarse, stats, o),
    stats.min,
    stats.max,
  );
  coarse.free();
  full.free();
  handle.free();
}

if (failures) {
  console.log(`${failures} of ${compared} comparison(s) failed`);
  process.exit(1);
}
console.log(`the two hosts report the same colour range in all ${compared} comparisons`);
