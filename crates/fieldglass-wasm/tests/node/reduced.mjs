// A reduced-resolution decode through the browser module (#1082).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/reduced.mjs
//
// `decodeDisplay(i, { reduce })` decodes a JPEG 2000 message at a coarser
// wavelet level, which is the slowest decode in this module made fast for a
// zoomed-out map. What comes back is a `WasmDisplayField`, not a `WasmField`:
// `renderDisplay`, `warpDisplay` and `paletteDisplay` draw it, and its colour
// range is the full field's, from the stats the page passes in. The Rust
// tests (`crates/fieldglass/tests/display_field.rs`) check the rules; this
// checks the binding's part: the options object reaching the decode, the
// derived grid crossing as `grid()`, the stats crossing back in, and a
// refusal arriving with its `code`.
//
// That a display field cannot be handed to `probe`, `contours` or `combine` is
// a TypeScript error, checked by `package.mjs` against the published
// declarations. At runtime wasm-bindgen refuses the wrong class too, and that
// is checked here.
//
// The RAP fixture is NCEP's own surface temperature on a Lambert grid,
// 451 × 337 at 13,545 m. A CI gate: it fails rather than skips when the bundle
// or the fixture is missing.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { runInNewContext } from 'node:vm';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const RAP = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2');
const SIMPLE = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [RAP, 'it is committed; check out the repository'],
  [SIMPLE, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);

let failures = 0;
function equal(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}: got ${a}, expected ${e}`);
    failures += 1;
  }
}

/** What a call threw: its `code` and `message`, or `null` when it did not. */
function thrown(call) {
  try {
    call();
    return null;
  } catch (e) {
    return { code: e.code ?? null, message: String(e.message) };
  }
}

const handle = wasm.open(new Uint8Array(readFileSync(RAP)));
const full = handle.decode(0, {});
const fullGrid = full.grid();
const stats = full.stats();

// --- the decode ---------------------------------------------------------------
const coarse = handle.decodeDisplay(0, { reduce: 1 });
equal('the reduction is reported', coarse.reduction(), 1);
equal('half the columns, rounded up', coarse.ni(), Math.ceil(full.ni() / 2));
equal('half the rows, rounded up', coarse.nj(), Math.ceil(full.nj() / 2));

const grid = coarse.grid();
equal('the grid states the coarse shape', [grid.ni, grid.nj], [coarse.ni(), coarse.nj()]);
equal('the family is the message’s', [grid.kind, grid.label], [fullGrid.kind, fullGrid.label]);
// The derived grid: the same first point and projection, every second point,
// so twice the spacing. Pairing the coarse values with the message's own grid
// would draw the field at half its size.
equal(
  'the first point and the projection are kept',
  [grid.geometry.lat_first, grid.geometry.lon_first, grid.geometry.lov, grid.geometry.latin1],
  [fullGrid.geometry.lat_first, fullGrid.geometry.lon_first, fullGrid.geometry.lov, fullGrid.geometry.latin1],
);
equal(
  'the spacing doubles',
  [grid.geometry.dx_metres, grid.geometry.dy_metres, grid.affine.dx, grid.affine.dy],
  [2 * fullGrid.geometry.dx_metres, 2 * fullGrid.geometry.dy_metres, 2 * fullGrid.affine.dx, 2 * fullGrid.affine.dy],
);
equal('the scan is the message’s', grid.scan, fullGrid.scan);
equal('the labels are the message’s', [coarse.parameter(), coarse.units()], [full.parameter(), full.units()]);
equal('no truncation', coarse.truncation(), null);
equal('no values, stats or mask to measure with', [coarse.values, coarse.stats, coarse.mask], [undefined, undefined, undefined]);

// Reduction zero, and no options at all, are `decode`'s field.
for (const [what, options] of [['reduce: 0', { reduce: 0 }], ['no options', undefined]]) {
  const same = handle.decodeDisplay(0, options);
  equal(`${what} is the message's own grid`, [same.reduction(), same.grid()], [0, fullGrid]);
  same.free();
}

// --- drawing ------------------------------------------------------------------
const painted = handle.renderDisplay(coarse, stats, { colormap: 'viridis' }, false);
const { rgba } = painted;
equal('renderDisplay paints the coarse raster', [rgba.length, painted.width, painted.height], [
  coarse.ni() * coarse.nj() * 4,
  coarse.ni(),
  coarse.nj(),
]);
let opaque = 0;
for (let k = 3; k < rgba.length; k += 4) if (rgba[k] !== 0) opaque += 1;
equal('every coarse cell is painted', opaque, coarse.ni() * coarse.nj());

// The automatic range is the full field's: the palette is the full field's
// palette, whatever the coarse values reach.
const shown = handle.paletteDisplay(coarse, stats, {});
const reference = handle.palette(full, {});
equal('paletteDisplay ranges over the full field’s stats', [shown.t0, shown.t1], [reference.t0, reference.t1]);
equal('and is the same lookup table', Array.from(shown.lut), Array.from(reference.lut));

// Both report the range they used (#1156): the full field's ends, with the
// coarse field's own cells counted. A low-pass overshoots the full field's
// maximum here, and those cells take the top colour and say so.
equal('renderDisplay reports the full field’s range', [painted.range.usedMin, painted.range.usedMax], [stats.min, stats.max]);
equal('paletteDisplay reports the range renderDisplay used', shown.range, painted.range);
equal('counted over the coarse cells', [painted.range.total, painted.range.present], [
  coarse.ni() * coarse.nj(),
  coarse.ni() * coarse.nj(),
]);
equal('the overshoot is counted above', painted.range.above > 0, true);

// One window at one size, for both fields: they cover the same ground.
const window = handle.warp(full, { bilinear: false }).bounds;
const box = { bilinear: true, bounds: window, width: 200, height: 150 };
const fullWarp = handle.warp(full, box);
const coarseWarp = handle.warpDisplay(coarse, box);
equal('warpDisplay hands back the raster asked for', [coarseWarp.width, coarseWarp.height, coarseWarp.bounds], [200, 150, fullWarp.bounds]);
equal('as typed arrays', [coarseWarp.values.constructor.name, coarseWarp.mask.constructor.name], ['Float32Array', 'Uint8Array']);
const present = (m) => m.reduce((n, v) => n + v, 0);
const lost = Math.abs(present(fullWarp.mask) - present(coarseWarp.mask)) / present(fullWarp.mask);
equal('and covers the ground the full field covers', lost < 0.02, true);

// --- refusals -----------------------------------------------------------------
equal(
  'no stats and an automatic range is invalid_option',
  thrown(() => handle.paletteDisplay(coarse, null, {}))?.code,
  'invalid_option',
);
for (const bad of [[0, 1], 'stats', 3]) {
  equal(
    `stats ${JSON.stringify(bad)} are invalid_option`,
    thrown(() => handle.renderDisplay(coarse, bad, {}, false))?.code,
    'invalid_option',
  );
}
equal(
  'stats that are not a Stats object are invalid_option',
  thrown(() => handle.renderDisplay(coarse, { min: 'cold' }, {}, false))?.code,
  'invalid_option',
);
equal(
  'both ends manual needs no stats',
  handle.renderDisplay(coarse, null, { min: 270, max: 320 }, false).rgba.length,
  coarse.ni() * coarse.nj() * 4,
);
const simple = wasm.open(new Uint8Array(readFileSync(SIMPLE)));
equal(
  'a message with no pyramid refuses a reduction as unsupported',
  thrown(() => simple.decodeDisplay(0, { reduce: 1 }))?.code,
  'unsupported',
);
equal(
  'a reduction that is not a u8 is refused while reading the options',
  thrown(() => handle.decodeDisplay(0, { reduce: 300 })) !== null,
  true,
);
// An options argument is an object, `null` or left out; an array, a string or a
// number is the caller's mistake, refused as the Node addon refuses it. serde
// reads a struct from an array field by field, so `["f64"]` once decoded at
// `auto` width instead (#1210).
for (const bad of [['f64'], 'f64', 3]) {
  const what = JSON.stringify(bad);
  equal(`decode refuses ${what} as options`, thrown(() => handle.decode(0, bad))?.code, 'invalid_option');
  equal(`decodeDisplay refuses ${what} as options`, thrown(() => handle.decodeDisplay(0, bad))?.code, 'invalid_option');
  equal(`warp refuses ${what} as options`, thrown(() => handle.warp(full, bad))?.code, 'invalid_option');
  equal(`palette refuses ${what} as options`, thrown(() => handle.palette(full, bad))?.code, 'invalid_option');
}
equal('null and undefined stay the default', [thrown(() => handle.palette(full, null)), thrown(() => handle.palette(full, undefined))], [null, null]);
// Binary data is not an options object or stats either. serde reads each as an
// object with no fields, so an `ArrayBuffer` was the defaults; the Node addon
// refuses all three (#1239).
for (const [what, make] of [
  ['a Uint8Array', () => new Uint8Array(1)],
  ['a DataView', () => new DataView(new ArrayBuffer(4))],
  ['an ArrayBuffer', () => new ArrayBuffer(4)],
]) {
  for (const [call, read] of [
    ['decode', (bad) => handle.decode(0, bad)],
    ['decodeDisplay', (bad) => handle.decodeDisplay(0, bad)],
    ['warp', (bad) => handle.warp(full, bad)],
    ['palette', (bad) => handle.palette(full, bad)],
    ['renderDisplay stats', (bad) => handle.renderDisplay(coarse, bad, {}, false)],
  ]) {
    const e = thrown(() => read(make()));
    equal(`${call} refuses ${what}`, [e?.code, String(e?.message).includes('it is not an object')], ['invalid_option', true]);
  }
}
// A `SharedArrayBuffer` is binary data too, and so is a buffer made in another
// realm, for which `instanceof ArrayBuffer` is false. Each was read as an object
// with no fields, the defaults; the Node addon refuses them all (#1242).
for (const [what, make] of [
  ['a SharedArrayBuffer', () => new SharedArrayBuffer(4)],
  ["another realm's ArrayBuffer", () => runInNewContext('new ArrayBuffer(4)')],
  ["another realm's SharedArrayBuffer", () => runInNewContext('new SharedArrayBuffer(4)')],
  ["another realm's Uint8Array", () => runInNewContext('new Uint8Array(1)')],
]) {
  for (const [call, read] of [
    ['decode', (bad) => handle.decode(0, bad)],
    ['decodeDisplay', (bad) => handle.decodeDisplay(0, bad)],
    ['warp', (bad) => handle.warp(full, bad)],
    ['palette', (bad) => handle.palette(full, bad)],
    ['renderDisplay stats', (bad) => handle.renderDisplay(coarse, bad, {}, false)],
  ]) {
    const e = thrown(() => read(make()));
    equal(`${call} refuses ${what}`, [e?.code, String(e?.message).includes('it is not an object')], ['invalid_option', true]);
  }
}
// Another realm's plain object is still an object.
equal("another realm's options object reads", thrown(() => handle.decode(0, runInNewContext('({ dtype: "f64" })'))), null);

// The binding refuses the wrong class at runtime as well as in TypeScript.
equal(
  'probe refuses a display field',
  thrown(() => handle.probe(coarse, 40, -100)) !== null,
  true,
);
equal(
  'renderDisplay refuses a full field',
  thrown(() => handle.renderDisplay(full, stats, {}, false)) !== null,
  true,
);

// A closed display field: `close()` twice is harmless, every read after it
// and every draw of it throws, and `free()` still releases it (#1073).
coarse.close();
coarse.close();
equal('a closed display field refuses ni()', /closed/.test(thrown(() => coarse.ni())?.message ?? ''), true);
equal(
  'and refuses to be drawn',
  /closed/.test(thrown(() => handle.renderDisplay(coarse, stats, {}, false))?.message ?? ''),
  true,
);
coarse.free();
full.free();

if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log('reduced-resolution decode through the browser module: ok');
