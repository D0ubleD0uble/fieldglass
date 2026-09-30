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
  const at = new Uint32Array(variable.dims.length);
  const field = handle.decodeSlice(variable.index, 0, 1, at, {});
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
shrinks: call `free()` when you are done with one, or use `using` and let
`Symbol.dispose` do it.

## What it opens

| Format | Notes |
|---|---|
| GRIB2 | Every registered §5 packing template, and every grid family the project projects. |
| GRIB1 | Including second-order packing and spherical-harmonic fields. |
| NetCDF | Classic (CDF-1/2/5) and NetCDF-4 over HDF5, with the CF conventions applied. |

## Types

`fieldglass_wasm.d.ts` ships with the package. `wasm-bindgen` generates the class
and method signatures, so they cannot drift from the module. The values those
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
