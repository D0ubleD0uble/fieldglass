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

./pack.sh           # the npm package, from a web build → pkg/npm + a .tgz
```

`pack.sh` assembles what `release.yml` publishes as
[`@fieldglass/wasm`](https://www.npmjs.com/package/@fieldglass/wasm) on a `v*`
tag (#466). The version comes from the workspace `Cargo.toml`, never from an
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
field.grid();                      // kind, boundsLonlat, proj4, x0/y0/dx/dy, scan

const palette = handle.palette(field, {});    // { lut, t0, t1, span, scale, maskedRgba }
handle.shaderValues(field, {});    // Float32Array: transformed and rebased by t0
handle.shaderMask(field, {});      // Uint8Array the shader tests

// A window at a pixel size — what a map view asks for (#465). `width` and
// `height` go together; one alone throws `invalid_option`. Without them the
// output is the source `ni × nj`, as before.
handle.warp(field, { bounds: [24, 50, -125, -66], width: 512, height: 512 });
handle.render(field, {}, false);   // RGBA, north up — see below
handle.probe(field, lat, lon);
handle.contours(field, new Float64Array([280, 290]));

// Difference maps and their siblings. `combineOps()` is the picker's list:
// [{ value: 'a_minus_b', label: 'A − B' }, …]. Both fields must sit on the
// same grid; a mismatch throws with `code: 'unsupported'` naming what differs.
const other = handle.decode(1, {});
const diff = handle.combine(field, other, 'a_minus_b');

diff.free();
other.free();
field.free();
handle.free();
```

### Which way up a raster comes out

`render(field, options, flipY)` composes `flipY` with the message's own scan
order rather than replacing it. Grid point `(i, j)` paints at pixel `(i, j)`, so
a field whose rows run south to north (`grid().scan.jPositive`) arrives upside
down on a canvas whose first row is the top — and `false` therefore means
**north up**, not "rows as stored". Pass the user's own request straight
through; composing the flag yourself would flip twice.

`grid().scan` is still on the DTO, because it is the one thing the geometry
cannot answer — and a **GPU host needs it**: `shaderValues()` and
`shaderMask()` are in the field's own data order, so a host uploading them owes
the same composition `render()` makes for you. `examples/smoke` does exactly
that where it lines the two pictures up.

### The memory contract

Linear memory never shrinks and an animation holds many fields at once, so
**the façade keeps no decode cache**. `decode()` hands a field to JS and JS owns
it; `warp`, `render`, `probe`, `contours`, `combine`, and the shader accessors take it back
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

<!-- checked by tools/check_wasm_bundle_size.py -->

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| baseline | 1,510,802 | 572,451 |
| `+simd128` | 1,495,307 | 567,065 |

The table **is** the gate: `python3 tools/check_wasm_bundle_size.py` fails when a
build drifts more than 5% from these figures in either direction, so a change
that moves the bundle has to say so here. Update both cells when it does.

The figures are from one x86-64 Linux machine; CI measures the same tree about
0.1% smaller, which is nowhere near the tolerance. It is the drift between
*builds* the gate is for, not between machines.

Two things worth knowing before optimising further:

- `wasm-opt -Oz` is a **raw** win and a **transfer** loss. It takes the module
  from 1,342,887 to 1,262,479 bytes (-6.0%) and takes it from 489,627 to 501,678
  gzipped (+2.5%). Its size passes trade repetition for smaller encodings, and
  DEFLATE was already being paid for the repetition. It stays on because parse
  and instantiate cost track the raw module, but a transfer-size-only argument
  for `-Oz` does not survive measurement. The trade held its shape when NetCDF
  was added (#662): it was -5.2% / +1.9% on the GRIB-only bundle.
- `+simd128` buys 7,197 raw bytes and nothing measurable in time (below). The
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
and reduced-resolution decode
([#463](https://github.com/D0ubleD0uble/fieldglass/issues/463)).

`+simd128` is measured above and not enabled: it buys nothing here.

## Licence

MIT OR Apache-2.0.
