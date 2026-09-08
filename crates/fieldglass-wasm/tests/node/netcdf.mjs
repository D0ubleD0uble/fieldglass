// The browser host opens a NetCDF file (#662).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/netcdf.mjs
//
// Until #662 the same file rendered in the VS Code editor and was refused in a
// browser: `fieldglass-wasm` took the umbrella with `grib1, grib2` only, so
// `open` on a NetCDF file threw `unsupported_format`. This is the check that
// the divergence is closed, driven through the **built module's JavaScript
// surface** — the only place the binding's own work (serde-wasm-bindgen
// conversion, the typed arrays, the error mapping) is under test.
//
// # Why this is its own file rather than a conformance case
//
// The ADR-0006 suite is 280 cases and every one of them is GRIB, because until
// #671 there was no `Session` path to a NetCDF variable to record observations
// from. Giving the suite a NetCDF subject means teaching all three runners the
// `variables` / `dimensions` / `decodeSlice` ops and regenerating the JSON,
// which is its own change; this file is the narrower claim — that the browser
// host can open one at all — and it is a CI gate in the meantime.
//
// # The oracle
//
// `cf_packed_data.nc` is 640 bytes and deliberately awkward: `temp` is `int16`
// with `scale_factor` 0.0625, `add_offset` 250, `_FillValue` -9999 and a
// `valid_range` of [0, 10000]. The expected values below come from
// **netCDF4-python** reading the same file with `set_auto_maskandscale(True)`,
// not from this project's decoder, so a shared bug cannot make both agree.
//
// It is the right fixture for this and not just a small one: a caller that
// reached the values but not the CF conventions would get integer codes about
// two orders of magnitude out, and one that ignored `valid_range` would report
// three garbage numbers as data.

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
const FIXTURE = join(
  repoRoot,
  'crates/fieldglass-netcdf/tests/fixtures/cf_packed_data.nc',
);

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [FIXTURE, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);

let failures = 0;
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail === undefined ? '' : `: ${detail}`}`);
    failures += 1;
  }
}

function equal(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  check(name, a === e, `got ${a}, expected ${e}`);
}

// ---------------------------------------------------------------------------

console.log(`netcdf through ${pkgDir}`);

const handle = wasm.open(new Uint8Array(readFileSync(FIXTURE)));

equal('format is netcdf', handle.format(), 'netcdf');

// The first question a host asks. A GRIB file answers "messages"; this is the
// other mode, and without it the bundle would open the file and have no way to
// reach a variable.
equal('addressing is variables', handle.addressing(), 'variables');

// A variable container holds no messages, and says so rather than throwing.
equal('count is zero', handle.count(), 0);

equal('dimensions', handle.dimensions(), [
  { name: 'lat', length: 3 },
  { name: 'lon', length: 4 },
]);

const variables = handle.variables();
// `lat` and `lon` are coordinate variables of one dimension each: axes, not
// fields, so a renderable list must not offer them.
equal('one renderable variable', variables.length, 1);
equal('its name', variables[0].name, 'temp');
equal('its dtype', variables[0].dtype, 'short');
equal('its units', variables[0].units, 'kelvin');
equal(
  'its dims',
  variables[0].dims,
  [{ name: 'lat', length: 3 }, { name: 'lon', length: 4 }],
);

// Two dimensions, so `sliceIndices` has two entries and both are ignored: the
// contract is one entry per dimension in declared order, never a reduced list.
const field = handle.decodeSlice(variables[0].index, 0, 1, new Uint32Array([0, 0]), {});

equal('raster width', field.ni(), 4);
equal('raster height', field.nj(), 3);
equal('parameter', field.parameter(), 'temp');
equal('units', field.units(), 'kelvin');

// netCDF4-python, `set_auto_maskandscale(True)`, flattened row-major. `null`
// where it returns a masked element.
const ORACLE = [
  null, 250, 406.25, 875,
  null, null, 562.5, 874.9375,
  250.0625, 718.75, 265.625, 875,
];

const values = Array.from(field.values());
const mask = Array.from(field.mask());
check('twelve values', values.length === 12, `got ${values.length}`);
check('twelve mask entries', mask.length === 12, `got ${mask.length}`);

// The mask is the presence plane: a masked element's value is not meaningful,
// so it is the mask and not the number that has to agree there.
const present = ORACLE.map((v) => (v === null ? 0 : 1));
equal('mask matches netCDF4-python', mask, present);

let worst = 0;
for (let i = 0; i < ORACLE.length; i += 1) {
  if (ORACLE[i] === null) continue;
  worst = Math.max(worst, Math.abs(values[i] - ORACLE[i]));
}
// Exact would also pass today — every expected value is a multiple of 1/16 and
// so exact in binary — but the tolerance is what the claim actually needs, and
// pinning bit equality across a target boundary is what ADR-0009 argues against.
check(
  'values match netCDF4-python within 1e-9',
  worst <= 1e-9,
  `worst absolute difference ${worst}`,
);

// CF is the point of this fixture: the raw codes are around 0..10000 and the
// physical values are around 250..875. A decoder that skipped the unpacking
// would still produce twelve finite numbers.
const unpacked = values.every((v, i) => present[i] === 0 || (v >= 250 && v <= 875));
check('values are physical units, not stored codes', unpacked);

// Asking the message question of a variable container names the call to make
// instead of failing on the index.
try {
  handle.decode(0, {});
  check('decode() on a variable container throws', false, 'it returned');
} catch (e) {
  check('decode() on a variable container throws wrong_addressing', e.code === 'wrong_addressing', `code was ${e.code}`);
  check('and names the call to make', String(e.message ?? e).includes('decode_slice'), String(e.message ?? e));
}

// A slice index list of the wrong length is refused rather than defaulted:
// silently taking zero is how a viewer shows the first time step and labels it
// the last.
try {
  handle.decodeSlice(variables[0].index, 0, 1, new Uint32Array([0]), {});
  check('a short sliceIndices throws', false, 'it returned');
} catch (e) {
  check('a short sliceIndices throws invalid_option', e.code === 'invalid_option', `code was ${e.code}`);
}

field.free();
handle.free();

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
