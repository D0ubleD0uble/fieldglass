// The colour-range rule through the browser module (#1155).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/colour_range.mjs
//
// Both hosts choose a colour range by one rule, `fieldglass::colour_range`.
// The umbrella tests it on numbers and through `Session`
// (`crates/fieldglass/tests/colour_range_rules.rs`), and the addon through its
// handle (`fieldglass-napi`'s `range_rule_tests`). This checks the binding's
// part, over the same committed GFS message #880 measured: real JavaScript
// `NaN` and `Infinity` reach the rule, a refusal arrives as an `Error` with
// `code` and `rangeRefusal` on it, and an empty field paints rather than
// throwing.
//
// A CI gate, like `netcdf.mjs`: it needs only a committed fixture and a built
// `pkg/nodejs`, and it fails loudly when either is missing.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const GFS = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [GFS, 'it is committed; check out the repository'],
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

/** What a call threw: its `code`, `rangeRefusal` and `message`, or `null`
 *  when it did not throw. */
function thrown(call) {
  try {
    call();
    return null;
  } catch (e) {
    return { code: e.code, rangeRefusal: e.rangeRefusal, message: e.message };
  }
}

const handle = wasm.open(new Uint8Array(readFileSync(GFS)));
const field = handle.decode(0, null);
const { min: lo, max: hi } = field.stats();
equal('the field #880 measured', [lo, hi].map((v) => v.toFixed(1)), ['53502.3', '99919.6']);

const allClear = (rgba) => rgba.every((b, i) => i % 4 !== 3 || b === 0);

// Rule 1: an empty field with an automatic range paints transparent, on
// either scale, and neither `render` nor `palette` throws.
{
  const zeros = handle.combine(field, field, 'a_minus_b');
  const empty = handle.combine(field, zeros, 'ratio');
  equal('rule 1: the ratio has no present cell', empty.stats().validCount, 0);
  for (const scale of ['linear', 'log10']) {
    let rgba = null;
    equal(`rule 1 (${scale}): render does not throw`, thrown(() => {
      rgba = handle.render(empty, { scale }, false);
    }), null);
    equal(`rule 1 (${scale}): every pixel is transparent`, rgba !== null && allClear(rgba), true);
    equal(`rule 1 (${scale}): palette does not throw`, thrown(() => handle.palette(empty, { scale })), null);
  }
  empty.free();
  zeros.free();
}

// Rule 2: one manual end is used and the other comes from the data; past the
// data's other end, the refusal says where that end came from.
{
  const p = handle.palette(field, { min: 76711 });
  equal('rule 2: min alone, max from the data', [p.t0, p.t1], [76711, hi]);
  const q = handle.palette(field, { max: 60000 });
  equal('rule 2: max alone, min from the data', [q.t0, q.t1], [lo, 60000]);
  const e = thrown(() => handle.render(field, { min: 100000 }, false));
  equal('rule 2: refused as invalid_option', e && e.code, 'invalid_option');
  equal('rule 2: the refusal as fields', e && e.rangeRefusal, {
    reason: 'inverted',
    end: 'min',
    given: 100000,
    fieldMin: lo,
    fieldMax: hi,
  });
  equal(
    'rule 2: the message names the other end and where it came from',
    e && e.message,
    `minimum 100000 is above the field's maximum ${hi} (taken from the data); set a maximum too`,
  );
}

// Rule 3: an inverted or zero-width manual range is refused, naming both.
for (const [min, max] of [[80000, 60000], [70000, 70000]]) {
  const e = thrown(() => handle.palette(field, { min, max }));
  equal(`rule 3: ${min}..${max} refused`, e && [e.code, e.rangeRefusal.reason, e.rangeRefusal.end], [
    'invalid_option',
    'inverted',
    'both',
  ]);
  equal(
    `rule 3: ${min}..${max} names both ends`,
    e && e.message.includes(`minimum ${min}`) && e.message.includes(`maximum ${max}`),
    true,
  );
}

// Rule 4: a non-finite end is refused on both scales (#880 measured each of
// these painting one colour); log10 needs a positive minimum.
for (const scale of ['linear', 'log10']) {
  for (const [label, options, end] of [
    ['min NaN', { min: NaN }, 'min'],
    ['max NaN', { max: NaN }, 'max'],
    ['min -Infinity', { min: -Infinity }, 'min'],
    ['max Infinity', { max: Infinity }, 'max'],
  ]) {
    const e = thrown(() => handle.palette(field, { ...options, scale }));
    equal(`rule 4 (${scale}): ${label}`, e && [e.code, e.rangeRefusal], [
      'invalid_option',
      { reason: 'not_finite', end, given: null, fieldMin: lo, fieldMax: hi },
    ]);
  }
}
{
  const e = thrown(() => handle.render(field, { min: 0, scale: 'log10' }, false));
  equal('rule 4: log10 with a zero minimum', e && [e.rangeRefusal.reason, e.rangeRefusal.end], [
    'log10_not_positive',
    'min',
  ]);
}

// Rule 5: an error that is not about the range carries a null refusal, and a
// misspelling is worded as the render options word it.
{
  const e = thrown(() => handle.palette(field, { colormap: 'viridiss' }));
  equal('rule 5: an unknown colormap has no range refusal', e && [e.code, e.rangeRefusal], [
    'invalid_option',
    null,
  ]);
  equal('rule 5: one wording', e && e.message.startsWith('unknown colormap "viridiss"'), true);
}

// Rule 6: the automatic range is the whole field's whatever a warp shows, and
// the automatic contour levels span it at the addon's count. The level list is
// the one `colour_range_rules.rs` pins for `Session`.
{
  const warped = handle.warp(field, { bounds: [30, 60, -10, 30] });
  let wlo = Infinity;
  let whi = -Infinity;
  warped.values.forEach((v, i) => {
    if (warped.mask[i] === 1) {
      wlo = Math.min(wlo, v);
      whi = Math.max(whi, v);
    }
  });
  equal('rule 6: the window holds a narrower range', wlo > lo && whi < hi, true);
  const p = handle.palette(field, null);
  equal('rule 6: the palette spans the whole field', [p.t0, p.t1], [lo, hi]);
  const levels = handle.contours(field, new Float64Array([]), null).map((l) => l.value);
  equal('rule 6: automatic contour levels', levels, [
    55000, 60000, 65000, 70000, 75000, 80000, 85000, 90000, 95000,
  ]);
  // A manual range spaces them, as it does in the addon; the list is the one
  // `colour_range_rules.rs` pins for `Session`.
  const manual = handle
    .contours(field, new Float64Array([]), { min: 60000, max: 90000 })
    .map((l) => l.value);
  equal('rule 6: contour levels over a manual range', manual, [65000, 70000, 75000, 80000, 85000]);
  const e = thrown(() => handle.contours(field, new Float64Array([]), { min: 90000, max: 60000 }));
  equal('rule 6: contours refuse what render refuses', e && [e.code, e.rangeRefusal.reason], [
    'invalid_option',
    'inverted',
  ]);
}

// A center makes the range symmetric about it.
{
  const p = handle.palette(field, { center: 80000 });
  const reach = Math.max(80000 - lo, hi - 80000);
  equal('center: symmetric about it', [p.t0, p.t1], [80000 - reach, 80000 + reach]);
  const e = thrown(() => handle.palette(field, { center: 80000, min: 70000, max: 90000 }));
  equal('center: refused beside two ends', e && e.rangeRefusal.reason, 'center_with_both_ends');
}

field.free();
handle.free();

if (failures) {
  console.log(`${failures} check(s) failed`);
  process.exit(1);
}
console.log('every colour-range rule holds through the browser module');
