// Zarr stores in the browser module (#1083).
//
//   crates/fieldglass-wasm/build.sh nodejs --features=fetchplan,zarr
//   node crates/fieldglass-wasm/tests/node/zarr.mjs
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
// A sharded store (#1123) is read in part: each shard's index by range, then
// the inner chunks the plan names, by range; see its section below.
//
// A CI gate, like `fetchplan.mjs`: it needs only committed files and a build
// of the full entry (`@fieldglass/wasm/full`, #1158), and it fails loudly when
// either is missing. Against the default build it fails at the first call,
// `openStore is not a function`.

import { createRequire } from 'node:module';
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { isDeepStrictEqual } from 'node:util';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative, sep } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs-fetchplan-zarr' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const FIXTURES = join(repoRoot, 'crates/fieldglass-zarr/tests/fixtures');
const ORACLE = join(FIXTURES, 'windows_oracle.json');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs --features=fetchplan,zarr'],
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

/** The message a call throws, or a note that it did not throw. */
function messageOf(f) {
  try {
    f();
    return 'did not throw';
  } catch (e) {
    return String(e.message ?? e);
  }
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
    // The planned chunks are held, so only the guard can refuse these (#1221).
    same(
      `${name}: an array is not a window to decode`,
      messageOf(() => windowed.decodeWindow([t2m.index, 1, 2, window.bounds, [window.time, 0, 0]], null)).includes('it is not an object'),
      true,
    );
    same(
      `${name}: a typed array is not decode options`,
      messageOf(() => windowed.decodeWindow(request, new Uint8Array(1))).includes('it is not an object'),
      true,
    );
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
        forgot.decodeSlice(
          { variable: t2m.index, yDim: 1, xDim: 2, sliceIndices: [window.time, 0, 0] },
          null,
        );
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
  // Each is refused by the object guard itself, not by a later check: an array
  // read by position is a well-formed request (#1221).
  const positional = [t2m.index, 1, 2, [0, 40, 0, 90], [0, 0, 0]];
  for (const [what, f] of [
    ['an array is not a window request', () => handle.planWindow(positional)],
    ['an empty array is not a window request', () => handle.planWindow([])],
    ['an array is not a window to decode', () => handle.decodeWindow(positional)],
    ['a typed array is not a window request', () => handle.planWindow(new Uint8Array(1))],
    ['an ArrayBuffer is not a window request', () => handle.planWindow(new ArrayBuffer(4))],
  ]) {
    same(`${store}: ${what}`, codeOf(f), 'invalid_option');
    same(`${store}: ${what}, by the guard`, messageOf(f).includes('it is not an object'), true);
  }
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
    shardIndexes: null,
    innerChunks: null,
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
// A store made in another realm (a `vm` context stands for an iframe) opens and
// reads as one made here. `instanceof` is false for both its `Map` and its
// `Uint8Array`s, which read as an empty store and as "must be a Uint8Array"
// (#1244).
{
  const root = join(FIXTURES, 'windows', 'window_v3');
  const keys = listing(root).filter(isMetadata);
  const bytes = (k) => new Uint8Array(readFileSync(join(root, ...k.split('/'))));
  const local = wasm.openStore(new Map(keys.map((k) => [k, bytes(k)])));
  const expected = local.variables();
  check('the local store lists variables', expected.length > 0);
  local.free();

  const foreign = vm.runInNewContext('({ Map, Uint8Array, Uint16Array, Object })');
  const foreignBytes = (k) => {
    const b = bytes(k);
    const copy = new foreign.Uint8Array(b.length);
    copy.set(b);
    return copy;
  };
  check('the foreign Map is not an instance here', !(new foreign.Map() instanceof Map));
  check('nor the foreign Uint8Array', !(foreignBytes(keys[0]) instanceof Uint8Array));

  const read = (objects) => {
    try {
      const handle = wasm.openStore(objects);
      const variables = handle.variables();
      handle.free();
      return variables;
    } catch (e) {
      return `threw ${e.code}: ${e.message}`;
    }
  };
  same(
    'a foreign Map of foreign Uint8Arrays reads as the local store',
    read(new foreign.Map(keys.map((k) => [k, foreignBytes(k)]))),
    expected,
  );
  same(
    'a foreign Map of local Uint8Arrays reads as the local store',
    read(new foreign.Map(keys.map((k) => [k, bytes(k)]))),
    expected,
  );
  same(
    'a local Map of foreign Uint8Arrays reads as the local store',
    read(new Map(keys.map((k) => [k, foreignBytes(k)]))),
    expected,
  );
  const record = new foreign.Object();
  for (const k of keys) record[k] = foreignBytes(k);
  same('a foreign object of foreign Uint8Arrays reads as the local store', read(record), expected);
  const nulled = new foreign.Map(keys.map((k) => [k, foreignBytes(k)]));
  nulled.set('t2m/c/0/0/0', null);
  same('a foreign null is a key the store does not hold', read(nulled), expected);

  // Never read as an empty store, and never trust a tag.
  const wide = new foreign.Map([['zarr.json', new foreign.Uint16Array(4)]]);
  same('a foreign Uint16Array is refused', codeOf(() => wasm.openStore(wide)), 'invalid_option');
  const tagged = { length: 2, 0: 1, 1: 2, [Symbol.toStringTag]: 'Uint8Array' };
  same(
    'an object tagged Uint8Array is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', tagged]]))),
    'invalid_option',
  );
  const viewTagged = new Uint16Array(4);
  Object.defineProperty(viewTagged, Symbol.toStringTag, { value: 'Uint8Array' });
  same(
    'a Uint16Array tagged Uint8Array is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', viewTagged]]))),
    'invalid_option',
  );
  const forgedMap = { [Symbol.toStringTag]: 'Map', forEach: () => {} };
  same('an object tagged Map is not a Map', codeOf(() => wasm.openStore(forgedMap)), 'invalid_option');
  const bare = { [Symbol.toStringTag]: 'Map' };
  same('a tagged object with no keys is refused, not an empty store', codeOf(() => wasm.openStore(bare)), 'invalid_option');
  same(
    'Object.create(Map.prototype) is refused, not an empty store',
    codeOf(() => wasm.openStore(Object.create(Map.prototype))),
    'invalid_option',
  );
  same(
    'a Proxy over a Map is refused, not an empty store',
    codeOf(() => wasm.openStore(new Proxy(new Map(), {}))),
    'invalid_option',
  );

  // The tag and `byteLength` are both forgeable; the element type is not.
  const forged = new Uint16Array([0x0102, 0x0304, 5, 6]);
  Object.defineProperty(forged, Symbol.toStringTag, { value: 'Uint8Array' });
  Object.defineProperty(forged, 'byteLength', { value: 4 });
  same(
    'a Uint16Array with a forged tag and byteLength is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', forged]]))),
    'invalid_option',
  );
  const forgedForeign = new foreign.Uint16Array([0x0102, 0x0304, 5, 6]);
  Object.defineProperty(forgedForeign, Symbol.toStringTag, { value: 'Uint8Array' });
  Object.defineProperty(forgedForeign, 'byteLength', { value: 4 });
  same(
    'and so is a foreign one',
    codeOf(() => wasm.openStore(new Map([['zarr.json', forgedForeign]]))),
    'invalid_option',
  );
  const dataView = vm.runInNewContext('new DataView(new ArrayBuffer(8))');
  same(
    'a foreign DataView is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', dataView]]))),
    'invalid_option',
  );
  const tagView = new DataView(new ArrayBuffer(8));
  Object.defineProperty(tagView, Symbol.toStringTag, { value: 'Uint8Array' });
  same(
    'a DataView tagged Uint8Array is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', tagView]]))),
    'invalid_option',
  );

  // A view whose buffer was transferred away is refused, from either realm.
  const detached = vm.runInNewContext(
    'const b = new ArrayBuffer(4); const v = new Uint8Array(b); structuredClone(b, { transfer: [b] }); v',
    { structuredClone },
  );
  same(
    'a detached foreign Uint8Array is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', detached]]))),
    'invalid_option',
  );
  const buffer = new ArrayBuffer(4);
  const detachedLocal = new Uint8Array(buffer);
  structuredClone(buffer, { transfer: [buffer] });
  same(
    'a detached local Uint8Array is refused',
    codeOf(() => wasm.openStore(new Map([['zarr.json', detachedLocal]]))),
    'invalid_option',
  );
}
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
// A sharded store read in part (#1123): each shard's index by range, then the
// inner chunks the window intersects, by range, and never a whole shard. The
// "fetch" serves HTTP-style ranges off the committed shard files: a suffix
// range for an index at the end, an explicit one otherwise, and reports the
// shard's size as a `Content-Range` would. The shards, index ranges and inner
// chunk ranges expected are the oracle's, read from the shard files with
// Python's `struct`, and the values are xarray's.
{
  const store = 'window_sharded_v3';
  console.log(`\n${store}`);
  const root = join(FIXTURES, 'windows', store);
  const expected = oracle[store];
  const pathOf = (key) => join(root, ...key.split('/'));
  const whole = (key) => (existsSync(pathOf(key)) ? new Uint8Array(readFileSync(pathOf(key))) : null);
  const served = [];
  /** `Range: bytes=-length` or `bytes=offset-(offset+length-1)`: the bytes, and the size. */
  const fetchRange = (key, { suffix, offset, length }) => {
    const file = readFileSync(pathOf(key));
    const start = suffix ? file.length - length : offset;
    served.push([key, start, length]);
    return { size: file.length, offset: start, bytes: new Uint8Array(file.subarray(start, start + length)) };
  };
  const base = new Map(listing(root).filter(isMetadata).map((k) => [k, whole(k)]));
  for (const { key } of expected.coordinate_chunks) base.set(key, whole(key));

  for (const name of expected.variables) {
    const opened = wasm.openStore(base);
    const variable = opened.variables().find((v) => v.name === name);
    check(`${name}: listed`, variable !== undefined);
    opened.free();
    for (const window of expected.windows[name]) {
      const label = `${name}/${window.name}`;
      const request = { variable: variable.index, yDim: 1, xDim: 2, bounds: window.bounds, sliceIndices: [window.time, 0, 0] };
      served.length = 0;

      // 1. The shards and their indexes; no inner chunk yet.
      let handle = wasm.openStore(base);
      let plan = handle.planWindow(request);
      handle.free();
      same(`${label}: shards`, plan.chunks, window.chunks);
      same(`${label}: shard indexes`, plan.shardIndexes, window.shard_indexes);
      same(`${label}: no inner chunk before the indexes`, plan.innerChunks, null);

      // Each index by range, given back with the shard's size.
      const parts = new Map();
      for (const index of plan.shardIndexes) {
        const got = fetchRange(index.key, {
          suffix: index.location === 'end',
          offset: 0,
          length: index.length,
        });
        same(`${label}: ${index.key} is the size the oracle read`, got.size, window.shard_sizes[index.key]);
        parts.set(index.key, { size: got.size, ranges: [{ offset: got.offset, bytes: got.bytes }] });
      }
      const withParts = () => {
        const objects = new Map(base);
        for (const [key, value] of parts) objects.set(key, value);
        return objects;
      };

      // 2. Exactly the inner chunks the window intersects, as byte ranges.
      handle = wasm.openStore(withParts());
      plan = handle.planWindow(request);
      same(`${label}: inner chunks`, plan.innerChunks, window.inner_chunks);
      const owed = codeOf(() => handle.decodeWindow(request));
      same(`${label}: decoding before the inner chunks is invalid_option`, owed, 'invalid_option');
      handle.free();

      // 3. Those ranges, and decode.
      for (const inner of plan.innerChunks) {
        const got = fetchRange(inner.key, { suffix: false, offset: inner.offset, length: inner.length });
        parts.get(inner.key).ranges.push({ offset: got.offset, bytes: got.bytes });
      }
      handle = wasm.openStore(withParts());
      const field = handle.decodeWindow(request);
      const values = Array.from(field.values());
      const mask = Array.from(field.mask());
      let right = values.length === window.values.length;
      for (let at = 0; at < window.values.length && right; at += 1) {
        const want = window.values[at];
        right = want === 'NaN' ? mask[at] === 0 : mask[at] === 1 && values[at] === want;
      }
      check(`${label}: values are xarray's, the unwritten inner chunk masked`, right);
      field.free();
      handle.free();

      // Only the indexes and the planned inner chunks were ever served.
      const planned = [
        ...window.shard_indexes.map((i) => [i.key, i.location === 'end' ? window.shard_sizes[i.key] - i.length : 0, i.length]),
        ...window.inner_chunks.map((c) => [c.key, c.offset, c.length]),
      ];
      same(`${label}: served only the planned ranges`, served, planned);
      // A window that needs every inner chunk of its shards (the whole grid,
      // whose shards hold one step) fetches at most the shards' bytes; any
      // other fetches fewer.
      const servedBytes = planned.reduce((n, [, , len]) => n + len, 0);
      const shardBytes = window.chunks.reduce((n, c) => n + window.shard_sizes[c.key], 0);
      check(
        `${label}: ${window.name === 'whole_grid' ? 'no more' : 'fewer'} bytes than the whole shards`,
        window.name === 'whole_grid' ? servedBytes <= shardBytes : servedBytes < shardBytes,
        `${servedBytes} of ${shardBytes}`,
      );
    }
  }

  // The shape `openStore` takes for part of an object is checked.
  const key = 't2m/c/0/0/0';
  const size = statSync(pathOf(key)).size;
  const bytes = new Uint8Array(4);
  for (const [what, value] of [
    ['a range past the size', { size: 2, ranges: [{ offset: 0, bytes }] }],
    ['a size that is not a byte count', { size: -1, ranges: [] }],
    ['a misspelt key', { size, ranges: [{ offest: 0, bytes }] }],
    ['bytes that are not a Uint8Array', { size, ranges: [{ offset: 0, bytes: [1, 2] }] }],
    ['an unknown key beside size and ranges', { size, ranges: [], etag: 'x' }],
  ]) {
    same(`object ranges: ${what} is refused`, codeOf(() => wasm.openStore(new Map([...base, [key, value]]))), 'invalid_option');
  }
  // An index altered on the way is never sliced: the plan still names the
  // shard and leaves `innerChunks` null (fetch it whole), and the decode
  // refuses the index.
  const index = fetchRange(key, { suffix: true, offset: 0, length: expected.index_length });
  index.bytes[0] ^= 0xff;
  const tampered = wasm.openStore(new Map([...base, [key, { size: index.size, ranges: [{ offset: index.offset, bytes: index.bytes }] }]]));
  const t2m = tampered.variables().find((v) => v.name === 't2m');
  const corner = { variable: t2m.index, yDim: 1, xDim: 2, bounds: [55, 75, 0, 30], sliceIndices: [0, 0, 0] };
  const tamperedPlan = tampered.planWindow(corner);
  same('a tampered shard index still plans its shard', tamperedPlan.chunks.map((c) => c.key), [key]);
  same('and no inner chunk', tamperedPlan.innerChunks, null);
  same('and the decode refuses it', codeOf(() => tampered.decodeWindow(corner)), 'decode');
  tampered.free();
}

// A message stream has no windows to plan.
const grib = wasm.open(
  new Uint8Array(readFileSync(join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2'))),
);
same('a GRIB file has no coordinate chunks', codeOf(() => grib.coordinateChunks(0)), 'wrong_addressing');
grib.free();

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
