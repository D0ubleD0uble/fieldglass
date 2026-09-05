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
```

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

## Measured

Both numbers are CI gates on every pull request, so what follows is what a check
printed rather than an estimate (#462). Machine-dependent, so read the ratio and
the shape, not the absolute milliseconds.

### Bundle

`build.sh web` — wasm-bindgen, then `wasm-opt -Oz` — gzipped, which is what a
browser actually downloads.

<!-- checked by tools/check_wasm_bundle_size.py -->

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| baseline | 933,960 | 361,746 |
| `+simd128` | 931,428 | 360,916 |

The table **is** the gate: `python3 tools/check_wasm_bundle_size.py` fails when a
build drifts more than 5% from these figures in either direction, so a change
that moves the bundle has to say so here. Update both cells when it does.

Two things worth knowing before optimising further:

- `wasm-opt -Oz` is a **raw** win and a **transfer** loss. It takes the module
  from 985,301 to 933,960 bytes (-5.2%) and takes it from 355,711 to 361,746
  gzipped (+1.7%). Its size passes trade repetition for smaller encodings, and
  DEFLATE was already being paid for the repetition. It stays on because parse
  and instantiate cost track the raw module, but a transfer-size-only argument
  for `-Oz` does not survive measurement.
- `+simd128` buys 2,532 raw bytes and nothing measurable in time (below). The
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

**`tests/node/parity.mjs`** — the cross-target check. `node
crates/fieldglass-wasm/tests/node/parity.mjs` decodes the real sample files with
both this build and the native Node addon and compares every present value.
Local only: it needs `samples/` (git-ignored — `tools/fetch_samples.sh`) and a
built addon, and it **fails loudly** when either is missing rather than
skipping, because a parity check that quietly passes because it compared nothing
is worse than no parity check.

## Not here yet

Threads (the façade is single-threaded by design — ADR-0005 — and shared memory
needs COOP/COEP), npm publishing
([#466](https://github.com/D0ubleD0uble/fieldglass/issues/466)), NetCDF, Zarr,
caller-sized output
([#465](https://github.com/D0ubleD0uble/fieldglass/issues/465)), and
reduced-resolution decode
([#463](https://github.com/D0ubleD0uble/fieldglass/issues/463)).

`+simd128` is measured above and not enabled: it buys nothing here.

## Licence

MIT OR Apache-2.0.
