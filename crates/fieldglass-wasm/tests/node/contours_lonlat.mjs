// Contour lines in longitude and latitude through the browser module (#1750).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/contours_lonlat.mjs
//
// `Handle.contoursLonLat` gives the levels `Handle.contours` traces, placed on
// the map, for a page that draws them with a map library. The umbrella tests
// the placement through `Session` (`crates/fieldglass/tests/contours_lonlat.rs`)
// and the conformance suite records it per subject. This checks the binding's
// part through the bundle: every vertex is `[lon, lat]`, and `probe` reads it
// back at the grid index `contours` traced it at, on a lat/lon grid and on a
// Lambert one; on a global grid a line across 180° ends on +180 and goes on
// from -180; and a grid with no position on a map throws with `code`
// `unsupported`.
//
// A CI gate, like `color_range.mjs`: it needs only committed fixtures and a
// built `pkg/nodejs`, and it fails loudly when either is missing.

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
const G2 = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures');
const LATLON = join(G2, 'regular_latlon_surface.grib2');
const LAMBERT = join(G2, 'eta_lambert_msg0.grib2');
const GFS = join(G2, 'gfs_c255_latlon.grib2');
const UNPLACED = join(G2, 'polar_stereographic_surface.grib2');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  ...[LATLON, LAMBERT, GFS, UNPLACED].map((p) => [p, 'it is committed; check out the repository']),
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);

let failures = 0;
function check(name, ok, detail = '') {
  if (ok) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail ? `: ${detail}` : ''}`);
    failures += 1;
  }
}

function open(path) {
  const handle = wasm.open(new Uint8Array(readFileSync(path)));
  return { handle, field: handle.decode(0, null) };
}

/** Walk `contours` and `contoursLonLat` together and probe every lon/lat
 *  vertex back to the grid index it was traced at. A vertex on the grid's
 *  outer row or column of a projected grid can be placed a hair outside it,
 *  where `probe` answers `null`; those are counted, not failed. */
function probesBack(name, path, tolerance) {
  const { handle, field } = open(path);
  const grid = handle.contours(field, new Float64Array(), null);
  const placed = handle.contoursLonLat(field, new Float64Array(), null);
  check(`${name}: the same levels`, JSON.stringify(grid.map((l) => l.value)) ===
    JSON.stringify(placed.map((l) => l.value)));
  let worst = 0;
  let probed = 0;
  let mismatch = null;
  for (let k = 0; k < grid.length; k += 1) {
    const g = grid[k].segments;
    const p = placed[k].segments;
    if (g.length !== p.length) {
      mismatch = `level ${grid[k].value}: ${g.length} grid, ${p.length} lon/lat segments`;
      break;
    }
    for (let s = 0; s < g.length; s += 1) {
      for (const [gi, gj, lon, lat] of [
        [g[s][0], g[s][1], p[s][0], p[s][1]],
        [g[s][2], g[s][3], p[s][2], p[s][3]],
      ]) {
        const at = handle.probe(field, lat, lon);
        if (at === null) continue;
        worst = Math.max(worst, Math.abs(at.i - gi), Math.abs(at.j - gj));
        probed += 1;
      }
    }
  }
  check(`${name}: one lon/lat segment per grid segment`, mismatch === null, mismatch ?? '');
  check(`${name}: vertices probed`, probed > 100, `${probed}`);
  check(
    `${name}: every [lon, lat] vertex probes back within ${tolerance} cells`,
    worst <= tolerance,
    `worst ${worst}`,
  );
  field.free();
  handle.free();
}

console.log('contoursLonLat through the browser module');
probesBack('lat/lon', LATLON, 1e-9);
// Placed linearly in latitude and longitude between two grid points, as the
// Node addon places a vertex: measured worst 1.2e-3 cells.
probesBack('Lambert', LAMBERT, 2e-3);

// GFS at 2.5°: column 72 is 180°. A vertex there takes its partner's side.
{
  const { handle, field } = open(GFS);
  const grid = handle.contours(field, new Float64Array(), null);
  const placed = handle.contoursLonLat(field, new Float64Array(), null);
  const east = new Set();
  const west = new Set();
  let wrongSide = 0;
  let span = 0;
  for (let k = 0; k < grid.length; k += 1) {
    grid[k].segments.forEach((g, s) => {
      const p = placed[k].segments[s];
      if (Math.abs(p[2] - p[0]) > 180) span += 1;
      for (const [here, there, lon, lat] of [
        [g[0], g[2], p[0], p[1]],
        [g[2], g[0], p[2], p[3]],
      ]) {
        if (here !== 72) continue;
        if (there < 72) {
          if (lon !== 180) wrongSide += 1;
          east.add(lat);
        } else if (there > 72) {
          if (lon !== -180) wrongSide += 1;
          west.add(lat);
        }
      }
    });
  }
  check('global: no segment spans the map', span === 0, `${span}`);
  check('global: a vertex on 180° takes its partner\'s side', wrongSide === 0, `${wrongSide}`);
  check(
    'global: a line ends on +180 and goes on from -180 at the same latitude',
    [...east].some((lat) => west.has(lat)),
  );
  field.free();
  handle.free();
}

// Dx = Dy = 0: the grid places no point, so its lines have no place on a map.
{
  const { handle, field } = open(UNPLACED);
  check('unplaced: supports.contours is false', field.georef().supports.contours === false);
  let code = null;
  try {
    handle.contoursLonLat(field, new Float64Array(), null);
  } catch (e) {
    code = e.code;
  }
  check('unplaced: refused with code unsupported', code === 'unsupported', `${code}`);
  field.free();
  handle.free();
}

if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log('all checks passed');
