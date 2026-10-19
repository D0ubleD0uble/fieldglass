# @fieldglass/wasm

Read GRIB1, GRIB2 and NetCDF in the browser.

Pure Rust decoders compiled to WebAssembly: no C library, no emscripten
runtime, no system dependency. One module, and a `.wasm` file beside it.

```sh
npm i @fieldglass/wasm
```

```js
import init, { open } from '@fieldglass/wasm';

await init();

const handle = open(new Uint8Array(await file.arrayBuffer()));

// Ask how the container is addressed before anything else. GRIB is a stream of
// messages; NetCDF holds named variables over shared dimensions.
if (handle.addressing() === 'messages') {
  const field = handle.decode(0, {});
  console.log(field.ni(), field.nj(), field.parameter(), field.units());
  field.free();
} else {
  const [variable] = handle.variables();
  // One entry per dimension, in declared order; the two horizontal ones are
  // ignored rather than absent.
  const sliceIndices = new Array(variable.dims.length).fill(0);
  const field = handle.decodeSlice({ variable: variable.index, yDim: 0, xDim: 1, sliceIndices }, {});
  console.log(field.parameter(), field.stats());
  field.free();
}

handle.free();
```

Decoding a large field blocks whoever calls it, so run this in a **Web Worker**
rather than on the main thread.

Everything after a decode — `warp`, `palette`, `render`, `probe`, `contours`,
`combine` — takes the field the two paths above both return, so the addressing
split stops at the decode and nothing downstream has two cases.

Fields and handles own memory the module allocated, and linear memory never
shrinks: call `close()` (or `free()`) when you are done with one, or use `using`
and let `Symbol.dispose` do it.

## Two entries

| Import | What it has |
|---|---|
| `@fieldglass/wasm` | Everything above: open, decode, place, warp, paint, probe, contour and combine GRIB1, GRIB2 and NetCDF. |
| `@fieldglass/wasm/full` | All of that, plus a fetch planner for GRIB sidecars and Zarr stores. About 29% larger to download. |

Use the default unless a page reads parts of remote files. The full entry plans
which byte ranges of a GRIB file to fetch from its `.idx` or `.index` sidecar,
and reads a lat/lon window of a Zarr store from just the chunks it touches.
Nothing in either entry does I/O: the page fetches, and passes the bytes in.

```js
import init, { parseWgrib2Idx, httpRangeHeader, verifyEnvelope, open } from '@fieldglass/wasm/full';

await init();

const sidecar = await fetch(`${url}.idx`);
if (!sidecar.ok) throw new Error(`HTTP ${sidecar.status}`);
const [item] = parseWgrib2Idx(key, await sidecar.text())
  .select({ abbreviation: 'TMP', levelText: '2 m above ground' });

const header = httpRangeHeader(item.range);
const response = await fetch(url, header ? { headers: { Range: header } } : {});
// Check the status first: a 200 to a ranged request means the server sent the
// whole file, and an error page is not a message.
if (response.status !== (header ? 206 : 200)) throw new Error(`HTTP ${response.status}`);
const bytes = new Uint8Array(await response.arrayBuffer());
verifyEnvelope(item, bytes);
const handle = open(bytes);
```

For a Zarr store, fetch each object the store names and pass a missing chunk
as `null`: a 404 for a chunk means the store never wrote it, which reads as its
fill value. Any other failed fetch is an error. Each entry has its own `.wasm`,
at `@fieldglass/wasm/fieldglass_wasm_bg.wasm` and
`@fieldglass/wasm/full/fieldglass_wasm_bg.wasm`.

## What it opens

| Format | Notes |
|---|---|
| GRIB2 | Every registered §5 packing template, and every grid family the project projects. |
| GRIB1 | Including second-order packing and spherical-harmonic fields. |
| NetCDF | Classic (CDF-1/2/5) and NetCDF-4 over HDF5, with the CF conventions applied. |

## Types

`fieldglass_wasm.d.ts` ships with the package, and `full/fieldglass_wasm.d.ts`
with the full entry; each declares only what its entry has. `wasm-bindgen`
generates the class and method signatures, so they cannot drift from the module. The values those
methods return and take (`MessageInfo`, `Georef`, `VariableInfo`, the option
objects and the rest) are declared in the same file, generated from the
project's JSON schema of its Rust types.

Every object the package returns carries all of its keys. A field with nothing
to report is `null`, never `undefined` or missing, and is declared as
`T | null`. `probe` returns `null` for a point off the grid.

## Licence

MIT or Apache-2.0, at your option. `NOTICE` lists every third-party crate
linked into the module and the terms it is published under.

Source, issues and the full documentation:
<https://github.com/D0ubleD0uble/fieldglass>
