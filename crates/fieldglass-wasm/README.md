# fieldglass-wasm

This is the browser version of Fieldglass. It takes the same Rust code that
reads weather files for the VS Code extension and compiles it to WebAssembly,
a compact format browsers run at close to native speed. It's built with
wasm-bindgen for the `wasm32-unknown-unknown` target. Every call is
synchronous and it runs on a single thread, so it's meant to live inside a Web
Worker, a background thread a page starts so heavy work doesn't freeze it. It
is published to npm as
[`@fieldglass/wasm`](https://www.npmjs.com/package/@fieldglass/wasm).

If you're using it from a web page, the package's own
[README](npm/README.md) is the guide. It covers what it opens, the two entries, drawing a field, reading
part of a remote GRIB file, reading a window of a Zarr store, memory and
errors. This page is about building the module, checking it, and measuring it.

This crate is only a thin layer over [`fieldglass`](../fieldglass). It hands
typed arrays across to JavaScript, turns Rust errors into JavaScript ones,
passes each method through, and supplies the small piece of GLSL shader code a
page pastes in to color a field on the graphics card. Every decision about
what a field is gets made in `fieldglass`, which is why the browser and the VS
Code extension behave the same.

## Build

```sh
./build.sh web      # ES module for a browser  → pkg/web
./build.sh nodejs   # CommonJS for Node        → pkg/nodejs
./build.sh web --simd    # the +simd128 variant → pkg/web-simd
./build.sh web --no-opt  # skip wasm-opt
./build.sh web --features=fetchplan,zarr   # the full entry → pkg/web-fetchplan-zarr

./pack.sh           # the npm package, both entries → pkg/npm + a .tgz
```
`pack.sh` puts together what `release.yml` publishes as `@fieldglass/wasm`
when a `v*` tag is pushed. It's made from two web builds: the default entry,
and the full one, which goes in the package's `full/` directory. The version
number comes from the workspace `Cargo.toml`, never from an argument, so the
npm package can't drift out of step with the crates and the extension. The
committed `npm/package.json` carries a `0.0.0` placeholder, and `pack.sh`
refuses to ship a package with that number.

`pkg/` is where builds land. It's a build product, so git ignores it. You need
two tools installed:

- `wasm-bindgen`, at the same version as the `wasm-bindgen` crate the build
  resolved. The script checks this for you, because a mismatch doesn't fail
  the build. Instead it shows up later as a confusing "invalid schema version"
  error when the page imports the module. The script names the version it
  wants, and you can install it with
  `cargo install wasm-bindgen-cli --version <the version it names>`.
- `wasm-opt`, from [binaryen](https://github.com/WebAssembly/binaryen/releases).
  It shrinks the module with `-Oz`, and that step is part of what ships, so
  the sizes under Measured are the shrunk ones. `--no-opt` builds without it,
  but the result is then not the module that ships.

The script runs `wasm-opt` with the target's own list of WebAssembly features,
read from `rustc --print cfg`. `wasm-opt`'s own defaults are narrower than the
target's, so a plain `wasm-opt -Oz` rejects every `memory.copy` instruction
rustc writes.

We don't use wasm-pack, or napi-rs's WebAssembly target. napi-rs only ships
loaders for `wasm32-wasip1-threads`, which needs the page to be cross-origin
isolated (served with the COOP and COEP headers) and brings about half a
megabyte of emnapi and WASI support code with it.

## Two entries

The package ships the module built two ways, and a page loads one of them:

| Import | Built with | What it has |
|---|---|---|
| `@fieldglass/wasm` | the default features | GRIB1, GRIB2 and NetCDF: open, decode, place, warp, paint, probe, contour, combine |
| `@fieldglass/wasm/full` | `--features=fetchplan,zarr` | all of that, plus the fetch planner and Zarr stores |

Each entry has its own `.wasm` file and its own `.d.ts` type file, which
declares only what that entry has. The two could later become one bundle,
with `full` kept as another name for it, without breaking an import of
either.

## One shape with the Node binding

The repository also has a Node binding, `fieldglass-napi`, which the VS Code
extension uses. It wraps the same `Session` in the same shape. `open`,
`format`, `addressing`, `count`, `message`, `variables`, `leftOut`,
`undrawable`, `decode`, `decodeSlice`, `decodeLine`, `decodeDisplay` and
`close` take the same arguments and mean the same things in both, and
`tools/check_js_api.py` compares the two generated type files to keep it that
way.

`decodeSlice` and `decodeLine` each take one object:
`{ variable, yDim, xDim, sliceIndices }` and
`{ variable, alongDim, sliceIndices }`. Here `variable` is a position in
`variables()`, and `sliceIndices` can be a `Uint32Array` or a plain object
keyed `"0"`, `"1"` and so on, which is the form the Node binding receives a
typed array in. Both refuse a key they don't know. Everything is synchronous
today. The names `load(source, { signal })` and `<name>Async` are kept free
for an asynchronous layer, which will add to these names rather than rename
them.

## Checking it

We check the module several ways, from a real browser down to the installed
npm package.

`examples/smoke/` is the browser check. Run `./build.sh web`, serve the
repository root over HTTP, and open `/crates/fieldglass-wasm/examples/smoke/`.
The page decodes a committed file in a Worker and colors it twice: on the CPU
through `render()`, and on the graphics card through the exported shader. It
then reads the graphics card's pixels back with WebGL's `readPixels` and
compares the two. It passes when they differ by at most one color-table entry
where a value sits right on the edge between two entries.

`tests/node/package.mjs` installs the packed `.tgz` into a throwaway project
and decodes through what got installed. The other Node tests load
`pkg/nodejs/fieldglass_wasm.js` straight from its path, so on their own they
would still pass even if the `package.json` shipped none of it. Both run on
every pull request.

`tests/node/fetchplan.mjs` checks the fetch planner against the full entry
built for Node (`build.sh nodejs --features=fetchplan,zarr`, read from
`pkg/nodejs-fetchplan-zarr`). It plans one GFS field from a committed `.idx`
sidecar and checks the `Range` header it makes. It checks the envelope of the
bytes that range fetched, and uses `verifyMessage` to check they really are
the field the sidecar line named, and that a neighboring line's GFS and ECMWF
records are refused. Then it replays every answer the Rust planner recorded
in `crates/fieldglass/tests/fetch_plan_wire.json`.

`tests/node/zarr.mjs` checks Zarr stores against the same build. It opens the
committed v2 (blosc) and v3 (zstd) window stores, with their rows in both
orders, starting from their metadata alone. It fetches the coordinate chunks,
then the chunks each plan names, and decodes each window from only those. The
rows, columns, chunks and values are checked against
`crates/fieldglass-zarr/tests/fixtures/windows_oracle.json`, which the fixture
script works out from the coordinates and reads back through xarray, and the
test fails if anything outside the plan gets fetched. It also checks that a
missing planned chunk throws and names it, and that a coordinate that can't
place the grid is named in the refusal.

The same test covers sharded stores, with the shard index at the end of the
shard (through blosc) and at the start (through zstd). It fetches each
shard's index by range, then the chunks inside it the plan names, and decodes
each window from those ranges alone. The ranges are checked against ones the
fixture script read with Python's `struct`, and the values against xarray's.

`tests/node/color_range.mjs` checks the rule both hosts follow for picking
the color range, over the committed GFS file. An empty field paints
transparent. Setting one end by hand takes the other end from the data. A
range that is upside down, zero wide or not a finite number is refused with
`e.code` and `e.rangeRefusal`. And the automatic range and contour levels
always come from the whole field, whatever part a warp shows.

`tests/node/parity.mjs` decodes the real sample files with both this build and
the native Node addon, and compares every value that's present. It only runs
locally, because it needs `samples/` (which git ignores, and
`tools/fetch_samples.sh` downloads) and a built addon. If either is missing it
fails loudly rather than skipping, because a check that passes by comparing
nothing is worse than no check at all.

## Measured

A check prints both sets of numbers below on every pull request, but only the
bundle size is a gate. CI's `Bundle-size gate` step
(`tools/check_wasm_bundle_size.py`) fails when a build drifts more than 5% from
the table in either direction. The decode timings are printed and never
compared, because a timing gate on a shared CI machine would just be noise.
Both depend on the machine, so read the ratios and the overall shape, not the
exact numbers.

### Bundle

These sizes are of `build.sh web` (wasm-bindgen, then `wasm-opt -Oz`),
gzipped, which is what a browser actually downloads.

<!-- checked by tools/check_wasm_bundle_size.py -->

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| baseline | 1,686,964 | 662,056 |
| `+simd128` | 1,673,231 | 659,047 |
| full | 2,285,313 | 895,421 |

When to re-record: CI's `wasm` job prints each build's measured size, and
how far it has drifted from these figures, in its run summary. A pull request
re-records the table when that drift reads above 1.0% for either build. The
release prep pull request re-records it every time, so the figures match CI
at each release and a run of small increases shows up in one diff
(RELEASING.md §1). Either way, copy the measured `.wasm` and gzipped columns
from the summary into the table, and say in the pull request what moved.
Don't record from a local build: its toolchain differs from CI's, and a local
build has come out about 1.1% larger in raw bytes. CI uses the current stable
rustc, so a new Rust release on its own can push the drift over 1.0%. The next
pull request then re-records it and says so.

Here's what makes up those figures:

- All three default decoders. NetCDF is the expensive one. When it joined the
  browser build it added about 30% raw and 33% gzipped, because it brings a
  whole container format with it: the HDF5 object model and its filter
  pipeline, and the half of the API that reads variables. The umbrella
  crate's per-format features already make a GRIB-only module buildable for a
  page that wants to leave NetCDF out.
- Tables, not only code. Every ECMWF GRIB1 local parameter table (2,255
  entries of names and units) took about 157 KB raw and 42 KB gzipped, 7.9%.
  The WMO and common code tables that name a message's center, sub-center,
  discipline and data type took about 33 KB raw and 15 KB gzipped, 2.9%. The
  format crates don't yet have a feature to leave them out.
- The `full` row adds the fetch planner and Zarr stores, about 35% above the
  baseline, raw and gzipped.

When we chose a second entry over putting everything in the default, we
measured the full entry's cost locally (rustc 1.98, wasm-bindgen 0.2.129,
binaryen 132, `build.sh web`, gzip level 9 as the gate measures it):

| Build (local) | `.wasm` bytes | gzipped bytes | gzipped vs default |
|---|---:|---:|---:|
| default | 1,627,492 | 632,104 | |
| `--features=zarr` | 1,859,338 | 738,550 | +106,446 (+16.8%) |
| `--features=fetchplan` | 1,920,568 | 749,553 | +117,449 (+18.6%) |
| `--features=fetchplan,zarr` | 2,092,532 | 817,175 | +185,071 (+29.3%) |
| a Zarr-only module (`zarr`, `render`, `analysis`; no GRIB or NetCDF) | 639,242 | 280,935 | a page loads it as well as, or instead of, the default |

Zarr adds the store reader, its metadata parser and `serde_json`, blosc with
LZ4 and BloscLZ, and sharding. zstd and DEFLATE are already in the default
build for NetCDF. Next to `fetchplan`, which already brings `serde_json`, it
adds 67,622 gzipped bytes rather than 106,446.

Two things worth knowing before trying to make it smaller:

- `wasm-opt -Oz` makes the raw file smaller but the download bigger. Measured
  before NetCDF joined, it took the module from 1,342,887 to 1,262,479 bytes
  (−6.0%), but from 489,627 to 501,678 gzipped (+2.5%). Its size passes swap
  repeated code for more compact encodings, and gzip was already squeezing
  out that repetition. We keep it on because the time a browser spends
  parsing and starting the module follows the raw size, but the download size
  alone doesn't argue for `-Oz`. The trade looked the same once NetCDF was
  added, at −5.2% raw and +1.9% gzipped.
- `+simd128` (WebAssembly's instructions for doing the same math on several
  numbers at once) saves about 14 KB raw on the table above and buys nothing
  measurable in speed (see below). The decoders spend their time unpacking
  bits in loops whose branches depend on the data, not on the
  one-number-per-lane arithmetic the compiler can speed up that way. And the
  standard library isn't rebuilt with it unless you use `-Zbuild-std`.

### Decode

`tests/node/bench.mjs` times each decode, taking the middle of five runs after
a warm-up, against the native release build of the same code going through
the same `Session::decode`. On one x86-64 Linux machine, with Node 22:

| Field | Points | native ms | wasm ms | wasm / native |
|---|---:|---:|---:|---:|
| ECMWF IFS 0.25°, CCSDS (5.42) | 1,038,240 | 33.1 | 69.3 | 2.1× |
| HRRR 3 km Lambert, complex+spd (5.3) | 1,905,141 | 35.9 | 57.4 | 1.6× |
| RAP 13 km Lambert, JPEG 2000 (5.40) | 151,987 | 45.0 | 147.4 | 3.3× |

Both columns move by about 10% from run to run on a busy machine, so read the
ratios as "1.5 to 3.5 times slower" rather than to two figures. That's in line
with published measurements of number-crunching WebAssembly. JPEG 2000 stands
out at both ends: it's the slowest per point natively and has the worst
ratio. So it's the one decode a browser user should expect to notice, and it's
why the reduced-resolution JPEG 2000 decode matters more in a browser than
natively. `+simd128` moves every row by less than that run-to-run spread.

To reproduce:

```sh
cargo run --release -p fieldglass --example bench_decode > native.json
crates/fieldglass-wasm/build.sh nodejs
node crates/fieldglass-wasm/tests/node/bench.mjs --native native.json
```
The files are the GRIB2 files from real forecast models that are committed
with the tests, not `samples/`. Git ignores `samples/`, so a CI benchmark over
it would either skip and measure nothing, or download a live model run and
measure a different file every day. Every field is checked against its
eccodes reference before it's timed. A bad `wasm-opt` rewrite fails as a fast
wrong answer, and a timing harness on its own would report that as an
improvement.

## Not here yet

Threads. The module is single-threaded by design, and sharing memory between
threads needs the page to be cross-origin isolated (the COOP and COEP
headers).

`+simd128` is measured above and isn't turned on, because it buys nothing
here.

## License

MIT OR Apache-2.0.
