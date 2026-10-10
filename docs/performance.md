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
| `grib1-second-order-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,722 | 27,784 | 1,638,400 | 1,638,400 |
| `grib1-second-order-L/decode` | 65,160 | 4 | 26,524 | 26,620 | 1 | 20 | 2,106,981 | 12,094,743 | 17,640,544 | 3,735,552 | 3,735,552 |
| `grib1-second-order-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,143 | 30,358 | — | — |
| `grib1-second-order-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,553 | 27,535 | 1,638,400 | 1,638,400 |
| `grib1-second-order-S/decode` | 16,380 | 4 | 10,454 | 10,550 | 1 | 20 | 532,188 | 3,295,776 | 4,664,992 | 2,293,760 | 2,293,760 |
| `grib1-second-order-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,013 | 30,143 | — | — |
| `grib1-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,647 | 27,662 | 1,835,008 | 1,835,008 |
| `grib1-simple-L/decode` | 65,160 | 4 | 130,332 | 130,428 | 1 | 15 | 1,889,666 | 12,618,084 | 17,660,636 | 3,670,016 | 3,670,016 |
| `grib1-simple-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,244 | 30,534 | — | — |
| `grib1-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 16,105 | 28,440 | 1,703,936 | 1,703,936 |
| `grib1-simple-S/decode` | 16,380 | 4 | 32,772 | 32,868 | 1 | 15 | 475,046 | 3,240,326 | 4,480,185 | 2,162,688 | 2,162,688 |
| `grib1-simple-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 16,108 | 30,272 | — | — |
| `grib1-spectral-complex-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,512 | 27,334 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-L/decode` | 259,920 | 8 | 33,966 | 34,062 | 1 | 24 | 6,498,082 | 253,298,777 | 362,289,887 | 10,551,296 | 10,551,296 |
| `grib1-spectral-complex-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,913 | 28,881 | — | — |
| `grib1-spectral-complex-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,487 | 27,309 | 1,638,400 | 1,638,400 |
| `grib1-spectral-complex-S/decode` | 259,920 | 8 | 9,262 | 9,358 | 1 | 24 | 6,498,082 | 122,296,104 | 180,339,980 | 9,109,504 | 9,109,504 |
| `grib1-spectral-complex-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,910 | 28,874 | — | — |
| `grib1-spectral-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,487 | 27,305 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-L/decode` | 259,920 | 8 | 33,038 | 33,134 | 1 | 23 | 6,498,082 | 253,051,097 | 361,887,333 | 10,551,296 | 10,551,296 |
| `grib1-spectral-simple-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,974 | 28,914 | — | — |
| `grib1-spectral-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,380 | 27,125 | 1,638,400 | 1,638,400 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8 | 8,334 | 8,430 | 1 | 23 | 6,498,082 | 122,196,538 | 180,194,963 | 9,109,504 | 9,109,504 |
| `grib1-spectral-simple-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 16,186 | 29,214 | — | — |
| `grib2-5.0-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 85,324 | 133,988 | 1,900,544 | 1,900,544 |
| `grib2-5.0-L/decode` | 65,160 | 4 | 130,331 | 130,499 | 1 | 16 | 1,889,658 | 12,618,475 | 17,659,715 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 17,463 | 32,327 | — | — |
| `grib2-5.0-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,732,075 | 38,484,469 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 141,053 | 199,556 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,643,361 | 9,056,535 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/contours` | 65,160 | — | 0 | 0 | 0 | 54 | 1,165,776 | 15,310,991 | 19,025,045 | 3,735,552 | 3,735,552 |
| `grib2-5.0-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 83,873 | 131,555 | 1,703,936 | 1,703,936 |
| `grib2-5.0-S/decode` | 16,380 | 4 | 32,771 | 32,939 | 1 | 16 | 475,038 | 3,239,041 | 4,484,828 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,559 | 30,967 | — | — |
| `grib2-5.0-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,033,540 | 9,769,895 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 140,542 | 198,745 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,808,050 | 2,494,817 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/contours` | 16,380 | — | 0 | 0 | 0 | 49 | 332,048 | 4,017,783 | 5,053,377 | 2,162,688 | 2,162,688 |
| `grib2-5.200-S/open` | 0 | — | 211 | 211 | 7 | 4 | 2,090 | 109,058 | 169,116 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/decode` | 496 | 4 | 20 | 211 | 1 | 18 | 14,402 | 141,199 | 221,621 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/place` | 0 | — | 0 | 211 | 0 | 4 | 70 | 17,077 | 31,776 | — | — |
| `grib2-5.3-L/open` | 0 | — | 403 | 783 | 7 | 3 | 2,080 | 83,534 | 131,303 | 1,703,936 | 1,703,936 |
| `grib2-5.3-L/decode` | 65,160 | 4 | 57,954 | 58,150 | 1 | 18 | 1,889,658 | 14,817,315 | 20,547,780 | 3,604,480 | 3,604,480 |
| `grib2-5.3-L/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,850 | 31,452 | — | — |
| `grib2-5.3-S/open` | 0 | — | 403 | 783 | 7 | 3 | 2,080 | 83,133 | 130,736 | 1,638,400 | 1,638,400 |
| `grib2-5.3-S/decode` | 16,380 | 4 | 18,586 | 18,782 | 1 | 18 | 475,038 | 3,952,559 | 5,443,992 | 2,097,152 | 2,097,152 |
| `grib2-5.3-S/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,438 | 30,744 | — | — |
| `grib2-5.40-L/open` | 0 | — | 403 | 757 | 7 | 3 | 2,080 | 82,982 | 130,357 | 1,638,400 | 1,638,400 |
| `grib2-5.40-L/decode` | 65,160 | 4 | 14,681 | 14,851 | 1 | 567 | 1,889,658 | 114,211,356 | 146,554,134 | 3,866,624 | 3,866,624 |
| `grib2-5.40-L/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,713 | 31,123 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | — | 551 | 859,764 | 108,710,708 | 137,671,632 | — | — |
| `grib2-5.40-S/open` | 0 | — | 403 | 757 | 7 | 3 | 2,080 | 82,951 | 130,321 | 1,638,400 | 1,638,400 |
| `grib2-5.40-S/decode` | 16,380 | 4 | 6,278 | 6,448 | 1 | 429 | 475,038 | 34,526,186 | 44,486,124 | 2,162,688 | 2,162,688 |
| `grib2-5.40-S/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 17,192 | 31,871 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | — | 413 | 221,460 | 33,110,497 | 42,271,814 | — | — |
| `grib2-5.41-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 82,190 | 129,265 | 1,835,008 | 1,835,008 |
| `grib2-5.41-L/decode` | 65,160 | 4 | 80,490 | 80,658 | 1 | 25 | 1,898,632 | 11,506,236 | 16,791,608 | 3,735,552 | 3,735,552 |
| `grib2-5.41-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,466 | 30,727 | — | — |
| `grib2-5.41-L/codec` | 65,160 | — | — | — | — | 9 | 334,744 | 5,573,114 | 7,447,976 | — | — |
| `grib2-5.41-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,080 | 82,009 | 128,961 | 1,638,400 | 1,638,400 |
| `grib2-5.41-S/decode` | 16,380 | 4 | 23,501 | 23,669 | 1 | 25 | 511,568 | 3,143,892 | 4,578,568 | 2,293,760 | 2,293,760 |
| `grib2-5.41-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,762 | 31,170 | — | — |
| `grib2-5.41-S/codec` | 16,380 | — | — | — | — | 9 | 118,400 | 1,623,094 | 2,220,341 | — | — |
| `grib2-5.42-L/open` | 0 | — | 403 | 759 | 7 | 3 | 2,080 | 81,758 | 128,663 | 1,835,008 | 1,835,008 |
| `grib2-5.42-L/decode` | 65,160 | 4 | 80,767 | 80,939 | 1 | 16 | 1,889,658 | 9,819,153 | 13,874,398 | 3,670,016 | 3,670,016 |
| `grib2-5.42-L/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,746 | 31,140 | — | — |
| `grib2-5.42-L/codec` | 65,160 | — | — | — | — | 1 | 130,320 | 5,199,885 | 6,325,425 | — | — |
| `grib2-5.42-S/open` | 0 | — | 403 | 759 | 7 | 3 | 2,080 | 81,551 | 128,337 | 1,638,400 | 1,638,400 |
| `grib2-5.42-S/decode` | 16,380 | 4 | 22,449 | 22,621 | 1 | 16 | 475,038 | 2,540,755 | 3,541,647 | 2,097,152 | 2,097,152 |
| `grib2-5.42-S/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,782 | 31,247 | — | — |
| `grib2-5.42-S/codec` | 16,380 | — | — | — | — | 1 | 32,760 | 1,384,019 | 1,709,764 | — | — |
| `netcdf-classic-D/open` | 0 | — | 2,098,344 | 1,704 | 1 | 102 | 4,982 | 37,747 | 66,315 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/variables` | 0 | — | 2,098,344 | 1,704 | 1 | 41 | 1,290 | 27,747 | 42,155 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/slice` | 16,380 | 4 | 2,098,344 | 67,224 | 1 | 104 | 476,012 | 1,790,617 | 2,687,557 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/scrub` | 16,380 | 4 | 2,098,344 | 525,864 | 1 | 559 | 476,012 | 13,587,466 | 18,926,100 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/place` | 0 | — | 2,098,344 | 1,704 | 1 | 85 | 6,179 | 122,832 | 189,229 | — | — |
| `netcdf-classic-L/open` | 0 | — | 2,087,712 | 2,592 | 1 | 102 | 4,982 | 38,019 | 66,755 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/variables` | 0 | — | 2,087,712 | 2,592 | 1 | 41 | 1,290 | 13,381 | 20,467 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/slice` | 65,160 | 4 | 2,087,712 | 263,232 | 1 | 104 | 1,890,632 | 6,745,633 | 10,102,812 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/scrub` | 65,160 | 4 | 2,087,712 | 2,087,712 | 1 | 559 | 1,890,632 | 53,113,362 | 74,191,395 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/place` | 0 | — | 2,087,712 | 2,592 | 1 | 85 | 11,219 | 138,858 | 209,700 | — | — |
| `netcdf-classic-S/open` | 0 | — | 525,672 | 1,512 | 1 | 102 | 4,982 | 37,880 | 66,536 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/variables` | 0 | — | 525,672 | 1,512 | 1 | 41 | 1,290 | 13,381 | 20,539 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/slice` | 16,380 | 4 | 525,672 | 67,032 | 1 | 104 | 476,012 | 1,790,378 | 2,680,913 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/scrub` | 16,380 | 4 | 525,672 | 525,672 | 1 | 559 | 476,012 | 13,590,442 | 18,924,566 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/place` | 0 | — | 525,672 | 1,512 | 1 | 85 | 6,179 | 122,578 | 188,848 | — | — |
| `netcdf4-zlib-D/open` | 0 | — | 1,116,265 | 12,942 | 1 | 378 | 14,991 | 352,837 | 518,929 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/variables` | 0 | — | 1,116,265 | 12,942 | 1 | 41 | 1,290 | 13,404 | 20,486 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/slice` | 16,380 | 4 | 1,116,265 | 47,445 | 1 | 257 | 544,324 | 4,778,710 | 6,905,814 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/scrub` | 16,380 | 4 | 1,116,265 | 288,808 | 1 | 1,008 | 1,003,736 | 36,405,037 | 50,999,540 | 4,128,768 | 4,128,768 |
| `netcdf4-zlib-D/place` | 0 | — | 1,116,265 | 12,942 | 1 | 155 | 6,179 | 221,797 | 325,360 | — | — |
| `netcdf4-zlib-D/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,227,555 | 1,666,873 | — | — |
| `netcdf4-zlib-L/open` | 0 | — | 1,006,286 | 14,990 | 1 | 378 | 14,991 | 350,932 | 515,841 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/variables` | 0 | — | 1,006,286 | 14,990 | 1 | 41 | 1,290 | 13,379 | 20,445 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/slice` | 65,160 | 4 | 1,006,286 | 139,063 | 1 | 232 | 2,152,528 | 16,801,176 | 24,097,058 | 5,242,880 | 5,242,880 |
| `netcdf4-zlib-L/scrub` | 65,160 | 4 | 1,006,286 | 1,006,286 | 1 | 990 | 3,977,780 | 134,659,338 | 189,379,365 | 8,388,608 | 8,388,608 |
| `netcdf4-zlib-L/place` | 0 | — | 1,006,286 | 14,990 | 1 | 155 | 11,219 | 250,365 | 364,015 | — | — |
| `netcdf4-zlib-L/codec` | 65,160 | — | — | — | — | 3 | 506,796 | 4,001,216 | 5,663,421 | — | — |
| `netcdf4-zlib-S/open` | 0 | — | 288,808 | 12,942 | 1 | 378 | 14,991 | 352,188 | 517,497 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/variables` | 0 | — | 288,808 | 12,942 | 1 | 41 | 1,290 | 13,380 | 20,454 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/slice` | 16,380 | 4 | 288,808 | 47,445 | 1 | 231 | 542,788 | 4,745,193 | 6,857,910 | 2,490,368 | 2,490,368 |
| `netcdf4-zlib-S/scrub` | 16,380 | 4 | 288,808 | 288,808 | 1 | 982 | 1,002,200 | 36,365,126 | 50,943,196 | 3,014,656 | 3,014,656 |
| `netcdf4-zlib-S/place` | 0 | — | 288,808 | 12,942 | 1 | 155 | 6,179 | 220,804 | 323,908 | — | — |
| `netcdf4-zlib-S/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,224,169 | 1,660,286 | — | — |
| `netcdf4-zlib-span-D/open` | 0 | — | 1,088,484 | 12,942 | 1 | 378 | 14,991 | 352,167 | 517,341 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/variables` | 0 | — | 1,088,484 | 12,942 | 1 | 41 | 1,290 | 13,371 | 20,439 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/slice` | 16,380 | 4 | 1,088,484 | 147,279 | 1 | 231 | 739,348 | 10,397,596 | 14,716,662 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/scrub` | 16,380 | 4 | 1,088,484 | 281,949 | 1 | 944 | 1,001,468 | 35,797,135 | 49,728,576 | 3,801,088 | 3,801,088 |
| `netcdf4-zlib-span-D/place` | 0 | — | 1,088,484 | 12,942 | 1 | 155 | 6,179 | 220,746 | 324,124 | — | — |
| `netcdf4-zlib-span-L/open` | 0 | — | 985,072 | 14,990 | 1 | 378 | 14,991 | 349,604 | 513,854 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-span-L/variables` | 0 | — | 985,072 | 14,990 | 1 | 41 | 1,290 | 13,388 | 20,464 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-span-L/slice` | 65,160 | 4 | 985,072 | 499,406 | 1 | 225 | 3,242,339 | 35,841,891 | 51,332,012 | 7,667,712 | 7,667,712 |
| `netcdf4-zlib-span-L/scrub` | 65,160 | 4 | 985,072 | 985,072 | 1 | 939 | 4,290,860 | 131,902,071 | 186,323,668 | 8,716,288 | 8,716,288 |
| `netcdf4-zlib-span-L/place` | 0 | — | 985,072 | 14,990 | 1 | 155 | 11,219 | 249,374 | 362,843 | — | — |
| `netcdf4-zlib-span-S/open` | 0 | — | 281,949 | 12,942 | 1 | 378 | 14,991 | 351,907 | 517,242 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-span-S/variables` | 0 | — | 281,949 | 12,942 | 1 | 41 | 1,290 | 13,371 | 20,455 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-span-S/slice` | 16,380 | 4 | 281,949 | 147,279 | 1 | 224 | 739,044 | 10,388,359 | 14,703,100 | 2,752,512 | 2,752,512 |
| `netcdf4-zlib-span-S/scrub` | 16,380 | 4 | 281,949 | 281,949 | 1 | 937 | 1,001,164 | 35,792,663 | 49,722,001 | 3,014,656 | 3,014,656 |
| `netcdf4-zlib-span-S/place` | 0 | — | 281,949 | 12,942 | 1 | 155 | 6,179 | 219,713 | 322,412 | — | — |
| `zarr-v2-blosc-D/open` | 0 | — | 1,158 | 1,584 | 4 | 370 | 14,038 | 288,331 | 456,906 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | — | 0 | 1,584 | 0 | 35 | 1,186 | 10,903 | 17,056 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 4 | 37,996 | 39,154 | 3 | 173 | 656,211 | 4,155,047 | 6,444,684 | — | — |
| `zarr-v2-blosc-D/scrub` | 16,380 | 4 | 300,515 | 301,673 | 10 | 782 | 656,584 | 32,183,510 | 47,596,670 | — | — |
| `zarr-v2-blosc-D/place` | 0 | — | 426 | 1,584 | 2 | 123 | 9,347 | 164,587 | 257,945 | — | — |
| `zarr-v2-blosc-D/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,510,561 | 2,415,599 | — | — |
| `zarr-v2-blosc-L/open` | 0 | — | 1,161 | 1,900 | 4 | 321 | 14,038 | 250,007 | 404,086 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | — | 0 | 1,900 | 0 | 35 | 1,186 | 10,903 | 17,068 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 4 | 144,560 | 145,721 | 3 | 176 | 2,607,411 | 15,987,259 | 25,162,986 | — | — |
| `zarr-v2-blosc-L/scrub` | 65,160 | 4 | 1,141,368 | 1,142,529 | 10 | 792 | 2,607,784 | 126,336,063 | 187,266,109 | — | — |
| `zarr-v2-blosc-L/place` | 0 | — | 739 | 1,900 | 2 | 125 | 17,267 | 209,523 | 318,414 | — | — |
| `zarr-v2-blosc-L/codec` | 65,160 | — | — | — | — | 6 | 781,920 | 5,799,323 | 9,341,600 | — | — |
| `zarr-v2-blosc-S/open` | 0 | — | 1,157 | 1,583 | 4 | 321 | 14,038 | 249,585 | 403,344 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | — | 0 | 1,583 | 0 | 35 | 1,186 | 10,885 | 17,056 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 4 | 37,996 | 39,153 | 3 | 173 | 656,211 | 4,153,754 | 6,442,679 | — | — |
| `zarr-v2-blosc-S/scrub` | 16,380 | 4 | 300,515 | 301,672 | 10 | 782 | 656,584 | 32,183,631 | 47,591,773 | — | — |
| `zarr-v2-blosc-S/place` | 0 | — | 426 | 1,583 | 2 | 123 | 9,347 | 163,726 | 256,837 | — | — |
| `zarr-v2-blosc-S/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,509,387 | 2,406,948 | — | — |
| `zarr-v3-sharded-D/open` | 0 | — | 2,800 | 3,413 | 3 | 393 | 35,909 | 294,413 | 462,059 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | — | 0 | 3,413 | 0 | 35 | 1,186 | 10,825 | 16,971 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 4 | 139,799 | 142,599 | 3 | 403 | 1,967,139 | 20,334,837 | 32,255,138 | — | — |
| `zarr-v3-sharded-D/scrub` | 16,380 | 4 | 278,830 | 281,630 | 10 | 2,300 | 1,967,512 | 160,603,920 | 248,265,197 | — | — |
| `zarr-v3-sharded-D/place` | 0 | — | 613 | 3,413 | 2 | 169 | 17,437 | 316,475 | 480,692 | — | — |
| `zarr-v3-sharded-L/open` | 0 | — | 2,804 | 4,019 | 3 | 380 | 35,909 | 288,207 | 453,178 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | — | 0 | 4,019 | 0 | 35 | 1,186 | 10,798 | 16,922 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 4 | 526,253 | 529,057 | 3 | 557 | 7,820,739 | 83,263,348 | 133,345,057 | — | — |
| `zarr-v3-sharded-L/scrub` | 65,160 | 4 | 1,058,791 | 1,061,595 | 10 | 3,524 | 7,821,112 | 687,765,633 | 1,069,645,108 | — | — |
| `zarr-v3-sharded-L/place` | 0 | — | 1,215 | 4,019 | 2 | 173 | 24,632 | 411,018 | 608,528 | — | — |
| `zarr-v3-sharded-S/open` | 0 | — | 2,799 | 3,412 | 3 | 380 | 35,909 | 287,005 | 451,229 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | — | 0 | 3,412 | 0 | 35 | 1,186 | 10,786 | 16,922 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 4 | 139,799 | 142,598 | 3 | 403 | 1,967,139 | 20,337,460 | 32,259,586 | — | — |
| `zarr-v3-sharded-S/scrub` | 16,380 | 4 | 278,830 | 281,629 | 10 | 2,300 | 1,967,512 | 160,597,154 | 248,266,097 | — | — |
| `zarr-v3-sharded-S/place` | 0 | — | 613 | 3,412 | 2 | 169 | 17,437 | 314,131 | 476,927 | — | — |
| `zarr-v3-zstd-D/open` | 0 | — | 2,186 | 2,799 | 3 | 387 | 32,208 | 316,444 | 488,816 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | — | 0 | 2,799 | 0 | 35 | 1,186 | 10,789 | 16,949 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 4 | 52,381 | 54,567 | 3 | 242 | 656,219 | 6,317,933 | 9,595,957 | — | — |
| `zarr-v3-zstd-D/scrub` | 16,380 | 4 | 414,719 | 416,905 | 10 | 1,030 | 656,592 | 48,716,040 | 71,602,418 | — | — |
| `zarr-v3-zstd-D/place` | 0 | — | 613 | 2,799 | 2 | 169 | 17,437 | 314,064 | 477,897 | — | — |
| `zarr-v3-zstd-D/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,091,096 | 6,015,643 | — | — |
| `zarr-v3-zstd-L/open` | 0 | — | 2,189 | 3,404 | 3 | 337 | 32,208 | 264,771 | 418,323 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | — | 0 | 3,404 | 0 | 35 | 1,186 | 10,780 | 16,888 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 4 | 207,207 | 209,396 | 3 | 260 | 2,607,419 | 24,130,070 | 37,156,133 | — | — |
| `zarr-v3-zstd-L/scrub` | 65,160 | 4 | 1,645,094 | 1,647,283 | 10 | 1,119 | 2,607,792 | 191,923,500 | 282,312,236 | — | — |
| `zarr-v3-zstd-L/place` | 0 | — | 1,215 | 3,404 | 2 | 173 | 24,632 | 408,875 | 606,155 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,721,086 | 38,467,488 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 131,058 | 184,270 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,634,396 | 9,042,184 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | — | 0 | 0 | 0 | 55 | 1,182,160 | 15,299,742 | 19,007,734 | — | — |
| `zarr-v3-zstd-L/codec` | 65,160 | — | — | — | — | 43 | 769,444 | 15,905,189 | 23,342,951 | — | — |
| `zarr-v3-zstd-S/open` | 0 | — | 2,185 | 2,798 | 3 | 337 | 32,208 | 257,649 | 407,302 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | — | 0 | 2,798 | 0 | 35 | 1,186 | 10,780 | 16,896 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 4 | 52,381 | 54,566 | 3 | 242 | 656,219 | 6,333,743 | 9,646,374 | — | — |
| `zarr-v3-zstd-S/scrub` | 16,380 | 4 | 414,719 | 416,904 | 10 | 1,030 | 656,592 | 48,712,021 | 71,602,119 | — | — |
| `zarr-v3-zstd-S/place` | 0 | — | 613 | 2,798 | 2 | 169 | 17,437 | 312,459 | 475,490 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,022,956 | 9,716,675 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 130,094 | 182,788 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,797,486 | 2,446,726 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | — | 0 | 0 | 0 | 48 | 323,856 | 3,993,510 | 4,961,578 | — | — |
| `zarr-v3-zstd-S/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,075,661 | 5,993,046 | — | — |
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
| `netcdf4-zlib/slice` | 231 | 232 | grows +1 |
| `netcdf4-zlib/scrub` | 982 | 990 | grows +8 |
| `netcdf4-zlib/place` | 155 | 155 | holds |
| `netcdf4-zlib/codec` | 2 | 3 | grows +1 |
| `netcdf4-zlib-span/open` | 378 | 378 | holds |
| `netcdf4-zlib-span/variables` | 41 | 41 | holds |
| `netcdf4-zlib-span/slice` | 224 | 225 | grows +1 |
| `netcdf4-zlib-span/scrub` | 937 | 939 | grows +2 |
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
| `netcdf4-zlib/slice` | 33.0 B | 2,248 B | 9 B | 3.7× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 61.0 B | 3,020 B | 9 B | 6.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib-span/slice` | 51.3 B | -101,546 B | 21 B | 2.4× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
| `netcdf4-zlib-span/scrub` | 67.4 B | -103,494 B | 21 B | 3.2× | output value (4) + mask (1) + one decompressed chunk of four f32 planes (16) |
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
| `grib1-second-order/decode` | 3,295,776 | 12,094,743 | 3.67 |
| `grib1-simple/decode` | 3,240,326 | 12,618,084 | 3.89 |
| `grib1-spectral-complex/decode` | 122,296,104 | 253,298,777 | 2.07 |
| `grib1-spectral-simple/decode` | 122,196,538 | 253,051,097 | 2.07 |
| `grib2-5.0/decode` | 3,239,041 | 12,618,475 | 3.90 |
| `grib2-5.0/warp` | 7,033,540 | 27,732,075 | 3.94 |
| `grib2-5.0/render` | 1,808,050 | 6,643,361 | 3.67 |
| `grib2-5.0/contours` | 4,017,783 | 15,310,991 | 3.81 |
| `grib2-5.3/decode` | 3,952,559 | 14,817,315 | 3.75 |
| `grib2-5.40/decode` | 34,526,186 | 114,211,356 | 3.31 |
| `grib2-5.40/codec` | 33,110,497 | 108,710,708 | 3.28 |
| `grib2-5.41/decode` | 3,143,892 | 11,506,236 | 3.66 |
| `grib2-5.41/codec` | 1,623,094 | 5,573,114 | 3.43 |
| `grib2-5.42/decode` | 2,540,755 | 9,819,153 | 3.86 |
| `grib2-5.42/codec` | 1,384,019 | 5,199,885 | 3.76 |
| `netcdf-classic/slice` | 1,790,378 | 6,745,633 | 3.77 |
| `netcdf-classic/scrub` | 13,590,442 | 53,113,362 | 3.91 |
| `netcdf4-zlib/slice` | 4,745,193 | 16,801,176 | 3.54 |
| `netcdf4-zlib/scrub` | 36,365,126 | 134,659,338 | 3.70 |
| `netcdf4-zlib/codec` | 1,224,169 | 4,001,216 | 3.27 |
| `netcdf4-zlib-span/slice` | 10,388,359 | 35,841,891 | 3.45 |
| `netcdf4-zlib-span/scrub` | 35,792,663 | 131,902,071 | 3.69 |
| `zarr-v2-blosc/slice` | 4,153,754 | 15,987,259 | 3.85 |
| `zarr-v2-blosc/scrub` | 32,183,631 | 126,336,063 | 3.93 |
| `zarr-v2-blosc/codec` | 1,509,387 | 5,799,323 | 3.84 |
| `zarr-v3-sharded/slice` | 20,337,460 | 83,263,348 | 4.09 |
| `zarr-v3-sharded/scrub` | 160,597,154 | 687,765,633 | 4.28 |
| `zarr-v3-zstd/slice` | 6,333,743 | 24,130,070 | 3.81 |
| `zarr-v3-zstd/scrub` | 48,712,021 | 191,923,500 | 3.94 |
| `zarr-v3-zstd/codec` | 4,075,661 | 15,905,189 | 3.90 |
| `zarr-v3-zstd/warp` | 7,022,956 | 27,721,086 | 3.95 |
| `zarr-v3-zstd/render` | 1,797,486 | 6,634,396 | 3.69 |
| `zarr-v3-zstd/contours` | 3,993,510 | 15,299,742 | 3.83 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,211,356 | 108,710,708 | 5% |
| `grib2-5.40-S` | 34,526,186 | 33,110,497 | 4% |
| `grib2-5.41-L` | 11,506,236 | 5,573,114 | 52% |
| `grib2-5.41-S` | 3,143,892 | 1,623,094 | 48% |
| `grib2-5.42-L` | 9,819,153 | 5,199,885 | 47% |
| `grib2-5.42-S` | 2,540,755 | 1,384,019 | 46% |
| `netcdf4-zlib-D` | 4,778,710 | 1,227,555 | 74% |
| `netcdf4-zlib-L` | 16,801,176 | 4,001,216 | 76% |
| `netcdf4-zlib-S` | 4,745,193 | 1,224,169 | 74% |
| `zarr-v2-blosc-D` | 4,155,047 | 1,510,561 | 64% |
| `zarr-v2-blosc-L` | 15,987,259 | 5,799,323 | 64% |
| `zarr-v2-blosc-S` | 4,153,754 | 1,509,387 | 64% |
| `zarr-v3-zstd-D` | 6,317,933 | 4,091,096 | 35% |
| `zarr-v3-zstd-L` | 24,130,070 | 15,905,189 | 34% |
| `zarr-v3-zstd-S` | 6,333,743 | 4,075,661 | 36% |
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
