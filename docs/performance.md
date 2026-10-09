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
| NetCDF | classic; NetCDF-4 chunked one step per chunk with zlib and shuffle |
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

Corpus digest: `b0779e8680df78e24b2936c65df6c11125507e0250d2b1720be6dfbc715353be`

A pull request that moves any number here fails the `perf` job until the table
is re-recorded with `crates/fieldglass-perf/run.sh --write`, and the pull request
says why the number moved. Both directions: an improvement nobody records is
lost to the next regression.

<!-- perf-gate:table:begin — regenerate with tools/check_perf_gate.py --write -->
| Scenario | Cells | Value bytes | Bytes read | Bound | Requests | Allocations | Peak heap | Instructions | Est. cycles | wasm memory | wasm +simd128 memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `grib1-second-order-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,938 | 26,588 | 1,638,400 | 1,638,400 |
| `grib1-second-order-L/decode` | 65,160 | 4 | 26,524 | 26,620 | 1 | 20 | 2,106,981 | 12,090,161 | 17,629,775 | 3,735,552 | 3,735,552 |
| `grib1-second-order-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,518 | 29,364 | — | — |
| `grib1-second-order-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,906 | 26,553 | 1,638,400 | 1,638,400 |
| `grib1-second-order-S/decode` | 16,380 | 4 | 10,454 | 10,550 | 1 | 20 | 532,188 | 3,303,609 | 4,678,923 | 2,293,760 | 2,293,760 |
| `grib1-second-order-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,534 | 29,397 | — | — |
| `grib1-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,852 | 26,493 | 1,835,008 | 1,835,008 |
| `grib1-simple-L/decode` | 65,160 | 4 | 130,332 | 130,428 | 1 | 15 | 1,889,666 | 12,613,804 | 17,650,186 | 3,670,016 | 3,670,016 |
| `grib1-simple-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,559 | 29,466 | — | — |
| `grib1-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,363 | 27,249 | 1,703,936 | 1,703,936 |
| `grib1-simple-S/decode` | 16,380 | 4 | 32,772 | 32,868 | 1 | 15 | 475,046 | 3,236,130 | 4,471,709 | 2,162,688 | 2,162,688 |
| `grib1-simple-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,530 | 29,414 | — | — |
| `grib1-spectral-complex-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,035 | 26,548 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-L/decode` | 259,920 | 8 | 33,966 | 34,062 | 1 | 24 | 6,498,082 | 253,298,826 | 362,307,887 | 10,551,296 | 10,551,296 |
| `grib1-spectral-complex-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,459 | 28,133 | — | — |
| `grib1-spectral-complex-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,048 | 26,566 | 1,638,400 | 1,638,400 |
| `grib1-spectral-complex-S/decode` | 259,920 | 8 | 9,262 | 9,358 | 1 | 24 | 6,498,082 | 122,294,040 | 180,345,098 | 9,109,504 | 9,109,504 |
| `grib1-spectral-complex-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,874 | 28,750 | — | — |
| `grib1-spectral-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,010 | 26,515 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-L/decode` | 259,920 | 8 | 33,038 | 33,134 | 1 | 23 | 6,498,082 | 253,051,086 | 361,905,151 | 10,551,296 | 10,551,296 |
| `grib1-spectral-simple-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,848 | 28,735 | — | — |
| `grib1-spectral-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,072 | 26,582 | 1,638,400 | 1,638,400 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8 | 8,334 | 8,430 | 1 | 23 | 6,498,082 | 122,194,580 | 180,198,612 | 9,109,504 | 9,109,504 |
| `grib1-spectral-simple-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,881 | 28,717 | — | — |
| `grib2-5.0-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 80,358 | 126,402 | 1,900,544 | 1,900,544 |
| `grib2-5.0-L/decode` | 65,160 | 4 | 130,331 | 130,499 | 1 | 16 | 1,889,658 | 12,613,796 | 17,649,266 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 15,978 | 30,062 | — | — |
| `grib2-5.0-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,727,500 | 38,477,284 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 137,565 | 194,566 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,641,434 | 9,054,772 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/contours` | 65,160 | — | 0 | 0 | 0 | 54 | 1,165,776 | 15,299,721 | 18,995,567 | 3,735,552 | 3,735,552 |
| `grib2-5.0-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 79,973 | 125,829 | 1,703,936 | 1,703,936 |
| `grib2-5.0-S/decode` | 16,380 | 4 | 32,771 | 32,939 | 1 | 16 | 475,038 | 3,235,590 | 4,470,095 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,694 | 31,084 | — | — |
| `grib2-5.0-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,029,801 | 9,756,676 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 136,414 | 192,670 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,804,414 | 2,489,526 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/contours` | 16,380 | — | 0 | 0 | 0 | 49 | 332,048 | 4,014,815 | 5,049,366 | 2,162,688 | 2,162,688 |
| `grib2-5.200-S/open` | 0 | — | 211 | 211 | 7 | 4 | 2,058 | 79,549 | 125,370 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/decode` | 496 | 4 | 20 | 211 | 1 | 18 | 14,402 | 124,277 | 197,315 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/place` | 0 | — | 0 | 211 | 0 | 4 | 70 | 15,983 | 30,073 | — | — |
| `grib2-5.3-L/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 78,800 | 124,144 | 1,703,936 | 1,703,936 |
| `grib2-5.3-L/decode` | 65,160 | 4 | 57,954 | 58,150 | 1 | 18 | 1,889,658 | 14,812,994 | 20,537,589 | 3,604,480 | 3,604,480 |
| `grib2-5.3-L/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 15,784 | 29,811 | — | — |
| `grib2-5.3-S/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 78,361 | 123,538 | 1,638,400 | 1,638,400 |
| `grib2-5.3-S/decode` | 16,380 | 4 | 18,586 | 18,782 | 1 | 18 | 475,038 | 3,948,298 | 5,433,862 | 2,097,152 | 2,097,152 |
| `grib2-5.3-S/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 16,022 | 30,138 | — | — |
| `grib2-5.40-L/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 78,054 | 122,867 | 1,638,400 | 1,638,400 |
| `grib2-5.40-L/decode` | 65,160 | 4 | 14,681 | 14,851 | 1 | 567 | 1,889,658 | 114,251,929 | 146,637,105 | 3,866,624 | 3,866,624 |
| `grib2-5.40-L/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 15,813 | 29,824 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | — | 551 | 859,764 | 108,726,420 | 137,709,091 | — | — |
| `grib2-5.40-S/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 78,035 | 122,840 | 1,638,400 | 1,638,400 |
| `grib2-5.40-S/decode` | 16,380 | 4 | 6,278 | 6,448 | 1 | 429 | 475,038 | 34,511,992 | 44,454,859 | 2,162,688 | 2,162,688 |
| `grib2-5.40-S/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,350 | 30,636 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | — | 413 | 221,460 | 33,117,772 | 42,275,349 | — | — |
| `grib2-5.41-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 77,811 | 122,505 | 1,835,008 | 1,835,008 |
| `grib2-5.41-L/decode` | 65,160 | 4 | 80,490 | 80,658 | 1 | 25 | 1,898,632 | 11,589,865 | 16,967,135 | 3,735,552 | 3,735,552 |
| `grib2-5.41-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,016 | 30,080 | — | — |
| `grib2-5.41-L/codec` | 65,160 | — | — | — | — | 9 | 334,744 | 5,479,028 | 7,256,052 | — | — |
| `grib2-5.41-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 77,513 | 122,145 | 1,638,400 | 1,638,400 |
| `grib2-5.41-S/decode` | 16,380 | 4 | 23,501 | 23,669 | 1 | 25 | 511,568 | 3,139,673 | 4,565,967 | 2,293,760 | 2,293,760 |
| `grib2-5.41-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,093 | 30,176 | — | — |
| `grib2-5.41-S/codec` | 16,380 | — | — | — | — | 9 | 118,400 | 1,603,326 | 2,181,838 | — | — |
| `grib2-5.42-L/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 77,210 | 121,820 | 1,835,008 | 1,835,008 |
| `grib2-5.42-L/decode` | 65,160 | 4 | 80,767 | 80,939 | 1 | 16 | 1,889,658 | 9,814,970 | 13,865,610 | 3,670,016 | 3,670,016 |
| `grib2-5.42-L/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 15,939 | 29,943 | — | — |
| `grib2-5.42-L/codec` | 65,160 | — | — | — | — | 1 | 130,320 | 5,275,324 | 6,483,538 | — | — |
| `grib2-5.42-S/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 76,903 | 121,245 | 1,638,400 | 1,638,400 |
| `grib2-5.42-S/decode` | 16,380 | 4 | 22,449 | 22,621 | 1 | 16 | 475,038 | 2,536,140 | 3,527,743 | 2,097,152 | 2,097,152 |
| `grib2-5.42-S/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,276 | 30,481 | — | — |
| `grib2-5.42-S/codec` | 16,380 | — | — | — | — | 1 | 32,760 | 1,388,383 | 1,722,100 | — | — |
| `netcdf-classic-D/open` | 0 | — | 2,098,344 | 1,704 | 1 | 102 | 4,982 | 37,395 | 65,683 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/variables` | 0 | — | 2,098,344 | 1,704 | 1 | 41 | 1,290 | 27,167 | 41,295 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/slice` | 16,380 | 4 | 2,098,344 | 67,224 | 1 | 104 | 476,012 | 1,800,390 | 2,695,257 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/scrub` | 16,380 | 4 | 2,098,344 | 525,864 | 1 | 559 | 476,012 | 13,598,217 | 18,935,395 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/place` | 0 | — | 2,098,344 | 1,704 | 1 | 85 | 6,179 | 118,754 | 182,945 | — | — |
| `netcdf-classic-L/open` | 0 | — | 2,087,712 | 2,592 | 1 | 102 | 4,982 | 36,972 | 65,050 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/variables` | 0 | — | 2,087,712 | 2,592 | 1 | 41 | 1,290 | 27,253 | 41,366 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/slice` | 65,160 | 4 | 2,087,712 | 263,232 | 1 | 104 | 1,890,632 | 6,754,461 | 10,114,859 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/scrub` | 65,160 | 4 | 2,087,712 | 2,087,712 | 1 | 559 | 1,890,632 | 53,120,680 | 74,200,980 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/place` | 0 | — | 2,087,712 | 2,592 | 1 | 85 | 11,219 | 148,180 | 223,309 | — | — |
| `netcdf-classic-S/open` | 0 | — | 525,672 | 1,512 | 1 | 102 | 4,982 | 37,304 | 65,655 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/variables` | 0 | — | 525,672 | 1,512 | 1 | 41 | 1,290 | 27,175 | 41,233 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/slice` | 16,380 | 4 | 525,672 | 67,032 | 1 | 104 | 476,012 | 1,786,193 | 2,679,689 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/scrub` | 16,380 | 4 | 525,672 | 525,672 | 1 | 559 | 476,012 | 13,595,299 | 18,936,128 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/place` | 0 | — | 525,672 | 1,512 | 1 | 85 | 6,179 | 118,240 | 182,162 | — | — |
| `netcdf4-zlib-D/open` | 0 | — | 1,116,265 | 12,942 | 1 | 377 | 14,887 | 353,681 | 517,456 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/variables` | 0 | — | 1,116,265 | 12,942 | 1 | 41 | 1,290 | 13,372 | 20,498 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/slice` | 16,380 | 4 | 1,116,265 | 47,445 | 1 | 253 | 478,216 | 4,764,451 | 6,853,619 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/scrub` | 16,380 | 4 | 1,116,265 | 288,808 | 1 | 988 | 478,216 | 36,225,675 | 50,413,130 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/place` | 0 | — | 1,116,265 | 12,942 | 1 | 155 | 6,179 | 220,049 | 322,315 | — | — |
| `netcdf4-zlib-D/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,246,625 | 1,708,698 | — | — |
| `netcdf4-zlib-L/open` | 0 | — | 1,006,286 | 14,990 | 1 | 377 | 14,887 | 351,979 | 515,040 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/variables` | 0 | — | 1,006,286 | 14,990 | 1 | 41 | 1,290 | 13,372 | 20,498 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/slice` | 65,160 | 4 | 1,006,286 | 139,063 | 1 | 228 | 1,891,300 | 16,787,790 | 24,077,000 | 5,242,880 | 5,242,880 |
| `netcdf4-zlib-L/scrub` | 65,160 | 4 | 1,006,286 | 1,006,286 | 1 | 970 | 1,891,300 | 132,636,481 | 183,896,513 | 5,242,880 | 5,242,880 |
| `netcdf4-zlib-L/place` | 0 | — | 1,006,286 | 14,990 | 1 | 155 | 11,219 | 248,006 | 360,292 | — | — |
| `netcdf4-zlib-L/codec` | 65,160 | — | — | — | — | 3 | 506,796 | 3,995,740 | 5,654,925 | — | — |
| `netcdf4-zlib-S/open` | 0 | — | 288,808 | 12,942 | 1 | 377 | 14,887 | 353,186 | 516,788 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/variables` | 0 | — | 288,808 | 12,942 | 1 | 41 | 1,290 | 13,372 | 20,494 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/slice` | 16,380 | 4 | 288,808 | 47,445 | 1 | 227 | 476,680 | 4,732,123 | 6,809,044 | 2,490,368 | 2,490,368 |
| `netcdf4-zlib-S/scrub` | 16,380 | 4 | 288,808 | 288,808 | 1 | 962 | 476,680 | 36,193,880 | 50,370,803 | 2,490,368 | 2,490,368 |
| `netcdf4-zlib-S/place` | 0 | — | 288,808 | 12,942 | 1 | 155 | 6,179 | 218,938 | 320,685 | — | — |
| `netcdf4-zlib-S/codec` | 16,380 | — | — | — | — | 2 | 79,510 | 1,240,005 | 1,695,581 | — | — |
| `zarr-v2-blosc-D/open` | 0 | — | 1,158 | 1,584 | 4 | 370 | 14,038 | 285,292 | 451,329 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | — | 0 | 1,584 | 0 | 35 | 1,186 | 10,904 | 17,081 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 4 | 37,996 | 39,154 | 3 | 173 | 656,211 | 4,151,929 | 6,440,311 | — | — |
| `zarr-v2-blosc-D/scrub` | 16,380 | 4 | 300,515 | 301,673 | 10 | 782 | 656,584 | 32,181,239 | 47,594,300 | — | — |
| `zarr-v2-blosc-D/place` | 0 | — | 426 | 1,584 | 2 | 123 | 9,347 | 161,152 | 252,872 | — | — |
| `zarr-v2-blosc-D/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,507,487 | 2,412,254 | — | — |
| `zarr-v2-blosc-L/open` | 0 | — | 1,161 | 1,900 | 4 | 321 | 14,038 | 247,564 | 399,554 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | — | 0 | 1,900 | 0 | 35 | 1,186 | 10,886 | 17,047 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 4 | 144,560 | 145,721 | 3 | 176 | 2,607,411 | 15,984,038 | 25,159,117 | — | — |
| `zarr-v2-blosc-L/scrub` | 65,160 | 4 | 1,141,368 | 1,142,529 | 10 | 792 | 2,607,784 | 126,334,170 | 187,265,224 | — | — |
| `zarr-v2-blosc-L/place` | 0 | — | 739 | 1,900 | 2 | 125 | 17,267 | 207,149 | 315,099 | — | — |
| `zarr-v2-blosc-L/codec` | 65,160 | — | — | — | — | 6 | 781,920 | 5,796,305 | 9,339,381 | — | — |
| `zarr-v2-blosc-S/open` | 0 | — | 1,157 | 1,583 | 4 | 321 | 14,038 | 247,298 | 399,368 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | — | 0 | 1,583 | 0 | 35 | 1,186 | 10,910 | 17,107 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 4 | 37,996 | 39,153 | 3 | 173 | 656,211 | 4,151,847 | 6,442,740 | — | — |
| `zarr-v2-blosc-S/scrub` | 16,380 | 4 | 300,515 | 301,672 | 10 | 782 | 656,584 | 32,185,264 | 47,603,184 | — | — |
| `zarr-v2-blosc-S/place` | 0 | — | 426 | 1,583 | 2 | 123 | 9,347 | 161,015 | 252,855 | — | — |
| `zarr-v2-blosc-S/codec` | 16,380 | — | — | — | — | 5 | 196,560 | 1,506,215 | 2,409,848 | — | — |
| `zarr-v3-sharded-D/open` | 0 | — | 2,800 | 3,413 | 3 | 393 | 35,909 | 292,211 | 459,051 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | — | 0 | 3,413 | 0 | 35 | 1,186 | 10,790 | 16,952 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 4 | 139,799 | 142,599 | 3 | 403 | 1,967,139 | 20,332,433 | 32,251,406 | — | — |
| `zarr-v3-sharded-D/scrub` | 16,380 | 4 | 278,830 | 281,630 | 10 | 2,300 | 1,967,512 | 160,647,824 | 248,422,676 | — | — |
| `zarr-v3-sharded-D/place` | 0 | — | 613 | 3,413 | 2 | 169 | 17,437 | 311,951 | 474,256 | — | — |
| `zarr-v3-sharded-L/open` | 0 | — | 2,804 | 4,019 | 3 | 380 | 35,909 | 285,058 | 448,667 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | — | 0 | 4,019 | 0 | 35 | 1,186 | 10,781 | 16,953 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 4 | 526,253 | 529,057 | 3 | 557 | 7,820,739 | 83,258,195 | 133,358,235 | — | — |
| `zarr-v3-sharded-L/scrub` | 65,160 | 4 | 1,058,791 | 1,061,595 | 10 | 3,524 | 7,821,112 | 687,390,298 | 1,068,854,956 | — | — |
| `zarr-v3-sharded-L/place` | 0 | — | 1,215 | 4,019 | 2 | 173 | 24,632 | 408,299 | 604,712 | — | — |
| `zarr-v3-sharded-S/open` | 0 | — | 2,799 | 3,412 | 3 | 380 | 35,909 | 282,499 | 444,508 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | — | 0 | 3,412 | 0 | 35 | 1,186 | 10,789 | 16,947 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 4 | 139,799 | 142,598 | 3 | 403 | 1,967,139 | 20,334,095 | 32,253,681 | — | — |
| `zarr-v3-sharded-S/scrub` | 16,380 | 4 | 278,830 | 281,629 | 10 | 2,300 | 1,967,512 | 160,147,251 | 246,902,782 | — | — |
| `zarr-v3-sharded-S/place` | 0 | — | 613 | 3,412 | 2 | 169 | 17,437 | 312,560 | 474,991 | — | — |
| `zarr-v3-zstd-D/open` | 0 | — | 2,186 | 2,799 | 3 | 387 | 32,208 | 313,590 | 484,839 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | — | 0 | 2,799 | 0 | 35 | 1,186 | 10,781 | 16,959 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 4 | 52,381 | 54,567 | 3 | 242 | 656,219 | 6,314,676 | 9,591,374 | — | — |
| `zarr-v3-zstd-D/scrub` | 16,380 | 4 | 414,719 | 416,905 | 10 | 1,030 | 656,592 | 48,573,713 | 71,180,864 | — | — |
| `zarr-v3-zstd-D/place` | 0 | — | 613 | 2,799 | 2 | 169 | 17,437 | 311,071 | 474,091 | — | — |
| `zarr-v3-zstd-D/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,070,429 | 5,956,798 | — | — |
| `zarr-v3-zstd-L/open` | 0 | — | 2,189 | 3,404 | 3 | 337 | 32,208 | 263,077 | 416,037 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | — | 0 | 3,404 | 0 | 35 | 1,186 | 10,796 | 16,942 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 4 | 207,207 | 209,396 | 3 | 260 | 2,607,419 | 24,126,519 | 37,151,037 | — | — |
| `zarr-v3-zstd-L/scrub` | 65,160 | 4 | 1,645,094 | 1,647,283 | 10 | 1,119 | 2,607,792 | 191,838,987 | 282,057,494 | — | — |
| `zarr-v3-zstd-L/place` | 0 | — | 1,215 | 3,404 | 2 | 173 | 24,632 | 406,393 | 603,143 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,717,489 | 38,462,164 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 127,742 | 179,370 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,630,779 | 9,036,822 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | — | 0 | 0 | 0 | 55 | 1,182,160 | 15,295,459 | 19,000,197 | — | — |
| `zarr-v3-zstd-L/codec` | 65,160 | — | — | — | — | 43 | 769,444 | 15,887,854 | 23,326,332 | — | — |
| `zarr-v3-zstd-S/open` | 0 | — | 2,185 | 2,798 | 3 | 337 | 32,208 | 262,304 | 414,619 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | — | 0 | 2,798 | 0 | 35 | 1,186 | 10,805 | 16,991 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 4 | 52,381 | 54,566 | 3 | 242 | 656,219 | 6,313,718 | 9,590,886 | — | — |
| `zarr-v3-zstd-S/scrub` | 16,380 | 4 | 414,719 | 416,904 | 10 | 1,030 | 656,592 | 48,574,871 | 71,184,243 | — | — |
| `zarr-v3-zstd-S/place` | 0 | — | 613 | 2,798 | 2 | 169 | 17,437 | 310,803 | 474,007 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,019,767 | 9,710,539 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 126,738 | 177,786 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,794,124 | 2,441,787 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | — | 0 | 0 | 0 | 48 | 323,856 | 3,989,860 | 4,956,339 | — | — |
| `zarr-v3-zstd-S/codec` | 16,380 | — | — | — | — | 29 | 258,467 | 4,085,877 | 6,009,481 | — | — |
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
| `netcdf4-zlib/open` | 377 | 377 | holds |
| `netcdf4-zlib/variables` | 41 | 41 | holds |
| `netcdf4-zlib/slice` | 227 | 228 | grows +1 |
| `netcdf4-zlib/scrub` | 962 | 970 | grows +8 |
| `netcdf4-zlib/place` | 155 | 155 | holds |
| `netcdf4-zlib/codec` | 2 | 3 | grows +1 |
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
| `netcdf4-zlib/slice` | 29.0 B | 1,660 B | 9 B | 3.2× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 29.0 B | 1,660 B | 9 B | 3.2× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
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
| `netcdf-classic/slice` | 1.00 | 1.00 | 1.01 |
| `netcdf-classic/scrub` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib/slice` | 1.00 | 1.11 | 1.01 |
| `netcdf4-zlib/scrub` | 1.00 | 1.03 | 1.00 |
| `zarr-v2-blosc/open` | 1.00 | 1.15 | 1.15 |
| `zarr-v2-blosc/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v2-blosc/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/open` | 1.00 | 1.03 | 1.03 |
| `zarr-v3-sharded/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/open` | 1.00 | 1.15 | 1.20 |
| `zarr-v3-zstd/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-zstd/scrub` | 1.00 | 1.00 | 1.00 |

#### Work against cell count

Instructions at `L` over `S`. Proportional work is a ratio of about 4
(the cell ratio); spectral inputs grow coefficients, not cells.

| Operation | Instructions `S` | Instructions `L` | `L`/`S` |
|---|---:|---:|---:|
| `grib1-second-order/decode` | 3,303,609 | 12,090,161 | 3.66 |
| `grib1-simple/decode` | 3,236,130 | 12,613,804 | 3.90 |
| `grib1-spectral-complex/decode` | 122,294,040 | 253,298,826 | 2.07 |
| `grib1-spectral-simple/decode` | 122,194,580 | 253,051,086 | 2.07 |
| `grib2-5.0/decode` | 3,235,590 | 12,613,796 | 3.90 |
| `grib2-5.0/warp` | 7,029,801 | 27,727,500 | 3.94 |
| `grib2-5.0/render` | 1,804,414 | 6,641,434 | 3.68 |
| `grib2-5.0/contours` | 4,014,815 | 15,299,721 | 3.81 |
| `grib2-5.3/decode` | 3,948,298 | 14,812,994 | 3.75 |
| `grib2-5.40/decode` | 34,511,992 | 114,251,929 | 3.31 |
| `grib2-5.40/codec` | 33,117,772 | 108,726,420 | 3.28 |
| `grib2-5.41/decode` | 3,139,673 | 11,589,865 | 3.69 |
| `grib2-5.41/codec` | 1,603,326 | 5,479,028 | 3.42 |
| `grib2-5.42/decode` | 2,536,140 | 9,814,970 | 3.87 |
| `grib2-5.42/codec` | 1,388,383 | 5,275,324 | 3.80 |
| `netcdf-classic/slice` | 1,786,193 | 6,754,461 | 3.78 |
| `netcdf-classic/scrub` | 13,595,299 | 53,120,680 | 3.91 |
| `netcdf4-zlib/slice` | 4,732,123 | 16,787,790 | 3.55 |
| `netcdf4-zlib/scrub` | 36,193,880 | 132,636,481 | 3.66 |
| `netcdf4-zlib/codec` | 1,240,005 | 3,995,740 | 3.22 |
| `zarr-v2-blosc/slice` | 4,151,847 | 15,984,038 | 3.85 |
| `zarr-v2-blosc/scrub` | 32,185,264 | 126,334,170 | 3.93 |
| `zarr-v2-blosc/codec` | 1,506,215 | 5,796,305 | 3.85 |
| `zarr-v3-sharded/slice` | 20,334,095 | 83,258,195 | 4.09 |
| `zarr-v3-sharded/scrub` | 160,147,251 | 687,390,298 | 4.29 |
| `zarr-v3-zstd/slice` | 6,313,718 | 24,126,519 | 3.82 |
| `zarr-v3-zstd/scrub` | 48,574,871 | 191,838,987 | 3.95 |
| `zarr-v3-zstd/codec` | 4,085,877 | 15,887,854 | 3.89 |
| `zarr-v3-zstd/warp` | 7,019,767 | 27,717,489 | 3.95 |
| `zarr-v3-zstd/render` | 1,794,124 | 6,630,779 | 3.70 |
| `zarr-v3-zstd/contours` | 3,989,860 | 15,295,459 | 3.83 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,251,929 | 108,726,420 | 5% |
| `grib2-5.40-S` | 34,511,992 | 33,117,772 | 4% |
| `grib2-5.41-L` | 11,589,865 | 5,479,028 | 53% |
| `grib2-5.41-S` | 3,139,673 | 1,603,326 | 49% |
| `grib2-5.42-L` | 9,814,970 | 5,275,324 | 46% |
| `grib2-5.42-S` | 2,536,140 | 1,388,383 | 45% |
| `netcdf4-zlib-D` | 4,764,451 | 1,246,625 | 74% |
| `netcdf4-zlib-L` | 16,787,790 | 3,995,740 | 76% |
| `netcdf4-zlib-S` | 4,732,123 | 1,240,005 | 74% |
| `zarr-v2-blosc-D` | 4,151,929 | 1,507,487 | 64% |
| `zarr-v2-blosc-L` | 15,984,038 | 5,796,305 | 64% |
| `zarr-v2-blosc-S` | 4,151,847 | 1,506,215 | 64% |
| `zarr-v3-zstd-D` | 6,314,676 | 4,070,429 | 36% |
| `zarr-v3-zstd-L` | 24,126,519 | 15,887,854 | 34% |
| `zarr-v3-zstd-S` | 6,313,718 | 4,085,877 | 35% |
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
