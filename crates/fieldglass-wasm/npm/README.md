# @fieldglass/wasm

`@fieldglass/wasm` lets a web page open weather and climate data and draw it as
a map. Your page hands it a file's bytes, and it hands back the numbers inside,
ready to color, contour, compare or read at the point a user clicks.

Weather files mostly come in three formats: GRIB1, the more modern GRIB2, and
NetCDF, which is more common than GRIB for research and climate models. Very
large datasets kept in the cloud are often saved as Zarr stores instead. This
package reads all four, right in the browser. Zarr comes in the larger of the
package's two builds, `@fieldglass/wasm/full` (see [Two entries](#two-entries)).

Fieldglass is written in Rust and compiled to WebAssembly, a compact format
that browsers run a few times slower than native code. Nothing else comes along
with it: no C library, no emscripten runtime, and nothing to install on the
system.

```sh
npm i @fieldglass/wasm
```

## A first look

The thing you'll work with most is a field: one grid of numbers, such as the
temperature at every point on a map. The module decodes fields synchronously,
which means each call holds the thread it runs on until it's done. The page's
main thread is the one that draws the page and answers clicks, so a decode
there would freeze the page. The module belongs in a Web Worker, a background
thread the page starts for heavy work like this.

That makes the shortest working setup two files. The page fetches the file
and passes its bytes to the Worker:

```js
// main.js: fetch the file and hand its bytes to the Worker.
const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
worker.onmessage = ({ data }) => {
  if (data.error) console.error(data.error);
  else console.log(data);
};

const response = await fetch(url);
if (!response.ok) throw new Error(`HTTP ${response.status}`);
const bytes = await response.arrayBuffer();
// Listing `bytes` a second time hands the buffer over instead of copying it.
worker.postMessage(bytes, [bytes]);
```

The Worker loads the module once, decodes a field from the bytes it receives,
and posts what it found back to the page:

```js
// worker.js: everything that touches the module runs here.
import init, { open } from '@fieldglass/wasm';

const ready = init();

self.onmessage = async ({ data }) => {
  try {
    await ready;
    const handle = open(new Uint8Array(data));

    // Ask how the container is addressed before anything else. GRIB is a stream of
    // messages; NetCDF holds named variables over shared dimensions.
    let summary;
    if (handle.addressing() === 'messages') {
      const field = handle.decode(0, {});
      summary = { ni: field.ni(), nj: field.nj(), parameter: field.parameter(), units: field.units() };
      field.free();
    } else {
      const [variable] = handle.variables();
      // One entry per dimension, in declared order; the two horizontal ones are
      // ignored rather than absent. `defaultYDim` and `defaultXDim` are the plane
      // the file's own axes name, or its last two dimensions.
      const sliceIndices = new Array(variable.dims.length).fill(0);
      const field = handle.decodeSlice(
        { variable: variable.index, yDim: variable.defaultYDim, xDim: variable.defaultXDim, sliceIndices },
        {},
      );
      summary = { parameter: field.parameter(), stats: field.stats() };
      field.free();
    }

    handle.free();
    self.postMessage(summary);
  } catch (e) {
    // Every error carries a stable `code`; see Errors below.
    self.postMessage({ error: `${e.code ? `[${e.code}] ` : ''}${e.message}` });
  }
};
```

The examples further down show only the code that runs inside `worker.js`.

The Worker also shows the one place where the formats differ, which is how you
ask for a field. A GRIB file is a list of fields, called messages, so you ask
for one by its position in the list. A NetCDF file or a Zarr store holds named
variables instead. A variable can have extra axes, such as time or height, so
you ask for one flat slice of it, like the first time step. Either way, you
get back the same kind of field. Everything you do next (`warp`, `palette`,
`render`, `probe`, `contours`, `combine`) works the same way whichever kind of
file the field came from.

### Is there an asynchronous path?

Today every call is synchronous: it returns its answer directly, with no
promises to wait on. I plan to add an async layer in the future. The plan is
for every synchronous call to keep its name and behavior, and async versions
such as `decodeAsync`, `decodeSliceAsync` and `warpAsync` will sit beside them.
Each will take the same arguments plus `{ signal }` and resolve to what the
synchronous call returns. `load(source, { signal })` will sit beside
`open(bytes)` as an optional way for the module to do the fetching itself,
though passing bytes in stays the main path. The `signal` follows the web
standard: an already-aborted signal rejects at once, and an abort rejects with
`signal.reason`. Async won't make decoding faster in the browser, and it won't
move the work off the thread that calls it, because the module is built to run
on one thread and the async layer won't start any of its own. What it adds is a
way to cancel between steps, and a `load()` to fetch a file for you. So keep
the module in a Worker after async arrives, too.

## What it opens

| Format | What it reads |
|---|---|
| GRIB2 | Every way of packing the numbers listed in the [GRIB2 packing modes table](https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes). The [fieldglass-grib2 page](https://github.com/D0ubleD0uble/fieldglass/tree/master/crates/fieldglass-grib2#what-it-reads) lists the grids it can place on the Earth. |
| GRIB1 | The packings in real use, including second-order packing, and spherical-harmonic fields. The [fieldglass-grib1 page](https://github.com/D0ubleD0uble/fieldglass/tree/master/crates/fieldglass-grib1#what-it-reads) lists the grids it can place on the Earth. |
| NetCDF | The [classic format](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) (CDF-1, 2 and 5) and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec), which is built on [HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html). It applies the [CF conventions](https://cfconventions.org/cf-conventions/cf-conventions.html), the shared rules for things like units, scaling and missing values. |
| Zarr | Stores in [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html), with or without a consolidated index at the top, including blosc compression and v3 [sharding](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html). Only in `@fieldglass/wasm/full`. |

It doesn't fetch anything itself: your page downloads the file and passes the
bytes in. And it runs on one thread, the one that called it.

## Two entries

The package comes in two sizes, and a page loads one of them:

| Import | What it has |
|---|---|
| `@fieldglass/wasm` | Everything for GRIB1, GRIB2 and NetCDF: open, decode, place on the Earth, warp, paint, probe, contour and combine. |
| `@fieldglass/wasm/full` | All of that, plus Zarr stores and a fetch planner for GRIB sidecars (see [Reading part of a remote GRIB file](#reading-part-of-a-remote-grib-file)). About 35% larger to download. |

Use the default unless your page opens a Zarr store or reads only part of a
remote file. Each entry has its own `.wasm` file, which you can point a bundler
or a Worker's `init` at:

- `@fieldglass/wasm/fieldglass_wasm_bg.wasm`
- `@fieldglass/wasm/full/fieldglass_wasm_bg.wasm`

Each entry also has its own `.d.ts` type file that declares only what that
entry has. If you import `openStore` from the default entry by mistake,
TypeScript tells you straight away, instead of you finding an `undefined` when
the page runs.

## Drawing a field

Once you have a field, you can look at its values, color them, cut out the
window a map view shows, read the value at a point, draw contour lines, or
compare it with another field:

```js
import init, { open, glslSnippet, combineOps } from '@fieldglass/wasm';
await init();

const handle = open(new Uint8Array(await response.arrayBuffer()));
handle.count();                              // messages in the file
handle.message(0);                           // one message's metadata, built on demand

const field = handle.decode(0, {});          // { dtype?: 'auto' | 'f32' | 'f64' }
field.values();                              // Float32Array or Float64Array; see `dtype()`
field.mask();                                // Uint8Array, 1 present / 0 absent
field.georef();                              // kind, boundsLonlat, placement, proj4, affine, scan

// The GPU path: a 256-entry color table, and the values and mask to upload.
const palette = handle.palette(field, {});   // { lut, t0, t1, span, scale, maskedRgba, range }
handle.shaderValues(field, {});              // Float32Array: transformed and rebased by t0
handle.shaderMask(field, {});                // Uint8Array the shader tests
glslSnippet();                               // the GLSL to paste into your shader

// The CPU path: RGBA pixels, north up (see "Which way up" below).
const { rgba, width, height, range } = handle.render(field, {}, false);

// A window at a pixel size, which is what a map view asks for. `width` and
// `height` go together; one alone throws `invalid_option`. Without them the
// output is the grid's own `ni × nj`.
handle.warp(field, { bounds: [24, 50, -125, -66], width: 512, height: 512 });

handle.probe(field, lat, lon);               // the value at a point, or null off the grid
handle.contours(field, new Float64Array([280, 290]), {});
// No levels: round ones over the range `render` uses for the same options.
handle.contours(field, new Float64Array([]), { min: 270, max: 300 });

// Difference maps and the other ways to combine two fields. `combineOps()` is
// the list for a picker: [{ value: 'a_minus_b', label: 'A − B' }, …]. Both
// fields must sit on the same grid; a mismatch throws with
// `code: 'unsupported'` naming what differs.
const other = handle.decode(1, {});
const diff = handle.combine(field, other, 'a_minus_b');

diff.free();
other.free();
field.free();
handle.free();
```

When `render` colors a field, it stretches the colors across a range of values,
and the `range` it returns tells you what that range was: the two ends, where
each end came from, the middle, and how many cells fell below or above it.
`palette.range` gives you the same thing for the graphics-card path, so you can
draw a color key beside it.

### Values first, pixels second

There are two ways to turn a field into colors. `render()` colors it on the CPU
and hands back finished pixels. It's there as a fallback. The intended way is
`palette()` plus `glslSnippet()`, which lets the graphics card do the coloring.

Fieldglass works out a list of 256 colors once, and each value maps to this
color lookup table. Changing the color scheme reapplies only the lookup, so
you don't decode the values again.

### A zoomed-out view of a JPEG 2000 field

Some GRIB2 fields are compressed with [JPEG 2000](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml), the same family of
compression used for some images. It's the slowest kind to decode, which
matters most in a browser, but it can also be decoded at a coarser level. That
costs much less and is all a zoomed-out map needs.
`reduction: 1` gives you half the size on each axis.

```js
// `handle` is still open, and `field` is a full-resolution decode of message 0.
const coarse = handle.decodeDisplay(0, { reduction: 1 });
coarse.georef();                             // the derived grid: every second point
handle.renderDisplay(coarse, field.stats(), {}, false);
handle.warpDisplay(coarse, { bounds: [24, 50, -125, -66], width: 512, height: 512 });
handle.paletteDisplay(coarse, field.stats(), {});
coarse.free();
```

The coarse values are smoothed approximations, fine for a picture but not real
data, so they come back as their own type, a `DisplayFieldHandle`. Only the
three `*Display` calls take it, and TypeScript won't let you hand it to
`probe`, `contours` or `combine`, which need the real values.

The colors come from the full field's stats, so the coarse view is colored the
same way the full one would be. You can pass `null` for the stats only when you
set both `min` and `max` yourself. Some messages have no coarse level, and
asking for one throws `unsupported`. When that happens, ask again with
`reduction: 0`.

### Which way up a raster comes out

Files don't all store their rows in the same order. Some go from north to
south, others from south to north, and `georef().scan.jPositive` tells you
which. `render` paints grid point `(i, j)` at pixel `(i, j)`, and a canvas's
first row is its top. So a field stored south to north would arrive upside
down if nothing fixed it.

`render` fixes it for you. In `render(field, options, flipY)`, your `flipY` is
combined with the file's own order rather than replacing it. `false` means
north up. It doesn't mean "rows in the order the file stored them." Pass your
user's choice straight through. If you also work out the flip yourself, the
picture gets flipped twice.

Anything you draw on top of the picture by grid index needs half a pixel
added. `render` paints grid point `(i, j)` as the square from `(i, j)` to
`(i + 1, j + 1)`, so the point itself sits in the middle of its pixel.
`contours()`, for example, gives its points as grid indices
(`Isoline.segments`, as `[i0, j0, i1, j1]`), so draw a point at
`x = (i + 0.5) * sx` and `y = (j + 0.5) * sy`, where `sx` and `sy` are canvas
pixels per raster pixel. Without the half pixel, a line sits up and to
the left of the cells it separates.

Two more cases change that formula. First, if `render` flipped the rows (that
is, `georef().scan.jPositive` differs from the `flipY` you passed), then on a
raster `nj` rows tall, row `j` goes at `y = (nj - j - 0.5) * sy`. `x` never
flips. Second, on a global grid that runs west to east, `i` can reach `ni`, one
past the last column. That point is column `i - ni` again, back at the left
edge, so draw that segment a second time at `i - ni` to carry the line on
across the edge. `probe()` reports `i` and `j` the same way.

A page that colors on the graphics card needs `georef().scan` too.
`shaderValues()` and `shaderMask()` come in the field's own stored order, so
your page has to make the same flip that `render()` makes for you.

## Reading part of a remote GRIB file

This needs `@fieldglass/wasm/full`.

A GRIB file from a forecast model can be hundreds of megabytes, holding
hundreds of fields, when you only want one of them. Most publishers put a small
text file, a sidecar index, next to each GRIB file. It lists each field and
where in the big file it starts, as an `.idx` file from wgrib2 or an `.index`
file from ECMWF. The planner reads the sidecar and tells you which bytes to
fetch for the field you want, so your page doesn't need a sidecar parser of its
own. You then ask the server for only those bytes, with an [HTTP Range request](https://www.rfc-editor.org/rfc/rfc9110.html#range.requests).

The example below also picks which model run to read. Forecast models run a few
times a day, and each run takes a while to be posted, so the example tries the
newest run that should be out by now and works back from there.

```js
import init, {
  open, parseWgrib2Idx, httpRangeHeader, verifyEnvelope, verifyMessage, candidates,
} from '@fieldglass/wasm/full';
await init();

// The runs that should have posted by now, newest first. It's an estimate, so
// take the first whose sidecar exists.
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
// 'bytes=…', or null for the whole object. Throws `invalid_option` for an exact
// range of no bytes or past 2^64; the planner never makes one.
const header = httpRangeHeader(item.range);
const response = await fetch(`${bucket}/${key}`, header ? { headers: { Range: header } } : {});
// 206 for a ranged request, 200 for the whole object. A 200 to a ranged
// request is a server that ignored `Range` and sent the whole file, and an
// error page is not a message. Either one passed on reads as a stale sidecar
// or a short read, and sends you after the wrong fix.
if (response.status !== (header ? 206 : 200)) throw new Error(`${key}: HTTP ${response.status}`);
const bytes = new Uint8Array(await response.arrayBuffer());

verifyEnvelope(item, bytes);                 // throws short_read (retry) or decode (stale sidecar)
const handle = open(bytes);
verifyMessage(item, handle.message(0));      // throws decode if it is another field
idx.free();
```

`parseEcmwfIndex` reads an ECMWF `.index` file the same way. Its parameters
are ECMWF's own short names, called MARS names, such as `2t` for the
temperature two meters above the ground. No table here turns those into WMO
codes, so select them by `abbreviation`.

## Reading a window of a Zarr store

This needs `@fieldglass/wasm/full`.

A Zarr store is a folder, a bucket or a set of web addresses holding many small
objects. Some are metadata, describing the arrays, and the rest are the arrays
themselves, cut into small pieces called chunks. So a page can fetch only the
pieces a view needs, and the store tells it which ones. Reading a window takes
three rounds of fetching: first the metadata, then the coordinates (so it knows
where on the Earth each row and column is), and then only the chunks your
window touches.

```js
import init, { openStore } from '@fieldglass/wasm/full';
await init();

// An object's bytes, or `null` for one the store doesn't hold: a sparse array
// leaves chunks unwritten, and a 404 for one is its fill value, not an error.
// Pass `null` rather than leaving the key out: a key not passed is one you
// haven't fetched, and a read that needs it throws.
const get = async (key) => {
  const response = await fetch(`${store}/${key}`);
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`${key}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
};

// 1. The metadata: `zarr.json` for v3, `.zmetadata` for a consolidated v2 store.
// If the store's root is itself an array, pass its name as a second argument,
// `openStore(objects, 'air')`: it has no name inside the store.
const objects = new Map([['zarr.json', await get('zarr.json')]]);
let ds = openStore(objects);
const t2m = ds.variables().find((v) => v.name === 't2m');

// 2. The coordinate arrays a box is matched against.
for (const { key } of ds.coordinateChunks(t2m.index)) objects.set(key, await get(key));
ds.free();
ds = openStore(objects);

// 3. The chunks a box and a time step touch, and only those.
const request = {
  variable: t2m.index, yDim: 1, xDim: 2,
  bounds: [20, 55, -130, -60],               // [latMin, latMax, lonMin, lonMax]
  sliceIndices: [0, 0, 0],                   // the time step; the two map axes are ignored
};
const plan = ds.planWindow(request);         // { rows, columns, ni, nj, chunks, shardIndexes, innerChunks }
for (const { key } of plan.chunks) objects.set(key, await get(key));
ds.free();
ds = openStore(objects);

const field = ds.decodeWindow(request);      // a field like any other: palette, warp, probe
```

A cell counts as inside the box when its center is, and cells right on the
edge count too. A box that crosses the 180° line runs past 180
(`[.., 170, 190]`) rather than wrapping around to -180. On a global grid, that
box comes back as two runs of columns, decoded together as one window. A
window is at least two cells each way, because a single row or column has no
spacing to place it by. Windows work on slices placed on a regular latitude and
longitude grid by one-dimensional coordinates. Anything else throws
`unsupported`.

### A missing object versus one you haven't fetched

When the server answers 404, pass that key as `null`. `null` tells the module
the store holds no such object, and the module reads it as the array's fill
value, its "no data" value. That's how a sparse store works. zarr-python, for
instance, leaves out any chunk that is entirely fill.

A key you don't pass at all, or pass as `undefined`, means something else: one
you haven't fetched yet. The module never reads it as fill. `planWindow` and
`decodeWindow` throw `invalid_option` naming each such key, and any other read
that needs one (`decodeSlice`, `decodeLine`) throws `decode` naming it. So pass
every key a plan names, using `null` for the ones that came back 404.

An unconsolidated [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) store, one with no single index file at the top, also
needs each array's and group's `.zattrs` file, or `null` for a 404, because
that's where the dimension names and the [CF scale and fill values](https://cfconventions.org/cf-conventions/cf-conventions.html#packed-data) live.

### Reading only part of a shard

[Zarr v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) can bundle many chunks into one bigger object called a [shard](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html). A small
index inside the shard says where each chunk sits. In a sharded store, each key
in `plan.chunks` is a whole shard, which may cover many time steps or a large
area. Fetching each shard whole works fine. To fetch only what the window
shows, read each shard's index first, then only the chunks inside it that the
window needs, both with [Range requests](https://www.rfc-editor.org/rfc/rfc9110.html#range.requests). Give them to `openStore` as
`{ size, ranges }` under the shard's key:

```js
// Each shard index is the shard's last `length` bytes, or its first. The store
// also needs each shard's whole size. A page can read that from Content-Range
// only where the bucket exposes the header, so this asks a HEAD for
// Content-Length, which a page can always read, alongside the range.
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
  // No size, no ranges: take this shard whole instead.
  if (size === null) { objects.set(index.key, await get(index.key)); continue; }
  const offset = index.location === 'end' ? size - index.length : 0;
  objects.set(index.key, { size, ranges: [{ offset, bytes }] });
}
ds.free();
ds = openStore(objects);

// Now that the indexes are in: each inner chunk the window touches, as an
// `offset` and `length` within its shard. Chunks the index marks as never
// written are left out; they are the fill value.
const { innerChunks } = ds.planWindow(request);
if (innerChunks === null) {
  // The plan couldn't go by range (an index that fails its checksum, say).
  for (const { key } of plan.chunks) objects.set(key, await get(key));
} else {
  for (const inner of innerChunks) {
    if (inner.length === 0) continue;        // zero bytes can't be named by a range
    const bytes = await range(inner.key, `bytes=${inner.offset}-${inner.offset + inner.length - 1}`);
    objects.get(inner.key).ranges.push({ offset: inner.offset, bytes });
  }
}
ds.free();
ds = openStore(objects);
const windowField = ds.decodeWindow(request);
```

For a variable that isn't sharded, `shardIndexes` and `innerChunks` are both
`null`. `shardIndexes` is also `null` for a sharding layout this build doesn't
read, and for a shard of more than 1,048,576 inner chunks.

`innerChunks` is also `null` until every shard's index is in the map. A shard
given whole, or as `null` for a 404, counts as in. If it's still `null` once
every index is in, fetch the `chunks` whole instead.

Give each range exactly as you fetched it, because a read takes all of its
bytes from a single range. `decodeWindow` throws `invalid_option` naming each
index or inner-chunk range it's missing, and `decode` for an index that fails
its checksum or points outside its shard.

## Memory

Fields and handles hold memory inside the module, and WebAssembly memory never
shrinks once it has grown. So the module keeps no cache of its own: you decide
which fields to keep, say for an animation, and let go of the rest. When
`decode()` hands you a field, you own it. `warp`, `render`, `probe`,
`contours`, `combine` and the shader calls only borrow it.

When you're done with a field or handle, call `close()` on it (`free()` does
the same thing), or create it with `using`, which calls `Symbol.dispose` for
you at the end of the block. Dropping your last reference isn't enough. The
memory stays taken until the browser's `FinalizationRegistry` gets around to
it, if it ever does. Calling `close()` twice is harmless, and any other call on
a closed object throws.

Every call that returns a typed array gives you a copy of the data rather than
a view into the module's memory, because a view would go stale the moment that
memory grew. If your page wants to reuse a buffer, copy into one of your own
once and keep that.

## Errors

Every failure throws a JavaScript `Error` with a `code` property that won't
change between versions:

| `code` | Meaning |
|---|---|
| `unsupported_format` | The bytes aren't in a format this module reads. |
| `decode` | The file opened, but part of it didn't parse. |
| `no_such_message` | You asked for a message or variable past the end of the file. |
| `unsupported` | The call doesn't apply to this field or file, such as warping a grid with no geometry or probing a spectral field, or the read is too large to make at once. |
| `wrong_addressing` | A message call on a file of variables, or a variable call on a file of messages. |
| `invalid_option` | An option is out of range or contradicts another one. |
| `short_read` | A fetch returned fewer bytes than asked for. Retry it. |
| `closed` | A call on an object you already closed. |

Check `code` in your code. The `message` is written for people to read and may
be reworded. `short_read` only ever comes from the planner's `verifyEnvelope`.

If the module itself hits a bug, it stops rather than carry on in an unknown
state, and that ends the Worker it was running in. I fuzz test the decoders
with damaged and hostile files to make that rare. Treat the Worker as
disposable: if it dies, start another.

## Types

The type files ship with the package: `fieldglass_wasm.d.ts` for the default
entry and `full/fieldglass_wasm.d.ts` for the full one. The class and method
signatures are generated by `wasm-bindgen` from the module itself, so they
can't drift out of step with it. The values those methods take and return
(`MessageInfo`, `Georef`, `VariableInfo`, the option objects and the rest) are
declared in the same file. Most are generated from a JSON schema of the Rust
types, and a few are written by hand.

Every object the package returns carries all of its keys. A key with nothing
to report holds `null`, never `undefined`, and the types declare it as
`T | null`. `probe` returns `null` for a point off the
grid.

## License

MIT or Apache-2.0, at your option. `NOTICE` lists every third-party crate built
into the module and the terms it's published under.

Source, issues, and how the module is built and measured:
<https://github.com/D0ubleD0uble/fieldglass/tree/master/crates/fieldglass-wasm>
