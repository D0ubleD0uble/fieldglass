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
| Gate | Instructions, estimated cycles | Gungraun on Callgrind, cache geometry fixed | 2% |
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
| `grib1-second-order-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,926 | 26,655 | 1,638,400 | 1,638,400 |
| `grib1-second-order-L/decode` | 65,160 | 4 | 26,524 | 26,620 | 1 | 20 | 2,106,981 | 12,241,522 | 17,784,457 | 3,735,552 | 3,735,552 |
| `grib1-second-order-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,428 | 29,024 | — | — |
| `grib1-second-order-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,894 | 26,644 | 1,638,400 | 1,638,400 |
| `grib1-second-order-S/decode` | 16,380 | 4 | 10,454 | 10,550 | 1 | 20 | 532,188 | 3,421,415 | 4,887,140 | 2,293,760 | 2,293,760 |
| `grib1-second-order-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,444 | 29,041 | — | — |
| `grib1-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,840 | 26,580 | 1,835,008 | 1,835,008 |
| `grib1-simple-L/decode` | 65,160 | 4 | 130,332 | 130,428 | 1 | 15 | 1,889,666 | 12,874,347 | 17,974,761 | 3,670,016 | 3,670,016 |
| `grib1-simple-L/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,469 | 29,088 | — | — |
| `grib1-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,351 | 27,344 | 1,703,936 | 1,703,936 |
| `grib1-simple-S/decode` | 16,380 | 4 | 32,772 | 32,868 | 1 | 15 | 475,046 | 3,301,659 | 4,552,560 | 2,162,688 | 2,162,688 |
| `grib1-simple-S/place` | 0 | — | 0 | 491 | 0 | 3 | 64 | 15,440 | 29,056 | — | — |
| `grib1-spectral-complex-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,023 | 26,657 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-L/decode` | 259,920 | 8 | 33,966 | 34,062 | 1 | 22 | 6,498,082 | 303,913,181 | 439,189,988 | 9,764,864 | 9,764,864 |
| `grib1-spectral-complex-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,476 | 27,954 | — | — |
| `grib1-spectral-complex-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,036 | 26,649 | 1,638,400 | 1,638,400 |
| `grib1-spectral-complex-S/decode` | 259,920 | 8 | 9,262 | 9,358 | 1 | 22 | 6,498,082 | 135,161,342 | 203,018,741 | 8,716,288 | 8,716,288 |
| `grib1-spectral-complex-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,891 | 28,583 | — | — |
| `grib1-spectral-simple-L/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 14,998 | 26,602 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-L/decode` | 259,920 | 8 | 33,038 | 33,134 | 1 | 21 | 6,498,082 | 303,715,142 | 438,886,075 | 9,764,864 | 9,764,864 |
| `grib1-spectral-simple-L/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,865 | 28,556 | — | — |
| `grib1-spectral-simple-S/open` | 0 | — | 192 | 491 | 6 | 3 | 800 | 15,060 | 26,673 | 1,638,400 | 1,638,400 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8 | 8,334 | 8,430 | 1 | 21 | 6,498,082 | 135,074,644 | 202,890,054 | 8,716,288 | 8,716,288 |
| `grib1-spectral-simple-S/place` | 0 | — | 0 | 491 | 0 | 4 | 70 | 15,898 | 28,598 | — | — |
| `grib2-5.0-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 80,364 | 126,552 | 1,900,544 | 1,900,544 |
| `grib2-5.0-L/decode` | 65,160 | 4 | 130,331 | 130,499 | 1 | 16 | 1,889,658 | 12,874,411 | 18,038,830 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 15,914 | 29,872 | — | — |
| `grib2-5.0-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,727,741 | 38,477,321 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 137,572 | 194,456 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,641,486 | 8,924,440 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/contours` | 65,160 | — | 0 | 0 | 0 | 54 | 1,165,776 | 15,302,448 | 19,062,835 | 3,735,552 | 3,735,552 |
| `grib2-5.0-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 79,981 | 125,883 | 1,703,936 | 1,703,936 |
| `grib2-5.0-S/decode` | 16,380 | 4 | 32,771 | 32,939 | 1 | 16 | 475,038 | 3,301,080 | 4,567,068 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,630 | 30,902 | — | — |
| `grib2-5.0-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,029,994 | 9,756,665 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 136,421 | 192,586 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,804,418 | 2,456,604 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/contours` | 16,380 | — | 0 | 0 | 0 | 49 | 332,048 | 4,015,972 | 5,066,501 | 2,162,688 | 2,162,688 |
| `grib2-5.200-S/open` | 0 | — | 211 | 211 | 7 | 4 | 2,058 | 79,556 | 125,599 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/decode` | 496 | 4 | 20 | 211 | 1 | 18 | 14,402 | 124,466 | 196,543 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/place` | 0 | — | 0 | 211 | 0 | 4 | 70 | 15,919 | 29,891 | — | — |
| `grib2-5.3-L/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 78,801 | 124,338 | 1,703,936 | 1,703,936 |
| `grib2-5.3-L/decode` | 65,160 | 4 | 57,954 | 58,150 | 1 | 18 | 1,889,658 | 15,515,183 | 21,485,069 | 3,604,480 | 3,604,480 |
| `grib2-5.3-L/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 15,720 | 29,621 | — | — |
| `grib2-5.3-S/open` | 0 | — | 403 | 783 | 7 | 3 | 2,048 | 78,362 | 123,764 | 1,638,400 | 1,638,400 |
| `grib2-5.3-S/decode` | 16,380 | 4 | 18,586 | 18,782 | 1 | 18 | 475,038 | 4,125,795 | 5,673,095 | 2,097,152 | 2,097,152 |
| `grib2-5.3-S/place` | 0 | — | 0 | 783 | 0 | 4 | 70 | 15,958 | 30,008 | — | — |
| `grib2-5.40-L/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 78,062 | 123,047 | 1,638,400 | 1,638,400 |
| `grib2-5.40-L/decode` | 65,160 | 4 | 14,681 | 14,851 | 1 | 567 | 1,889,658 | 114,250,979 | 146,634,482 | 3,866,624 | 3,866,624 |
| `grib2-5.40-L/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 15,749 | 29,658 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | — | 551 | 859,764 | 108,724,186 | 137,705,947 | — | — |
| `grib2-5.40-S/open` | 0 | — | 403 | 757 | 7 | 3 | 2,048 | 78,043 | 123,012 | 1,638,400 | 1,638,400 |
| `grib2-5.40-S/decode` | 16,380 | 4 | 6,278 | 6,448 | 1 | 429 | 475,038 | 34,512,426 | 44,453,753 | 2,162,688 | 2,162,688 |
| `grib2-5.40-S/place` | 0 | — | 0 | 757 | 0 | 4 | 70 | 16,286 | 30,470 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | — | 413 | 221,460 | 33,115,052 | 42,271,638 | — | — |
| `grib2-5.41-L/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 77,818 | 122,800 | 1,835,008 | 1,835,008 |
| `grib2-5.41-L/decode` | 65,160 | 4 | 80,490 | 80,658 | 1 | 25 | 1,898,632 | 11,589,889 | 16,965,374 | 3,735,552 | 3,735,552 |
| `grib2-5.41-L/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 15,952 | 29,962 | — | — |
| `grib2-5.41-L/codec` | 130,320 | — | — | — | — | 9 | 334,744 | 5,478,132 | 7,254,791 | — | — |
| `grib2-5.41-S/open` | 0 | — | 424 | 755 | 7 | 3 | 2,048 | 77,522 | 122,338 | 1,638,400 | 1,638,400 |
| `grib2-5.41-S/decode` | 16,380 | 4 | 23,501 | 23,669 | 1 | 25 | 511,568 | 3,139,708 | 4,564,585 | 2,293,760 | 2,293,760 |
| `grib2-5.41-S/place` | 0 | — | 0 | 755 | 0 | 4 | 70 | 16,029 | 30,002 | — | — |
| `grib2-5.41-S/codec` | 32,760 | — | — | — | — | 9 | 118,400 | 1,603,133 | 2,181,448 | — | — |
| `grib2-5.42-L/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 77,216 | 121,950 | 1,835,008 | 1,835,008 |
| `grib2-5.42-L/decode` | 65,160 | 4 | 80,767 | 80,939 | 1 | 2,054 | 1,889,658 | 30,573,243 | 40,857,626 | 3,801,088 | 3,801,088 |
| `grib2-5.42-L/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 15,875 | 29,785 | — | — |
| `grib2-5.42-L/codec` | 130,320 | — | — | — | — | 2,038 | 130,448 | 23,764,496 | 30,369,906 | — | — |
| `grib2-5.42-S/open` | 0 | — | 403 | 759 | 7 | 3 | 2,048 | 76,909 | 121,441 | 1,638,400 | 1,638,400 |
| `grib2-5.42-S/decode` | 16,380 | 4 | 22,449 | 22,621 | 1 | 529 | 475,038 | 8,053,498 | 10,694,431 | 2,162,688 | 2,162,688 |
| `grib2-5.42-S/place` | 0 | — | 0 | 759 | 0 | 4 | 70 | 16,212 | 30,315 | — | — |
| `grib2-5.42-S/codec` | 32,760 | — | — | — | — | 513 | 32,888 | 6,303,682 | 8,045,049 | — | — |
| `netcdf-classic-D/open` | 0 | — | 2,098,344 | 1,704 | 1 | 102 | 4,966 | 37,125 | 65,341 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/variables` | 0 | — | 2,098,344 | 1,704 | 1 | 41 | 1,290 | 26,770 | 40,684 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/slice` | 16,380 | 4 | 2,098,344 | 67,224 | 1 | 119 | 8,649,499 | 20,429,855 | 32,351,280 | 14,286,848 | 14,286,848 |
| `netcdf-classic-D/scrub` | 16,380 | 4 | 2,098,344 | 525,864 | 1 | 609 | 8,649,872 | 162,576,890 | 256,139,566 | 14,286,848 | 14,286,848 |
| `netcdf-classic-D/place` | 0 | — | 2,098,344 | 1,704 | 1 | 95 | 7,723 | 124,329 | 193,042 | — | — |
| `netcdf-classic-L/open` | 0 | — | 2,087,712 | 2,592 | 1 | 102 | 4,966 | 36,558 | 64,590 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/variables` | 0 | — | 2,087,712 | 2,592 | 1 | 41 | 1,290 | 13,322 | 20,389 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/slice` | 65,160 | 4 | 2,087,712 | 263,232 | 1 | 119 | 9,383,899 | 24,495,869 | 40,429,647 | 14,155,776 | 14,155,776 |
| `netcdf-classic-L/scrub` | 65,160 | 4 | 2,087,712 | 2,087,712 | 1 | 609 | 9,384,272 | 195,094,357 | 318,117,735 | 14,155,776 | 14,155,776 |
| `netcdf-classic-L/place` | 0 | — | 2,087,712 | 2,592 | 1 | 95 | 14,203 | 154,402 | 235,003 | — | — |
| `netcdf-classic-S/open` | 0 | — | 525,672 | 1,512 | 1 | 102 | 4,966 | 36,876 | 65,093 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/variables` | 0 | — | 525,672 | 1,512 | 1 | 41 | 1,290 | 26,781 | 40,605 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/slice` | 16,380 | 4 | 525,672 | 67,032 | 1 | 119 | 2,359,579 | 6,276,125 | 10,336,056 | 4,849,664 | 4,849,664 |
| `netcdf-classic-S/scrub` | 16,380 | 4 | 525,672 | 525,672 | 1 | 609 | 2,359,952 | 49,370,773 | 71,272,194 | 4,849,664 | 4,849,664 |
| `netcdf-classic-S/place` | 0 | — | 525,672 | 1,512 | 1 | 95 | 7,723 | 136,757 | 211,153 | — | — |
| `netcdf4-zlib-D/open` | 0 | — | 1,116,265 | 12,942 | 1 | 378 | 14,503 | 349,969 | 509,737 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/variables` | 0 | — | 1,116,265 | 12,942 | 1 | 41 | 1,290 | 26,928 | 40,740 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/slice` | 16,380 | 4 | 1,116,265 | 47,445 | 1 | 419 | 10,486,291 | 183,714,510 | 248,809,019 | 14,417,920 | 14,417,920 |
| `netcdf4-zlib-D/scrub` | 16,380 | 4 | 1,116,265 | 288,808 | 1 | 2,232 | 10,486,664 | 1,475,528,017 | 1,995,563,023 | 14,417,920 | 14,417,920 |
| `netcdf4-zlib-D/place` | 0 | — | 1,116,265 | 12,942 | 1 | 167 | 7,699 | 250,213 | 359,879 | — | — |
| `netcdf4-zlib-D/codec` | 65,520 | — | — | — | — | 2 | 79,510 | 1,246,618 | 1,708,549 | — | — |
| `netcdf4-zlib-L/open` | 0 | — | 1,006,286 | 14,990 | 1 | 378 | 14,503 | 347,730 | 506,728 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/variables` | 0 | — | 1,006,286 | 14,990 | 1 | 41 | 1,290 | 26,859 | 40,643 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/slice` | 65,160 | 4 | 1,006,286 | 139,063 | 1 | 281 | 10,427,155 | 180,863,767 | 248,876,433 | 14,155,776 | 14,155,776 |
| `netcdf4-zlib-L/scrub` | 65,160 | 4 | 1,006,286 | 1,006,286 | 1 | 1,310 | 10,427,528 | 1,456,872,918 | 1,994,797,380 | 14,155,776 | 14,155,776 |
| `netcdf4-zlib-L/place` | 0 | — | 1,006,286 | 14,990 | 1 | 167 | 14,179 | 283,222 | 405,700 | — | — |
| `netcdf4-zlib-L/codec` | 260,640 | — | — | — | — | 3 | 506,796 | 3,995,711 | 5,654,796 | — | — |
| `netcdf4-zlib-S/open` | 0 | — | 288,808 | 12,942 | 1 | 378 | 14,503 | 349,196 | 508,918 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/variables` | 0 | — | 288,808 | 12,942 | 1 | 41 | 1,290 | 26,914 | 40,704 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/slice` | 16,380 | 4 | 288,808 | 47,445 | 1 | 273 | 2,622,355 | 47,220,552 | 64,518,139 | 4,849,664 | 4,849,664 |
| `netcdf4-zlib-S/scrub` | 16,380 | 4 | 288,808 | 288,808 | 1 | 1,246 | 2,622,728 | 378,016,114 | 506,647,939 | 4,849,664 | 4,849,664 |
| `netcdf4-zlib-S/place` | 0 | — | 288,808 | 12,942 | 1 | 167 | 7,699 | 249,703 | 358,997 | — | — |
| `netcdf4-zlib-S/codec` | 65,520 | — | — | — | — | 2 | 79,510 | 1,240,038 | 1,695,482 | — | — |
| `zarr-v2-blosc-D/open` | 0 | — | 1,158 | 1,584 | 4 | 370 | 14,038 | 286,693 | 454,464 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | — | 0 | 1,584 | 0 | 35 | 1,186 | 10,827 | 17,049 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 4 | 37,996 | 39,154 | 3 | 170 | 738,092 | 4,213,472 | 6,776,161 | — | — |
| `zarr-v2-blosc-D/scrub` | 16,380 | 4 | 300,515 | 301,673 | 10 | 772 | 738,092 | 32,662,528 | 49,371,318 | — | — |
| `zarr-v2-blosc-D/place` | 0 | — | 426 | 1,584 | 2 | 121 | 9,347 | 160,938 | 251,743 | — | — |
| `zarr-v2-blosc-D/codec` | 65,520 | — | — | — | — | 5 | 196,560 | 1,507,549 | 2,412,502 | — | — |
| `zarr-v2-blosc-L/open` | 0 | — | 1,161 | 1,900 | 4 | 321 | 14,038 | 248,617 | 402,083 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | — | 0 | 1,900 | 0 | 35 | 1,186 | 10,851 | 17,105 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 4 | 144,560 | 145,721 | 3 | 173 | 2,933,192 | 16,234,622 | 26,546,127 | — | — |
| `zarr-v2-blosc-L/scrub` | 65,160 | 4 | 1,141,368 | 1,142,529 | 10 | 782 | 2,933,192 | 128,333,690 | 194,451,582 | — | — |
| `zarr-v2-blosc-L/place` | 0 | — | 739 | 1,900 | 2 | 123 | 17,267 | 206,550 | 313,401 | — | — |
| `zarr-v2-blosc-L/codec` | 260,640 | — | — | — | — | 6 | 781,920 | 5,796,257 | 9,339,300 | — | — |
| `zarr-v2-blosc-S/open` | 0 | — | 1,157 | 1,583 | 4 | 321 | 14,038 | 248,412 | 402,016 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | — | 0 | 1,583 | 0 | 35 | 1,186 | 10,827 | 17,057 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 4 | 37,996 | 39,153 | 3 | 170 | 738,092 | 4,212,825 | 6,770,681 | — | — |
| `zarr-v2-blosc-S/scrub` | 16,380 | 4 | 300,515 | 301,672 | 10 | 772 | 738,092 | 32,667,497 | 49,380,866 | — | — |
| `zarr-v2-blosc-S/place` | 0 | — | 426 | 1,583 | 2 | 121 | 9,347 | 160,436 | 251,347 | — | — |
| `zarr-v2-blosc-S/codec` | 65,520 | — | — | — | — | 5 | 196,560 | 1,506,328 | 2,410,166 | — | — |
| `zarr-v3-sharded-D/open` | 0 | — | 2,800 | 3,413 | 3 | 393 | 35,909 | 294,175 | 462,234 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | — | 0 | 3,413 | 0 | 35 | 1,186 | 10,731 | 16,962 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 4 | 139,799 | 142,599 | 3 | 396 | 1,967,115 | 20,566,004 | 33,032,894 | — | — |
| `zarr-v3-sharded-D/scrub` | 16,380 | 4 | 278,830 | 281,630 | 10 | 2,258 | 1,967,488 | 162,378,503 | 254,281,489 | — | — |
| `zarr-v3-sharded-D/place` | 0 | — | 613 | 3,413 | 2 | 167 | 17,437 | 311,397 | 473,336 | — | — |
| `zarr-v3-sharded-L/open` | 0 | — | 2,804 | 4,019 | 3 | 380 | 35,909 | 287,052 | 451,700 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | — | 0 | 4,019 | 0 | 35 | 1,186 | 10,722 | 16,947 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 4 | 526,253 | 529,057 | 3 | 550 | 7,820,715 | 84,245,474 | 136,548,303 | — | — |
| `zarr-v3-sharded-L/scrub` | 65,160 | 4 | 1,058,791 | 1,061,595 | 10 | 3,482 | 7,821,088 | 695,274,113 | 1,093,585,874 | — | — |
| `zarr-v3-sharded-L/place` | 0 | — | 1,215 | 4,019 | 2 | 171 | 24,632 | 407,716 | 603,959 | — | — |
| `zarr-v3-sharded-S/open` | 0 | — | 2,799 | 3,412 | 3 | 380 | 35,909 | 288,120 | 453,174 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | — | 0 | 3,412 | 0 | 35 | 1,186 | 10,731 | 16,954 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 4 | 139,799 | 142,598 | 3 | 396 | 1,967,115 | 20,566,181 | 33,031,609 | — | — |
| `zarr-v3-sharded-S/scrub` | 16,380 | 4 | 278,830 | 281,629 | 10 | 2,258 | 1,967,488 | 162,074,093 | 253,349,589 | — | — |
| `zarr-v3-sharded-S/place` | 0 | — | 613 | 3,412 | 2 | 167 | 17,437 | 311,620 | 473,627 | — | — |
| `zarr-v3-zstd-D/open` | 0 | — | 2,186 | 2,799 | 3 | 387 | 32,208 | 315,838 | 488,445 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | — | 0 | 2,799 | 0 | 35 | 1,186 | 10,746 | 16,991 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 4 | 52,381 | 54,567 | 3 | 239 | 738,092 | 6,572,500 | 10,485,396 | — | — |
| `zarr-v3-zstd-D/scrub` | 16,380 | 4 | 414,719 | 416,905 | 10 | 1,020 | 738,092 | 50,626,379 | 77,714,881 | — | — |
| `zarr-v3-zstd-D/place` | 0 | — | 613 | 2,799 | 2 | 167 | 17,437 | 310,372 | 472,649 | — | — |
| `zarr-v3-zstd-D/codec` | 65,520 | — | — | — | — | 29 | 258,467 | 4,070,400 | 5,956,138 | — | — |
| `zarr-v3-zstd-L/open` | 0 | — | 2,189 | 3,404 | 3 | 337 | 32,208 | 264,675 | 418,905 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | — | 0 | 3,404 | 0 | 35 | 1,186 | 10,728 | 16,961 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 4 | 207,207 | 209,396 | 3 | 257 | 2,933,192 | 25,158,999 | 40,883,553 | — | — |
| `zarr-v3-zstd-L/scrub` | 65,160 | 4 | 1,645,094 | 1,647,283 | 10 | 1,109 | 2,933,192 | 200,092,220 | 308,158,125 | — | — |
| `zarr-v3-zstd-L/place` | 0 | — | 1,215 | 3,404 | 2 | 171 | 24,632 | 405,586 | 601,698 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | — | 0 | 0 | 0 | 4 | 847,088 | 27,718,080 | 38,309,747 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 127,681 | 179,298 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | — | 0 | 0 | 0 | 2 | 781,920 | 6,758,623 | 9,047,717 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | — | 0 | 0 | 0 | 55 | 1,182,160 | 15,298,873 | 18,579,620 | — | — |
| `zarr-v3-zstd-L/codec` | 260,640 | — | — | — | — | 43 | 769,444 | 15,887,908 | 23,326,127 | — | — |
| `zarr-v3-zstd-S/open` | 0 | — | 2,185 | 2,798 | 3 | 337 | 32,208 | 263,909 | 417,670 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | — | 0 | 2,798 | 0 | 35 | 1,186 | 10,737 | 16,976 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 4 | 52,381 | 54,566 | 3 | 239 | 738,092 | 6,570,371 | 10,483,159 | — | — |
| `zarr-v3-zstd-S/scrub` | 16,380 | 4 | 414,719 | 416,904 | 10 | 1,020 | 738,092 | 50,628,253 | 77,717,354 | — | — |
| `zarr-v3-zstd-S/place` | 0 | — | 613 | 2,798 | 2 | 167 | 17,437 | 310,130 | 472,736 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | — | 0 | 0 | 0 | 4 | 212,948 | 7,020,100 | 9,710,476 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | — | 0 | 0 | 0 | 0 | 0 | 126,890 | 178,042 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | — | 0 | 0 | 0 | 2 | 196,560 | 1,794,300 | 2,409,093 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | — | 0 | 0 | 0 | 48 | 323,856 | 3,990,893 | 4,891,452 | — | — |
| `zarr-v3-zstd-S/codec` | 65,520 | — | — | — | — | 29 | 258,467 | 4,085,867 | 6,009,073 | — | — |
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
| `grib1-spectral-complex/decode` | 22 | 22 | holds |
| `grib1-spectral-complex/place` | 4 | 4 | holds |
| `grib1-spectral-simple/open` | 3 | 3 | holds |
| `grib1-spectral-simple/decode` | 21 | 21 | holds |
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
| `grib2-5.42/decode` | 529 | 2,054 | grows +1,525 |
| `grib2-5.42/place` | 4 | 4 | holds |
| `grib2-5.42/codec` | 513 | 2,038 | grows +1,525 |
| `netcdf-classic/open` | 102 | 102 | holds |
| `netcdf-classic/variables` | 41 | 41 | holds |
| `netcdf-classic/slice` | 119 | 119 | holds |
| `netcdf-classic/scrub` | 609 | 609 | holds |
| `netcdf-classic/place` | 95 | 95 | holds |
| `netcdf4-zlib/open` | 378 | 378 | holds |
| `netcdf4-zlib/variables` | 41 | 41 | holds |
| `netcdf4-zlib/slice` | 273 | 281 | grows +8 |
| `netcdf4-zlib/scrub` | 1,246 | 1,310 | grows +64 |
| `netcdf4-zlib/place` | 167 | 167 | holds |
| `netcdf4-zlib/codec` | 2 | 3 | grows +1 |
| `zarr-v2-blosc/open` | 321 | 321 | holds |
| `zarr-v2-blosc/variables` | 35 | 35 | holds |
| `zarr-v2-blosc/slice` | 170 | 173 | grows +3 |
| `zarr-v2-blosc/scrub` | 772 | 782 | grows +10 |
| `zarr-v2-blosc/place` | 121 | 123 | grows +2 |
| `zarr-v2-blosc/codec` | 5 | 6 | grows +1 |
| `zarr-v3-sharded/open` | 380 | 380 | holds |
| `zarr-v3-sharded/variables` | 35 | 35 | holds |
| `zarr-v3-sharded/slice` | 396 | 550 | grows +154 |
| `zarr-v3-sharded/scrub` | 2,258 | 3,482 | grows +1,224 |
| `zarr-v3-sharded/place` | 167 | 171 | grows +4 |
| `zarr-v3-zstd/open` | 337 | 337 | holds |
| `zarr-v3-zstd/variables` | 35 | 35 | holds |
| `zarr-v3-zstd/slice` | 239 | 257 | grows +18 |
| `zarr-v3-zstd/scrub` | 1,020 | 1,109 | grows +89 |
| `zarr-v3-zstd/place` | 167 | 171 | grows +4 |
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
| `netcdf-classic/slice` | 144.0 B | 859 B | 5 B | 28.8× | output value (4) + mask (1); the plane is read in place |
| `netcdf-classic/scrub` | 144.0 B | 1,232 B | 5 B | 28.8× | output value (4) + mask (1); the plane is read in place |
| `netcdf4-zlib/slice` | 160.0 B | 1,555 B | 9 B | 17.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 160.0 B | 1,928 B | 9 B | 17.8× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/slice` | 45.0 B | 992 B | 9 B | 5.0× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/scrub` | 45.0 B | 992 B | 9 B | 5.0× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/slice` | 120.0 B | 1,515 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/scrub` | 120.0 B | 1,888 B | 9 B | 13.3× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/slice` | 45.0 B | 992 B | 9 B | 5.0× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/scrub` | 45.0 B | 992 B | 9 B | 5.0× | output value (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/warp` | 13.0 B | 8 B | 5 B | 2.6× | f32 output (4) + mask (1) per output pixel |
| `zarr-v3-zstd/render` | 12.0 B | 0 B | 4 B | 3.0× | one RGBA pixel |
| `zarr-v3-zstd/contours` | 17.6 B | 35,643 B | 0 B | +17.6 B | marching squares needs a row of state, not a cell's |

#### Work against the variable, not the plane

An open, a slice and a scrub at `D` (four times the variable, the same plane)
against `S`. The bound is a ratio of 1: the planes nobody asked for cost nothing.

| Operation | Peak heap `D`/`S` | Allocations `D`/`S` | Instructions `D`/`S` |
|---|---:|---:|---:|
| `netcdf-classic/open` | 1.00 | 1.00 | 1.01 |
| `netcdf-classic/slice` | 3.67 | 1.00 | 3.26 |
| `netcdf-classic/scrub` | 3.67 | 1.00 | 3.29 |
| `netcdf4-zlib/open` | 1.00 | 1.00 | 1.00 |
| `netcdf4-zlib/slice` | 4.00 | 1.53 | 3.89 |
| `netcdf4-zlib/scrub` | 4.00 | 1.79 | 3.90 |
| `zarr-v2-blosc/open` | 1.00 | 1.15 | 1.15 |
| `zarr-v2-blosc/slice` | 1.00 | 1.00 | 1.00 |
| `zarr-v2-blosc/scrub` | 1.00 | 1.00 | 1.00 |
| `zarr-v3-sharded/open` | 1.00 | 1.03 | 1.02 |
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
| `grib1-second-order/decode` | 3,421,415 | 12,241,522 | 3.58 |
| `grib1-simple/decode` | 3,301,659 | 12,874,347 | 3.90 |
| `grib1-spectral-complex/decode` | 135,161,342 | 303,913,181 | 2.25 |
| `grib1-spectral-simple/decode` | 135,074,644 | 303,715,142 | 2.25 |
| `grib2-5.0/decode` | 3,301,080 | 12,874,411 | 3.90 |
| `grib2-5.0/warp` | 7,029,994 | 27,727,741 | 3.94 |
| `grib2-5.0/render` | 1,804,418 | 6,641,486 | 3.68 |
| `grib2-5.0/contours` | 4,015,972 | 15,302,448 | 3.81 |
| `grib2-5.3/decode` | 4,125,795 | 15,515,183 | 3.76 |
| `grib2-5.40/decode` | 34,512,426 | 114,250,979 | 3.31 |
| `grib2-5.40/codec` | 33,115,052 | 108,724,186 | 3.28 |
| `grib2-5.41/decode` | 3,139,708 | 11,589,889 | 3.69 |
| `grib2-5.41/codec` | 1,603,133 | 5,478,132 | 3.42 |
| `grib2-5.42/decode` | 8,053,498 | 30,573,243 | 3.80 |
| `grib2-5.42/codec` | 6,303,682 | 23,764,496 | 3.77 |
| `netcdf-classic/slice` | 6,276,125 | 24,495,869 | 3.90 |
| `netcdf-classic/scrub` | 49,370,773 | 195,094,357 | 3.95 |
| `netcdf4-zlib/slice` | 47,220,552 | 180,863,767 | 3.83 |
| `netcdf4-zlib/scrub` | 378,016,114 | 1,456,872,918 | 3.85 |
| `netcdf4-zlib/codec` | 1,240,038 | 3,995,711 | 3.22 |
| `zarr-v2-blosc/slice` | 4,212,825 | 16,234,622 | 3.85 |
| `zarr-v2-blosc/scrub` | 32,667,497 | 128,333,690 | 3.93 |
| `zarr-v2-blosc/codec` | 1,506,328 | 5,796,257 | 3.85 |
| `zarr-v3-sharded/slice` | 20,566,181 | 84,245,474 | 4.10 |
| `zarr-v3-sharded/scrub` | 162,074,093 | 695,274,113 | 4.29 |
| `zarr-v3-zstd/slice` | 6,570,371 | 25,158,999 | 3.83 |
| `zarr-v3-zstd/scrub` | 50,628,253 | 200,092,220 | 3.95 |
| `zarr-v3-zstd/codec` | 4,085,867 | 15,887,908 | 3.89 |
| `zarr-v3-zstd/warp` | 7,020,100 | 27,718,080 | 3.95 |
| `zarr-v3-zstd/render` | 1,794,300 | 6,758,623 | 3.77 |
| `zarr-v3-zstd/contours` | 3,990,893 | 15,298,873 | 3.83 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,250,979 | 108,724,186 | 5% |
| `grib2-5.40-S` | 34,512,426 | 33,115,052 | 4% |
| `grib2-5.41-L` | 11,589,889 | 5,478,132 | 53% |
| `grib2-5.41-S` | 3,139,708 | 1,603,133 | 49% |
| `grib2-5.42-L` | 30,573,243 | 23,764,496 | 22% |
| `grib2-5.42-S` | 8,053,498 | 6,303,682 | 22% |
| `netcdf4-zlib-D` | 183,714,510 | 1,246,618 | 99% |
| `netcdf4-zlib-L` | 180,863,767 | 3,995,711 | 98% |
| `netcdf4-zlib-S` | 47,220,552 | 1,240,038 | 97% |
| `zarr-v2-blosc-D` | 4,213,472 | 1,507,549 | 64% |
| `zarr-v2-blosc-L` | 16,234,622 | 5,796,257 | 64% |
| `zarr-v2-blosc-S` | 4,212,825 | 1,506,328 | 64% |
| `zarr-v3-zstd-D` | 6,572,500 | 4,070,400 | 38% |
| `zarr-v3-zstd-L` | 25,158,999 | 15,887,908 | 37% |
| `zarr-v3-zstd-S` | 6,570,371 | 4,085,867 | 38% |
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
- **Instructions** are allowed 2%. Two runs on one machine differ by under
  0.02% (hash-map seeding), so 2% is room for the compiler, not for noise.
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

1. **NetCDF reads and decodes the whole variable for every plane.** The reader
   takes the whole file (no range seam), so every NetCDF operation's bytes read
   is the file: an open, variable listing, placement or slice reads 6× to
   1,231× its bound, and a scrub of the deep variable 4×. A slice's peak heap and instructions
   grow with the variable, not the plane (`D`/`S` 3.3–4.0), at 144–160 B per
   cell against a floor of 5–9. On ERA5 a frame takes 197 ms against netCDF4's
   3.5 ms and needs the other half of the day's file.
2. **A decoded cell costs 29 B where the floor is 5.** The readers return
   `Vec<Option<f64>>` (16 B), `pack_values` copies into a `Vec<f64>` (8 B) and
   the mask (1 B), then `Dtype::Auto` narrows to `f32` (4 B) while all three are
   alive. Zarr slices hold 45 B per cell against a floor of 9.
3. **The AEC decoder allocates per block.** 5.42 decodes allocate 529 times at
   `S` and 2,054 at `L`; `rust_aec` alone accounts for 513 and 2,038.
4. **JPEG 2000's cost is the codec's.** 95–96% of a 5.40 decode's instructions
   are in `rust_j2k`, which also allocates more as the grid grows (413 → 551).
   It is 3.7× eccodes (OpenJPEG) at 1°. This answers the issue's question: the
   ceiling is the codec's, not the decode path's.
5. **NetCDF-4 slices spend 97–99% of their instructions outside zlib**, which is
   finding 1 again from the other side.
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
