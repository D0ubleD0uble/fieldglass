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
```

`pkg/` is a build product and is gitignored. The script checks that the
`wasm-bindgen` CLI matches the `wasm-bindgen` crate version the build resolved,
because a mismatch surfaces as an opaque "invalid schema version" at import
time rather than as a build failure. Install the matching one with
`cargo install wasm-bindgen-cli --version <the version it names>`.

Not wasm-pack, and not napi-rs's wasm target: napi-rs only ships loaders for
`wasm32-wasip1-threads`, which needs COOP/COEP cross-origin isolation and about
half a megabyte of emnapi/WASI shims.

## Use

```js
import init, { open, glslSnippet } from './pkg/web/fieldglass_wasm.js';
await init();

const handle = open(new Uint8Array(await response.arrayBuffer()));
handle.count();                    // messages in the file
handle.message(0);                 // one message's metadata, built on demand

const field = handle.decode(0, {});           // { dtype?: 'auto' | 'f32' | 'f64' }
field.values();                    // Float32Array or Float64Array — see `dtype()`
field.mask();                      // Uint8Array, 1 present / 0 absent
field.grid();                      // kind, boundsLonlat, proj4, x0/y0/dx/dy, scan

const palette = handle.palette(field, {});    // { lut, t0, t1, span, scale, maskedRgba }
handle.shaderValues(field, {});    // Float32Array: transformed and rebased by t0
handle.shaderMask(field, {});      // Uint8Array the shader tests

handle.warp(field, {});            // resampled values, no paint
handle.render(field, {}, false);   // RGBA, the CPU fallback
handle.probe(field, lat, lon);
handle.contours(field, new Float64Array([280, 290]));

field.free();
handle.free();
```

### The memory contract

Linear memory never shrinks and an animation holds many fields at once, so
**the façade keeps no decode cache**. `decode()` hands a field to JS and JS owns
it; `warp`, `render`, `probe`, `contours`, and the shader accessors take it back
by reference. Call `free()` on a field and on the handle when you are done —
a dropped JS reference does not release the wasm allocation until the host's
`FinalizationRegistry` runs, if it runs at all.

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
`invalid_option`. Branch on `code`; the `message` is prose and may be reworded.

## Values first, pixels second

`render()` is a CPU fallback. The intended path is `palette()` plus
`glslSnippet()`: colour is decided once, in Rust, and exported as a 256-entry
lookup table, so restyling never re-decodes and the CPU painter stays the oracle
the shader is checked against rather than a second colour implementation. See
[`../fieldglass/README.md`](../fieldglass/README.md) for why the field is
rebased in Rust before it reaches the shader.

## Checking it

**`examples/smoke/`** — the browser check. Serve the repository root over HTTP
and open `/crates/fieldglass-wasm/examples/smoke/` after `./build.sh web`. It
decodes a committed fixture in a Worker, colours it on the CPU through
`render()` and on the GPU through the exported shader, and compares a WebGL
`readPixels` of the second against the first. The acceptance rule is one
lookup-table entry at a bin edge and nothing else.

**`tests/node/parity.mjs`** — the cross-target check. `node
crates/fieldglass-wasm/tests/node/parity.mjs` decodes the real sample files with
both this build and the native Node addon and compares every present value.
Local only: it needs `samples/` (git-ignored — `tools/fetch_samples.sh`) and a
built addon, and it **fails loudly** when either is missing rather than
skipping, because a parity check that quietly passes because it compared nothing
is worse than no parity check.

## Not here yet

Threads and SIMD ([#462](https://github.com/D0ubleD0uble/fieldglass/issues/462)),
npm publishing ([#466](https://github.com/D0ubleD0uble/fieldglass/issues/466)),
NetCDF, Zarr, caller-sized output
([#465](https://github.com/D0ubleD0uble/fieldglass/issues/465)), and
reduced-resolution decode
([#463](https://github.com/D0ubleD0uble/fieldglass/issues/463)).

## Licence

MIT OR Apache-2.0.
