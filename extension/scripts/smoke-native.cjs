// Loads one built native module and decodes a committed fixture of every
// format with it, checking known values (#1868).
//
// The release workflow runs this on each target's own runner, after
// `build-native`, so the five targets no test otherwise runs on (Windows x64
// and arm64, macOS x64 and arm64, Linux arm64) load and decode before anything
// is published. Every expected value is read from a committed oracle file
// written by eccodes, netCDF4 or zarr-python, not restated here.
//
//   node extension/scripts/smoke-native.cjs <path/to/fieldglass.<target>.node>
//
// Uses only Node's standard library, so it needs no `npm ci`.

'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const fixture = (...parts) => path.join(root, 'crates', ...parts);
const readJson = (p) => JSON.parse(fs.readFileSync(p, 'utf8'));

const addonPath = process.argv[2];
if (!addonPath) {
  process.stderr.write('usage: node smoke-native.cjs <fieldglass.<target>.node>\n');
  process.exit(2);
}

// The file name the extension loads on this platform, built the way
// `nativeBinaryName()` in extension/src/native.ts builds it. The module must
// carry exactly that name, so an artifact whose name drifted fails here rather
// than on a user's machine. A runner that ran the module under emulation
// would report another arch, so this is also what proves the job ran on the
// target's own hardware.
{
  const abi =
    process.platform === 'linux' ? '-gnu' : process.platform === 'win32' ? '-msvc' : '';
  const loaded = `fieldglass.${process.platform}-${process.arch}${abi}.node`;
  assert.equal(
    path.basename(addonPath),
    loaded,
    'the module is not the file the extension loads on this platform and arch',
  );
}

const native = require(path.resolve(addonPath));
let checks = 0;

function report(what, detail) {
  checks += 1;
  process.stdout.write(`ok  ${what}: ${detail}\n`);
}

// Compare one decoded field with an oracle. `samples` is `[index, value]`
// pairs over the field in the order the host returns it, `null` for a masked
// cell. `tol(expected)` is the allowed absolute difference.
function checkField(what, grid, want, tol) {
  const n = grid.ni * grid.nj;
  assert.equal(grid.values.length, n, `${what}: values length`);
  assert.equal(grid.mask.length, n, `${what}: mask length`);
  if (want.count !== undefined) assert.equal(n, want.count, `${what}: cell count`);
  if (want.present !== undefined) {
    assert.equal(grid.stats.validCount, want.present, `${what}: present count`);
  }
  const near = (got, exp, label) =>
    assert.ok(
      Math.abs(got - exp) <= tol(exp),
      `${what}: ${label} is ${got}, expected ${exp}`,
    );
  if (want.min !== undefined) near(grid.stats.min, want.min, 'minimum');
  if (want.max !== undefined) near(grid.stats.max, want.max, 'maximum');
  for (const [i, exp] of want.samples) {
    if (exp === null) {
      assert.equal(grid.mask[i], 0, `${what}: cell ${i} should be masked`);
    } else {
      assert.equal(grid.mask[i], 1, `${what}: cell ${i} should be present`);
      near(grid.values[i], exp, `cell ${i}`);
    }
  }
  report(what, `${grid.ni}x${grid.nj}, ${grid.stats.validCount} present, ` +
    `${want.samples.length} samples match`);
}

// eccodes' `grib_get` prints six significant digits.
const sixDigits = (exp) => Math.max(Math.abs(exp) * 5e-6, 1e-9);
const absolute = (t) => () => t;

// Open a NetCDF file and decode the first 2-D slice of `name`.
function decodeVariable(handle, name) {
  const v = handle.variables().find((x) => x.name === name);
  assert.ok(v, `no variable ${name}`);
  return handle.decodeSlice(
    {
      variable: v.index,
      yDim: v.defaultYDim,
      xDim: v.defaultXDim,
      sliceIndices: v.dims.map(() => 0),
    },
    { dtype: 'f64' },
  );
}

// Colormaps: the registry, with the default first
// (`colormaps_export_feeds_the_picker_and_the_legend` in fieldglass-napi).
{
  const maps = native.colormaps();
  assert.ok(maps.length >= 8, `only ${maps.length} colormaps`);
  assert.equal(maps[0].name, 'viridis');
  report('colormaps', `${maps.length}, first ${maps[0].name}`);
}

// GRIB1, simple packing on a polar stereographic grid, against eccodes 2.34.1.
{
  const file = fixture('fieldglass-grib1', 'tests', 'fixtures', 'cmc_wind_300_2010052400_p012.grib');
  const ref = readJson(`${file}.eccodes.ref.json`).values[0];
  const handle = native.open(fs.readFileSync(file));
  assert.equal(handle.format(), 'grib1');
  checkField('GRIB1 grid_simple', handle.decode(0, { dtype: 'f64' }), {
    count: ref.count,
    present: ref.count - ref.numberOfMissing,
    min: ref.minimum,
    max: ref.maximum,
    samples: ref.sample,
  }, sixDigits);
  handle.close();
}

// GRIB2, JPEG 2000 (template 5.40), against eccodes 2.34.1 grib_get_data.
{
  const dir = fixture('fieldglass-grib2', 'tests', 'fixtures');
  const want = readJson(path.join(dir, 'jpeg2000_regular_latlon_expected.json'));
  const handle = native.open(fs.readFileSync(path.join(dir, 'jpeg2000_regular_latlon.grib2')));
  assert.equal(handle.format(), 'grib2');
  checkField('GRIB2 grid_jpeg', handle.decode(0, { dtype: 'f64' }), {
    count: want.count,
    present: want.count - want.missing_count,
    min: want.min,
    max: want.max,
    samples: Object.entries(want.samples).map(([i, v]) => [Number(i), v]),
  }, absolute(want.tolerance_absolute));
  handle.close();
}

// NetCDF classic (CDF-1), against netCDF4. The oracle's samples keep the
// fill value, which the host masks.
{
  const file = fixture('fieldglass-netcdf', 'tests', 'fixtures', 'ersst_v5_187001_cdf1.nc');
  const want = readJson(`${file}.values.json`).variables.sst;
  const handle = native.open(fs.readFileSync(file));
  assert.equal(handle.format(), 'netcdf');
  checkField('NetCDF classic sst', decodeVariable(handle, 'sst'), {
    count: want.count,
    present: want.present_count,
    min: want.min,
    max: want.max,
    samples: Object.entries(want.samples).map(([i, v]) => [
      Number(i),
      v === want.fill_value ? null : v,
    ]),
  }, absolute(1e-6));
  handle.close();
}

// NetCDF-4 / HDF5, chunked with shuffle and deflate and CF-packed, against
// netCDF4 with auto mask and scale.
{
  const file = fixture('fieldglass-netcdf', 'tests', 'fixtures', 'oisst_avhrr_v2.nc');
  const want = readJson(`${file}.oracle.json`).sst;
  const handle = native.open(fs.readFileSync(file));
  assert.equal(handle.format(), 'netcdf');
  checkField('NetCDF-4 sst', decodeVariable(handle, 'sst'), {
    count: want.present_count + want.missing_count,
    present: want.present_count,
    min: want.min,
    max: want.max,
    samples: Object.entries(want.scaled_samples).map(([i, v]) => [Number(i), v]),
  }, absolute(1e-6));
  handle.close();
}

// Zarr v3 directory store with `/`-separated chunk keys, CF-packed, against
// zarr-python and xarray.
{
  const store = fixture('fieldglass-zarr', 'tests', 'fixtures', 'stores', 'cf_v3');
  const oracle = readJson(
    fixture('fieldglass-zarr', 'tests', 'fixtures', 'stores_oracle.json'),
  ).cf_v3.arrays.t;
  assert.ok(native.isStore(store), 'cf_v3 is a store');
  const handle = native.openStore(store);
  assert.equal(handle.format(), 'zarr');
  checkField('Zarr directory store t', decodeVariable(handle, 't'), {
    count: oracle.physical.length,
    present: oracle.physical.filter((v) => v !== null).length,
    samples: oracle.physical.map((v, i) => [i, v]),
  }, absolute(1e-9));
  handle.close();
}

process.stdout.write(`${checks} checks passed on ${process.platform}-${process.arch}\n`);
