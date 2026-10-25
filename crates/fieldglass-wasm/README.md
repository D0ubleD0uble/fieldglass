# fieldglass-wasm

A synchronous browser façade over the Fieldglass decoders. wasm-bindgen on
`wasm32-unknown-unknown`, single-threaded, meant to run inside a Web Worker.

It is a binding of [`fieldglass`](../fieldglass) and nothing more: the typed-array
handoff, the error mapping, method forwarding, and the GLSL snippet a GPU host
pastes into its own shader. Every decision about what a field *is* was made
upstream ([ADR-0006](../../docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)).

## Build

```sh
./build.sh web      # ES module for a browser  → pkg/web
./build.sh nodejs   # CommonJS for Node        → pkg/nodejs
./build.sh web --simd    # the +simd128 variant → pkg/web-simd
./build.sh web --no-opt  # skip wasm-opt
./build.sh web --features=fetchplan,zarr   # the full entry → pkg/web-fetchplan-zarr

./pack.sh           # the npm package, both entries → pkg/npm + a .tgz
```

`pack.sh` assembles what `release.yml` publishes as
[`@fieldglass/wasm`](https://www.npmjs.com/package/@fieldglass/wasm) on a `v*`
tag (#466), from two web builds: the default entry, and the full one in the
package's `full/` directory (see **Two entries** below). The version comes from the workspace `Cargo.toml`, never from an
argument, so the package cannot drift from the crates and the extension; the
committed `npm/package.json` carries a `0.0.0` placeholder that `pack.sh`
refuses to ship.

`tests/node/package.mjs` installs the resulting tarball into a throwaway project
and decodes through the installed tree. That is a different question from the
other Node tests, which load `pkg/nodejs/fieldglass_wasm.js` by path and so
would pass with a `package.json` that shipped none of it. Both run on every pull
request.

`pkg/` is a build product and is gitignored. Two tools have to be there:

- **`wasm-bindgen`**, at the same version as the `wasm-bindgen` crate the build
  resolved. The script checks, because a mismatch surfaces as an opaque
  "invalid schema version" at import time rather than as a build failure.
  Install the matching one with
  `cargo install wasm-bindgen-cli --version <the version it names>`.
- **`wasm-opt`** ([binaryen](https://github.com/WebAssembly/binaryen/releases)),
  because `-Oz` is part of what ships and the sizes under **Measured** are the
  optimised ones. `--no-opt` builds without it; the bundle is then not the
  shipped one.

`wasm-opt` is invoked with the target's own feature list, read out of
`rustc --print cfg`. Its defaults are narrower than the target's, so a bare
`wasm-opt -Oz` rejects every `memory.copy` rustc emits.

Not wasm-pack, and not napi-rs's wasm target: napi-rs only ships loaders for
`wasm32-wasip1-threads`, which needs COOP/COEP cross-origin isolation and about
half a megabyte of emnapi/WASI shims.

## Two entries

The package ships the module built two ways (#1158), and a page loads one of
them:

| Import | Built with | What it has |
|---|---|---|
| `@fieldglass/wasm` | the default features | GRIB1, GRIB2 and NetCDF: open, decode, place, warp, paint, probe, contour, combine |
| `@fieldglass/wasm/full` | `--features=fetchplan,zarr` | all of that, plus the fetch planner and Zarr stores |

Use the default unless the page plans fetches from a GRIB sidecar or reads a
Zarr store: the full module is about 29% larger gzipped (both are gated under
**Measured**). Each entry has its own `.wasm`
(`@fieldglass/wasm/fieldglass_wasm_bg.wasm` and
`@fieldglass/wasm/full/fieldglass_wasm_bg.wasm`, for a bundler or a Worker's
`init`) and its own `.d.ts`, which declares only what that entry has. Importing
`openStore` or `PlanItem` from the default entry is a type error, not an
`undefined` at run time.

The default entry's module and glue are byte for byte what they were before the
full entry existed; its `.d.ts` only lost the planner and Zarr types it used to
declare without containing them. The split can later become one bundle, with
`full` kept as an alias, without breaking an import of either.

## Use

```js
import init, { open, glslSnippet, combineOps } from './pkg/web/fieldglass_wasm.js';
await init();

const handle = open(new Uint8Array(await response.arrayBuffer()));
handle.count();                    // messages in the file
handle.message(0);                 // one message's metadata, built on demand

const field = handle.decode(0, {});           // { dtype?: 'auto' | 'f32' | 'f64' }
field.values();                    // Float32Array or Float64Array — see `dtype()`
field.mask();                      // Uint8Array, 1 present / 0 absent
field.grid();                      // kind, boundsLonlat, placement, proj4, affine, scan

const palette = handle.palette(field, {});    // { lut, t0, t1, span, scale, maskedRgba, range }
handle.shaderValues(field, {});    // Float32Array: transformed and rebased by t0
handle.shaderMask(field, {});      // Uint8Array the shader tests

// A window at a pixel size — what a map view asks for (#465). `width` and
// `height` go together; one alone throws `invalid_option`. Without them the
// output is the source `ni × nj`, as before.
handle.warp(field, { bounds: [24, 50, -125, -66], width: 512, height: 512 });
const { rgba, width, height, range } = handle.render(field, {}, false);   // RGBA, north up — see below
// `range` is the color range used, as the VS Code extension reports it: usedMin,
// usedMax, rangeSource, center, and how many cells fell below and above it.
// The ends match the extension's everywhere; the counts only in the source
// projection, as the extension counts the pixels of a warped map.
// `palette.range` is the same range, for a legend drawn beside the GPU path.
handle.probe(field, lat, lon);
handle.contours(field, new Float64Array([280, 290]), {});
// No levels: nice ones over the range render uses for the same min, max, center and scale.
handle.contours(field, new Float64Array([]), { min: 270, max: 300 });

// Difference maps and their siblings. `combineOps()` is the picker's list:
// [{ value: 'a_minus_b', label: 'A − B' }, …]. Both fields must sit on the
// same grid; a mismatch throws with `code: 'unsupported'` naming what differs.
const other = handle.decode(1, {});
const diff = handle.combine(field, other, 'a_minus_b');

// A zoomed-out map at a coarser JPEG 2000 level (#1082). `reduce: 1` is half
// the size on each axis. The values are not the file's, so this is a
// WasmDisplayField: only the three *Display calls take it, and TypeScript
// refuses it to probe, contours and combine. The colour range is the full
// field's, from its stats; pass `null` only with both `min` and `max` set.
// A message with no coarse field throws `unsupported`; ask again at 0.
const coarse = handle.decodeDisplay(0, { reduce: 1 });
coarse.grid();                     // the derived grid: every second point
handle.renderDisplay(coarse, field.stats(), {}, false);
handle.warpDisplay(coarse, { bounds: [24, 50, -125, -66], width: 512, height: 512 });
handle.paletteDisplay(coarse, field.stats(), {});

coarse.free();
diff.free();
other.free();
field.free();
handle.free();
```

### Planning a fetch

The planner reads a GRIB sidecar and says which bytes of the object to fetch
for a field, so an app needs no sidecar parser of its own. Nothing here does
I/O; the fetches are yours.

**In the full entry only**, `@fieldglass/wasm/full` (built locally with
`./build.sh web --features=fetchplan,zarr`).

```js
import init, {
  open, parseWgrib2Idx, httpRangeHeader, verifyEnvelope, verifyMessage, candidates,
} from '@fieldglass/wasm/full';
await init();

// Which runs should have posted by now, newest first. A guess: take the first
// whose sidecar exists.
const source = {
  keyPattern: 'gfs.{yyyy}{mm}{dd}/{HH}/atmos/gfs.t{HH}z.pgrb2.0p25.f{fff}',
  cycleHours: [0, 6, 12, 18], latencyMinutes: 240, maxCandidates: 4,
};
let key, text;
for (const run of candidates(source, { nowUnixSecs: Math.floor(Date.now() / 1000) })) {
  const sidecar = await fetch(`${bucket}/${run.key}.idx`);
  if (sidecar.status === 404) continue;            // not posted yet: try the run before
  if (!sidecar.ok) throw new Error(`${run.key}.idx: HTTP ${sidecar.status}`);
  [key, text] = [run.key, await sidecar.text()];
  break;
}
if (key === undefined) throw new Error('none of the recent runs has posted');
const idx = parseWgrib2Idx(key, text);

// By the sidecar's own words, or by WMO codes and a level. Every match comes back.
const items = idx.select({ abbreviation: 'TMP', levelText: '2 m above ground' });
// idx.select({ parameter: { discipline: 0, category: 0, number: 0 },
//              level: { surface: 'height_above_ground', value: 2, value2: null } });

const item = items[0];
const header = httpRangeHeader(item.range);          // 'bytes=…', or null for the whole object; throws (invalid_option) for an exact range of no bytes or past 2^64
const response = await fetch(`${bucket}/${key}`, header ? { headers: { Range: header } } : {});
// 206 for a ranged request, 200 for the whole object. Check before trusting the
// bytes: a 200 to a ranged request is a server that ignored `Range` and sent the
// whole object, and an error page is not a message. Either one passed on reads
// as a stale sidecar or a short read, and sends you after the wrong fix.
if (response.status !== (header ? 206 : 200)) throw new Error(`${key}: HTTP ${response.status}`);
const bytes = new Uint8Array(await response.arrayBuffer());

verifyEnvelope(item, bytes);   // throws short_read (retry) or decode (stale sidecar)
const handle = open(bytes);
verifyMessage(item, handle.message(0));   // throws decode if it is another field
idx.free();
```

`parseEcmwfIndex` reads an ECMWF `.index` the same way. Its parameters are MARS
short names (`2t`), which no table here resolves to WMO codes, so select those
by `abbreviation`.

### Reading a window of a Zarr store

A Zarr store is many objects, and a page fetches only the ones a view needs.
The store says which. Nothing here does I/O; the fetches are yours.

**In the full entry only**, `@fieldglass/wasm/full`, as the planner is.

```js
import init, { openStore } from '@fieldglass/wasm/full';
await init();

// An object's bytes, or `null` for one the store does not hold: a sparse array
// leaves chunks unwritten, and a 404 for one is its fill value, not an error.
// Pass `null` rather than leaving the key out: a key not passed is one you
// have not fetched, and a read that needs it throws.
const get = async (key) => {
  const response = await fetch(`${store}/${key}`);
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`${key}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
};

// 1. The metadata: `zarr.json` for v3, `.zmetadata` for a consolidated v2 store.
// If the store's root is itself an array, pass its name as a second argument,
// `openStore(objects, 'air')`: it has none inside the store, and is listed as
// an empty name without one.
const objects = new Map([['zarr.json', await get('zarr.json')]]);
let ds = openStore(objects);
const t2m = ds.variables().find((v) => v.name === 't2m');

// 2. The coordinate arrays a box is matched against (and that label the steps).
for (const { key } of ds.coordinateChunks(t2m.index)) objects.set(key, await get(key));
ds.free();
ds = openStore(objects);

// 3. The chunks a box and a time step touch, and only those.
const request = {
  variable: t2m.index, yDim: 1, xDim: 2,
  bounds: [20, 55, -130, -60],          // [latMin, latMax, lonMin, lonMax]
  sliceIndices: [0, 0, 0],               // the time step; the two map axes are ignored
};
const plan = ds.planWindow(request);     // { rows, columns, ni, nj, chunks, shardIndexes, innerChunks }
for (const { key } of plan.chunks) objects.set(key, await get(key));
ds.free();
ds = openStore(objects);

const field = ds.decodeWindow(request);  // a field like any other: palette, warp, probe
```

A cell is in the box when its centre is, edges included (to a billionth of a
cell, so an edge written as a centre's coordinate holds it). A box over the
antimeridian runs past 180 (`[.., 170, 190]`) rather than wrapping, and on a
global grid it comes back as two column runs decoded as one window. A window is
at least two cells each way: one row or column has no spacing to place it by,
so a box that holds one takes the next one as well, and the plan names its
chunks. Windows are planned over slices placed on a regular lat/lon grid, as
1-D latitude and longitude coordinates place them; anything else throws
`unsupported`.

**Pass a 404 as `null`, and pass every key a plan names.** `null` says the store
holds no such object. A chunk the store never wrote is its array's fill value,
which is how a sparse store works and how zarr-python leaves any chunk that is
all fill, coordinate chunks included: a coordinate chunk passed as `null` reads
as its fill value, and when that value places no grid (NaN, say) `planWindow`
throws `unsupported` naming the chunk, and `decodeSlice` throws `decode` naming
the coordinate. A key you do **not** pass at all, or
pass as `undefined`, is one you have not fetched, and it is never read as fill: `planWindow` and
`decodeWindow` throw `invalid_option` naming every such coordinate or planned
chunk, and any other read that needs one (`decodeSlice`, `decodeLine`,
`axisValues`) throws `decode` naming it. A window over a coordinate the store
leaves out (an element type this build does not read, say) throws
`unsupported` naming the coordinate and why. An unconsolidated v2 store needs
each array's and group's `.zattrs` too, or `null` for a 404: it holds the
dimension names and the CF scale and fill, so `openStore` throws `decode`
naming one it was not given.

**A sharded array can be read in part.** In a v3 store with `sharding_indexed`,
each planned key in `chunks` is a shard, which may hold many time steps or a
large area. Fetching each whole still works. To fetch only what the window
shows, read each shard's index first and then the inner chunks it needs, both
by range, and give them to `openStore` as `{ size, ranges }` under the shard's
key:

```js
// `shardIndexes`: each index is the shard's last `length` bytes, or its first.
// The store also needs each shard's whole size. A page can read it from
// `Content-Range` only where the bucket lists that header in
// `Access-Control-Expose-Headers`, and dynamical.org's does not (checked
// 2026-10-21). So this asks a `HEAD` for `Content-Length`, which a page can
// always read, at the same time as the range.
const range = async (key, header) => {
  const response = await fetch(`${store}/${key}`, { headers: { Range: header } });
  if (response.status === 404) return null;
  if (response.status !== 206) throw new Error(`${key}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
};
const sizeOf = async (key) => {
  const response = await fetch(`${store}/${key}`, { method: 'HEAD' });
  return response.ok ? Number(response.headers.get('Content-Length')) : null;
};
for (const index of plan.shardIndexes ?? []) {
  const header = index.location === 'end' ? `bytes=-${index.length}` : `bytes=0-${index.length - 1}`;
  const [bytes, size] = await Promise.all([range(index.key, header), sizeOf(index.key)]);
  if (bytes === null) { objects.set(index.key, null); continue; } // a 404: all fill
  // No size, no ranges: `openStore` refuses `{ size, ranges }` without a
  // byte-count `size` (`invalid_option`), so take this shard whole instead.
  if (size === null) { objects.set(index.key, await get(index.key)); continue; }
  const offset = index.location === 'end' ? size - index.length : 0;
  objects.set(index.key, { size, ranges: [{ offset, bytes }] });
}
ds.free();
ds = openStore(objects);

// `innerChunks`, now that the indexes are in: each inner chunk the window
// intersects, as an `offset` and `length` within its shard. Inner chunks the
// index marks as never written are left out; they are the fill value. Still
// `null` with every index in the map means the plan could not go by range
// (an index that fails its checksum, say): fetch the shards whole.
const { innerChunks } = ds.planWindow(request);
if (innerChunks === null) {
  for (const { key } of plan.chunks) objects.set(key, await get(key));
} else {
  for (const inner of innerChunks) {
    // Zero bytes cannot be named by a range, and the decode refuses such a chunk.
    if (inner.length === 0) continue;
    const bytes = await range(inner.key, `bytes=${inner.offset}-${inner.offset + inner.length - 1}`);
    objects.get(inner.key).ranges.push({ offset: inner.offset, bytes });
  }
}
ds.free();
ds = openStore(objects);
const field = ds.decodeWindow(request);
```

`shardIndexes` and `innerChunks` are `null` for a variable that is not sharded,
and `innerChunks` is `null` until every index is in the map (a shard given
whole, or as `null` for a 404, counts). **If `innerChunks` is still `null` once
every `shardIndexes` entry is in the map, fetch `chunks` whole**: the plan could
not go by range. The same holds when a shard's size is unknown (a `HEAD` that
failed): `openStore` refuses `{ size, ranges }` without a byte-count `size` as
`invalid_option`, so give that shard whole instead. A read takes each range from
one range you gave that holds it, so give each range as fetched. `decodeWindow` throws
`invalid_option` naming each index or inner-chunk range it lacks. A shard index
that fails its checksum, or names bytes outside its shard or over the index
itself, is never read: `planWindow` leaves `innerChunks` `null`, so fetch
`chunks` whole, and `decodeWindow` throws `decode` naming what is wrong.
`shardIndexes` is `null` too for a sharding configuration this build does not
read, or a shard of more than 1,048,576 inner chunks.

### Which way up a raster comes out

`render(field, options, flipY)` composes `flipY` with the message's own scan
order rather than replacing it. Grid point `(i, j)` paints at pixel `(i, j)`, so
a field whose rows run south to north (`grid().scan.jPositive`) arrives upside
down on a canvas whose first row is the top — and `false` therefore means
**north up**, not "rows as stored". Pass the user's own request straight
through; composing the flag yourself would flip twice.

Grid point `(i, j)` is the middle of that pixel: `render` paints it as the
square from `(i, j)` to `(i + 1, j + 1)`. A host that draws on the raster adds
half a pixel to anything it places by grid index. `contours()` returns its
vertices as grid indices (`Isoline.segments`, `[i0, j0, i1, j1]`), so stroke a
vertex at `x = (i + 0.5) * sx` and `y = (j + 0.5) * sy`, where `sx` and `sy` are
the canvas pixels per raster pixel. Without the half pixel a line sits up and
left of the cells it separates, short of the right and bottom edges. When
`render` flipped the rows (`grid().scan.jPositive` differs from the `flipY` you
passed), raster height `nj` turns row `j` into `y = (nj - j - 0.5) * sy`; `x` is
never flipped. On a global west-to-east grid `i` can reach `ni`, past the last
column: that vertex is column `i - ni` again and lands half a pixel off the
right edge. `probe()` reports `i` and `j` the same way. The Node binding's
`projectContours` already returns pixels with all of this applied.

`grid().scan` is still on the DTO, because it is the one thing the geometry
cannot answer — and a **GPU host needs it**: `shaderValues()` and
`shaderMask()` are in the field's own data order, so a host uploading them owes
the same composition `render()` makes for you. `examples/smoke` does exactly
that where it lines the two pictures up.

### The memory contract

Linear memory never shrinks and an animation holds many fields at once, so
**the façade keeps no decode cache**. `decode()` hands a field to JS and JS owns
it; `warp`, `render`, `probe`, `contours`, `combine`, and the shader accessors take it back
by reference; `decodeDisplay()` hands over a display field the same way. Call
`close()` (or `free()`, the same) on a field and on the handle when you are
done, or open it with `using`, which calls `[Symbol.dispose]` at the end of the
block — a dropped JS reference does not release the wasm allocation until the
host's `FinalizationRegistry` runs, if it runs at all.

### One shape with the Node binding

The repository's Node binding (`fieldglass-napi`, internal to the VS Code
extension) binds the same `Session` with the same shape (#1073): `open`,
`format`, `addressing`, `count`, `message`, `variables`, `leftOut`,
`undrawable`, `decode`, `decodeSlice`, `decodeLine`, `decodeDisplay` and `close`
take the same arguments and mean the same things in both, and
`tools/check_js_api.py` holds both generated declarations to that.
`decodeSlice` and `decodeLine` take one object, `{ variable, yDim, xDim,
sliceIndices }` and `{ variable, alongDim, sliceIndices }`, where `variable` is a
position in `variables()` and `sliceIndices` may be a `Uint32Array` or a plain object
keyed `"0"`, `"1"`, … (the form the Node binding receives a typed array as), and
refuse a key they do not know. `close()` is harmless twice, and every other call on a
closed object throws. Everything here is synchronous; `load(source, { signal })`
and `<name>Async` are reserved for the asynchronous layer (#1085), which adds to
these names rather than renaming them.

Every accessor that returns a typed array **copies** out of linear memory. A
view into it dangles the moment wasm grows the heap, so a host that wants to
reuse a buffer copies into its own once and keeps that.

### Panics

Built with `panic = "abort"` (the `wasm-release` profile in the workspace
manifest): a decoder panic kills the Worker. The fuzz targets make that rare.
Treat the Worker as disposable and start another.

### Errors

Every failure throws a JS `Error` with a stable `code` property —
`unsupported_format`, `decode`, `no_such_message`, `unsupported`,
`wrong_addressing`, `invalid_option`, `short_read`, `closed` (any call on an
object you closed). Branch on `code`; the
`message` is prose and may be reworded. `short_read` comes only from the
planner's `verifyEnvelope`, for a fetch that returned fewer bytes than the plan
asked for or the message declares. A sidecar that does not parse is `decode`,
and a source entry the planner cannot use is `invalid_option`.

## Values first, pixels second

`render()` is a CPU fallback. The intended path is `palette()` plus
`glslSnippet()`: colour is decided once, in Rust, and exported as a 256-entry
lookup table, so restyling never re-decodes and the CPU painter stays the oracle
the shader is checked against rather than a second colour implementation. See
[`../fieldglass/README.md`](../fieldglass/README.md) for why the field is
rebased in Rust before it reaches the shader.

## Measured

Both numbers are what a check printed on every pull request rather than an
estimate (#462), but only one of them is a gate. The bundle table is: CI's
`Bundle-size gate` step fails when a build drifts more than 5% from it in either
direction. The decode timings are printed and never compared — a per-run timing
gate on a shared runner would be noise, so the number is the deliverable.
Machine-dependent either way, so read the ratio and the shape, not the absolute
milliseconds.

### Bundle

`build.sh web` — wasm-bindgen, then `wasm-opt -Oz` — gzipped, which is what a
browser actually downloads.

These figures carry **all three** decoders. NetCDF joined the browser build in
#662, and it is the expensive one: opting in cost +293,001 raw bytes (+30.2%)
and +125,927 gzipped (+33.5%), because it brings a whole container format, the
HDF5 object model and filter pipeline behind it, and the variable-addressing
half of the façade. It is the one entry in this table that a consumer might
reasonably want to undo, and the umbrella's per-format features already make a
GRIB-only bundle buildable. It was not an accident: the same file rendering in
the VS Code editor and being refused in a browser was the divergence the issue
was about.

Measure the bundle you are recording, not an earlier one. The first figures
written here for #662 were 100 KB light because they were taken before the
façade grew `variables`, `dimensions` and `decodeSlice`, and the gate — running
on CI against the finished tree — is what caught it.

Moving the HDF5 reader onto `ByteSource` (#682) added ~31,000 raw bytes, about
2.4%: its traversal is generic over the source now, so the browser build carries
one instantiation of a tree that used to be a single concrete one. That is
within the gate's tolerance, and the figures above are re-recorded anyway — a
documented size that is merely *close* measures the next change against a
generous number instead of the real one.

Opening a session over a `ByteSource` (#709) took about 8,900 raw bytes, 0.7%,
and 2,800 gzipped. It was expected to *shrink*: the GRIB readers used to be
monomorphised over their source type and are now instantiated once behind
`Box<dyn ByteSource>`, which is the opposite of the second copy of every decode
path the issue warned a source-taking constructor might add. It grew because the
same change adds what a source is for — `Session::open_source`,
`open_message_at`, the detection prefix read — plus `left_out`, `LeftOutArray`,
`Error::ShortRead` and its rendering, and the vtables the erasure needs. Measured
against binaryen 132 as CI pins it, and re-recorded: 0.7% is comfortably inside
the gate, and leaving a generous number in this table would measure the next
change against it rather than against the real one.

Building the NetCDF view from core's array model (#684) took about 17,600 raw
bytes out (1.4%, and 2.9% gzipped), measured against a build of the commit
before it on the same machine.

Moving both GRIB readers onto `ByteSource` (#697) added 8,527 raw bytes (0.7%)
and 2,403 gzipped (0.5%): the windowed scan and the exact-length reads that
replace indexing a buffer. The readers are generic over their source, but the
browser build only ever instantiates `Vec<u8>`, so there is one copy of each.

Moving the CF placement rules into core (#704) left the raw size 240 bytes
smaller and the gzipped size 419 bytes larger (under 0.1% either way). The
browser build reads GRIB only, so the new code it carries is the array helpers
core now shares with the NetCDF and Zarr readers.

Memoising slice placement (#662, ADR-0011) added 4,984 raw bytes and 2,086
gzipped, 0.4% either way — a `HashMap` and its key type on the array path, which
this build carries because it opens NetCDF. It buys the browser the cache it had
no version of: a curvilinear field's spatial index is now built once per open
file rather than once per repaint.

Answering a message's identification in `Session` (#726) added 32,875 raw bytes
and 14,768 gzipped, 2.9%. That is string tables, not code: the browser now
resolves the originating centre and sub-centre from the CCT common code tables
and the GRIB2 discipline, production status and data type from their WMO tables,
where before it reported none of them. The alternative — hand a host the raw
code numbers and let it name them — is what the conventions rule out, since it
puts WMO table maintenance at each binding layer.

Carrying a placement's corner pair (#726) added 2,916 raw bytes and 1,223
gzipped, 0.2%: two accessors in `core` that place a grid's first and last points,
and one more field on every `Georef` the browser hands back.

Exporting a reduced grid's own points rather than its widened raster (#244) added
1,374 raw bytes and 585 gzipped, 0.1%: the per-row geolocation in the long CSV,
and the row counts every `Georef` for a reduced grid now carries.

Reading a line through a variable (#172) added 5,683 raw bytes and 2,503
gzipped, 0.5%: the `decodeLine` binding, the region read and CF unpacking behind
it, and the serialisation of the `Line` it returns.

Carrying every ECMWF GRIB1 local parameter table (#601) added 157,353 raw bytes
and 41,657 gzipped, 7.9%: the 27 tables past 128 and 129, 2,255 entries of names and
units, which GRIB1 message metadata reports in the browser as it does
everywhere else. Unlike the growth above, this is data rather than code, and it
is the one change here a browser host might reasonably want to opt out of; the
format crate has no feature to do that with today.

Saying why a grid can or can't be placed (#776) added 1,387 raw bytes and 810
gzipped, 0.1%, against the figures recorded before it: the `Placement`
vocabulary and two more fields on every `Georef` and `MessageInfo` the browser
hands back.

Decoding GRIB2 5.42 with `fieldglass-aec` instead of an external crate (#762)
added 692 raw bytes and 1,384 gzipped (0.2%), measured against a build of the
commit before it on the same machine. The new decoder is about the same size as
the old one; the sink that scales samples straight into the field replaces the
byte buffer and re-parse.

Rendering the addon on the geometry the library placed (#574) took 361 raw
bytes and 189 gzipped out, measured against a build of the commit before it on
the same machine. The browser build carries the parts of that change that live
in `fieldglass`: the combine gate lost the arm that compared two refusals, a
refusal names an unplaced grid by its own label, and a planar grid's refusal
names the Earth radius it declares. The figures below also carry the two
changes merged after the table was last recorded (#781, #786), which between
them added about 1,300 raw bytes.

Spreading GRIB values over a bitmap through one proved function (#785) took
712 raw bytes and 301 gzipped out of the baseline build (under 0.1%), and 720
and 345 out of `+simd128`, measured against a build of the commit before it
(#787) on the same machine:

| Build | before `.wasm` | after `.wasm` | before gzipped | after gzipped |
|---|---:|---:|---:|---:|
| baseline | 1,514,338 | 1,513,626 | 574,707 | 574,406 |
| `+simd128` | 1,498,791 | 1,498,071 | 569,029 | 568,684 |

Returning `null` for an absent value (#574, `serialize_missing_as_null` on every
object the façade returns) took 397 raw bytes and 218 gzipped out of the
baseline build, and 397 and 105 out of `+simd128`, measured against master
after #785. The generated TypeScript declarations cost nothing here:
wasm-bindgen moves them into the `.d.ts` and drops the custom section from the
module.

Decoding the HDF5 szip filter (#421), with the byte-plane end-of-stream check
it relies on (#794), added about 5.5 KB raw and 2.3 to 2.9 KB gzipped (0.4%
and 0.5%), measured against a build of the commit before it (#799) on the same
machine with binaryen 132. The coder was already in the bundle for GRIB2; what
is new is its szip layer, the reader's framing and the length checks:

| Build | before `.wasm` | after `.wasm` | before gzipped | after gzipped |
|---|---:|---:|---:|---:|
| baseline | 1,513,182 | 1,518,725 | 574,136 | 577,027 |
| `+simd128` | 1,497,627 | 1,503,113 | 568,547 | 570,891 |

Drawing a spectral field finer than the map at the map's resolution (#637) added
7,840 raw bytes and 3,878 gzipped to the baseline build (0.5% and 0.7%), and
7,684 and 4,096 to `+simd128`, measured with binaryen 132 against the figures
recorded before it: the band-limit label on every `Field` and `MessageInfo`,
the `probeMessage` and `fullDetail` bindings and the two wire types they
return, and the transform's extended-exponent column starts.

Bounding deflate and zstd by the chunk's own length (#813) added 804 raw bytes
and 314 gzipped to the baseline build, and 795 and 393 to `+simd128` (under
0.1%), measured with binaryen 132 against a build of the commit before it
(#831's merge) on the same machine: the per-chunk limit, the worst-case
growth chained through the filters before a codec, and the error that names
which bound a chunk broke.

Declining a GRIB2 grid whose rotation or scale factor is not finite, and
decoding and placing such a grid on the rows and columns its section
declares (#823), added 429 raw bytes and 236 gzipped to the baseline build,
and 429 and 162 to `+simd128`, measured with binaryen 132 against a build of #832's merge on the
same machine, which itself measured exactly the figures recorded before it.

Folding `Georef`'s origin, step and units into one nullable `affine` (#870)
added 193 raw bytes and 172 gzipped to the baseline build, and 197 and 194 to
`+simd128`, measured with binaryen 132 against a build of #877's merge on the
same machine. That build was itself 5,342 raw bytes and 560 gzipped above the
figures recorded before it, from merges that did not re-record the table, so
the table now records the measured build rather than adding to a stale one.

From 2026-10-08 the table holds CI's own measurement, copied from the `wasm`
job's run summary, not a local build. CI builds with the current stable rustc
(1.99.0 here), and a local build on an older toolchain came out about 1.1%
larger in raw bytes (gzipped agreed to within 0.1%). That was enough to make the
1% re-record rule below fire on every PR. The figures are from the `ci.yml` run
on `ee428723` (#929's merge): 22,029 raw and 12,424 gzipped above the previous
baseline figures, and 21,383 and 13,141 above `+simd128`'s. That is the
local-versus-CI gap plus the growth of the merges since #870.

The 0.6.0 release prep (#854) re-recorded it from the `ci.yml` run on
`42c1fd0f`: 30,147 raw bytes and 18,657 gzipped above the previous baseline
figures (3.1%), and 33,276 and 19,471 above `+simd128`'s (3.3%). That is the
growth of the merges since #929.

#1155 re-recorded it from the `ci.yml` run on its PR (#1167, `22ad0823` merged
onto `3bc0c205`), whose drift read 5.3% gzipped for the baseline and 5.0% for
`+simd128`: 58,842 raw bytes and 32,719 gzipped above the previous baseline
figures, and 59,035 and 30,901 above `+simd128`'s. Master at `3bc0c205`
already measured 1,637,153 and 643,516 (4.6%), and 1,623,582 and 638,372
(4.2%), so 8,921 raw and 4,431 gzipped of the baseline's growth, and 9,273 and
5,005 of `+simd128`'s, are #1155's: the one colour-range rule, its range
result and refusal types, and the contour options. The rest is the merges since
#854.

#1082 re-recorded it from the `ci.yml` run on its PR (#1185, `214eb524` merged
onto `f0eebc2f`), whose drift read 1.1% for both builds: 18,733 raw bytes and
5,965 gzipped above the previous baseline figures, and 18,283 and 7,231 above
`+simd128`'s. That is the reduced-resolution decode, its display field and its
three drawing calls.

`full` is the `@fieldglass/wasm/full` entry (#1158), `build.sh web
--features=fetchplan,zarr`, recorded from the `ci.yml` run on its PR (#1177,
`3d897b4e`, rebased onto #1082's merge): 28.4% above the baseline gzipped,
and 28.0% raw. That is the fetch planner and the Zarr stores.

All three rows were re-recorded from the `ci.yml` run on #1298's PR (run
39625919587), whose `full` build had drifted 5.0% from the row above it. Most
of that drift is the MARS short-name table, which now keeps each eccodes
block's surface, probability and template conditions and the rival blocks
that can outrank them. The rest is what merged since the previous recording.

All three rows were re-recorded from the `ci.yml` run on #1123's PR (run
39652231318), whose `full` build had drifted 2.2% from the row above it. That
is the sharded-window planner and the ranged `openStore`.

<!-- checked by tools/check_wasm_bundle_size.py -->

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| baseline | 1,686,964 | 662,056 |
| `+simd128` | 1,673,231 | 659,047 |
| full | 2,285,313 | 895,421 |

The table **is** the gate: `python3 tools/check_wasm_bundle_size.py` fails when a
build drifts more than 5% from these figures in either direction, so a change
that moves the bundle that far has to say so here.

**When to re-record.** CI's `wasm` job prints each build's measured size and its
drift from these figures in its run summary. A PR re-records the table when
that drift reads above 1.0% for either build. The release prep PR re-records it
every time, so the figures match CI at each release and a cycle of small
increases shows up in one diff (RELEASING.md §1). In both cases, copy the
measured `.wasm` and gzipped columns from the summary into the table, and add a
paragraph above it saying what moved. Don't record from a local build: its
toolchain differs from CI's, and so does its size. CI takes the current stable
rustc, so a new Rust release alone can push the drift over 1.0%; the next PR
then re-records it, saying so.

**Zarr and the planner, as measured for the decision (#1083, #1081).** The
owner chose to ship both in a second entry rather than in the default one
(#1158); the `full` row above is that entry, gated like the others. Measured
locally (rustc 1.98, wasm-bindgen 0.2.129, binaryen 132, `build.sh web`, gzip
level 9 as the gate measures it) on #1083's branch, against its default build:

| Build (local) | `.wasm` bytes | gzipped bytes | gzipped vs default |
|---|---:|---:|---:|
| default | 1,627,492 | 632,104 | |
| `--features=zarr` | 1,859,338 | 738,550 | +106,446 (+16.8%) |
| `--features=fetchplan` | 1,920,568 | 749,553 | +117,449 (+18.6%) |
| `--features=fetchplan,zarr` | 2,092,532 | 817,175 | +185,071 (+29.3%) |
| a Zarr-only module (`zarr`, `render`, `analysis`; no GRIB or NetCDF) | 639,242 | 280,935 | a page loads it as well as, or instead of, the default |

Zarr adds the store walker, its metadata parser and `serde_json`, blosc with
LZ4 and BloscLZ, and sharding; zstd and DEFLATE are already in the default
build for NetCDF. Beside `fetchplan`, which already carries `serde_json`, it adds
67,622 gzipped bytes rather than 106,446.

Two things worth knowing before optimising further:

- `wasm-opt -Oz` is a **raw** win and a **transfer** loss (measured locally
  before NetCDF, #662; the module has grown since). It took the module
  from 1,342,887 to 1,262,479 bytes (-6.0%) and took it from 489,627 to 501,678
  gzipped (+2.5%). Its size passes trade repetition for smaller encodings, and
  DEFLATE was already being paid for the repetition. It stays on because parse
  and instantiate cost track the raw module, but a transfer-size-only argument
  for `-Oz` does not survive measurement. The trade held its shape when NetCDF
  was added (#662): it was -5.2% / +1.9% on the GRIB-only bundle.
- `+simd128` saves 16,541 raw bytes on the table above and buys nothing
  measurable in time (below). The
  decode kernels are bit-unpacking loops with data-dependent control flow, not
  the float-per-lane arithmetic autovectorisation looks for, and `std` is not
  rebuilt with it without `-Zbuild-std`. Recorded so nobody re-derives it.

### Decode

`tests/node/bench.mjs`, median of five decodes after a warm-up, against the
native release build of the same code through the same `Session::decode`. On one
x86-64 Linux machine, Node 22:

| Field | Points | native ms | wasm ms | wasm / native |
|---|---:|---:|---:|---:|
| ECMWF IFS 0.25°, CCSDS (5.42) | 1,038,240 | 33.1 | 69.3 | 2.1× |
| HRRR 3 km Lambert, complex+spd (5.3) | 1,905,141 | 35.9 | 57.4 | 1.6× |
| RAP 13 km Lambert, JPEG 2000 (5.40) | 151,987 | 45.0 | 147.4 | 3.3× |

Both columns move about 10% run to run on an unquiesced machine, so read the
ratios as 1.5–3.5× rather than to two figures. That is where the literature puts
numeric wasm. JPEG 2000 is the outlier at both ends — the slowest per point
natively and the worst ratio — so it is the one decode a browser host should
expect to feel, and the reason reduced-resolution 5.40 decode
([#463](https://github.com/D0ubleD0uble/fieldglass/issues/463)) matters more in
a browser than it does natively. `+simd128` moves every row by less than that
spread.

One correction to the scale #462 assumed. It put the HRRR field at 193 ms
native, and it is not: 36 ms through `Session::decode`, and 37 ms through the
napi addon's `decodeGrid` on a cold cache, on this machine. Where the older
figure came from is not known — a different machine, a debug build, or a
different message — so the useful part is that the browser cost of a
1.9-million-point complex-packed field is tens of milliseconds, not a fifth of a
second.

Reproduce:

```sh
cargo run --release -p fieldglass --example bench_decode > native.json
crates/fieldglass-wasm/build.sh nodejs
node crates/fieldglass-wasm/tests/node/bench.mjs --native native.json
```

The corpus is the committed real-producer GRIB2 fixtures, not `samples/`:
`samples/` is git-ignored, so a CI benchmark over it would either skip — and a
benchmark that measures nothing is worse than none — or fetch a live model run
and measure a different file every day. Every field is checked against its
eccodes oracle before it is timed, because the failure mode of a bad `wasm-opt`
rewrite is a fast wrong answer, which a timing harness alone reports as an
improvement.

## Checking it

**`examples/smoke/`** — the browser check. Serve the repository root over HTTP
and open `/crates/fieldglass-wasm/examples/smoke/` after `./build.sh web`. It
decodes a committed fixture in a Worker, colours it on the CPU through
`render()` and on the GPU through the exported shader, and compares a WebGL
`readPixels` of the second against the first. The acceptance rule is one
lookup-table entry at a bin edge and nothing else.

**`tests/node/fetchplan.mjs`** — the planner, against the full entry built for
Node (`build.sh nodejs --features=fetchplan,zarr`, read from
`pkg/nodejs-fetchplan-zarr`). It plans one GFS
field from a committed `.idx`, checks the `Range` header and the envelope of the
committed bytes that range fetched, checks with `verifyMessage` that they are the
field the line named (and that a neighbouring line's GFS and ECMWF records are
refused), and replays every answer the Rust planner recorded in `crates/fieldglass/tests/fetch_plan_wire.json`.

**`tests/node/zarr.mjs`** — Zarr stores, against the same full-entry build. It
opens the committed v2 (blosc) and v3 (zstd) window stores, north-to-south and
south-to-north (whose v3 edition never wrote one latitude chunk), from their
metadata alone, fetches the
coordinate chunks and then the chunks each plan names from the committed files,
and decodes each window from those alone. The rows, columns, chunks and values
are checked against `crates/fieldglass-zarr/tests/fixtures/windows_oracle.json`,
which the fixture script computes from the coordinates and reads back through
xarray, and nothing unplanned may be fetched. It also checks that a planned
chunk left out of the map throws, naming it, and that a coordinate whose fill
value places nothing, or that the store leaves out, is named in the refusal.
Over a sharded store (index at the end through blosc, and at the start through
zstd), it fetches each shard's index by range, then the inner chunks the plan
names, and decodes each window from those ranges alone; the ranges are checked
against ones the fixture script read from the shard files with Python's
`struct`, and the values against xarray's.

**`tests/node/colour_range.mjs`** — the colour-range rule both hosts follow
(#1155), over the committed GFS fixture. An empty field paints transparent; one
manual end takes the other from the data; an inverted, zero-width or non-finite
range is refused with `e.code` and `e.rangeRefusal`; and the automatic range and
contour levels are the whole field's whatever a warp shows.

**`tests/node/parity.mjs`** — the cross-target check. `node
crates/fieldglass-wasm/tests/node/parity.mjs` decodes the real sample files with
both this build and the native Node addon and compares every present value.
Local only: it needs `samples/` (git-ignored — `tools/fetch_samples.sh`) and a
built addon, and it **fails loudly** when either is missing rather than
skipping, because a parity check that quietly passes because it compared nothing
is worse than no parity check.

## Not here yet

Threads (the façade is single-threaded by design — ADR-0005 — and shared memory
needs COOP/COEP) and the reduced-resolution JPEG 2000 decode `fieldglass-grib2` offers
([#463](https://github.com/D0ubleD0uble/fieldglass/issues/463)).

`+simd128` is measured above and not enabled: it buys nothing here.

## Licence

MIT OR Apache-2.0.
