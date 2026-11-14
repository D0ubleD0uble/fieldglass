# Performance and memory

*Bootstrapped 2026-09-14 ([#743](https://github.com/D0ubleD0uble/fieldglass/issues/743)).*

This is the cost half of [verification](verification.md). The correctness half
proves the numbers are right; this half measures what it costs to produce them.
The gate holds each number to the value recorded in this file, so a number that
moves either way by more than its tolerance fails until the table is
re-recorded. Bytes, requests, allocations and peak heap have no tolerance at
all. Each cost is also compared with a **stated bound**, such as "one chunk per
frame", and the gap is reported, not gated: a scenario over its bound is a
finding with its own issue. Comparing a number only with its last recording lets
it drift slowly forever; the bound is the fixed point that drift is measured
against.

## Running it

```sh
crates/fieldglass-perf/run.sh            # every gated tier, checked against this file
crates/fieldglass-perf/run.sh --write    # the same, then re-record the tables below
crates/fieldglass-perf/run.sh --report   # also the timed and real-data report
crates/fieldglass-perf/run.sh --clean    # empty the real-data cache and the outputs
```

It needs, and fails without:

| Tier | Needs |
|---|---|
| all | Python with `crates/fieldglass-perf/requirements.txt` installed at exactly those versions |
| all | The Rust release pinned in `crates/fieldglass-perf/rust-toolchain.toml` |
| instructions | Valgrind (Linux) and `cargo install gungraun-runner --version 0.19.4 --locked` |
| wasm | Node.js, the `wasm32-unknown-unknown` target for the pinned release, and what `crates/fieldglass-wasm/build.sh` needs (the matching `wasm-bindgen` CLI and binaryen) |
| report, real data | network access once, for `tools/fetch_perf_data.py` |

A missing prerequisite stops the run and names what to install. It never skips a
tier: a gate that passes because Valgrind was absent has measured nothing.

## Where it lives

`crates/fieldglass-perf` is a nested workspace with its own `Cargo.lock`, like
the `fuzz/` crates and `crates/fieldglass-verify`. Its heavy tooling — dhat,
Gungraun, the codecs linked on their own — stays out of the root workspace
graph, out of `cargo deny`, and out of every published crate: `cargo tree` for
each published crate is the same with it as without it. It is not in
`crates/fieldglass-verify`, although that crate is the other half of
verification, because that crate is pinned to Verus's toolchain for reasons that
have nothing to do with benchmarks.

## Tiers

Only a deterministic metric may gate a pull request.

| Tier | Metric | How | In CI |
|---|---|---|---|
| Gate | Bytes read and requests per operation | `fieldglass_core::testing::Recording` around the source | exact |
| Gate | Allocation count, peak heap | dhat's testing profiler around the operation alone | exact |
| Gate | Instructions, estimated cycles | Gungraun on Callgrind, cache geometry fixed | 2%, or 40,000 for small operations |
| Gate | wasm linear-memory high-water mark | `memory.buffer.byteLength` after preparing and running the operation, one Node process per scenario | one 64 KiB page |
| Report | Wall time, native and wasm (baseline and `+simd128`) | `report` binary, `wasm/measure.mjs` | printed |
| Report | Ratio to a reference tool | `report.py`: eccodes, netCDF4, h5py, zarr-python | printed |
| Report | Real data: requests, bytes and time on the remote path | the pinned ERA5 manifest | printed |
| Local | Flame graphs, long scrubs | `perf`, Callgrind's own output under `target/gungraun` | manual |

**Why these are deterministic.** dhat counts the sizes the code *requests*, not
what the system allocator rounds them to, so the heap numbers are a property of
the code and the input. The recording counts the ranges the reader asks for.
Callgrind counts instructions, not time, and its cache simulation uses a fixed
geometry rather than the host CPU's, so a laptop and a runner agree. Bytes read
and requests follow from the reader's range logic alone. Allocations and peak
heap count the standard library's own allocations too, and instruction counts
and wasm memory depend on the generated code, so all of these can move with the
compiler, which is why the harness pins one Rust release (see
[Re-recording](#re-recording)). Instruction counts also wander a little from
run to run, hence their tolerance, and wasm memory is allowed one 64 KiB page.

Each scenario is **prepared** first and **measured** second: the input is read,
the session opened and anything the operation takes as given (a render starts
from a decoded field) is done before the profiler starts, and dhat's figures are
read the moment the operation returns, before the harness keeps its output
alive. A heap or I/O row costs exactly the operation it names. Two tiers are
looser, and say so: an instruction count also includes the one box the harness
wraps the output in (a constant, the same at every size), and a wasm memory
figure is the high-water mark of preparation *and* operation, because linear
memory cannot be read for one without the other. A render-side row whose cost
fits inside what preparing its decoded field already grew will read the same as
the decode.

## The corpus

**Generated, never committed.** `crates/fieldglass-perf/corpus/generate.py`
writes every gated input into a temporary directory at the start of a run, and
the run deletes it at the end. The values are closed-form and the writers are
pinned, so the bytes are the same every time; the digest below says which bytes
the table describes.

A bound about scaling needs two points, so every input exists at `S` and `L`,
except [5.200](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-200.shtml), which has one size. Array inputs also exist at `D`:

| Size | Grid | Time steps | Used for |
|---|---|---|---|
| `S` | 2° (180 × 91) | 8 | the baseline |
| `L` | 1° (360 × 181), four times the cells | 8 | cost against cell count |
| `D` | 2° (180 × 91) | 32, four times the variable | cost against the variable, not the plane |

| Format | Inputs |
|---|---|
| GRIB1 | simple; second-order; spectral simple and spectral complex (`S` = T63, `L` = T127) |
| GRIB2 [§5](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_sect5.shtml) | [5.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-0.shtml), [5.3](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-3.shtml), [5.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml), [5.41](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-41.shtml), [5.42](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) at `S` and `L`; [5.200](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-200.shtml) at one size |
| NetCDF | [classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec); [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) with zlib and shuffle, chunked one step per chunk, and four steps per chunk (`netcdf4-zlib-span`); NetCDF-4 with szip (`netcdf4-szip`, written with h5py: nearest-neighbor, 16 pixels per block, one step per chunk) |
| Zarr | [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) with blosc (lz4, shuffle); [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) with zstd; v3 [sharded](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html), four steps per shard |

| Operation | On |
|---|---|
| open, decode, place | every message input |
| open, variables, slice, scrub (eight frames), place | every array input |
| warp, palette, render, contours | `grib2-5.0` and `zarr-v3-zstd`, at `S` and `L` |
| codec alone | [5.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml) (JPEG 2000), [5.41](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-41.shtml) (PNG), [5.42](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) (AEC), [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) (zlib and szip), [Zarr v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) (zstd), [Zarr v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) (blosc) |

What is deliberately not covered, and why:

- **5.200 has one size.** eccodes decodes run-length packing but cannot encode
  it, so the committed fixture stands in and has no second size to scale against.
- **Zarr has no wasm rows.** The wasm host opens bytes, not stores.
- **Spectral inputs do not scale in cells.** Every spectral field is synthesized
  onto the same 0.5° grid, so `S` and `L` differ in coefficients only.

## Bounds

Every bound is a formula over facts the *writer* recorded (eccodes keys, HDF5's
own chunk records, the layout the [classic format](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) fixes), never over what the
reader under test reports.

- **Bytes read.** A message's decode needs the message. Its open and place need
  everything but the data section's payload, plus one 64-byte look-ahead window
  per section: the scanner reads through a `FileCursor` whose first window is
  wider than a length field on purpose, trading a few bytes for half the round
  trips. An array's open, variables and place need everything but the variable's
  planes; a slice needs everything but the other planes, and a scrub everything
  but the planes past its frames. For a sharded store the unit is the shard,
  because an object source fetches whole objects.
- **Allocations** do not grow with cell count: the same operation at `S` and `L`
  allocates the same number of times.
- **Peak heap** is at most `cells × B + C`. `B` is measured as the slope between
  `S` and `L`, so the fixed overhead cancels, and printed beside its floor: the
  least any implementation could hold per cell for that operation.
- **Work** is proportional to the cells an operation touches, not to the size of
  the variable: a slice at `D` costs what it costs at `S`.
- **Codec ceiling.** Each decompressor runs alone over the bytes the decode
  hands it, so the share of a decode spent outside its codec is visible.

## Gated measurements

Corpus digest: `9d502b493b90cea14fe5d7edfa32911f540be1ac35c3cf72fba543d2254d630c`

A pull request that moves any number here by more than its tolerance fails the
`perf` job until the table is re-recorded with
`crates/fieldglass-perf/run.sh --write`, and the pull request says why the
number moved. Both directions: an improvement nobody records is
lost to the next regression.

<!-- perf-gate:table:begin — regenerate with tools/check_perf_gate.py --write -->
| Scenario | Cells | Value bytes | Bytes read | Bound | Requests | Allocations | Peak heap | Instructions | Est. cycles | wasm memory | wasm +simd128 memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `grib1-second-order-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,294 | 27,198 | 1,703,936 | 1,703,936 |
| `grib1-second-order-L/decode` | 65,160 | 4 | 26,524 | 26,620 | 1 | 20 | 2,106,981 | 12,025,694 | 17,536,371 | 3,801,088 | 3,801,088 |
| `grib1-second-order-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,116 | 30,707 | — | — |
| `grib1-second-order-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,263 | 27,153 | 1,703,936 | 1,703,936 |
| `grib1-second-order-S/decode` | 16,380 | 4 | 10,454 | 10,550 | 1 | 20 | 532,188 | 3,227,144 | 4,567,662 | 2,359,296 | 2,359,296 |
| `grib1-second-order-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,060 | 30,635 | — | — |
| `grib1-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,294 | 27,166 | 1,900,544 | 1,900,544 |
| `grib1-simple-L/decode` | 65,160 | 4 | 130,332 | 130,428 | 1 | 15 | 1,889,666 | 12,549,432 | 17,556,442 | 3,735,552 | 3,735,552 |
| `grib1-simple-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,999 | 30,556 | — | — |
| `grib1-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,233 | 27,055 | 1,769,472 | 1,769,472 |
| `grib1-simple-S/decode` | 16,380 | 4 | 32,772 | 32,868 | 1 | 15 | 475,046 | 3,171,547 | 4,383,868 | 2,228,224 | 2,228,224 |
| `grib1-simple-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,999 | 30,544 | — | — |
| `grib1-spectral-complex-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,091 | 26,626 | 1,769,472 | 1,769,472 |
| `grib1-spectral-complex-L/decode` | 259,920 | 8 | 33,966 | 34,062 | 1 | 24 | 6,498,082 | 253,234,898 | 362,198,011 | 10,616,832 | 10,616,832 |
| `grib1-spectral-complex-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,835 | 28,873 | — | — |
| `grib1-spectral-complex-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,091 | 26,630 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-S/decode` | 259,920 | 8 | 9,262 | 9,358 | 1 | 24 | 6,498,082 | 122,230,392 | 180,243,331 | 9,175,040 | 9,175,040 |
| `grib1-spectral-complex-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,823 | 28,873 | — | — |
| `grib1-spectral-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,091 | 26,642 | 1,769,472 | 1,769,472 |
| `grib1-spectral-simple-L/decode` | 259,920 | 8 | 33,038 | 33,134 | 1 | 23 | 6,498,082 | 252,987,431 | 361,797,418 | 10,616,832 | 10,616,832 |
| `grib1-spectral-simple-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,644 | 28,617 | — | — |
| `grib1-spectral-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,091 | 26,634 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8 | 8,334 | 8,430 | 1 | 23 | 6,498,082 | 122,131,135 | 180,099,213 | 9,175,040 | 9,175,040 |
| `grib1-spectral-simple-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,739 | 28,757 | — | — |
| `grib2-5.0-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 16,582 | 33,314 | 1,966,080 | 1,966,080 |
| `grib2-5.0-L/decode` | 65,160 | 4 | 130,331 | 130,499 | 1 | 16 | 1,889,658 | 12,550,139 | 17,558,843 | 3,801,088 | 3,801,088 |
| `grib2-5.0-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,211 | 31,079 | — | — |
| `grib2-5.0-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,667,194 | 38,711,172 | 3,801,088 | 3,801,088 |
| `grib2-5.0-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 1,378,022 | 1,559,552 | 3,801,088 | 3,801,088 |
| `grib2-5.0-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,252,404 | 8,368,023 | 3,801,088 | 3,801,088 |
| `grib2-5.0-L/contours` | 65,160 | — | 0 | 0 | 0 | 54 | 1,165,776 | 15,247,459 | 18,930,046 | 3,801,088 | 3,801,088 |
| `grib2-5.0-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 16,582 | 33,314 | 1,769,472 | 1,769,472 |
| `grib2-5.0-S/decode` | 16,380 | 4 | 32,771 | 32,939 | 1 | 16 | 475,038 | 3,172,344 | 4,386,447 | 2,228,224 | 2,228,224 |
| `grib2-5.0-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,313 | 31,240 | — | — |
| `grib2-5.0-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 6,967,140 | 9,752,075 | 2,228,224 | 2,228,224 |
| `grib2-5.0-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 402,417 | 471,083 | 2,228,224 | 2,228,224 |
| `grib2-5.0-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,627,304 | 2,181,707 | 2,228,224 | 2,228,224 |
| `grib2-5.0-S/contours` | 16,380 | — | 0 | 0 | 0 | 49 | 332,048 | 3,954,535 | 4,958,079 | 2,228,224 | 2,228,224 |
| `grib2-5.200-S/open` | 0 | — | 211 | 211 | 7 | 4 | 2,090 | 17,122 | 34,537 | 1,703,936 | 1,703,936 |
| `grib2-5.200-S/decode` | 496 | 4 | 20 | 211 | 1 | 18 | 14,402 | 56,243 | 97,660 | 1,703,936 | 1,703,936 |
| `grib2-5.200-S/place` | 0 | — | 0 | 211 | 0 | 4 | 70 | 16,239 | 31,081 | — | — |
| `grib2-5.3-L/open` | 0 | — | 403 | 783 | 7 | 3 | 2,080 | 16,652 | 33,626 | 1,769,472 | 1,769,472 |
| `grib2-5.3-L/decode` | 65,160 | 4 | 57,954 | 58,150 | 1 | 18 | 1,889,658 | 14,750,747 | 20,452,348 | 3,670,016 | 3,670,016 |
| `grib2-5.3-L/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,306 | 31,220 | — | — |
| `grib2-5.3-S/open` | 0 | — | 403 | 783 | 7 | 3 | 2,080 | 16,717 | 33,787 | 1,703,936 | 1,703,936 |
| `grib2-5.3-S/decode` | 16,380 | 4 | 18,586 | 18,782 | 1 | 18 | 475,038 | 3,886,440 | 5,352,551 | 2,162,688 | 2,162,688 |
| `grib2-5.3-S/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,306 | 31,212 | — | — |
| `grib2-5.40-L/open` | 0 | — | 403 | 757 | 7 | 3 | 2,080 | 16,659 | 33,460 | 1,703,936 | 1,703,936 |
| `grib2-5.40-L/decode` | 65,160 | 4 | 14,681 | 14,851 | 1 | 567 | 1,889,658 | 114,167,687 | 146,509,271 | 3,932,160 | 3,932,160 |
| `grib2-5.40-L/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,306 | 31,200 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | — | 551 | 859,764 | 108,656,249 | 137,604,427 | — | — |
| `grib2-5.40-S/open` | 0 | — | 403 | 757 | 7 | 3 | 2,080 | 16,659 | 33,464 | 1,703,936 | 1,703,936 |
| `grib2-5.40-S/decode` | 16,380 | 4 | 6,278 | 6,448 | 1 | 429 | 475,038 | 34,384,956 | 44,236,699 | 2,228,224 | 2,228,224 |
| `grib2-5.40-S/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,306 | 31,216 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | — | 413 | 221,460 | 32,986,397 | 42,051,273 | — | — |
| `grib2-5.41-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 16,657 | 33,488 | 1,900,544 | 1,900,544 |
| `grib2-5.41-L/decode` | 65,160 | 4 | 80,490 | 80,658 | 1 | 25 | 1,898,632 | 11,427,049 | 16,668,050 | 3,801,088 | 3,801,088 |
| `grib2-5.41-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,328 | 31,256 | — | — |
| `grib2-5.41-L/codec` | 65,160 | — | — | — | — | 9 | 334,744 | 5,506,930 | 7,348,448 | — | — |
| `grib2-5.41-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 16,657 | 33,472 | 1,703,936 | 1,703,936 |
| `grib2-5.41-S/decode` | 16,380 | 4 | 23,501 | 23,669 | 1 | 25 | 511,568 | 3,065,459 | 4,456,094 | 2,359,296 | 2,359,296 |
| `grib2-5.41-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,334 | 31,252 | — | — |
| `grib2-5.41-S/codec` | 16,380 | — | — | — | — | 9 | 118,400 | 1,557,488 | 2,120,538 | — | — |
| `grib2-5.42-L/open` | 0 | — | 403 | 759 | 7 | 3 | 2,080 | 16,663 | 33,443 | 1,900,544 | 1,900,544 |
| `grib2-5.42-L/decode` | 65,160 | 4 | 80,767 | 80,939 | 1 | 16 | 1,889,658 | 9,754,556 | 13,779,425 | 3,735,552 | 3,735,552 |
| `grib2-5.42-L/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,290 | 31,195 | — | — |
| `grib2-5.42-L/codec` | 65,160 | — | — | — | — | 1 | 130,320 | 5,088,565 | 6,132,688 | — | — |
| `grib2-5.42-S/open` | 0 | — | 403 | 759 | 7 | 3 | 2,080 | 16,660 | 33,522 | 1,703,936 | 1,703,936 |
| `grib2-5.42-S/decode` | 16,380 | 4 | 22,449 | 22,621 | 1 | 16 | 475,038 | 2,476,532 | 3,446,986 | 2,162,688 | 2,162,688 |
| `grib2-5.42-S/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,290 | 31,215 | — | — |
| `grib2-5.42-S/codec` | 16,380 | — | — | — | — | 1 | 32,760 | 1,295,551 | 1,565,052 | — | — |
| `netcdf-classic-D/open` | 0 | — | 448 | 1,704 | 4 | 103 | 5,078 | 43,680 | 79,206 | 5,963,776 | 5,963,776 |
| `netcdf-classic-D/variables` | 0 | — | 1,084 | 1,704 | 2 | 109 | 5,900 | 70,931 | 102,360 | 5,963,776 | 5,963,776 |
| `netcdf-classic-D/slice` | 16,380 | 4 | 66,604 | 67,224 | 3 | 104 | 476,012 | 1,746,514 | 2,612,127 | 5,963,776 | 5,963,776 |
| `netcdf-classic-D/scrub` | 16,380 | 4 | 525,244 | 525,864 | 10 | 559 | 476,012 | 13,553,787 | 18,868,837 | 5,963,776 | 5,963,776 |
| `netcdf-classic-D/place` | 0 | — | 1,084 | 1,704 | 2 | 85 | 6,179 | 77,664 | 111,794 | — | — |
| `netcdf-classic-L/open` | 0 | — | 448 | 2,592 | 4 | 103 | 5,078 | 43,691 | 79,219 | 5,832,704 | 5,832,704 |
| `netcdf-classic-L/variables` | 0 | — | 2,164 | 2,592 | 2 | 109 | 10,220 | 91,432 | 126,770 | 5,832,704 | 5,832,704 |
| `netcdf-classic-L/slice` | 65,160 | 4 | 262,804 | 263,232 | 3 | 104 | 1,890,632 | 6,719,325 | 10,046,759 | 5,832,704 | 5,832,704 |
| `netcdf-classic-L/scrub` | 65,160 | 4 | 2,087,284 | 2,087,712 | 10 | 559 | 1,890,632 | 53,096,486 | 74,151,547 | 5,832,704 | 5,832,704 |
| `netcdf-classic-L/place` | 0 | — | 2,164 | 2,592 | 2 | 85 | 11,219 | 110,959 | 150,944 | — | — |
| `netcdf-classic-S/open` | 0 | — | 448 | 1,512 | 4 | 103 | 5,078 | 43,680 | 79,194 | 2,818,048 | 2,818,048 |
| `netcdf-classic-S/variables` | 0 | — | 1,084 | 1,512 | 2 | 109 | 5,900 | 71,188 | 102,687 | 2,818,048 | 2,818,048 |
| `netcdf-classic-S/slice` | 16,380 | 4 | 66,604 | 67,032 | 3 | 104 | 476,012 | 1,746,920 | 2,612,717 | 2,818,048 | 2,818,048 |
| `netcdf-classic-S/scrub` | 16,380 | 4 | 525,244 | 525,672 | 10 | 559 | 476,012 | 13,553,791 | 18,868,795 | 2,818,048 | 2,818,048 |
| `netcdf-classic-S/place` | 0 | — | 1,084 | 1,512 | 2 | 85 | 6,179 | 77,664 | 111,754 | — | — |
| `netcdf4-szip-D/open` | 0 | — | 3,069 | 11,804 | 46 | 313 | 13,723 | 279,321 | 431,141 | 3,997,696 | 3,997,696 |
| `netcdf4-szip-D/variables` | 0 | — | 1,084 | 11,804 | 2 | 141 | 5,900 | 133,398 | 185,505 | 3,997,696 | 3,997,696 |
| `netcdf4-szip-D/slice` | 16,380 | 4 | 38,379 | 47,107 | 9 | 203 | 544,324 | 6,589,936 | 8,869,514 | 3,997,696 | 3,997,696 |
| `netcdf4-szip-D/scrub` | 16,380 | 4 | 284,532 | 293,260 | 16 | 842 | 1,003,736 | 51,807,276 | 68,440,662 | 3,997,696 | 3,997,696 |
| `netcdf4-szip-D/place` | 0 | — | 1,084 | 11,804 | 2 | 117 | 6,179 | 139,930 | 194,249 | — | — |
| `netcdf4-szip-D/codec` | 16,380 | — | — | — | — | 1 | 65,520 | 4,247,546 | 5,217,158 | — | — |
| `netcdf4-szip-L/open` | 0 | — | 3,069 | 12,692 | 46 | 313 | 13,723 | 279,356 | 431,156 | 3,735,552 | 3,735,552 |
| `netcdf4-szip-L/variables` | 0 | — | 2,164 | 12,692 | 2 | 141 | 10,220 | 160,684 | 221,365 | 3,735,552 | 3,735,552 |
| `netcdf4-szip-L/slice` | 65,160 | 4 | 128,924 | 138,996 | 7 | 177 | 2,152,528 | 24,392,530 | 32,744,880 | 5,308,416 | 5,308,416 |
| `netcdf4-szip-L/scrub` | 65,160 | 4 | 1,011,807 | 1,021,879 | 14 | 816 | 3,977,780 | 197,196,391 | 260,871,665 | 7,405,568 | 7,405,568 |
| `netcdf4-szip-L/place` | 0 | — | 2,164 | 12,692 | 2 | 117 | 11,219 | 179,161 | 243,512 | — | — |
| `netcdf4-szip-L/codec` | 65,160 | — | — | — | — | 1 | 260,640 | 16,060,056 | 19,735,094 | — | — |
| `netcdf4-szip-S/open` | 0 | — | 3,069 | 11,612 | 46 | 313 | 13,723 | 279,336 | 431,087 | 2,293,760 | 2,293,760 |
| `netcdf4-szip-S/variables` | 0 | — | 1,084 | 11,612 | 2 | 141 | 5,900 | 133,339 | 185,512 | 2,293,760 | 2,293,760 |
| `netcdf4-szip-S/slice` | 16,380 | 4 | 36,843 | 46,915 | 7 | 177 | 542,788 | 6,561,079 | 8,829,099 | 2,752,512 | 2,752,512 |
| `netcdf4-szip-S/scrub` | 16,380 | 4 | 282,996 | 293,068 | 14 | 816 | 1,002,200 | 51,772,278 | 68,391,225 | 3,145,728 | 3,145,728 |
| `netcdf4-szip-S/place` | 0 | — | 1,084 | 11,612 | 2 | 117 | 6,179 | 139,871 | 194,136 | — | — |
| `netcdf4-szip-S/codec` | 16,380 | — | — | — | — | 1 | 65,520 | 4,244,102 | 5,210,218 | — | — |
| `netcdf4-zlib-D/open` | 0 | — | 2,788 | 12,942 | 29 | 390 | 16,689 | 299,878 | 459,602 | 3,997,696 | 3,997,696 |
| `netcdf4-zlib-D/variables` | 0 | — | 1,084 | 12,942 | 2 | 179 | 5,900 | 162,547 | 226,996 | 3,997,696 | 3,997,696 |
| `netcdf4-zlib-D/slice` | 16,380 | 4 | 37,587 | 47,445 | 10 | 257 | 544,324 | 4,757,734 | 6,897,720 | 3,997,696 | 3,997,696 |
| `netcdf4-zlib-D/scrub` | 16,380 | 4 | 278,950 | 288,808 | 17 | 1,008 | 1,003,736 | 36,638,373 | 51,608,512 | 4,194,304 | 4,194,304 |
| `netcdf4-zlib-D/place` | 0 | — | 1,084 | 12,942 | 2 | 155 | 6,179 | 169,162 | 236,040 | — | — |
| `netcdf4-zlib-D/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,129,811 | 1,501,027 | — | — |
| `netcdf4-zlib-L/open` | 0 | — | 2,788 | 14,990 | 29 | 390 | 16,689 | 299,867 | 459,631 | 3,735,552 | 3,735,552 |
| `netcdf4-zlib-L/variables` | 0 | — | 2,164 | 14,990 | 2 | 179 | 10,220 | 198,112 | 273,604 | 3,735,552 | 3,735,552 |
| `netcdf4-zlib-L/slice` | 65,160 | 4 | 126,701 | 139,063 | 8 | 231 | 2,152,528 | 16,886,060 | 24,330,993 | 5,046,272 | 5,046,272 |
| `netcdf4-zlib-L/scrub` | 65,160 | 4 | 993,924 | 1,006,286 | 15 | 982 | 3,977,780 | 135,162,771 | 190,528,612 | 7,405,568 | 7,405,568 |
| `netcdf4-zlib-L/place` | 0 | — | 2,164 | 14,990 | 2 | 155 | 11,219 | 216,778 | 296,365 | — | — |
| `netcdf4-zlib-L/codec` | 65,160 | — | — | — | — | 3 | 506,796 | 3,805,159 | 5,294,932 | — | — |
| `netcdf4-zlib-S/open` | 0 | — | 2,788 | 12,942 | 29 | 390 | 16,689 | 299,878 | 459,618 | 2,293,760 | 2,293,760 |
| `netcdf4-zlib-S/variables` | 0 | — | 1,084 | 12,942 | 2 | 179 | 5,900 | 162,412 | 226,930 | 2,293,760 | 2,293,760 |
| `netcdf4-zlib-S/slice` | 16,380 | 4 | 36,051 | 47,445 | 8 | 231 | 542,788 | 4,728,302 | 6,856,395 | 2,555,904 | 2,555,904 |
| `netcdf4-zlib-S/scrub` | 16,380 | 4 | 277,414 | 288,808 | 15 | 982 | 1,002,200 | 36,601,564 | 51,555,251 | 3,080,192 | 3,080,192 |
| `netcdf4-zlib-S/place` | 0 | — | 1,084 | 12,942 | 2 | 155 | 6,179 | 169,162 | 236,064 | — | — |
| `netcdf4-zlib-S/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,129,775 | 1,501,014 | — | — |
| `netcdf4-zlib-span-D/open` | 0 | — | 2,788 | 12,942 | 29 | 390 | 16,689 | 299,878 | 459,658 | 3,866,624 | 3,866,624 |
| `netcdf4-zlib-span-D/variables` | 0 | — | 1,084 | 12,942 | 2 | 179 | 5,900 | 162,202 | 226,774 | 3,866,624 | 3,866,624 |
| `netcdf4-zlib-span-D/slice` | 16,380 | 4 | 135,885 | 147,279 | 8 | 231 | 739,348 | 10,380,571 | 14,714,809 | 3,866,624 | 3,866,624 |
| `netcdf4-zlib-span-D/scrub` | 16,380 | 4 | 270,555 | 281,949 | 15 | 944 | 1,001,468 | 36,031,902 | 50,337,382 | 3,866,624 | 3,866,624 |
| `netcdf4-zlib-span-D/place` | 0 | — | 1,084 | 12,942 | 2 | 155 | 6,179 | 169,167 | 236,027 | — | — |
| `netcdf4-zlib-span-L/open` | 0 | — | 2,788 | 14,990 | 29 | 390 | 16,689 | 299,867 | 459,597 | 3,735,552 | 3,735,552 |
| `netcdf4-zlib-span-L/variables` | 0 | — | 2,164 | 14,990 | 2 | 179 | 10,220 | 197,585 | 273,269 | 3,735,552 | 3,735,552 |
| `netcdf4-zlib-span-L/slice` | 65,160 | 4 | 486,788 | 499,406 | 7 | 224 | 2,934,144 | 35,866,109 | 51,439,455 | 5,832,704 | 5,832,704 |
| `netcdf4-zlib-span-L/scrub` | 65,160 | 4 | 972,454 | 985,072 | 14 | 937 | 3,976,744 | 130,996,266 | 183,971,159 | 6,881,280 | 6,881,280 |
| `netcdf4-zlib-span-L/place` | 0 | — | 2,164 | 14,990 | 2 | 155 | 11,219 | 216,363 | 295,944 | — | — |
| `netcdf4-zlib-span-S/open` | 0 | — | 2,788 | 12,942 | 29 | 390 | 16,689 | 299,955 | 459,746 | 2,293,760 | 2,293,760 |
| `netcdf4-zlib-span-S/variables` | 0 | — | 1,084 | 12,942 | 2 | 179 | 5,900 | 162,547 | 226,984 | 2,293,760 | 2,293,760 |
| `netcdf4-zlib-span-S/slice` | 16,380 | 4 | 135,629 | 147,279 | 7 | 224 | 739,044 | 10,372,506 | 14,703,425 | 2,818,048 | 2,818,048 |
| `netcdf4-zlib-span-S/scrub` | 16,380 | 4 | 270,299 | 281,949 | 14 | 937 | 1,001,164 | 36,021,986 | 50,323,019 | 3,080,192 | 3,080,192 |
| `netcdf4-zlib-span-S/place` | 0 | — | 1,084 | 12,942 | 2 | 155 | 6,179 | 169,638 | 236,643 | — | — |
| `zarr-v2-blosc-D/open` | 0 | — | 1,584 | 1,584 | 6 | 506 | 15,663 | 314,965 | 514,557 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | — | 426 | 1,584 | 2 | 171 | 9,068 | 123,947 | 183,391 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 4 | 37,996 | 39,154 | 3 | 170 | 656,211 | 4,110,835 | 6,363,019 | — | — |
| `zarr-v2-blosc-D/scrub` | 16,380 | 4 | 300,515 | 301,673 | 10 | 772 | 656,584 | 32,134,776 | 47,513,017 | — | — |
| `zarr-v2-blosc-D/place` | 0 | — | 426 | 1,584 | 2 | 121 | 9,347 | 120,876 | 177,070 | — | — |
| `zarr-v2-blosc-D/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,448,672 | 2,320,076 | — | — |
| `zarr-v2-blosc-L/open` | 0 | — | 1,900 | 1,900 | 6 | 459 | 22,863 | 339,450 | 544,283 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | — | 739 | 1,900 | 2 | 173 | 16,268 | 173,954 | 248,716 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 4 | 144,560 | 145,721 | 3 | 173 | 2,607,411 | 15,956,310 | 25,093,903 | — | — |
| `zarr-v2-blosc-L/scrub` | 65,160 | 4 | 1,141,368 | 1,142,529 | 10 | 782 | 2,607,784 | 126,273,453 | 187,171,326 | — | — |
| `zarr-v2-blosc-L/place` | 0 | — | 739 | 1,900 | 2 | 123 | 17,267 | 183,786 | 257,582 | — | — |
| `zarr-v2-blosc-L/codec` | 65,160 | — | — | — | — | 6 | 781,920 | 5,734,902 | 9,251,888 | — | — |
| `zarr-v2-blosc-S/open` | 0 | — | 1,583 | 1,583 | 6 | 457 | 15,663 | 289,849 | 479,148 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | — | 426 | 1,583 | 2 | 171 | 9,068 | 123,916 | 182,753 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 4 | 37,996 | 39,153 | 3 | 170 | 656,211 | 4,110,242 | 6,362,260 | — | — |
| `zarr-v2-blosc-S/scrub` | 16,380 | 4 | 300,515 | 301,672 | 10 | 772 | 656,584 | 32,133,385 | 47,512,488 | — | — |
| `zarr-v2-blosc-S/place` | 0 | — | 426 | 1,583 | 2 | 121 | 9,347 | 120,818 | 176,910 | — | — |
| `zarr-v2-blosc-S/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,448,672 | 2,320,068 | — | — |
| `zarr-v3-sharded-D/open` | 0 | — | 3,413 | 3,413 | 5 | 568 | 35,905 | 493,024 | 769,241 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | — | 613 | 3,413 | 2 | 210 | 17,245 | 266,702 | 381,826 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 4 | 139,799 | 142,599 | 3 | 350 | 1,967,187 | 20,112,203 | 31,772,269 | — | — |
| `zarr-v3-sharded-D/scrub` | 16,380 | 4 | 278,830 | 281,630 | 10 | 1,939 | 1,967,560 | 159,036,059 | 244,137,492 | — | — |
| `zarr-v3-sharded-D/place` | 0 | — | 613 | 3,413 | 2 | 160 | 17,524 | 262,426 | 373,730 | — | — |
| `zarr-v3-sharded-L/open` | 0 | — | 4,019 | 4,019 | 5 | 557 | 38,958 | 584,212 | 889,451 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | — | 1,215 | 4,019 | 2 | 212 | 23,034 | 362,716 | 509,159 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 4 | 526,253 | 529,057 | 3 | 446 | 7,820,787 | 81,979,801 | 129,720,087 | — | — |
| `zarr-v3-sharded-L/scrub` | 65,160 | 4 | 1,058,791 | 1,061,595 | 10 | 2,713 | 7,821,160 | 678,934,868 | 1,048,610,038 | — | — |
| `zarr-v3-sharded-L/place` | 0 | — | 1,215 | 4,019 | 2 | 162 | 24,033 | 371,783 | 516,789 | — | — |
| `zarr-v3-sharded-S/open` | 0 | — | 3,412 | 3,412 | 5 | 555 | 35,905 | 486,376 | 759,631 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | — | 613 | 3,412 | 2 | 210 | 17,245 | 265,470 | 379,705 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 4 | 139,799 | 142,598 | 3 | 350 | 1,967,187 | 20,111,026 | 31,769,386 | — | — |
| `zarr-v3-sharded-S/scrub` | 16,380 | 4 | 278,830 | 281,629 | 10 | 1,939 | 1,967,560 | 159,032,482 | 244,131,231 | — | — |
| `zarr-v3-sharded-S/place` | 0 | — | 613 | 3,412 | 2 | 160 | 17,524 | 261,472 | 372,228 | — | — |
| `zarr-v3-zstd-D/open` | 0 | — | 2,799 | 2,799 | 5 | 562 | 32,204 | 492,376 | 767,355 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | — | 613 | 2,799 | 2 | 210 | 17,245 | 266,109 | 380,820 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 4 | 52,381 | 54,567 | 3 | 222 | 656,219 | 6,266,206 | 9,509,207 | — | — |
| `zarr-v3-zstd-D/scrub` | 16,380 | 4 | 414,719 | 416,905 | 10 | 933 | 656,592 | 48,453,595 | 71,095,209 | — | — |
| `zarr-v3-zstd-D/place` | 0 | — | 613 | 2,799 | 2 | 160 | 17,524 | 261,983 | 373,175 | — | — |
| `zarr-v3-zstd-D/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,003,775 | 5,859,757 | — | — |
| `zarr-v3-zstd-L/open` | 0 | — | 3,404 | 3,404 | 5 | 514 | 35,449 | 563,495 | 859,419 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | — | 1,215 | 3,404 | 2 | 212 | 23,034 | 364,092 | 511,426 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 4 | 207,207 | 209,396 | 3 | 236 | 2,607,419 | 23,698,919 | 36,004,715 | — | — |
| `zarr-v3-zstd-L/scrub` | 65,160 | 4 | 1,645,094 | 1,647,283 | 10 | 1,004 | 2,607,792 | 190,804,147 | 279,842,742 | — | — |
| `zarr-v3-zstd-L/place` | 0 | — | 1,215 | 3,404 | 2 | 162 | 24,033 | 372,993 | 518,741 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,664,286 | 38,704,101 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 1,378,310 | 1,558,001 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,249,610 | 8,359,372 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | — | 0 | 0 | 0 | 55 | 1,182,160 | 15,243,693 | 18,923,834 | — | — |
| `zarr-v3-zstd-L/codec` | 65,160 | — | — | — | — | 43 | 769,444 | 15,835,031 | 23,248,598 | — | — |
| `zarr-v3-zstd-S/open` | 0 | — | 2,798 | 2,798 | 5 | 512 | 32,204 | 466,089 | 730,317 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | — | 613 | 2,798 | 2 | 210 | 17,245 | 266,770 | 381,769 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 4 | 52,381 | 54,566 | 3 | 222 | 656,219 | 6,265,449 | 9,508,196 | — | — |
| `zarr-v3-zstd-S/scrub` | 16,380 | 4 | 414,719 | 416,904 | 10 | 933 | 656,592 | 48,449,124 | 71,090,042 | — | — |
| `zarr-v3-zstd-S/place` | 0 | — | 613 | 2,798 | 2 | 160 | 17,524 | 262,459 | 373,735 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 6,967,548 | 9,713,218 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 402,814 | 469,729 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,627,777 | 2,148,918 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | — | 0 | 0 | 0 | 48 | 323,856 | 3,938,929 | 4,881,082 | — | — |
| `zarr-v3-zstd-S/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,003,775 | 5,859,761 | — | — |
<!-- perf-gate:table:end -->

## Bounds today

Derived from the table above by the same checker, which checks that this
section still matches the table. The bounds themselves are reported, not gated.

<!-- perf-gate:bounds:begin — regenerate with tools/check_perf_gate.py --write -->
#### Bytes read

Each row that reads more than its bound. The bound is stated beside every
scenario in the table above; these are the ones over it.

| Scenario | Bytes read | Bound | Over by |
|---|---:|---:|---:|
| none | | | |

#### Allocations against cell count

The same operation at `S` and at `L` (four times the cells). The bound is
that the count does not change.

| Operation | `S` | `L` | Verdict |
|---|---:|---:|---|
| `grib1-second-order/open` | 3 | 3 | holds |
| `grib1-second-order/decode` | 20 | 20 | holds |
| `grib1-second-order/place` | 3 | 3 | holds |
| `grib1-simple/open` | 3 | 3 | holds |
| `grib1-simple/decode` | 15 | 15 | holds |
| `grib1-simple/place` | 3 | 3 | holds |
| `grib1-spectral-complex/open` | 3 | 3 | holds |
| `grib1-spectral-complex/decode` | 24 | 24 | holds |
| `grib1-spectral-complex/place` | 4 | 4 | holds |
| `grib1-spectral-simple/open` | 3 | 3 | holds |
| `grib1-spectral-simple/decode` | 23 | 23 | holds |
| `grib1-spectral-simple/place` | 4 | 4 | holds |
| `grib2-5.0/open` | 3 | 3 | holds |
| `grib2-5.0/decode` | 16 | 16 | holds |
| `grib2-5.0/place` | 4 | 4 | holds |
| `grib2-5.0/warp` | 4 | 4 | holds |
| `grib2-5.0/palette` | 0 | 0 | holds |
| `grib2-5.0/render` | 2 | 2 | holds |
| `grib2-5.0/contours` | 49 | 54 | grows +5 |
| `grib2-5.3/open` | 3 | 3 | holds |
| `grib2-5.3/decode` | 18 | 18 | holds |
| `grib2-5.3/place` | 4 | 4 | holds |
| `grib2-5.40/open` | 3 | 3 | holds |
| `grib2-5.40/decode` | 429 | 567 | grows +138 |
| `grib2-5.40/place` | 4 | 4 | holds |
| `grib2-5.40/codec` | 413 | 551 | grows +138 |
| `grib2-5.41/open` | 3 | 3 | holds |
| `grib2-5.41/decode` | 25 | 25 | holds |
| `grib2-5.41/place` | 4 | 4 | holds |
| `grib2-5.41/codec` | 9 | 9 | holds |
| `grib2-5.42/open` | 3 | 3 | holds |
| `grib2-5.42/decode` | 16 | 16 | holds |
| `grib2-5.42/place` | 4 | 4 | holds |
| `grib2-5.42/codec` | 1 | 1 | holds |
| `netcdf-classic/open` | 103 | 103 | holds |
| `netcdf-classic/variables` | 109 | 109 | holds |
| `netcdf-classic/slice` | 104 | 104 | holds |
| `netcdf-classic/scrub` | 559 | 559 | holds |
| `netcdf-classic/place` | 85 | 85 | holds |
| `netcdf4-szip/open` | 313 | 313 | holds |
| `netcdf4-szip/variables` | 141 | 141 | holds |
| `netcdf4-szip/slice` | 177 | 177 | holds |
| `netcdf4-szip/scrub` | 816 | 816 | holds |
| `netcdf4-szip/place` | 117 | 117 | holds |
| `netcdf4-szip/codec` | 1 | 1 | holds |
| `netcdf4-zlib/open` | 390 | 390 | holds |
| `netcdf4-zlib/variables` | 179 | 179 | holds |
| `netcdf4-zlib/slice` | 231 | 231 | holds |
| `netcdf4-zlib/scrub` | 982 | 982 | holds |
| `netcdf4-zlib/place` | 155 | 155 | holds |
| `netcdf4-zlib/codec` | 2 | 3 | grows +1 |
| `netcdf4-zlib-span/open` | 390 | 390 | holds |
| `netcdf4-zlib-span/variables` | 179 | 179 | holds |
| `netcdf4-zlib-span/slice` | 224 | 224 | holds |
| `netcdf4-zlib-span/scrub` | 937 | 937 | holds |
| `netcdf4-zlib-span/place` | 155 | 155 | holds |
| `zarr-v2-blosc/open` | 457 | 459 | grows +2 |
| `zarr-v2-blosc/variables` | 171 | 173 | grows +2 |
| `zarr-v2-blosc/slice` | 170 | 173 | grows +3 |
| `zarr-v2-blosc/scrub` | 772 | 782 | grows +10 |
| `zarr-v2-blosc/place` | 121 | 123 | grows +2 |
| `zarr-v2-blosc/codec` | 5 | 6 | grows +1 |
| `zarr-v3-sharded/open` | 555 | 557 | grows +2 |
| `zarr-v3-sharded/variables` | 210 | 212 | grows +2 |
| `zarr-v3-sharded/slice` | 350 | 446 | grows +96 |
| `zarr-v3-sharded/scrub` | 1,939 | 2,713 | grows +774 |
| `zarr-v3-sharded/place` | 160 | 162 | grows +2 |
| `zarr-v3-zstd/open` | 512 | 514 | grows +2 |
| `zarr-v3-zstd/variables` | 210 | 212 | grows +2 |
| `zarr-v3-zstd/slice` | 222 | 236 | grows +14 |
| `zarr-v3-zstd/scrub` | 933 | 1,004 | grows +71 |
| `zarr-v3-zstd/place` | 160 | 162 | grows +2 |
| `zarr-v3-zstd/codec` | 29 | 43 | grows +14 |
| `zarr-v3-zstd/warp` | 4 | 4 | holds |
| `zarr-v3-zstd/palette` | 0 | 0 | holds |
| `zarr-v3-zstd/render` | 2 | 2 | holds |
| `zarr-v3-zstd/contours` | 48 | 55 | grows +7 |

#### Peak heap per cell

`B` is the slope between `S` and `L`: `(peak(L) − peak(S)) / (cells(L) − cells(S))`,
so a fixed overhead `C` cancels out. The floor is the least any implementation
could hold per cell for that operation.

| Operation | `B` today | `C` today | Floor `B` | Gap | Floor is |
|---|---:|---:|---:|---:|---|
| `grib1-second-order/decode` | 32.3 B | 3,383 B | 5 B | 6.5× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib1-simple/decode` | 29.0 B | 26 B | 5 B | 5.8× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib2-5.0/decode` | 29.0 B | 18 B | 5 B | 5.8× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib2-5.0/warp` | 13.0 B | 8 B | 5 B | 2.6× | f32 output (4) + mask (1) per output pixel |
| `grib2-5.0/render` | 12.0 B | 0 B | 4 B | 3.0× | one RGBA pixel |
| `grib2-5.0/contours` | 17.1 B | 52,088 B | 0 B | +17.1 B | marching squares needs a row of state, not a cell's |
| `grib2-5.3/decode` | 29.0 B | 18 B | 5 B | 5.8× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib2-5.40/decode` | 29.0 B | 18 B | 5 B | 5.8× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib2-5.41/decode` | 28.4 B | 45,801 B | 5 B | 5.7× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `grib2-5.42/decode` | 29.0 B | 18 B | 5 B | 5.8× | output value (4) + mask byte (1); unpacking writes straight into the output |
| `netcdf-classic/slice` | 29.0 B | 992 B | 5 B | 5.8× | output value (4) + mask (1); the plane is read in place |
| `netcdf-classic/scrub` | 29.0 B | 992 B | 5 B | 5.8× | output value (4) + mask (1); the plane is read in place |
| `netcdf4-szip/slice` | 33.0 B | 2,248 B | 9 B | 3.7× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-szip/scrub` | 61.0 B | 3,020 B | 9 B | 6.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/slice` | 33.0 B | 2,248 B | 9 B | 3.7× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 61.0 B | 3,020 B | 9 B | 6.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib-span/slice` | 45.0 B | 1,944 B | 21 B | 2.1× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
| `netcdf4-zlib-span/scrub` | 61.0 B | 1,984 B | 21 B | 2.9× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
| `zarr-v2-blosc/slice` | 40.0 B | 1,011 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/scrub` | 40.0 B | 1,384 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/slice` | 120.0 B | 1,587 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/scrub` | 120.0 B | 1,960 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/slice` | 40.0 B | 1,019 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/scrub` | 40.0 B | 1,392 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/warp` | 13.0 B | 8 B | 5 B | 2.6× | f32 output (4) + mask (1) per output pixel |
| `zarr-v3-zstd/render` | 12.0 B | 0 B | 4 B | 3.0× | one RGBA pixel |
| `zarr-v3-zstd/contours` | 17.6 B | 35,643 B | 0 B | +17.6 B | marching squares needs a row of state, not a cell's |

#### Work against the variable, not the plane

An open, a slice and a scrub at `D` (four times the variable, the same plane)
against `S`. The bound is a ratio of 1: the planes nobody asked for cost nothing.

| Operation | Peak heap `D`/`S` | Allocations `D`/`S` | Instructions `D`/`S` |
|---|---:|---:|---:|
| `netcdf-classic/open` | 1.00 | 1.00 | 1.00 |
| `netcdf-classic/slice` | 1.00 | 1.00 | 1.00 |
| `netcdf-classic/scrub` | 1.00 | 1.00 | 1.00 |
| `netcdf4-szip/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-szip/slice` | 1.00 | 1.15 | 1.00 |
| `netcdf4-szip/scrub` | 1.00 | 1.03 | 1.00 |
| `netcdf4-zlib/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib/slice` | 1.00 | 1.11 | 1.01 |
| `netcdf4-zlib/scrub` | 1.00 | 1.03 | 1.00 |
| `netcdf4-zlib-span/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib-span/slice` | 1.00 | 1.03 | 1.00 |
| `netcdf4-zlib-span/scrub` | 1.00 | 1.01 | 1.00 |
| `zarr-v2-blosc/open` | 1.00 | 1.11 | 1.09 |
| `zarr-v2-blosc/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v2-blosc/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/open` | 1.00 | 1.02 | 1.01 |
| `zarr-v3-sharded/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/open` | 1.00 | 1.10 | 1.06 |
| `zarr-v3-zstd/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/scrub` | 1.00 | 1.00 | 1.00 |

#### Work against cell count

Instructions at `L` over `S`. Proportional work is a ratio of about 4
(the cell ratio); spectral inputs grow coefficients, not cells.

| Operation | Instructions `S` | Instructions `L` | `L`/`S` |
|---|---:|---:|---:|
| `grib1-second-order/decode` | 3,227,144 | 12,025,694 | 3.73 |
| `grib1-simple/decode` | 3,171,547 | 12,549,432 | 3.96 |
| `grib1-spectral-complex/decode` | 122,230,392 | 253,234,898 | 2.07 |
| `grib1-spectral-simple/decode` | 122,131,135 | 252,987,431 | 2.07 |
| `grib2-5.0/decode` | 3,172,344 | 12,550,139 | 3.96 |
| `grib2-5.0/warp` | 6,967,140 | 27,667,194 | 3.97 |
| `grib2-5.0/render` | 1,627,304 | 6,252,404 | 3.84 |
| `grib2-5.0/contours` | 3,954,535 | 15,247,459 | 3.86 |
| `grib2-5.3/decode` | 3,886,440 | 14,750,747 | 3.80 |
| `grib2-5.40/decode` | 34,384,956 | 114,167,687 | 3.32 |
| `grib2-5.40/codec` | 32,986,397 | 108,656,249 | 3.29 |
| `grib2-5.41/decode` | 3,065,459 | 11,427,049 | 3.73 |
| `grib2-5.41/codec` | 1,557,488 | 5,506,930 | 3.54 |
| `grib2-5.42/decode` | 2,476,532 | 9,754,556 | 3.94 |
| `grib2-5.42/codec` | 1,295,551 | 5,088,565 | 3.93 |
| `netcdf-classic/slice` | 1,746,920 | 6,719,325 | 3.85 |
| `netcdf-classic/scrub` | 13,553,791 | 53,096,486 | 3.92 |
| `netcdf4-szip/slice` | 6,561,079 | 24,392,530 | 3.72 |
| `netcdf4-szip/scrub` | 51,772,278 | 197,196,391 | 3.81 |
| `netcdf4-szip/codec` | 4,244,102 | 16,060,056 | 3.78 |
| `netcdf4-zlib/slice` | 4,728,302 | 16,886,060 | 3.57 |
| `netcdf4-zlib/scrub` | 36,601,564 | 135,162,771 | 3.69 |
| `netcdf4-zlib/codec` | 1,129,775 | 3,805,159 | 3.37 |
| `netcdf4-zlib-span/slice` | 10,372,506 | 35,866,109 | 3.46 |
| `netcdf4-zlib-span/scrub` | 36,021,986 | 130,996,266 | 3.64 |
| `zarr-v2-blosc/slice` | 4,110,242 | 15,956,310 | 3.88 |
| `zarr-v2-blosc/scrub` | 32,133,385 | 126,273,453 | 3.93 |
| `zarr-v2-blosc/codec` | 1,448,672 | 5,734,902 | 3.96 |
| `zarr-v3-sharded/slice` | 20,111,026 | 81,979,801 | 4.08 |
| `zarr-v3-sharded/scrub` | 159,032,482 | 678,934,868 | 4.27 |
| `zarr-v3-zstd/slice` | 6,265,449 | 23,698,919 | 3.78 |
| `zarr-v3-zstd/scrub` | 48,449,124 | 190,804,147 | 3.94 |
| `zarr-v3-zstd/codec` | 4,003,775 | 15,835,031 | 3.96 |
| `zarr-v3-zstd/warp` | 6,967,548 | 27,664,286 | 3.97 |
| `zarr-v3-zstd/render` | 1,627,777 | 6,249,610 | 3.84 |
| `zarr-v3-zstd/contours` | 3,938,929 | 15,243,693 | 3.87 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,167,687 | 108,656,249 | 5% |
| `grib2-5.40-S` | 34,384,956 | 32,986,397 | 4% |
| `grib2-5.41-L` | 11,427,049 | 5,506,930 | 52% |
| `grib2-5.41-S` | 3,065,459 | 1,557,488 | 49% |
| `grib2-5.42-L` | 9,754,556 | 5,088,565 | 48% |
| `grib2-5.42-S` | 2,476,532 | 1,295,551 | 48% |
| `netcdf4-szip-D` | 6,589,936 | 4,247,546 | 36% |
| `netcdf4-szip-L` | 24,392,530 | 16,060,056 | 34% |
| `netcdf4-szip-S` | 6,561,079 | 4,244,102 | 35% |
| `netcdf4-zlib-D` | 4,757,734 | 1,129,811 | 76% |
| `netcdf4-zlib-L` | 16,886,060 | 3,805,159 | 77% |
| `netcdf4-zlib-S` | 4,728,302 | 1,129,775 | 76% |
| `zarr-v2-blosc-D` | 4,110,835 | 1,448,672 | 65% |
| `zarr-v2-blosc-L` | 15,956,310 | 5,734,902 | 64% |
| `zarr-v2-blosc-S` | 4,110,242 | 1,448,672 | 65% |
| `zarr-v3-zstd-D` | 6,266,206 | 4,003,775 | 36% |
| `zarr-v3-zstd-L` | 23,698,919 | 15,835,031 | 33% |
| `zarr-v3-zstd-S` | 6,265,449 | 4,003,775 | 36% |
<!-- perf-gate:bounds:end -->

## Re-recording

A gated number that moves by more than its tolerance fails the `Performance`
workflow until the table is re-recorded, and the pull request that re-records it
says why it moved.

```sh
crates/fieldglass-perf/run.sh --write
```

Instruction counts differ a little between machines, so the table is recorded
from one CI run. A full `run.sh` without `--write`, which is what the workflow
runs, prints the patch `--write` would make to this file from that run's
numbers, under "what --write would record". Copy it from the log and apply it
with `patch -p1`. A raw log downloaded from GitHub starts every line with a
timestamp, so copy from the log view in the browser or strip that prefix
first.

- **Exact tiers** (bytes, requests, allocations, peak heap) move only when the
  code, the corpus or the compiler does. The standard library's own allocations
  are counted, so the harness is pinned to one Rust release in
  `crates/fieldglass-perf/rust-toolchain.toml` (`run.sh` builds the wasm bundles
  with it too, and refuses any other). Bumping it is a re-record with the bump
  named as the reason.
- **Instructions** are allowed 2%, or 40,000 instructions, whichever is
  larger. Two full runs on one machine differ by under 0.02% on almost every
  row, but a row can jump by a fixed amount when glibc's allocator does work
  that preparation set up. On 2026-09-14, `netcdf-classic-S/place` read 123,558
  run alone and 136,757 inside the full run, because malloc took its
  heap-growth path or not. Later, glibc's deferred merging of small freed
  chunks (`malloc_consolidate`) landed inside some operations and not others:
  a change that only added an unused field (#1799) moved `zarr-v3-zstd-S/open`
  and `netcdf-classic-S/slice` by 40,000 to 55,000 instructions and 2 to 5% in
  estimated cycles, and a longer corpus path moved `grib2-5.41-L/codec` by
  157,000 (#1818). The bench now ends preparation with `malloc_trim(0)`, which
  does that merging before the count starts. What is left is where
  preparation's free chunks sit, and on #1818's runs it moved no row by a
  quarter of its tolerance, where one row had moved by more than twice it.
  Turning fastbins off instead (`GLIBC_TUNABLES=glibc.malloc.mxfast=0`) left
  up to 44% of the tolerance in estimated cycles, and forcing the merge with
  one 96 KiB allocation left 60%, against 22% for the trim. That is why the
  bench makes an `unsafe` call to glibc for it. The 40,000 floor covers the
  remaining jump and nothing bigger: most injected faults in the next section
  moved their rows by far more, and the one that stayed inside the 2%
  tolerance on one sharded-store row still failed bytes read. The heap tier is
  not affected either way: dhat counts the sizes requested, which no allocator
  state changes, and the `measure` binary does not trim.
- **wasm memory** is allowed one 64 KiB page either way, the unit linear memory
  grows in.
- **The corpus digest** changes when a generator or a pinned writer does. Every
  number may then move with it, and the checker says so before listing them.

## Proving the gates can fail

A gate nobody has seen fail proves nothing, so each was checked by injecting the
fault it exists for, running the harness, and reverting. Measured on the table
above, 2026-09-14.

| Fault injected | Where | What tripped |
|---|---|---|
| An extra copy of the decoded raster, while it is alive | `Session::decode` | Peak heap on all 19 decode rows (+14% to +64%) |
| An allocation per cell | `Session::decode` | Allocations on all 19 decode rows (`grib2-5.0-L`: +65,160, one per cell), and the `S`/`L` verdict turns to "grows" |
| A read and decode of the whole variable in a region decode | `fieldglass-zarr` `read_region` | Bytes read on all nine Zarr slice rows (up to 31× the bound at `D`); instructions on eight of them, with blosc and zstd at a `D`/`S` ratio of 3.4–3.6 |

Two limits the checks showed, worth knowing before trusting a green run:

- **Peak heap only sees a copy that raises the high-water mark.** A first
  attempt copied the finished `Values` buffer after the reader's own buffers
  were freed. Peak heap moved only on the spectral rows; the allocation count
  caught the rest, at +1 each. The two metrics cover each other, which is why
  both are gated.
- **A cheap wasted read hides from instruction counts.** On the sharded store
  the injected decode of each shard failed early, so instructions moved 2–6%,
  inside tolerance on one row. Bytes read still failed on every row. Instruction
  counts are the gate for wasted work; bytes read are the gate for wasted reads.

## The report tier

`crates/fieldglass-perf/run.sh --report` prints, and never gates:

- **Wall time**, native and wasm (baseline and `+simd128`), median of five runs,
  for every scenario the wasm host can run.
- **Ratios to the reference tools** over the same in-memory bytes: eccodes
  for GRIB, netCDF4 for NetCDF (h5py for the szip input, which reads it as plain HDF5, as the file was written), zarr-python for Zarr, xarray beside zarr-python
  on the real data. Each tool's binding floor (one value through the same call)
  is measured and subtracted, but only where the reference took at least three
  floors; below that the row says "binding-dominated", because subtracting a
  floor from a number no larger than it produces nonsense. Spectral GRIB1 is not
  compared: eccodes returns coefficients and synthesizes nothing.
- **The real-data scrub** below.

On the generated corpus most reference rows are binding-dominated: a 2° field
decodes in well under a millisecond. The ERA5 rows are the comparison that means
something.

## Real data

`crates/fieldglass-perf/manifests/era5.json` pins one field held three ways by
the public ARCO-ERA5 bucket on Google Cloud: 2 m temperature, 2020-01-01
00Z–11Z, twelve hourly frames. No ERA5 bytes are committed. The manifest lists
each object's URL, byte range, length and SHA-256; `manifests/build_era5.py`
wrote it by reading the bucket once.

| Container | Source | What the manifest pins |
|---|---|---|
| Zarr | `ar/full_37-1h-0p25deg-chunk-1.zarr-v3` | twelve chunks, the coordinate arrays, and a small authored subset store around them |
| NetCDF classic | `raw/date-variable-single_level/2020/01/01/2m_temperature/surface.nc` | the whole day's file, 49.8 MB |
| GRIB1 | `raw/ERA5GRIB/HRES/Month/2020/202001_hres_sfc.grb2` | twelve message ranges of an 11.7 GB file |

Three things about the bucket were not what the issue assumed, each found by
reading the bytes rather than the names:

- **The `…zarr-v3` store is [Zarr v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html)** (`.zarray`, blosc lz4). Its `.zarray` and
  `.zmetadata` are rewritten daily as ERA5T is appended, so they cannot be pinned
  by hash. The manifest carries an authored `.zarray` with the shape cut to
  twelve frames and the chunk keys renumbered from 0, around the bucket's own
  chunk bytes.
- **The `.grb2` files are [GRIB edition 1](https://www.nco.ncep.noaa.gov/pmb/docs/on388/)**, on the native N320 reduced Gaussian
  grid, padded to multiples of 120 bytes, with no sidecar index. The builder
  walks message headers to find parameter 167.
- **2020-01-01T00 is time index 1,051,896**, read from the store's own `time`
  array (`hours since 1900-01-01`, one step per index). A calendar-computed
  index was wrong the first time.

**The cache** is `$XDG_CACHE_HOME/fieldglass-perf` (override with
`FIELDGLASS_PERF_CACHE`), one file per object named by its hash, capped at
256 MiB, least recently used evicted first. `tools/fetch_perf_data.py` fetches
what is missing and verifies everything; `--offline` verifies without the
network and fails on anything missing; `--clean` removes the cache. A cached
object that changed on disk fails the run and is not quietly replaced, a fetched
object whose hash is not the manifest's fails and leaves no partial file, and a
cache directory inside the repository is refused. In CI the cache is keyed on
the manifest's hash, so the network is touched only when the manifest changes.

**First run, 2026-09-14** (wall time on one machine, not gated):

| Container | Cells per frame | Bytes read | Bound | Requests | ms per frame | Reference ms per frame |
|---|---:|---:|---:|---:|---:|---|
| Zarr | 1,038,240 | 28,024,263 | 28,024,263 | 18 | 30.7 | zarr-python 2.9, xarray 3.2 |
| NetCDF classic | 1,038,240 | 49,845,328 | 24,927,568 | 1 | 197.1 | netCDF4 3.5 |
| GRIB1 | 819,200 | 13,026,576 | 13,026,576 | 77 | 11.7 | eccodes 2.4 |

eccodes returns the GRIB field's 542,080 reduced-grid points; `Session::decode`
expands them to a 1280 × 640 regular grid, so the comparison includes work
eccodes does not do.

ERA5 is Copernicus Climate Change Service information, used under the
Copernicus license.

## What the first run found

Every one of these was measured, not estimated. None was fixed in the bootstrap:
each bound violation is its own issue, and the findings fixed since are marked
Resolved.

1. **NetCDF read and decoded the whole variable for every plane. Resolved
   (#939 for the decode, #950 for the read).** A slice's peak heap and instructions grew
   with the variable, not the plane (`D`/`S` 3.3–4.0), at 144–160 B per cell
   against a floor of 5–9. The reader now decodes only the region asked for:
   a classic slice reads its runs, a NetCDF-4 slice inflates the chunks it
   covers, and the `D`/`S` ratios are 1.00 for peak heap and 1.00–1.01 for
   instructions on every NetCDF slice and scrub row, at 29 B per cell on
   classic (the decoded-cell cost of finding 2) and 33–61 B on NetCDF-4. A
   classic slice at `S` runs 28% of the old instructions (1,786,193 against
   6,276,125), a NetCDF-4 one 17% (4,732,123 against 28,602,616). Measured by
   hand on a 0.5° hourly `t2m(48, 361, 720)`, release build,
   `Session::decode_slice` per plane went from 84 ms to 1.8 ms (classic) and
   from 241 ms to 4.6 ms (NetCDF-4 zlib, one plane per chunk), against 2.5 ms
   for picking a plane out of a whole-variable memo. What remains is
   the bytes. A chunk spanning several time steps is kept decompressed (64 MiB
   a file, least recently used first), so a scrub inflates it once rather than
   once a frame: `netcdf4-zlib-span` scrubs eight frames over two chunks for
   3.4 times one slice's instructions. On `t2m(48, 361, 720)` with
   netCDF-C's default (24, 181, 360) chunks, a 48-frame scrub went from 3.0 s
   without it to 0.25 s, against 0.28 s for the old memo's decode and
   extraction. The bytes were the file, 6× to 1,231× each row's bound, until
   the reader read through a `ByteSource` (#950). Now opening a session reads a
   classic header, or an HDF5 file's [superblock](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html#subsec_fmt3_boot_super) and metadata, and every NetCDF bytes-read row is within its bound: an open reads 448 bytes of
   a classic file (bound 1,512 to 2,592) and 2,788 of a NetCDF-4 one (12,942 to
   14,990), a slice its runs or chunks and the metadata to find them
   (`netcdf-classic-D/slice` 66,604 of 2,098,344 bytes, bound 67,224). Reading
   by range costs round trips: a NetCDF-4 open takes 29 requests, one per
   structure the walk follows. On ERA5 the twelve-frame scrub read the whole
   49.8 MB file; it now reads 24,927,472 bytes in 21 requests, against a bound
   of 24,927,568 (re-measured 2026-10-11; the ERA5 table above is the first run).
2. **A decoded cell costs 29 B where the floor is 5.** The readers return
   `Vec<Option<f64>>` (16 B), `pack_values` copies into a `Vec<f64>` (8 B) and
   the mask (1 B), then `Dtype::Auto` narrows to `f32` (4 B) while all three are
   alive. Zarr slices hold 40 B per cell against a floor of 9.
3. **The AEC decoder allocated per block. Resolved (#762).** [5.42](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) decodes
   allocated 529 times at `S` and 2,054 at `L`, and the external decoder alone
   accounted for 513 and 2,038. On `fieldglass-aec` (ADR-0012) a codec call
   allocates once at both sizes (its output buffer) and a decode 16 times at
   both, so the allocation bound holds. The codec runs 22% of the old
   instructions at `L` (5,249,124 against 23,764,511) and the whole decode 32%
   (9,788,554 against 30,573,243), because samples now scale straight into the
   output instead of passing through a byte buffer.
4. **JPEG 2000's cost is the codec's.** 95–96% of a [5.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml) decode's instructions
   are in `rust_j2k`, which also allocates more as the grid grows (413 → 551).
   It is 3.7× eccodes (OpenJPEG) at 1°. This answers the issue's question: the
   ceiling is the codec's, not the decode path's.
5. **NetCDF-4 slices spent 97–99% of their instructions outside zlib**, which
   was finding 1 again from the other side. With region decode it is 74–76%,
   most of it the decoded-cell cost of finding 2.
6. **A [sharded](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html) Zarr slice decodes every chunk in its shard.** Fetching the whole
   shard is the bound (an object source fetches whole objects), but decoding
   it is not: 120 B per cell against 9, and allocations grow with cells (350 → 446).
7. **[Zarr v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) blosc on ERA5 is 10.7× zarr-python** (30.7 ms against 2.9 ms per
   frame). Two thirds of a blosc slice's instructions are outside the codec.
8. **Contours hold 17 B per cell** where the floor is a row of state, and
   allocate a few more times as the grid grows.
9. **Opening a Zarr store costs more as the variable grows** (allocations and
   instructions `D`/`S` up to 1.20): the walk does work per chunk key.
10. **`+simd128` buys nothing measurable**, and spectral synthesis is 13× slower
    under wasm than native, against 2–4× for everything else.
