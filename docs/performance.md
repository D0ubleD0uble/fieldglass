# Performance and memory

*Bootstrapped 2026-09-14 ([#743](https://github.com/D0ubleD0uble/fieldglass/issues/743)).*

This is the cost half of [verification](verification.md). The correctness half
proves the numbers are right; this half measures what it costs to produce them,
and holds each cost to a **stated bound** rather than to its own history. A
number compared only with last week's can drift slowly forever. A number
compared with "one chunk per frame" cannot.

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
| instructions | Valgrind (Linux) and `cargo install gungraun-runner --version 0.19.4 --locked` |
| wasm | Node.js, and what `crates/fieldglass-wasm/build.sh` needs (the matching `wasm-bindgen` CLI and binaryen) |
| report, real data | network access once, for `tools/fetch_perf_data.py` |

A missing prerequisite stops the run with the install command. It never skips a
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
| Report | Ratio to a reference tool | `reference.py`: eccodes, netCDF4, zarr-python | printed |
| Report | Real data: requests, bytes and time on the remote path | the pinned ERA5 manifest | printed |
| Local | Flame graphs, long scrubs | `perf`, Callgrind's own output under `target/gungraun` | manual |

**Why these are deterministic.** dhat counts the sizes the code *requests*, not
what the system allocator rounds them to, so the heap numbers are a property of
the code and the input. The recording counts the ranges the reader asks for.
Callgrind counts instructions, not time, and its cache simulation uses a fixed
geometry rather than the host CPU's, so a laptop and a runner agree. Only the
instruction counts move with the compiler, hence their tolerance.

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

Every input exists at two sizes, because a bound about scaling needs two points:

| Size | Grid | Time steps | Used for |
|---|---|---|---|
| `S` | 2° (180 × 91) | 8 | the baseline |
| `L` | 1° (360 × 181), four times the cells | 8 | cost against cell count |
| `D` | 2° (180 × 91) | 32, four times the variable | cost against the variable, not the plane |

| Format | Inputs |
|---|---|
| GRIB1 | simple; second-order; spectral simple and spectral complex (`S` = T63, `L` = T127) |
| GRIB2 §5 | 5.0, 5.3, 5.40, 5.41, 5.42 at `S` and `L`; 5.200 at one size |
| NetCDF | classic; NetCDF-4 with zlib and shuffle, chunked one step per chunk, and four steps per chunk (`netcdf4-zlib-span`) |
| Zarr | v2 with blosc (lz4, shuffle); v3 with zstd; v3 sharded, four steps per shard |

| Operation | On |
|---|---|
| open, decode, place | every message input |
| open, variables, slice, scrub (eight frames), place | every array input |
| warp, palette, render, contours | `grib2-5.0` and `zarr-v3-zstd`, at `S` and `L` |
| codec alone | 5.40 (JPEG 2000), 5.41 (PNG), 5.42 (AEC), NetCDF-4 (zlib), Zarr v3 (zstd), Zarr v2 (blosc) |

What is deliberately not covered, and why:

- **5.200 has one size.** eccodes decodes run-length packing but cannot encode
  it, so the committed fixture stands in and has no second size to scale against.
- **Zarr has no wasm rows.** The wasm host opens bytes, not stores.
- **NetCDF has no range-read rows.** The reader takes the whole file as a `Vec`,
  so every NetCDF operation's "bytes read" is the file. That is a finding, not a
  harness gap: see below.
- **Spectral inputs do not scale in cells.** Every spectral field is synthesised
  onto the same 0.5° grid, so `S` and `L` differ in coefficients only.

## Bounds

Every bound is a formula over facts the *writer* recorded (eccodes keys, HDF5's
own chunk records, the layout the classic format fixes), never over what the
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

Corpus digest: `f6c0f0083c35930c23232669843c3f37a320120d200cbe4955031377f3b182ed`

A pull request that moves any number here fails the `perf` job until the table
is re-recorded with `crates/fieldglass-perf/run.sh --write`, and the pull request
says why the number moved. Both directions: an improvement nobody records is
lost to the next regression.

<!-- perf-gate:table:begin — regenerate with tools/check_perf_gate.py --write -->
| Scenario | Cells | Value bytes | Bytes read | Bound | Requests | Allocations | Peak heap | Instructions | Est. cycles | wasm memory | wasm +simd128 memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `grib1-second-order-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,723 | 27,759 | 1,638,400 | 1,638,400 |
| `grib1-second-order-L/decode` | 65,160 | 4 | 26,524 | 26,620 | 1 | 20 | 2,106,981 | 12,094,734 | 17,640,475 | 3,735,552 | 3,735,552 |
| `grib1-second-order-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,134 | 30,145 | — | — |
| `grib1-second-order-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,554 | 27,510 | 1,638,400 | 1,638,400 |
| `grib1-second-order-S/decode` | 16,380 | 4 | 10,454 | 10,550 | 1 | 20 | 532,188 | 3,295,767 | 4,664,947 | 2,293,760 | 2,293,760 |
| `grib1-second-order-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,004 | 29,922 | — | — |
| `grib1-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,648 | 27,641 | 1,835,008 | 1,835,008 |
| `grib1-simple-L/decode` | 65,160 | 4 | 130,332 | 130,428 | 1 | 15 | 1,889,666 | 12,618,075 | 17,660,483 | 3,670,016 | 3,670,016 |
| `grib1-simple-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,235 | 30,321 | — | — |
| `grib1-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 16,106 | 28,423 | 1,703,936 | 1,703,936 |
| `grib1-simple-S/decode` | 16,380 | 4 | 32,772 | 32,868 | 1 | 15 | 475,046 | 3,240,317 | 4,480,028 | 2,162,688 | 2,162,688 |
| `grib1-simple-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,099 | 30,051 | — | — |
| `grib1-spectral-complex-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,513 | 27,279 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-L/decode` | 259,920 | 8 | 33,966 | 34,062 | 1 | 24 | 6,498,082 | 253,302,993 | 362,315,182 | 10,551,296 | 10,551,296 |
| `grib1-spectral-complex-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,904 | 28,742 | — | — |
| `grib1-spectral-complex-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,488 | 27,254 | 1,638,400 | 1,638,400 |
| `grib1-spectral-complex-S/decode` | 259,920 | 8 | 9,262 | 9,358 | 1 | 24 | 6,498,082 | 122,298,208 | 180,352,437 | 9,109,504 | 9,109,504 |
| `grib1-spectral-complex-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,901 | 28,731 | — | — |
| `grib1-spectral-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,488 | 27,250 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-L/decode` | 259,920 | 8 | 33,038 | 33,134 | 1 | 23 | 6,498,082 | 253,055,313 | 361,912,692 | 10,551,296 | 10,551,296 |
| `grib1-spectral-simple-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,965 | 28,779 | — | — |
| `grib1-spectral-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,381 | 27,066 | 1,638,400 | 1,638,400 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8 | 8,334 | 8,430 | 1 | 23 | 6,498,082 | 122,198,642 | 180,207,482 | 9,109,504 | 9,109,504 |
| `grib1-spectral-simple-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 16,177 | 29,079 | — | — |
| `grib2-5.0-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 85,020 | 133,223 | 1,900,544 | 1,900,544 |
| `grib2-5.0-L/decode` | 65,160 | 4 | 130,331 | 130,499 | 1 | 16 | 1,889,658 | 12,618,487 | 17,659,489 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 17,466 | 32,207 | — | — |
| `grib2-5.0-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,732,076 | 38,484,370 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 141,115 | 199,726 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,643,423 | 9,056,777 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/contours` | 65,160 | — | 0 | 0 | 0 | 54 | 1,165,776 | 15,311,053 | 19,025,073 | 3,735,552 | 3,735,552 |
| `grib2-5.0-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 83,569 | 130,770 | 1,703,936 | 1,703,936 |
| `grib2-5.0-S/decode` | 16,380 | 4 | 32,771 | 32,939 | 1 | 16 | 475,038 | 3,239,031 | 4,484,478 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,549 | 30,809 | — | — |
| `grib2-5.0-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,033,602 | 9,769,887 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 140,604 | 198,927 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,808,123 | 2,495,092 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/contours` | 16,380 | — | 0 | 0 | 0 | 49 | 332,048 | 4,017,784 | 5,053,266 | 2,162,688 | 2,162,688 |
| `grib2-5.200-S/open` | 0 | — | 211 | 211 | 7 | 4 | 2,058 | 108,754 | 168,385 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/decode` | 496 | 4 | 20 | 211 | 1 | 18 | 14,402 | 141,189 | 221,333 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/place` | 0 | — | 0 | 211 | 0 | 4 | 70 | 17,067 | 31,622 | — | — |
| `grib2-5.3-L/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 83,230 | 130,588 | 1,703,936 | 1,703,936 |
| `grib2-5.3-L/decode` | 65,160 | 4 | 57,954 | 58,150 | 1 | 18 | 1,889,658 | 14,817,327 | 20,547,316 | 3,604,480 | 3,604,480 |
| `grib2-5.3-L/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,840 | 31,302 | — | — |
| `grib2-5.3-S/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 82,829 | 130,005 | 1,638,400 | 1,638,400 |
| `grib2-5.3-S/decode` | 16,380 | 4 | 18,586 | 18,782 | 1 | 18 | 475,038 | 3,952,549 | 5,443,448 | 2,097,152 | 2,097,152 |
| `grib2-5.3-S/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,428 | 30,598 | — | — |
| `grib2-5.40-L/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 82,678 | 129,610 | 1,638,400 | 1,638,400 |
| `grib2-5.40-L/decode` | 65,160 | 4 | 14,681 | 14,851 | 1 | 567 | 1,889,658 | 114,210,514 | 146,553,455 | 3,866,624 | 3,866,624 |
| `grib2-5.40-L/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,709 | 30,956 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | — | 551 | 859,764 | 108,710,709 | 137,671,519 | — | — |
| `grib2-5.40-S/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 82,647 | 129,594 | 1,638,400 | 1,638,400 |
| `grib2-5.40-S/decode` | 16,380 | 4 | 6,278 | 6,448 | 1 | 429 | 475,038 | 34,526,283 | 44,486,264 | 2,162,688 | 2,162,688 |
| `grib2-5.40-S/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 17,182 | 31,701 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | — | 413 | 221,460 | 33,110,498 | 42,272,115 | — | — |
| `grib2-5.41-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 81,840 | 128,561 | 1,835,008 | 1,835,008 |
| `grib2-5.41-L/decode` | 65,160 | 4 | 80,490 | 80,658 | 1 | 25 | 1,898,632 | 11,506,226 | 16,791,842 | 3,735,552 | 3,735,552 |
| `grib2-5.41-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,446 | 30,524 | — | — |
| `grib2-5.41-L/codec` | 65,160 | — | — | — | — | 9 | 334,744 | 5,573,115 | 7,448,181 | — | — |
| `grib2-5.41-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 81,659 | 128,253 | 1,638,400 | 1,638,400 |
| `grib2-5.41-S/decode` | 16,380 | 4 | 23,501 | 23,669 | 1 | 25 | 511,568 | 3,143,882 | 4,578,616 | 2,293,760 | 2,293,760 |
| `grib2-5.41-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,783 | 31,071 | — | — |
| `grib2-5.41-S/codec` | 16,380 | — | — | — | — | 9 | 118,400 | 1,623,095 | 2,220,400 | — | — |
| `grib2-5.42-L/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 81,454 | 127,944 | 1,835,008 | 1,835,008 |
| `grib2-5.42-L/decode` | 65,160 | 4 | 80,767 | 80,939 | 1 | 16 | 1,889,658 | 9,819,130 | 13,874,172 | 3,670,016 | 3,670,016 |
| `grib2-5.42-L/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,758 | 31,042 | — | — |
| `grib2-5.42-L/codec` | 65,160 | — | — | — | — | 1 | 130,320 | 5,199,886 | 6,325,622 | — | — |
| `grib2-5.42-S/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 81,247 | 127,636 | 1,638,400 | 1,638,400 |
| `grib2-5.42-S/decode` | 16,380 | 4 | 22,449 | 22,621 | 1 | 16 | 475,038 | 2,540,745 | 3,541,355 | 2,097,152 | 2,097,152 |
| `grib2-5.42-S/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,772 | 31,105 | — | — |
| `grib2-5.42-S/codec` | 16,380 | — | — | — | — | 1 | 32,760 | 1,384,020 | 1,709,961 | — | — |
| `netcdf-classic-D/open` | 0 | — | 2,098,344 | 1,704 | 1 | 102 | 4,982 | 37,626 | 66,073 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/variables` | 0 | — | 2,098,344 | 1,704 | 1 | 41 | 1,290 | 27,748 | 42,148 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/slice` | 16,380 | 4 | 2,098,344 | 67,224 | 1 | 104 | 476,012 | 1,790,693 | 2,687,583 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/scrub` | 16,380 | 4 | 2,098,344 | 525,864 | 1 | 559 | 476,012 | 13,587,472 | 18,926,138 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/place` | 0 | — | 2,098,344 | 1,704 | 1 | 85 | 6,179 | 122,918 | 189,257 | — | — |
| `netcdf-classic-L/open` | 0 | — | 2,087,712 | 2,592 | 1 | 102 | 4,982 | 37,898 | 66,501 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/variables` | 0 | — | 2,087,712 | 2,592 | 1 | 41 | 1,290 | 13,382 | 20,508 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/slice` | 65,160 | 4 | 2,087,712 | 263,232 | 1 | 104 | 1,890,632 | 6,745,709 | 10,102,826 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/scrub` | 65,160 | 4 | 2,087,712 | 2,087,712 | 1 | 559 | 1,890,632 | 53,113,368 | 74,191,189 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/place` | 0 | — | 2,087,712 | 2,592 | 1 | 85 | 11,219 | 138,944 | 209,744 | — | — |
| `netcdf-classic-S/open` | 0 | — | 525,672 | 1,512 | 1 | 102 | 4,982 | 37,759 | 66,310 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/variables` | 0 | — | 525,672 | 1,512 | 1 | 41 | 1,290 | 13,382 | 20,580 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/slice` | 16,380 | 4 | 525,672 | 67,032 | 1 | 104 | 476,012 | 1,790,454 | 2,680,931 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/scrub` | 16,380 | 4 | 525,672 | 525,672 | 1 | 559 | 476,012 | 13,590,448 | 18,924,388 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/place` | 0 | — | 525,672 | 1,512 | 1 | 85 | 6,179 | 122,664 | 188,876 | — | — |
| `netcdf4-zlib-D/open` | 0 | — | 1,116,265 | 12,942 | 1 | 378 | 14,959 | 358,603 | 525,955 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/variables` | 0 | — | 1,116,265 | 12,942 | 1 | 41 | 1,290 | 13,423 | 20,549 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/slice` | 16,380 | 4 | 1,116,265 | 47,445 | 1 | 256 | 543,956 | 4,781,972 | 6,909,605 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/scrub` | 16,380 | 4 | 1,116,265 | 288,808 | 1 | 1,007 | 1,003,368 | 36,413,191 | 51,008,488 | 4,128,768 | 4,128,768 |
| `netcdf4-zlib-D/place` | 0 | — | 1,116,265 | 12,942 | 1 | 155 | 6,179 | 225,160 | 329,809 | — | — |
| `netcdf4-zlib-D/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,227,556 | 1,666,776 | — | — |
| `netcdf4-zlib-L/open` | 0 | — | 1,006,286 | 14,990 | 1 | 378 | 14,959 | 357,951 | 524,730 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/variables` | 0 | — | 1,006,286 | 14,990 | 1 | 41 | 1,290 | 13,380 | 20,494 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/slice` | 65,160 | 4 | 1,006,286 | 139,063 | 1 | 231 | 2,152,160 | 16,805,278 | 24,101,640 | 5,242,880 | 5,242,880 |
| `netcdf4-zlib-L/scrub` | 65,160 | 4 | 1,006,286 | 1,006,286 | 1 | 989 | 3,977,412 | 134,681,524 | 189,411,813 | 8,388,608 | 8,388,608 |
| `netcdf4-zlib-L/place` | 0 | — | 1,006,286 | 14,990 | 1 | 155 | 11,219 | 253,686 | 368,249 | — | — |
| `netcdf4-zlib-L/codec` | 65,160 | — | — | — | — | 3 | 506,796 | 4,001,217 | 5,663,446 | — | — |
| `netcdf4-zlib-S/open` | 0 | — | 288,808 | 12,942 | 1 | 378 | 14,959 | 358,392 | 524,973 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/variables` | 0 | — | 288,808 | 12,942 | 1 | 41 | 1,290 | 13,381 | 20,495 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/slice` | 16,380 | 4 | 288,808 | 47,445 | 1 | 230 | 542,420 | 4,748,791 | 6,862,096 | 2,490,368 | 2,490,368 |
| `netcdf4-zlib-S/scrub` | 16,380 | 4 | 288,808 | 288,808 | 1 | 981 | 1,001,832 | 36,373,020 | 50,951,041 | 3,014,656 | 3,014,656 |
| `netcdf4-zlib-S/place` | 0 | — | 288,808 | 12,942 | 1 | 155 | 6,179 | 224,055 | 328,112 | — | — |
| `netcdf4-zlib-S/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,224,170 | 1,660,157 | — | — |
| `netcdf4-zlib-span-D/open` | 0 | — | 1,088,484 | 12,942 | 1 | 378 | 14,959 | 358,317 | 524,836 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/variables` | 0 | — | 1,088,484 | 12,942 | 1 | 41 | 1,290 | 13,381 | 20,495 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/slice` | 16,380 | 4 | 1,088,484 | 147,279 | 1 | 230 | 738,980 | 10,399,955 | 14,719,239 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/scrub` | 16,380 | 4 | 1,088,484 | 281,949 | 1 | 943 | 1,001,100 | 35,807,225 | 49,738,823 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/place` | 0 | — | 1,088,484 | 12,942 | 1 | 155 | 6,179 | 223,997 | 328,435 | — | — |
| `netcdf4-zlib-span-L/open` | 0 | — | 985,072 | 14,990 | 1 | 378 | 14,959 | 357,521 | 523,813 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-span-L/variables` | 0 | — | 985,072 | 14,990 | 1 | 41 | 1,290 | 13,381 | 20,503 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-span-L/slice` | 65,160 | 4 | 985,072 | 499,406 | 1 | 224 | 3,242,339 | 35,845,369 | 51,335,793 | 7,667,712 | 7,667,712 |
| `netcdf4-zlib-span-L/scrub` | 65,160 | 4 | 985,072 | 985,072 | 1 | 938 | 4,290,492 | 131,906,952 | 186,327,102 | 8,716,288 | 8,716,288 |
| `netcdf4-zlib-span-L/place` | 0 | — | 985,072 | 14,990 | 1 | 155 | 11,219 | 252,780 | 367,292 | — | — |
| `netcdf4-zlib-span-S/open` | 0 | — | 281,949 | 12,942 | 1 | 378 | 14,959 | 356,930 | 523,156 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-span-S/variables` | 0 | — | 281,949 | 12,942 | 1 | 41 | 1,290 | 13,372 | 20,444 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-span-S/slice` | 16,380 | 4 | 281,949 | 147,279 | 1 | 223 | 738,676 | 10,391,808 | 14,706,888 | 2,752,512 | 2,752,512 |
| `netcdf4-zlib-span-S/scrub` | 16,380 | 4 | 281,949 | 281,949 | 1 | 936 | 1,000,796 | 35,798,327 | 49,725,777 | 3,014,656 | 3,014,656 |
| `netcdf4-zlib-span-S/place` | 0 | — | 281,949 | 12,942 | 1 | 155 | 6,179 | 222,912 | 326,574 | — | — |
| `zarr-v2-blosc-D/open` | 0 | — | 1,158 | 1,584 | 4 | 370 | 14,038 | 288,292 | 456,453 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | — | 0 | 1,584 | 0 | 35 | 1,186 | 10,904 | 17,195 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 4 | 37,996 | 39,154 | 3 | 173 | 656,211 | 4,155,135 | 6,444,514 | — | — |
| `zarr-v2-blosc-D/scrub` | 16,380 | 4 | 300,515 | 301,673 | 10 | 782 | 656,584 | 32,183,600 | 47,595,176 | — | — |
| `zarr-v2-blosc-D/place` | 0 | — | 426 | 1,584 | 2 | 123 | 9,347 | 164,664 | 257,794 | — | — |
| `zarr-v2-blosc-D/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,510,562 | 2,415,560 | — | — |
| `zarr-v2-blosc-L/open` | 0 | — | 1,161 | 1,900 | 4 | 321 | 14,038 | 249,840 | 403,463 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | — | 0 | 1,900 | 0 | 35 | 1,186 | 10,904 | 17,211 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 4 | 144,560 | 145,721 | 3 | 176 | 2,607,411 | 15,987,359 | 25,162,848 | — | — |
| `zarr-v2-blosc-L/scrub` | 65,160 | 4 | 1,141,368 | 1,142,529 | 10 | 792 | 2,607,784 | 126,336,105 | 187,264,407 | — | — |
| `zarr-v2-blosc-L/place` | 0 | — | 739 | 1,900 | 2 | 125 | 17,267 | 209,615 | 318,306 | — | — |
| `zarr-v2-blosc-L/codec` | 65,160 | — | — | — | — | 6 | 781,920 | 5,799,324 | 9,341,581 | — | — |
| `zarr-v2-blosc-S/open` | 0 | — | 1,157 | 1,583 | 4 | 321 | 14,038 | 249,396 | 402,659 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | — | 0 | 1,583 | 0 | 35 | 1,186 | 10,886 | 17,187 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 4 | 37,996 | 39,153 | 3 | 173 | 656,211 | 4,153,836 | 6,442,507 | — | — |
| `zarr-v2-blosc-S/scrub` | 16,380 | 4 | 300,515 | 301,672 | 10 | 782 | 656,584 | 32,183,477 | 47,589,773 | — | — |
| `zarr-v2-blosc-S/place` | 0 | — | 426 | 1,583 | 2 | 123 | 9,347 | 163,791 | 256,648 | — | — |
| `zarr-v2-blosc-S/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,509,388 | 2,406,921 | — | — |
| `zarr-v3-sharded-D/open` | 0 | — | 2,800 | 3,413 | 3 | 393 | 35,909 | 294,367 | 461,673 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | — | 0 | 3,413 | 0 | 35 | 1,186 | 10,826 | 17,094 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 4 | 139,799 | 142,599 | 3 | 403 | 1,967,139 | 20,334,866 | 32,254,405 | — | — |
| `zarr-v3-sharded-D/scrub` | 16,380 | 4 | 278,830 | 281,630 | 10 | 2,300 | 1,967,512 | 160,603,689 | 248,258,488 | — | — |
| `zarr-v3-sharded-D/place` | 0 | — | 613 | 3,413 | 2 | 169 | 17,437 | 316,546 | 480,889 | — | — |
| `zarr-v3-sharded-L/open` | 0 | — | 2,804 | 4,019 | 3 | 380 | 35,909 | 288,131 | 452,868 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | — | 0 | 4,019 | 0 | 35 | 1,186 | 10,799 | 17,049 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 4 | 526,253 | 529,057 | 3 | 557 | 7,820,739 | 83,263,431 | 133,342,838 | — | — |
| `zarr-v3-sharded-L/scrub` | 65,160 | 4 | 1,058,791 | 1,061,595 | 10 | 3,524 | 7,821,112 | 687,765,499 | 1,069,598,182 | — | — |
| `zarr-v3-sharded-L/place` | 0 | — | 1,215 | 4,019 | 2 | 173 | 24,632 | 411,112 | 608,714 | — | — |
| `zarr-v3-sharded-S/open` | 0 | — | 2,799 | 3,412 | 3 | 380 | 35,909 | 286,902 | 450,878 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | — | 0 | 3,412 | 0 | 35 | 1,186 | 10,787 | 17,053 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 4 | 139,799 | 142,598 | 3 | 403 | 1,967,139 | 20,337,499 | 32,258,917 | — | — |
| `zarr-v3-sharded-S/scrub` | 16,380 | 4 | 278,830 | 281,629 | 10 | 2,300 | 1,967,512 | 160,596,975 | 248,259,852 | — | — |
| `zarr-v3-sharded-S/place` | 0 | — | 613 | 3,412 | 2 | 169 | 17,437 | 314,196 | 477,162 | — | — |
| `zarr-v3-zstd-D/open` | 0 | — | 2,186 | 2,799 | 3 | 387 | 32,208 | 316,408 | 488,778 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | — | 0 | 2,799 | 0 | 35 | 1,186 | 10,790 | 17,076 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 4 | 52,381 | 54,567 | 3 | 242 | 656,219 | 6,318,005 | 9,595,977 | — | — |
| `zarr-v3-zstd-D/scrub` | 16,380 | 4 | 414,719 | 416,905 | 10 | 1,030 | 656,592 | 48,715,878 | 71,601,014 | — | — |
| `zarr-v3-zstd-D/place` | 0 | — | 613 | 2,799 | 2 | 169 | 17,437 | 314,154 | 478,179 | — | — |
| `zarr-v3-zstd-D/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,091,093 | 6,015,830 | — | — |
| `zarr-v3-zstd-L/open` | 0 | — | 2,189 | 3,404 | 3 | 337 | 32,208 | 264,703 | 417,971 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | — | 0 | 3,404 | 0 | 35 | 1,186 | 10,781 | 17,007 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 4 | 207,207 | 209,396 | 3 | 260 | 2,607,419 | 24,130,105 | 37,156,012 | — | — |
| `zarr-v3-zstd-L/scrub` | 65,160 | 4 | 1,645,094 | 1,647,283 | 10 | 1,119 | 2,607,792 | 191,923,435 | 282,310,665 | — | — |
| `zarr-v3-zstd-L/place` | 0 | — | 1,215 | 3,404 | 2 | 173 | 24,632 | 408,950 | 606,356 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,721,087 | 38,467,409 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 131,059 | 184,349 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,634,397 | 9,042,361 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | — | 0 | 0 | 0 | 55 | 1,182,160 | 15,299,743 | 19,007,697 | — | — |
| `zarr-v3-zstd-L/codec` | 65,160 | — | — | — | — | 43 | 769,444 | 15,905,184 | 23,343,234 | — | — |
| `zarr-v3-zstd-S/open` | 0 | — | 2,185 | 2,798 | 3 | 337 | 32,208 | 257,549 | 407,050 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | — | 0 | 2,798 | 0 | 35 | 1,186 | 10,781 | 17,015 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 4 | 52,381 | 54,566 | 3 | 242 | 656,219 | 6,333,820 | 9,646,427 | — | — |
| `zarr-v3-zstd-S/scrub` | 16,380 | 4 | 414,719 | 416,904 | 10 | 1,030 | 656,592 | 48,711,900 | 71,600,464 | — | — |
| `zarr-v3-zstd-S/place` | 0 | — | 613 | 2,798 | 2 | 169 | 17,437 | 312,553 | 475,730 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,022,957 | 9,716,584 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 130,095 | 182,867 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,797,487 | 2,446,907 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | — | 0 | 0 | 0 | 48 | 323,856 | 3,993,511 | 4,961,485 | — | — |
| `zarr-v3-zstd-S/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,075,658 | 5,993,225 | — | — |
<!-- perf-gate:table:end -->

## Bounds today

Derived from the table above by the same checker, and checked the same way.

<!-- perf-gate:bounds:begin — regenerate with tools/check_perf_gate.py --write -->
#### Bytes read

Each row that reads more than its bound. The bound is stated beside every
scenario in the table above; these are the ones over it.

| Scenario | Bytes read | Bound | Over by |
|---|---:|---:|---:|
| `netcdf-classic-D/open` | 2,098,344 | 1,704 | 1231.4× |
| `netcdf-classic-D/variables` | 2,098,344 | 1,704 | 1231.4× |
| `netcdf-classic-D/slice` | 2,098,344 | 67,224 | 31.2× |
| `netcdf-classic-D/scrub` | 2,098,344 | 525,864 | 4.0× |
| `netcdf-classic-D/place` | 2,098,344 | 1,704 | 1231.4× |
| `netcdf-classic-L/open` | 2,087,712 | 2,592 | 805.4× |
| `netcdf-classic-L/variables` | 2,087,712 | 2,592 | 805.4× |
| `netcdf-classic-L/slice` | 2,087,712 | 263,232 | 7.9× |
| `netcdf-classic-L/place` | 2,087,712 | 2,592 | 805.4× |
| `netcdf-classic-S/open` | 525,672 | 1,512 | 347.7× |
| `netcdf-classic-S/variables` | 525,672 | 1,512 | 347.7× |
| `netcdf-classic-S/slice` | 525,672 | 67,032 | 7.8× |
| `netcdf-classic-S/place` | 525,672 | 1,512 | 347.7× |
| `netcdf4-zlib-D/open` | 1,116,265 | 12,942 | 86.3× |
| `netcdf4-zlib-D/variables` | 1,116,265 | 12,942 | 86.3× |
| `netcdf4-zlib-D/slice` | 1,116,265 | 47,445 | 23.5× |
| `netcdf4-zlib-D/scrub` | 1,116,265 | 288,808 | 3.9× |
| `netcdf4-zlib-D/place` | 1,116,265 | 12,942 | 86.3× |
| `netcdf4-zlib-L/open` | 1,006,286 | 14,990 | 67.1× |
| `netcdf4-zlib-L/variables` | 1,006,286 | 14,990 | 67.1× |
| `netcdf4-zlib-L/slice` | 1,006,286 | 139,063 | 7.2× |
| `netcdf4-zlib-L/place` | 1,006,286 | 14,990 | 67.1× |
| `netcdf4-zlib-S/open` | 288,808 | 12,942 | 22.3× |
| `netcdf4-zlib-S/variables` | 288,808 | 12,942 | 22.3× |
| `netcdf4-zlib-S/slice` | 288,808 | 47,445 | 6.1× |
| `netcdf4-zlib-S/place` | 288,808 | 12,942 | 22.3× |
| `netcdf4-zlib-span-D/open` | 1,088,484 | 12,942 | 84.1× |
| `netcdf4-zlib-span-D/variables` | 1,088,484 | 12,942 | 84.1× |
| `netcdf4-zlib-span-D/slice` | 1,088,484 | 147,279 | 7.4× |
| `netcdf4-zlib-span-D/scrub` | 1,088,484 | 281,949 | 3.9× |
| `netcdf4-zlib-span-D/place` | 1,088,484 | 12,942 | 84.1× |
| `netcdf4-zlib-span-L/open` | 985,072 | 14,990 | 65.7× |
| `netcdf4-zlib-span-L/variables` | 985,072 | 14,990 | 65.7× |
| `netcdf4-zlib-span-L/slice` | 985,072 | 499,406 | 2.0× |
| `netcdf4-zlib-span-L/place` | 985,072 | 14,990 | 65.7× |
| `netcdf4-zlib-span-S/open` | 281,949 | 12,942 | 21.8× |
| `netcdf4-zlib-span-S/variables` | 281,949 | 12,942 | 21.8× |
| `netcdf4-zlib-span-S/slice` | 281,949 | 147,279 | 1.9× |
| `netcdf4-zlib-span-S/place` | 281,949 | 12,942 | 21.8× |

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
| `netcdf-classic/open` | 102 | 102 | holds |
| `netcdf-classic/variables` | 41 | 41 | holds |
| `netcdf-classic/slice` | 104 | 104 | holds |
| `netcdf-classic/scrub` | 559 | 559 | holds |
| `netcdf-classic/place` | 85 | 85 | holds |
| `netcdf4-zlib/open` | 378 | 378 | holds |
| `netcdf4-zlib/variables` | 41 | 41 | holds |
| `netcdf4-zlib/slice` | 230 | 231 | grows +1 |
| `netcdf4-zlib/scrub` | 981 | 989 | grows +8 |
| `netcdf4-zlib/place` | 155 | 155 | holds |
| `netcdf4-zlib/codec` | 2 | 3 | grows +1 |
| `netcdf4-zlib-span/open` | 378 | 378 | holds |
| `netcdf4-zlib-span/variables` | 41 | 41 | holds |
| `netcdf4-zlib-span/slice` | 223 | 224 | grows +1 |
| `netcdf4-zlib-span/scrub` | 936 | 938 | grows +2 |
| `netcdf4-zlib-span/place` | 155 | 155 | holds |
| `zarr-v2-blosc/open` | 321 | 321 | holds |
| `zarr-v2-blosc/variables` | 35 | 35 | holds |
| `zarr-v2-blosc/slice` | 173 | 176 | grows +3 |
| `zarr-v2-blosc/scrub` | 782 | 792 | grows +10 |
| `zarr-v2-blosc/place` | 123 | 125 | grows +2 |
| `zarr-v2-blosc/codec` | 5 | 6 | grows +1 |
| `zarr-v3-sharded/open` | 380 | 380 | holds |
| `zarr-v3-sharded/variables` | 35 | 35 | holds |
| `zarr-v3-sharded/slice` | 403 | 557 | grows +154 |
| `zarr-v3-sharded/scrub` | 2,300 | 3,524 | grows +1,224 |
| `zarr-v3-sharded/place` | 169 | 173 | grows +4 |
| `zarr-v3-zstd/open` | 337 | 337 | holds |
| `zarr-v3-zstd/variables` | 35 | 35 | holds |
| `zarr-v3-zstd/slice` | 242 | 260 | grows +18 |
| `zarr-v3-zstd/scrub` | 1,030 | 1,119 | grows +89 |
| `zarr-v3-zstd/place` | 169 | 173 | grows +4 |
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
| `netcdf4-zlib/slice` | 33.0 B | 1,880 B | 9 B | 3.7× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 61.0 B | 2,652 B | 9 B | 6.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib-span/slice` | 51.3 B | -102,037 B | 21 B | 2.4× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
| `netcdf4-zlib-span/scrub` | 67.4 B | -103,862 B | 21 B | 3.2× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
| `zarr-v2-blosc/slice` | 40.0 B | 1,011 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/scrub` | 40.0 B | 1,384 B | 9 B | 4.4× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/slice` | 120.0 B | 1,539 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/scrub` | 120.0 B | 1,912 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
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
| `netcdf4-zlib/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib/slice` | 1.00 | 1.11 | 1.01 |
| `netcdf4-zlib/scrub` | 1.00 | 1.03 | 1.00 |
| `netcdf4-zlib-span/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib-span/slice` | 1.00 | 1.03 | 1.00 |
| `netcdf4-zlib-span/scrub` | 1.00 | 1.01 | 1.00 |
| `zarr-v2-blosc/open` | 1.00 | 1.15 | 1.16 |
| `zarr-v2-blosc/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v2-blosc/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/open` | 1.00 | 1.03 | 1.03 |
| `zarr-v3-sharded/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/open` | 1.00 | 1.15 | 1.23 |
| `zarr-v3-zstd/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/scrub` | 1.00 | 1.00 | 1.00 |

#### Work against cell count

Instructions at `L` over `S`. Proportional work is a ratio of about 4
(the cell ratio); spectral inputs grow coefficients, not cells.

| Operation | Instructions `S` | Instructions `L` | `L`/`S` |
|---|---:|---:|---:|
| `grib1-second-order/decode` | 3,295,767 | 12,094,734 | 3.67 |
| `grib1-simple/decode` | 3,240,317 | 12,618,075 | 3.89 |
| `grib1-spectral-complex/decode` | 122,298,208 | 253,302,993 | 2.07 |
| `grib1-spectral-simple/decode` | 122,198,642 | 253,055,313 | 2.07 |
| `grib2-5.0/decode` | 3,239,031 | 12,618,487 | 3.90 |
| `grib2-5.0/warp` | 7,033,602 | 27,732,076 | 3.94 |
| `grib2-5.0/render` | 1,808,123 | 6,643,423 | 3.67 |
| `grib2-5.0/contours` | 4,017,784 | 15,311,053 | 3.81 |
| `grib2-5.3/decode` | 3,952,549 | 14,817,327 | 3.75 |
| `grib2-5.40/decode` | 34,526,283 | 114,210,514 | 3.31 |
| `grib2-5.40/codec` | 33,110,498 | 108,710,709 | 3.28 |
| `grib2-5.41/decode` | 3,143,882 | 11,506,226 | 3.66 |
| `grib2-5.41/codec` | 1,623,095 | 5,573,115 | 3.43 |
| `grib2-5.42/decode` | 2,540,745 | 9,819,130 | 3.86 |
| `grib2-5.42/codec` | 1,384,020 | 5,199,886 | 3.76 |
| `netcdf-classic/slice` | 1,790,454 | 6,745,709 | 3.77 |
| `netcdf-classic/scrub` | 13,590,448 | 53,113,368 | 3.91 |
| `netcdf4-zlib/slice` | 4,748,791 | 16,805,278 | 3.54 |
| `netcdf4-zlib/scrub` | 36,373,020 | 134,681,524 | 3.70 |
| `netcdf4-zlib/codec` | 1,224,170 | 4,001,217 | 3.27 |
| `netcdf4-zlib-span/slice` | 10,391,808 | 35,845,369 | 3.45 |
| `netcdf4-zlib-span/scrub` | 35,798,327 | 131,906,952 | 3.68 |
| `zarr-v2-blosc/slice` | 4,153,836 | 15,987,359 | 3.85 |
| `zarr-v2-blosc/scrub` | 32,183,477 | 126,336,105 | 3.93 |
| `zarr-v2-blosc/codec` | 1,509,388 | 5,799,324 | 3.84 |
| `zarr-v3-sharded/slice` | 20,337,499 | 83,263,431 | 4.09 |
| `zarr-v3-sharded/scrub` | 160,596,975 | 687,765,499 | 4.28 |
| `zarr-v3-zstd/slice` | 6,333,820 | 24,130,105 | 3.81 |
| `zarr-v3-zstd/scrub` | 48,711,900 | 191,923,435 | 3.94 |
| `zarr-v3-zstd/codec` | 4,075,658 | 15,905,184 | 3.90 |
| `zarr-v3-zstd/warp` | 7,022,957 | 27,721,087 | 3.95 |
| `zarr-v3-zstd/render` | 1,797,487 | 6,634,397 | 3.69 |
| `zarr-v3-zstd/contours` | 3,993,511 | 15,299,743 | 3.83 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,210,514 | 108,710,709 | 5% |
| `grib2-5.40-S` | 34,526,283 | 33,110,498 | 4% |
| `grib2-5.41-L` | 11,506,226 | 5,573,115 | 52% |
| `grib2-5.41-S` | 3,143,882 | 1,623,095 | 48% |
| `grib2-5.42-L` | 9,819,130 | 5,199,886 | 47% |
| `grib2-5.42-S` | 2,540,745 | 1,384,020 | 46% |
| `netcdf4-zlib-D` | 4,781,972 | 1,227,556 | 74% |
| `netcdf4-zlib-L` | 16,805,278 | 4,001,217 | 76% |
| `netcdf4-zlib-S` | 4,748,791 | 1,224,170 | 74% |
| `zarr-v2-blosc-D` | 4,155,135 | 1,510,562 | 64% |
| `zarr-v2-blosc-L` | 15,987,359 | 5,799,324 | 64% |
| `zarr-v2-blosc-S` | 4,153,836 | 1,509,388 | 64% |
| `zarr-v3-zstd-D` | 6,318,005 | 4,091,093 | 35% |
| `zarr-v3-zstd-L` | 24,130,105 | 15,905,184 | 34% |
| `zarr-v3-zstd-S` | 6,333,820 | 4,075,658 | 36% |
<!-- perf-gate:bounds:end -->

## Re-recording

A gated number that moves fails the `Performance` workflow until the table is
re-recorded, and the pull request that re-records it says why it moved.

```sh
crates/fieldglass-perf/run.sh --write
```

- **Exact tiers** (bytes, requests, allocations, peak heap) move only when the
  code, the corpus or the compiler does. The standard library's own allocations
  are counted, so the harness is pinned to one Rust release in
  `crates/fieldglass-perf/rust-toolchain.toml` (`run.sh` builds the wasm bundles
  with it too, and refuses any other). Bumping it is a re-record with the bump
  named as the reason.
- **Instructions** are allowed 2%, or 40,000 instructions, whichever is
  larger. Two full runs on one machine differ by under 0.02% on almost every
  row, but a small operation can jump by a fixed amount: `netcdf-classic-S/place`
  reads 123,558 run alone and 136,757 inside the full run, because malloc takes
  its heap-growth path or not depending on what preparation left behind. The
  floor covers that and nothing bigger: every injected fault in the next section
  moved its rows by far more.
- **wasm memory** is allowed one 64 KiB page, for the same reason.
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
- **Ratios to the reference tools** over the same bytes, from memory: eccodes
  for GRIB, netCDF4 for NetCDF, zarr-python for Zarr, xarray beside zarr-python
  on the real data. Each tool's binding floor (one value through the same call)
  is measured and subtracted, but only where the reference took at least three
  floors; below that the row says "binding-dominated", because subtracting a
  floor from a number no larger than it produces nonsense. Spectral GRIB1 is not
  compared: eccodes returns coefficients and synthesises nothing.
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

- **The `…zarr-v3` store is Zarr v2** (`.zarray`, blosc lz4). Its `.zarray` and
  `.zmetadata` are rewritten daily as ERA5T is appended, so they cannot be pinned
  by hash. The manifest carries an authored `.zarray` with the shape cut to
  twelve frames and the chunk keys renumbered from 0, around the bucket's own
  chunk bytes.
- **The `.grb2` files are GRIB edition 1**, on the native N320 reduced Gaussian
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
expands them to a 1280 × 640 regular grid, so that ratio includes work eccodes
does not do.

ERA5 is Copernicus Climate Change Service information, used under the
Copernicus licence.

## What the first run found

Every one of these was measured, not estimated, and none is fixed here: each
bound violation is its own issue.

1. **NetCDF read and decoded the whole variable for every plane. Resolved for
   decode (#939); the read is not.** A slice's peak heap and instructions grew
   with the variable, not the plane (`D`/`S` 3.3–4.0), at 144–160 B per cell
   against a floor of 5–9. The reader now decodes only the region asked for:
   a classic slice reads its runs, a NetCDF-4 slice inflates the chunks it
   covers, and the `D`/`S` ratios are 1.00 for peak heap and instructions on
   every NetCDF slice and scrub row, at 29 B per cell (the decoded-cell cost of
   finding 2). A classic slice at `S` runs 28% of the old instructions
   (1,786,193 against 6,276,125), a NetCDF-4 one 17% (4,732,123 against
   28,602,616). Measured by hand on a 0.5° hourly `t2m(48, 361, 720)`, release
   build, `Session::decode_slice` per plane went from 84 ms to 1.8 ms (classic)
   and from 241 ms to 4.6 ms (NetCDF-4 zlib, one plane per chunk), against
   2.5 ms for picking a plane out of a whole-variable memo. What remains is
   the bytes. A chunk spanning several time steps is kept decompressed (64 MiB
   a file, least recently used first), so a scrub inflates it once rather than
   once a frame: `netcdf4-zlib-span` scrubs eight frames over two chunks for
   3.4 times one slice's instructions. On `t2m(48, 361, 720)` with
   netCDF-C's default (24, 181, 360) chunks, a 48-frame scrub went from 3.0 s
   without it to 0.25 s, against 0.28 s for the old memo's decode and
   extraction. What remains is
   the bytes: the reader still takes the whole file (no range seam), so every
   NetCDF operation's bytes read is the file, 6× to 1,231× its bound. On ERA5
   a frame took 197 ms against netCDF4's 3.5 ms and needs the other half of
   the day's file.
2. **A decoded cell costs 29 B where the floor is 5.** The readers return
   `Vec<Option<f64>>` (16 B), `pack_values` copies into a `Vec<f64>` (8 B) and
   the mask (1 B), then `Dtype::Auto` narrows to `f32` (4 B) while all three are
   alive. Zarr slices hold 45 B per cell against a floor of 9.
3. **The AEC decoder allocated per block. Resolved (#762).** 5.42 decodes
   allocated 529 times at `S` and 2,054 at `L`, and the external decoder alone
   accounted for 513 and 2,038. On `fieldglass-aec` (ADR-0012) a codec call
   allocates once at both sizes (its output buffer) and a decode 16 times at
   both, so the allocation bound holds. The codec runs 22% of the old
   instructions at `L` (5,249,124 against 23,764,511) and the whole decode 32%
   (9,788,554 against 30,573,243), because samples now scale straight into the
   output instead of passing through a byte buffer.
4. **JPEG 2000's cost is the codec's.** 95–96% of a 5.40 decode's instructions
   are in `rust_j2k`, which also allocates more as the grid grows (413 → 551).
   It is 3.7× eccodes (OpenJPEG) at 1°. This answers the issue's question: the
   ceiling is the codec's, not the decode path's.
5. **NetCDF-4 slices spent 97–99% of their instructions outside zlib**, which
   was finding 1 again from the other side. With region decode it is 74–76%,
   most of it the decoded-cell cost of finding 2.
6. **A sharded Zarr slice decodes every chunk in its shard.** Fetching the whole
   shard is the bound (an object source fetches whole objects), but decoding
   it is not: 120 B per cell against 9, and allocations grow with cells (396 → 550).
7. **Zarr v2 blosc on ERA5 is 10.7× zarr-python** (30.7 ms against 2.9 ms per
   frame). Two thirds of a blosc slice's instructions are outside the codec.
8. **Contours hold 17 B per cell** where the floor is a row of state, and
   allocate a few more times as the grid grows.
9. **Opening a Zarr store costs more as the variable grows** (allocations and
   instructions `D`/`S` up to 1.20): the walk does work per chunk key.
10. **`+simd128` buys nothing measurable**, and spectral synthesis is 13× slower
    under wasm than native, against 2–4× for everything else.
