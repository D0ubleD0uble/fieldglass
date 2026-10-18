// Zarr stores in the browser module (#1083).
//
//   crates/fieldglass-wasm/build.sh nodejs --features=zarr
//   node crates/fieldglass-wasm/tests/node/zarr.mjs --pkg crates/fieldglass-wasm/pkg/nodejs-zarr
//
// One window of a forecast-shaped store, read the way a Worker reads a remote
// store: open the metadata alone, fetch the coordinate chunks it names, plan a
// lat/lon box and a time step, fetch exactly the chunks the plan names, and
// decode the window from those and nothing else. Once per edition: the v2
// store is consolidated and blosc-compressed, the v3 one consolidated inline
// and zstd-compressed.
//
// The "fetch" is a read of the committed store's files, by the keys the store
// handed back, so a plan that named a wrong key fetches nothing and the decode
// that follows comes back wrong. The expected rows, columns, chunks and values
// are `windows_oracle.json`, which `tools/build_zarr_fixtures.py
// --windows-only` computes from the coordinate values and reads back through
// xarray, not anything recorded from this project's Rust.
//
// A CI gate, like `fetchplan.mjs`: it needs only committed files and a build
// with the `zarr` feature, and it fails loudly when either is missing. Against
// the default build it fails at the first call, `openStore is not a function`.

import { createRequire } from 'node:module';
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { isDeepStrictEqual } from 'node:util';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative, sep } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs-zarr' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const FIXTURES = join(repoRoot, 'crates/fieldglass-zarr/tests/fixtures');
const ORACLE = join(FIXTURES, 'windows_oracle.json');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs --features=zarr'],
  [ORACLE, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);
const oracle = JSON.parse(readFileSync(ORACLE, 'utf8'));

let failures = 0;
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail === undefined ? '' : `: ${detail}`}`);
    failures += 1;
  }
}

function same(name, actual, expected) {
  check(
    name,
    isDeepStrictEqual(actual, expected),
    `got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`,
  );
}

/** The `code` a call throws, or a note that it did not throw. */
function codeOf(f) {
  try {
    f();
    return 'did not throw';
  } catch (e) {
    return e.code ?? `threw without a code: ${e}`;
  }
}

const METADATA = ['zarr.json', '.zmetadata', '.zgroup', '.zarray', '.zattrs'];
const isMetadata = (key) => METADATA.some((doc) => key === doc || key.endsWith(`/${doc}`));

/** Every key of a committed store, as a bucket listing would name it. */
function listing(root) {
  const out = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path);
      else out.push(relative(root, path).split(sep).join('/'));
    }
  };
  walk(root);
  return out.sort();
}

// North to south and south to north (#1163), each in both editions. The
// south-to-north v3 store's `latitude/c/2` was never written (it holds only
// the fill value, the equator), so its fetch below is a 404 passed as `null`.
for (const store of ['window_v2', 'window_v3', 'window_s2n_v2', 'window_s2n_v3']) {
  console.log(`\n${store}`);
  const root = join(FIXTURES, 'windows', store);
  const keys = listing(root);
  const fetched = [];
  // The README's `get`: bytes, or `null` for an object the store does not
  // hold (a 404), which a sparse array's unwritten chunk is.
  const fetch = (key) => {
    fetched.push(key);
    const path = join(root, ...key.split('/'));
    return existsSync(path) ? new Uint8Array(readFileSync(path)) : null;
  };
  const expected = oracle[store];

  // 1. The metadata, and the coordinate chunks it names.
  const objects = new Map(keys.filter(isMetadata).map((k) => [k, fetch(k)]));
  let handle = wasm.openStore(objects);
  same(`${store}: a store is addressed by variables`, handle.addressing(), 'variables');
  same(`${store}: its format`, handle.format(), 'zarr');
  const t2m = handle.variables().find((v) => v.name === 't2m');
  check(`${store}: lists t2m`, t2m !== undefined);
  const coordinates = handle.coordinateChunks(t2m.index);
  same(`${store}: the coordinate chunks`, coordinates, expected.coordinate_chunks);
  // Planning before they are fetched names what to fetch.
  let early;
  try {
    handle.planWindow({ variable: t2m.index, yDim: 1, xDim: 2, bounds: [0, 40, 0, 90], sliceIndices: [0, 0, 0] });
  } catch (e) {
    early = e;
  }
  same(`${store}: planning before the coordinates is invalid_option`, early?.code, 'invalid_option');
  check(
    `${store}: and names the coordinate keys`,
    early !== undefined && coordinates.filter((c) => c.array !== 'time').every((c) => early.message.includes(c.key)),
    early?.message,
  );
  for (const { key } of coordinates) objects.set(key, fetch(key));
  same(
    `${store}: the coordinate chunks that came back 404`,
    coordinates.filter((c) => objects.get(c.key) === null).map((c) => c.key),
    expected.unwritten_coordinate_chunks,
  );
  handle.free();
  handle = wasm.openStore(objects);

  for (const window of expected.windows) {
    const name = `${store}/${window.name}`;
    const request = {
      variable: t2m.index,
      yDim: 1,
      xDim: 2,
      bounds: window.bounds,
      sliceIndices: [window.time, 0, 0],
    };

    // 2. The plan: exactly the chunks the box and the step touch.
    const plan = handle.planWindow(request);
    same(`${name}: rows`, plan.rows, window.rows);
    same(`${name}: columns`, plan.columns, window.columns);
    same(`${name}: chunks`, plan.chunks, window.chunks);
    same(`${name}: ni x nj`, [plan.ni, plan.nj], [window.longitude.length, window.latitude.length]);

    // 3. A store holding the metadata, the coordinates and those chunks only.
    const held = new Map([...objects]);
    for (const { key } of plan.chunks) held.set(key, fetch(key));
    const windowed = wasm.openStore(held);
    const field = windowed.decodeWindow(request, null);
    same(`${name}: values`, Array.from(field.values()), window.values);
    same(`${name}: nothing masked`, Array.from(field.mask()).every((m) => m === 1), true);
    same(`${name}: shape`, [field.ni(), field.nj()], [window.longitude.length, window.latitude.length]);
    same(`${name}: parameter`, field.parameter(), 't2m');
    const georef = field.grid();
    same(`${name}: placed on lat/lon`, georef.kind, 'latlon');
    // The window is one more field: the palette and the probe take it.
    const lastLat = window.latitude[window.latitude.length - 1];
    const lastLon = window.longitude[window.longitude.length - 1];
    const probe = windowed.probe(field, lastLat, lastLon);
    same(
      `${name}: a probe at its last cell reads that cell`,
      probe === null ? null : probe.value,
      window.values[window.values.length - 1],
    );
    const palette = windowed.palette(field, {});
    same(`${name}: a palette over it`, [palette.t0, palette.t1], [
      Math.min(...window.values),
      Math.max(...window.values),
    ]);
    field.free();
    windowed.free();

    // A planned chunk that came back 404 is passed as `null`: its cells are
    // the fill value (NaN here, so masked) and every other cell is unchanged.
    if (window.name === 'crosses_chunk_edges') {
      const [gone, ...kept] = plan.chunks;
      const sparse = new Map([...objects]);
      sparse.set(gone.key, null);
      for (const { key } of kept) sparse.set(key, fetch(key));
      const partial = wasm.openStore(sparse);
      const f = partial.decodeWindow(request);
      const mask = Array.from(f.mask());
      const values = Array.from(f.values());
      const ni = window.longitude.length;
      const cols = window.columns.flatMap(([a, b]) => Array.from({ length: b - a }, (_, k) => a + k));
      let right = true;
      for (let at = 0; at < mask.length; at += 1) {
        const j = window.rows[0] + Math.floor(at / ni);
        const i = cols[at % ni];
        const inGone = Math.floor(j / 3) === gone.index[1] && Math.floor(i / 4) === gone.index[2];
        if (inGone ? mask[at] !== 0 : mask[at] !== 1 || values[at] !== window.values[at]) right = false;
      }
      check(`${name}: a 404 chunk passed as null reads as its fill, the rest unchanged`, right);
      check(`${name}: and some cells are masked`, mask.includes(0));
      f.free();
      partial.free();

      // Not passed at all, it is a fetch the page still owes (#1124):
      // `decodeWindow` names it rather than filling it, and so does any other
      // read that needs it.
      const owed = new Map([...objects]);
      for (const { key } of kept) owed.set(key, fetch(key));
      const forgot = wasm.openStore(owed);
      let thrown;
      try {
        forgot.decodeWindow(request);
      } catch (e) {
        thrown = e;
      }
      same(`${name}: a planned chunk not passed is invalid_option`, thrown?.code, 'invalid_option');
      check(`${name}: and names its key`, thrown?.message?.includes(`\`${gone.key}\``), thrown?.message);
      let slice;
      try {
        forgot.decodeSlice(t2m.index, 1, 2, [window.time, 0, 0], null);
      } catch (e) {
        slice = e;
      }
      // `undefined`, what a fetch helper returns when it got nothing, is not
      // passed either: refused the same way, never read as fill.
      owed.set(gone.key, undefined);
      const undef = wasm.openStore(owed);
      same(
        `${name}: a planned chunk passed as undefined is not passed`,
        codeOf(() => undef.decodeWindow(request)),
        'invalid_option',
      );
      undef.free();
      same(`${name}: decodeSlice over it is decode`, slice?.code, 'decode');
      check(`${name}: and names the key`, slice?.message?.includes(`\`${gone.key}\``), slice?.message);
      forgot.free();
    }
  }

  // Nothing beyond the metadata, the coordinates and the planned chunks was
  // ever read off the "bucket".
  const planned = new Set(expected.windows.flatMap((w) => w.chunks.map((c) => c.key)));
  const extra = fetched.filter(
    (k) => !isMetadata(k) && !planned.has(k) && !coordinates.some((c) => c.key === k),
  );
  same(`${store}: fetched nothing unplanned`, extra, []);

  // The request is strict, and the box has to be one.
  const good = { variable: t2m.index, yDim: 1, xDim: 2, bounds: [0, 40, 0, 90], sliceIndices: [0, 0, 0] };
  same(`${store}: an unknown key`, codeOf(() => handle.planWindow({ ...good, sliceIndex: 1 })), 'invalid_option');
  same(`${store}: a missing key`, codeOf(() => handle.planWindow({ ...good, bounds: undefined })), 'invalid_option');
  same(`${store}: an inverted box`, codeOf(() => handle.planWindow({ ...good, bounds: [40, 0, 0, 90] })), 'invalid_option');
  same(`${store}: a step past the end`, codeOf(() => handle.planWindow({ ...good, sliceIndices: [3, 0, 0] })), 'invalid_option');
  same(
    `${store}: time against longitude is not a map`,
    codeOf(() => handle.planWindow({ ...good, yDim: 0 })),
    'unsupported',
  );
  const off = { ...good, bounds: [75, 89, 0, 90] };
  same(`${store}: a box off the grid plans nothing`, handle.planWindow(off), {
    rows: [0, 0],
    columns: [],
    ni: 0,
    nj: 0,
    chunks: [],
  });
  same(`${store}: and decodes nothing`, codeOf(() => handle.decodeWindow(off)), 'invalid_option');
  handle.free();
}

// Refusals that name what is wrong with a coordinate (#1163), over the v3
// store's own files.
console.log('\ncoordinates that place nothing');
{
  const root = join(FIXTURES, 'windows', 'window_v3');
  const read = (key) => new Uint8Array(readFileSync(join(root, ...key.split('/'))));
  const keys = listing(root);
  const request = { variable: 0, yDim: 1, xDim: 2, bounds: [5, 45, 50, 130], sliceIndices: [0, 0, 0] };
  /** The `{ code, message }` a planWindow over these objects throws. */
  const refusal = (objects) => {
    const handle = wasm.openStore(objects);
    try {
      const t2m = handle.variables().find((v) => v.name === 't2m');
      handle.planWindow({ ...request, variable: t2m.index });
      return { code: 'did not throw', message: '' };
    } catch (e) {
      return { code: e.code, message: e.message };
    } finally {
      handle.free();
    }
  };

  // Every latitude is in `latitude/c/0`, whose fill value is NaN. Passed as
  // `null` (the store does not hold it), the latitudes are all NaN: refused,
  // naming the chunk.
  const objects = new Map(keys.filter(isMetadata).map((k) => [k, read(k)]));
  for (const k of ['time/c/0', 'longitude/c/0']) objects.set(k, read(k));
  objects.set('latitude/c/0', null);
  const fill = refusal(objects);
  same('a coordinate chunk passed as null whose fill places nothing is unsupported', fill.code, 'unsupported');
  check('and the refusal names the chunk', fill.message.includes('`latitude/c/0`'), fill.message);

  // `latitude` stored as a type this build does not read: left out of the
  // store, and named in the refusal with why.
  const document = JSON.parse(new TextDecoder().decode(read('zarr.json')));
  document.consolidated_metadata.metadata.latitude.data_type = 'complex128';
  const leftOut = new Map(objects);
  leftOut.set('zarr.json', new TextEncoder().encode(JSON.stringify(document)));
  leftOut.set('latitude/c/0', read('latitude/c/0'));
  const opened = wasm.openStore(leftOut);
  const why = opened.leftOut().find((l) => l.name === 'latitude');
  opened.free();
  check('a coordinate of an unread type is left out', why !== undefined);
  const named = refusal(leftOut);
  same('a window over it is unsupported', named.code, 'unsupported');
  check(
    'and the refusal names the coordinate and why',
    why !== undefined && named.message.includes('`latitude`') && named.message.includes(why.reason),
    named.message,
  );
}

console.log('\nopenStore');
same('a value that is not bytes', codeOf(() => wasm.openStore({ 'zarr.json': 'text' })), 'invalid_option');
same('a value that is not a map', codeOf(() => wasm.openStore(7)), 'invalid_option');
check(
  'objects that are not a store are refused',
  codeOf(() => wasm.openStore(new Map())) === 'decode',
);
same(
  'a null object is one the store does not hold',
  codeOf(() =>
    wasm.openStore({
      'zarr.json': new Uint8Array(readFileSync(join(FIXTURES, 'windows/window_v3/zarr.json'))),
      't2m/c/0/0/0': null,
    }),
  ),
  'did not throw',
);
// An unconsolidated v2 store: an array's `.zattrs` holds its dimension names
// and CF attributes, so one never passed is refused naming it, and one passed
// as `null` (a 404) is an array with no attributes.
{
  const root = join(FIXTURES, 'stores', 'v2_slash');
  const docs = listing(root).filter((k) => ['.zgroup', '.zarray', '.zattrs'].some((d) => k.endsWith(d)));
  const without = new Map(
    docs.filter((k) => k !== 'temp/.zattrs').map((k) => [k, new Uint8Array(readFileSync(join(root, ...k.split('/'))))]),
  );
  let refused;
  try {
    wasm.openStore(without).free();
  } catch (e) {
    refused = e;
  }
  same('an unconsolidated array whose .zattrs was not passed is decode', refused?.code, 'decode');
  check('and names it', refused?.message?.includes('`temp/.zattrs`'), refused?.message);
  without.set('temp/.zattrs', null);
  same('passed as null, the store opens', codeOf(() => wasm.openStore(without).free()), 'did not throw');
}
// A message stream has no windows to plan.
const grib = wasm.open(
  new Uint8Array(readFileSync(join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2'))),
);
same('a GRIB file has no coordinate chunks', codeOf(() => grib.coordinateChunks(0)), 'wrong_addressing');
grib.free();

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
