// The published npm package works when installed (#466).
//
//   crates/fieldglass-wasm/pack.sh
//   node crates/fieldglass-wasm/tests/node/package.mjs
//
// `pack.sh` produces the tarball `npm publish` would upload. This installs that
// tarball into a throwaway project and decodes through it, which is a different
// question from the one the other Node tests answer: they load
// `pkg/nodejs/fieldglass_wasm.js` by path, so they would pass with a
// `package.json` that shipped none of it.
//
// What only this can catch:
//
//   * `files` missing an artefact — the module resolves and then fails to load
//     its `.wasm`;
//   * `exports` not offering the `.wasm` subpath, which is what a bundler
//     resolves and what a consumer needs to hand to `init` in a Worker;
//   * `types` pointing somewhere that is not in the tarball;
//   * the version not matching the workspace, since `pack.sh` reads it from
//     `Cargo.toml` rather than being told.
//
// # What it is not
//
// Not a decode oracle. Whether the numbers are *right* is settled by the
// eccodes cross-checks and the conformance suite; the expectations below are
// this project's own output, and they are here so that a package which loads
// but computes nothing cannot pass. A field of the right shape carrying the
// wrong values would be caught upstream, long before it reached a tarball.

import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');
const packDir = join(repoRoot, 'crates/fieldglass-wasm/pkg/npm');

let failures = 0;
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail === undefined ? '' : `: ${detail}`}`);
    failures += 1;
  }
}

// ---------------------------------------------------------------------------
// Find what pack.sh built
// ---------------------------------------------------------------------------

let tarballs = [];
try {
  tarballs = readdirSync(packDir).filter((f) => f.endsWith('.tgz'));
} catch {
  console.error(`missing: ${packDir}\n  crates/fieldglass-wasm/pack.sh`);
  process.exit(1);
}
if (tarballs.length !== 1) {
  console.error(`expected exactly one .tgz in ${packDir}, found ${tarballs.length}`);
  process.exit(1);
}
const tarball = join(packDir, tarballs[0]);

// The version is the workspace's, not an argument, so the two cannot drift.
const workspaceVersion = readFileSync(join(repoRoot, 'Cargo.toml'), 'utf8')
  .split('\n')
  .find((line) => line.startsWith('version'))
  .match(/"([^"]+)"/)[1];
const packed = JSON.parse(readFileSync(join(packDir, 'package.json'), 'utf8'));
check(
  'the package version is the workspace version',
  packed.version === workspaceVersion,
  `package.json says ${packed.version}, Cargo.toml says ${workspaceVersion}`,
);

// ---------------------------------------------------------------------------
// Install it the way a consumer would
// ---------------------------------------------------------------------------

const project = mkdtempSync(join(tmpdir(), 'fieldglass-consumer-'));
try {
  writeFileSync(
    join(project, 'package.json'),
    JSON.stringify({ name: 'consumer', private: true, type: 'module' }, null, 2),
  );
  // `--no-audit --no-fund` keeps the output to the point; offline is not forced
  // because npm still has to unpack a local file, not reach the registry.
  execFileSync('npm', ['install', '--no-audit', '--no-fund', tarball], {
    cwd: project,
    stdio: 'pipe',
  });

  const modules = join(project, 'node_modules', '@fieldglass', 'wasm');
  const entry = pathToFileURL(join(modules, 'fieldglass_wasm.js')).href;
  const { default: init, open } = await import(entry);

  // The subpath a bundler resolves, and what a Worker hands to `init`. Read off
  // the installed tree rather than the source, so a `files` omission shows up.
  const wasmPath = join(modules, 'fieldglass_wasm_bg.wasm');
  check('the .wasm is in the tarball', packed.files.includes('fieldglass_wasm_bg.wasm'));
  check(
    'the .wasm has an exports subpath',
    packed.exports['./fieldglass_wasm_bg.wasm'] === './fieldglass_wasm_bg.wasm',
  );
  check('the declarations are in the tarball', packed.files.includes(packed.types.replace('./', '')));

  await init({ module_or_path: await readFile(wasmPath) });

  // GRIB2, the message-addressed half.
  const grib = new Uint8Array(
    await readFile(
      join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2'),
    ),
  );
  const handle = open(grib);
  check('format', handle.format() === 'grib2', handle.format());
  check('addressing', handle.addressing() === 'messages', handle.addressing());
  check('message count', handle.count() === 1, handle.count());

  const field = handle.decode(0, {});
  check('raster', field.ni() === 16 && field.nj() === 31, `${field.ni()}x${field.nj()}`);
  check('parameter', field.parameter() === 'Temperature', field.parameter());
  check('units', field.units() === 'K', field.units());
  const stats = field.stats();
  check('every cell present', stats.validCount === 496, stats.validCount);
  check(
    'values are plausible temperatures',
    stats.min > 270 && stats.min < 271 && stats.max > 311 && stats.max < 312,
    `${stats.min}..${stats.max}`,
  );
  field.free();
  handle.free();

  // NetCDF, the variable-addressed half — the package claims three formats and
  // this is the one that only arrived in #662.
  const nc = new Uint8Array(
    await readFile(join(repoRoot, 'crates/fieldglass-netcdf/tests/fixtures/cf_packed_data.nc')),
  );
  const dataset = open(nc);
  check('netcdf format', dataset.format() === 'netcdf', dataset.format());
  check('netcdf addressing', dataset.addressing() === 'variables', dataset.addressing());
  const [variable] = dataset.variables();
  const slice = dataset.decodeSlice(
    variable.index,
    0,
    1,
    new Uint32Array(variable.dims.length),
    {},
  );
  check('netcdf raster', slice.ni() === 4 && slice.nj() === 3, `${slice.ni()}x${slice.nj()}`);
  // Physical units, so the CF unpacking survived into the package.
  const ncStats = slice.stats();
  check(
    'netcdf values are physical units',
    ncStats.min >= 250 && ncStats.max <= 875,
    `${ncStats.min}..${ncStats.max}`,
  );
  slice.free();
  dataset.free();
} finally {
  rmSync(project, { recursive: true, force: true });
}

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
