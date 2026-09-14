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
| Gate | wasm linear-memory high-water mark | `memory.buffer.byteLength` after the operation, one Node process per scenario | one 64 KiB page |
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
from a decoded field) is done before the profiler starts. A row costs exactly
the operation it names.

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
| Scenario | Cells | Bytes read | Bound | Requests | Allocations | Peak heap | Instructions | Est. cycles | wasm memory | wasm +simd128 memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `grib1-second-order-L/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,083 | 26,908 | 1,638,400 | 1,638,400 |
| `grib1-second-order-L/decode` | 65,160 | 26,524 | 26,620 | 1 | 21 | 2,106,981 | 12,241,679 | 17,785,090 | 3,735,552 | 3,735,552 |
| `grib1-second-order-L/place` | 0 | 0 | 491 | 0 | 4 | 440 | 15,589 | 29,475 | — | — |
| `grib1-second-order-S/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,051 | 26,901 | 1,638,400 | 1,638,400 |
| `grib1-second-order-S/decode` | 16,380 | 10,454 | 10,550 | 1 | 21 | 532,188 | 3,421,572 | 4,887,795 | 2,293,760 | 2,293,760 |
| `grib1-second-order-S/place` | 0 | 0 | 491 | 0 | 4 | 440 | 15,605 | 29,480 | — | — |
| `grib1-simple-L/open` | 0 | 192 | 491 | 6 | 4 | 816 | 14,997 | 26,833 | 1,835,008 | 1,835,008 |
| `grib1-simple-L/decode` | 65,160 | 130,332 | 130,428 | 1 | 16 | 1,889,666 | 12,874,502 | 17,975,582 | 3,670,016 | 3,670,016 |
| `grib1-simple-L/place` | 0 | 0 | 491 | 0 | 4 | 440 | 15,630 | 29,551 | — | — |
| `grib1-simple-S/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,508 | 27,597 | 1,703,936 | 1,703,936 |
| `grib1-simple-S/decode` | 16,380 | 32,772 | 32,868 | 1 | 16 | 475,046 | 3,301,816 | 4,553,323 | 2,162,688 | 2,162,688 |
| `grib1-simple-S/place` | 0 | 0 | 491 | 0 | 4 | 440 | 15,601 | 29,527 | — | — |
| `grib1-spectral-complex-L/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,180 | 26,934 | 1,703,936 | 1,703,936 |
| `grib1-spectral-complex-L/decode` | 259,920 | 33,966 | 34,062 | 1 | 23 | 6,498,082 | 303,913,338 | 439,197,043 | 9,764,864 | 9,764,864 |
| `grib1-spectral-complex-L/place` | 0 | 0 | 491 | 0 | 5 | 440 | 15,637 | 28,339 | — | — |
| `grib1-spectral-complex-S/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,193 | 26,926 | 1,638,400 | 1,638,400 |
| `grib1-spectral-complex-S/decode` | 259,920 | 9,262 | 9,358 | 1 | 23 | 6,498,082 | 135,161,499 | 203,036,510 | 8,716,288 | 8,716,288 |
| `grib1-spectral-complex-S/place` | 0 | 0 | 491 | 0 | 5 | 440 | 16,052 | 28,972 | — | — |
| `grib1-spectral-simple-L/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,155 | 26,871 | 1,703,936 | 1,703,936 |
| `grib1-spectral-simple-L/decode` | 259,920 | 33,038 | 33,134 | 1 | 22 | 6,498,082 | 303,715,299 | 438,885,798 | 9,764,864 | 9,764,864 |
| `grib1-spectral-simple-L/place` | 0 | 0 | 491 | 0 | 5 | 440 | 16,026 | 28,941 | — | — |
| `grib1-spectral-simple-S/open` | 0 | 192 | 491 | 6 | 4 | 816 | 15,217 | 26,910 | 1,638,400 | 1,638,400 |
| `grib1-spectral-simple-S/decode` | 259,920 | 8,334 | 8,430 | 1 | 22 | 6,498,082 | 135,074,801 | 202,911,931 | 8,716,288 | 8,716,288 |
| `grib1-spectral-simple-S/place` | 0 | 0 | 491 | 0 | 5 | 440 | 16,059 | 28,967 | — | — |
| `grib2-5.0-L/open` | 0 | 424 | 755 | 7 | 4 | 2,064 | 80,523 | 127,179 | 1,900,544 | 1,900,544 |
| `grib2-5.0-L/decode` | 65,160 | 130,331 | 130,499 | 1 | 17 | 1,889,658 | 12,874,568 | 18,039,621 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/place` | 0 | 0 | 755 | 0 | 5 | 440 | 16,075 | 30,353 | — | — |
| `grib2-5.0-L/warp` | 65,160 | 0 | 0 | 0 | 5 | 847,088 | 27,727,674 | 38,477,598 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/palette` | 0 | 0 | 0 | 0 | 1 | 1,576 | 137,681 | 194,775 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/render` | 65,160 | 0 | 0 | 0 | 3 | 781,920 | 6,641,384 | 8,925,036 | 3,735,552 | 3,735,552 |
| `grib2-5.0-L/contours` | 65,160 | 0 | 0 | 0 | 55 | 1,165,776 | 15,302,545 | 19,063,348 | 3,735,552 | 3,735,552 |
| `grib2-5.0-S/open` | 0 | 424 | 755 | 7 | 4 | 2,064 | 80,138 | 126,572 | 1,703,936 | 1,703,936 |
| `grib2-5.0-S/decode` | 16,380 | 32,771 | 32,939 | 1 | 17 | 475,038 | 3,301,237 | 4,567,839 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/place` | 0 | 0 | 755 | 0 | 5 | 440 | 16,791 | 31,379 | — | — |
| `grib2-5.0-S/warp` | 16,380 | 0 | 0 | 0 | 5 | 212,948 | 7,029,949 | 9,756,902 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/palette` | 0 | 0 | 0 | 0 | 1 | 1,576 | 136,525 | 192,854 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/render` | 16,380 | 0 | 0 | 0 | 3 | 196,560 | 1,804,286 | 2,457,138 | 2,162,688 | 2,162,688 |
| `grib2-5.0-S/contours` | 16,380 | 0 | 0 | 0 | 50 | 332,048 | 4,016,006 | 5,066,835 | 2,162,688 | 2,162,688 |
| `grib2-5.200-S/open` | 0 | 211 | 778 | 7 | 5 | 2,074 | 79,715 | 126,258 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/decode` | 496 | 20 | 211 | 1 | 19 | 14,402 | 124,623 | 197,326 | 1,638,400 | 1,638,400 |
| `grib2-5.200-S/place` | 0 | 0 | 778 | 0 | 5 | 440 | 16,080 | 30,400 | — | — |
| `grib2-5.3-L/open` | 0 | 403 | 783 | 7 | 4 | 2,064 | 78,960 | 124,925 | 1,703,936 | 1,703,936 |
| `grib2-5.3-L/decode` | 65,160 | 57,954 | 58,150 | 1 | 19 | 1,889,658 | 15,515,340 | 21,485,724 | 3,604,480 | 3,604,480 |
| `grib2-5.3-L/place` | 0 | 0 | 783 | 0 | 5 | 440 | 15,881 | 30,118 | — | — |
| `grib2-5.3-S/open` | 0 | 403 | 783 | 7 | 4 | 2,064 | 78,521 | 124,319 | 1,638,400 | 1,638,400 |
| `grib2-5.3-S/decode` | 16,380 | 18,586 | 18,782 | 1 | 19 | 475,038 | 4,125,952 | 5,673,870 | 2,097,152 | 2,097,152 |
| `grib2-5.3-S/place` | 0 | 0 | 783 | 0 | 5 | 440 | 16,119 | 30,505 | — | — |
| `grib2-5.40-L/open` | 0 | 403 | 757 | 7 | 4 | 2,064 | 78,219 | 123,716 | 1,638,400 | 1,638,400 |
| `grib2-5.40-L/decode` | 65,160 | 14,681 | 14,851 | 1 | 568 | 1,889,658 | 114,251,134 | 146,635,037 | 3,866,624 | 3,866,624 |
| `grib2-5.40-L/place` | 0 | 0 | 757 | 0 | 5 | 440 | 15,910 | 30,151 | — | — |
| `grib2-5.40-L/codec` | 65,160 | — | — | — | 553 | 859,764 | 108,724,384 | 137,706,114 | — | — |
| `grib2-5.40-S/open` | 0 | 403 | 757 | 7 | 4 | 2,064 | 78,200 | 123,653 | 1,638,400 | 1,638,400 |
| `grib2-5.40-S/decode` | 16,380 | 6,278 | 6,448 | 1 | 430 | 475,038 | 34,512,583 | 44,454,438 | 2,162,688 | 2,162,688 |
| `grib2-5.40-S/place` | 0 | 0 | 757 | 0 | 5 | 440 | 16,447 | 31,003 | — | — |
| `grib2-5.40-S/codec` | 16,380 | — | — | — | 415 | 221,460 | 33,115,250 | 42,271,433 | — | — |
| `grib2-5.41-L/open` | 0 | 424 | 755 | 7 | 4 | 2,064 | 77,977 | 123,371 | 1,835,008 | 1,835,008 |
| `grib2-5.41-L/decode` | 65,160 | 80,490 | 80,658 | 1 | 26 | 1,898,632 | 11,590,046 | 16,966,715 | 3,735,552 | 3,735,552 |
| `grib2-5.41-L/place` | 0 | 0 | 755 | 0 | 5 | 440 | 16,113 | 30,491 | — | — |
| `grib2-5.41-L/codec` | 130,320 | — | — | — | 11 | 334,744 | 5,479,306 | 7,256,089 | — | — |
| `grib2-5.41-S/open` | 0 | 424 | 755 | 7 | 4 | 2,064 | 77,679 | 122,979 | 1,638,400 | 1,638,400 |
| `grib2-5.41-S/decode` | 16,380 | 23,501 | 23,669 | 1 | 26 | 511,568 | 3,139,865 | 4,565,604 | 2,293,760 | 2,293,760 |
| `grib2-5.41-S/place` | 0 | 0 | 755 | 0 | 5 | 440 | 16,190 | 30,519 | — | — |
| `grib2-5.41-S/codec` | 32,760 | — | — | — | 11 | 118,400 | 1,603,620 | 2,181,725 | — | — |
| `grib2-5.42-L/open` | 0 | 403 | 759 | 7 | 4 | 2,064 | 77,375 | 122,541 | 1,835,008 | 1,835,008 |
| `grib2-5.42-L/decode` | 65,160 | 80,767 | 80,939 | 1 | 2,055 | 1,889,658 | 30,573,400 | 40,858,313 | 3,801,088 | 3,801,088 |
| `grib2-5.42-L/place` | 0 | 0 | 759 | 0 | 5 | 440 | 16,036 | 30,278 | — | — |
| `grib2-5.42-L/codec` | 130,320 | — | — | — | 2,040 | 130,448 | 23,764,691 | 30,369,854 | — | — |
| `grib2-5.42-S/open` | 0 | 403 | 759 | 7 | 4 | 2,064 | 77,068 | 122,040 | 1,638,400 | 1,638,400 |
| `grib2-5.42-S/decode` | 16,380 | 22,449 | 22,621 | 1 | 530 | 475,038 | 8,053,655 | 10,695,094 | 2,162,688 | 2,162,688 |
| `grib2-5.42-S/place` | 0 | 0 | 759 | 0 | 5 | 440 | 16,373 | 30,796 | — | — |
| `grib2-5.42-S/codec` | 32,760 | — | — | — | 515 | 32,888 | 6,303,877 | 8,044,977 | — | — |
| `netcdf-classic-D/open` | 0 | 2,098,344 | 1,704 | 1 | 103 | 4,982 | 37,280 | 65,918 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/variables` | 0 | 2,098,344 | 1,704 | 1 | 42 | 1,290 | 26,931 | 40,899 | 5,898,240 | 5,898,240 |
| `netcdf-classic-D/slice` | 16,380 | 2,098,344 | 67,224 | 1 | 120 | 8,649,499 | 20,430,012 | 32,351,737 | 14,286,848 | 14,286,848 |
| `netcdf-classic-D/scrub` | 131,040 | 2,098,344 | 525,864 | 1 | 610 | 8,649,872 | 162,577,027 | 256,139,176 | 14,286,848 | 14,286,848 |
| `netcdf-classic-D/place` | 0 | 2,098,344 | 1,704 | 1 | 96 | 7,723 | 124,490 | 193,683 | — | — |
| `netcdf-classic-L/open` | 0 | 2,087,712 | 2,592 | 1 | 103 | 4,982 | 36,713 | 65,167 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/variables` | 0 | 2,087,712 | 2,592 | 1 | 42 | 1,290 | 13,483 | 20,572 | 5,767,168 | 5,767,168 |
| `netcdf-classic-L/slice` | 65,160 | 2,087,712 | 263,232 | 1 | 120 | 9,383,899 | 24,496,026 | 40,430,140 | 14,155,776 | 14,155,776 |
| `netcdf-classic-L/scrub` | 521,280 | 2,087,712 | 2,087,712 | 1 | 610 | 9,384,272 | 195,094,494 | 318,119,033 | 14,155,776 | 14,155,776 |
| `netcdf-classic-L/place` | 0 | 2,087,712 | 2,592 | 1 | 96 | 14,203 | 154,563 | 235,684 | — | — |
| `netcdf-classic-S/open` | 0 | 525,672 | 1,512 | 1 | 103 | 4,982 | 37,031 | 65,694 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/variables` | 0 | 525,672 | 1,512 | 1 | 42 | 1,290 | 26,942 | 40,836 | 2,752,512 | 2,752,512 |
| `netcdf-classic-S/slice` | 16,380 | 525,672 | 67,032 | 1 | 120 | 2,359,579 | 6,276,282 | 10,336,663 | 4,849,664 | 4,849,664 |
| `netcdf-classic-S/scrub` | 131,040 | 525,672 | 525,672 | 1 | 610 | 2,359,952 | 49,370,910 | 71,272,736 | 4,849,664 | 4,849,664 |
| `netcdf-classic-S/place` | 0 | 525,672 | 1,512 | 1 | 96 | 7,723 | 136,918 | 211,790 | — | — |
| `netcdf4-zlib-D/open` | 0 | 1,116,265 | 12,942 | 1 | 379 | 14,503 | 350,124 | 510,384 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/variables` | 0 | 1,116,265 | 12,942 | 1 | 42 | 1,290 | 27,089 | 40,899 | 3,932,160 | 3,932,160 |
| `netcdf4-zlib-D/slice` | 16,380 | 1,116,265 | 47,445 | 1 | 420 | 10,486,291 | 183,714,668 | 248,808,027 | 14,417,920 | 14,417,920 |
| `netcdf4-zlib-D/scrub` | 131,040 | 1,116,265 | 288,808 | 1 | 2,233 | 10,486,664 | 1,475,528,154 | 1,995,557,217 | 14,417,920 | 14,417,920 |
| `netcdf4-zlib-D/place` | 0 | 1,116,265 | 12,942 | 1 | 168 | 7,699 | 250,374 | 360,088 | — | — |
| `netcdf4-zlib-D/codec` | 65,520 | — | — | — | 4 | 79,510 | 1,246,813 | 1,708,266 | — | — |
| `netcdf4-zlib-L/open` | 0 | 1,006,286 | 14,990 | 1 | 379 | 14,503 | 347,880 | 507,560 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/variables` | 0 | 1,006,286 | 14,990 | 1 | 42 | 1,290 | 27,020 | 40,778 | 3,670,016 | 3,670,016 |
| `netcdf4-zlib-L/slice` | 65,160 | 1,006,286 | 139,063 | 1 | 282 | 10,427,155 | 180,863,924 | 248,876,242 | 14,155,776 | 14,155,776 |
| `netcdf4-zlib-L/scrub` | 521,280 | 1,006,286 | 1,006,286 | 1 | 1,311 | 10,427,528 | 1,456,872,997 | 1,994,789,654 | 14,155,776 | 14,155,776 |
| `netcdf4-zlib-L/place` | 0 | 1,006,286 | 14,990 | 1 | 168 | 14,179 | 283,384 | 405,804 | — | — |
| `netcdf4-zlib-L/codec` | 260,640 | — | — | — | 5 | 506,796 | 3,995,906 | 5,654,625 | — | — |
| `netcdf4-zlib-S/open` | 0 | 288,808 | 12,942 | 1 | 379 | 14,503 | 349,351 | 509,577 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/variables` | 0 | 288,808 | 12,942 | 1 | 42 | 1,290 | 27,075 | 40,855 | 2,228,224 | 2,228,224 |
| `netcdf4-zlib-S/slice` | 16,380 | 288,808 | 47,445 | 1 | 274 | 2,622,355 | 47,220,709 | 64,518,050 | 4,849,664 | 4,849,664 |
| `netcdf4-zlib-S/scrub` | 131,040 | 288,808 | 288,808 | 1 | 1,247 | 2,622,728 | 378,016,251 | 506,646,729 | 4,849,664 | 4,849,664 |
| `netcdf4-zlib-S/place` | 0 | 288,808 | 12,942 | 1 | 168 | 7,699 | 249,864 | 359,134 | — | — |
| `netcdf4-zlib-S/codec` | 65,520 | — | — | — | 4 | 79,510 | 1,240,233 | 1,695,179 | — | — |
| `zarr-v2-blosc-D/open` | 0 | 1,158 | 1,584 | 4 | 369 | 14,038 | 287,222 | 455,654 | — | — |
| `zarr-v2-blosc-D/variables` | 0 | 0 | 1,584 | 0 | 36 | 1,186 | 10,988 | 17,188 | — | — |
| `zarr-v2-blosc-D/slice` | 16,380 | 37,996 | 39,154 | 3 | 171 | 738,092 | 4,213,629 | 6,776,648 | — | — |
| `zarr-v2-blosc-D/scrub` | 131,040 | 300,515 | 301,673 | 10 | 773 | 738,092 | 32,662,767 | 49,371,722 | — | — |
| `zarr-v2-blosc-D/place` | 0 | 426 | 1,584 | 2 | 122 | 9,347 | 161,105 | 252,374 | — | — |
| `zarr-v2-blosc-D/codec` | 65,520 | — | — | — | 7 | 196,560 | 1,507,744 | 2,412,483 | — | — |
| `zarr-v2-blosc-L/open` | 0 | 1,161 | 1,900 | 4 | 320 | 14,038 | 249,072 | 403,227 | — | — |
| `zarr-v2-blosc-L/variables` | 0 | 0 | 1,900 | 0 | 36 | 1,186 | 11,012 | 17,228 | — | — |
| `zarr-v2-blosc-L/slice` | 65,160 | 144,560 | 145,721 | 3 | 174 | 2,933,192 | 16,234,806 | 26,546,671 | — | — |
| `zarr-v2-blosc-L/scrub` | 521,280 | 1,141,368 | 1,142,529 | 10 | 783 | 2,933,192 | 128,333,833 | 194,451,920 | — | — |
| `zarr-v2-blosc-L/place` | 0 | 739 | 1,900 | 2 | 124 | 17,267 | 206,711 | 313,990 | — | — |
| `zarr-v2-blosc-L/codec` | 260,640 | — | — | — | 8 | 781,920 | 5,796,452 | 9,339,289 | — | — |
| `zarr-v2-blosc-S/open` | 0 | 1,157 | 1,583 | 4 | 320 | 14,038 | 248,942 | 403,241 | — | — |
| `zarr-v2-blosc-S/variables` | 0 | 0 | 1,583 | 0 | 36 | 1,186 | 10,988 | 17,192 | — | — |
| `zarr-v2-blosc-S/slice` | 16,380 | 37,996 | 39,153 | 3 | 171 | 738,092 | 4,212,988 | 6,771,102 | — | — |
| `zarr-v2-blosc-S/scrub` | 131,040 | 300,515 | 301,672 | 10 | 773 | 738,092 | 32,667,640 | 49,381,290 | — | — |
| `zarr-v2-blosc-S/place` | 0 | 426 | 1,583 | 2 | 122 | 9,347 | 160,609 | 251,908 | — | — |
| `zarr-v2-blosc-S/codec` | 65,520 | — | — | — | 7 | 196,560 | 1,506,523 | 2,410,183 | — | — |
| `zarr-v3-sharded-D/open` | 0 | 2,800 | 3,413 | 3 | 393 | 35,909 | 295,200 | 464,955 | — | — |
| `zarr-v3-sharded-D/variables` | 0 | 0 | 3,413 | 0 | 36 | 1,186 | 10,892 | 17,069 | — | — |
| `zarr-v3-sharded-D/slice` | 16,380 | 139,799 | 142,599 | 3 | 397 | 1,967,115 | 20,566,180 | 33,032,644 | — | — |
| `zarr-v3-sharded-D/scrub` | 131,040 | 278,830 | 281,630 | 10 | 2,259 | 1,967,488 | 162,378,873 | 254,283,880 | — | — |
| `zarr-v3-sharded-D/place` | 0 | 613 | 3,413 | 2 | 168 | 17,437 | 311,550 | 473,239 | — | — |
| `zarr-v3-sharded-L/open` | 0 | 2,804 | 4,019 | 3 | 380 | 35,909 | 288,090 | 454,524 | — | — |
| `zarr-v3-sharded-L/variables` | 0 | 0 | 4,019 | 0 | 36 | 1,186 | 10,883 | 17,082 | — | — |
| `zarr-v3-sharded-L/slice` | 65,160 | 526,253 | 529,057 | 3 | 551 | 7,820,715 | 84,245,651 | 136,548,070 | — | — |
| `zarr-v3-sharded-L/scrub` | 521,280 | 1,058,791 | 1,061,595 | 10 | 3,483 | 7,821,088 | 695,274,314 | 1,093,691,958 | — | — |
| `zarr-v3-sharded-L/place` | 0 | 1,215 | 4,019 | 2 | 172 | 24,632 | 407,877 | 603,792 | — | — |
| `zarr-v3-sharded-S/open` | 0 | 2,799 | 3,412 | 3 | 380 | 35,909 | 289,192 | 456,038 | — | — |
| `zarr-v3-sharded-S/variables` | 0 | 0 | 3,412 | 0 | 36 | 1,186 | 10,892 | 17,065 | — | — |
| `zarr-v3-sharded-S/slice` | 16,380 | 139,799 | 142,598 | 3 | 397 | 1,967,115 | 20,566,349 | 33,031,309 | — | — |
| `zarr-v3-sharded-S/scrub` | 131,040 | 278,830 | 281,629 | 10 | 2,259 | 1,967,488 | 162,074,206 | 253,350,891 | — | — |
| `zarr-v3-sharded-S/place` | 0 | 613 | 3,412 | 2 | 168 | 17,437 | 311,789 | 473,570 | — | — |
| `zarr-v3-zstd-D/open` | 0 | 2,186 | 2,799 | 3 | 387 | 32,208 | 316,756 | 490,633 | — | — |
| `zarr-v3-zstd-D/variables` | 0 | 0 | 2,799 | 0 | 36 | 1,186 | 10,907 | 17,106 | — | — |
| `zarr-v3-zstd-D/slice` | 16,380 | 52,381 | 54,567 | 3 | 240 | 738,092 | 6,572,673 | 10,485,659 | — | — |
| `zarr-v3-zstd-D/scrub` | 131,040 | 414,719 | 416,905 | 10 | 1,021 | 738,092 | 50,626,452 | 77,715,111 | — | — |
| `zarr-v3-zstd-D/place` | 0 | 613 | 2,799 | 2 | 168 | 17,437 | 310,533 | 472,852 | — | — |
| `zarr-v3-zstd-D/codec` | 65,520 | — | — | — | 31 | 258,467 | 4,070,495 | 5,955,837 | — | — |
| `zarr-v3-zstd-L/open` | 0 | 2,189 | 3,404 | 3 | 337 | 32,208 | 265,629 | 421,203 | — | — |
| `zarr-v3-zstd-L/variables` | 0 | 0 | 3,404 | 0 | 36 | 1,186 | 10,889 | 17,072 | — | — |
| `zarr-v3-zstd-L/slice` | 65,160 | 207,207 | 209,396 | 3 | 258 | 2,933,192 | 25,159,156 | 40,883,780 | — | — |
| `zarr-v3-zstd-L/scrub` | 521,280 | 1,645,094 | 1,647,283 | 10 | 1,110 | 2,933,192 | 200,092,357 | 308,157,917 | — | — |
| `zarr-v3-zstd-L/place` | 0 | 1,215 | 3,404 | 2 | 172 | 24,632 | 405,755 | 601,919 | — | — |
| `zarr-v3-zstd-L/warp` | 65,160 | 0 | 0 | 0 | 5 | 847,088 | 27,718,147 | 38,309,795 | — | — |
| `zarr-v3-zstd-L/palette` | 0 | 0 | 0 | 0 | 1 | 1,576 | 127,760 | 179,344 | — | — |
| `zarr-v3-zstd-L/render` | 65,160 | 0 | 0 | 0 | 3 | 781,920 | 6,758,654 | 9,047,931 | — | — |
| `zarr-v3-zstd-L/contours` | 65,160 | 0 | 0 | 0 | 56 | 1,182,160 | 15,298,940 | 18,579,856 | — | — |
| `zarr-v3-zstd-L/codec` | 260,640 | — | — | — | 45 | 769,444 | 15,887,968 | 23,325,794 | — | — |
| `zarr-v3-zstd-S/open` | 0 | 2,185 | 2,798 | 3 | 337 | 32,208 | 264,842 | 420,003 | — | — |
| `zarr-v3-zstd-S/variables` | 0 | 0 | 2,798 | 0 | 36 | 1,186 | 10,906 | 17,101 | — | — |
| `zarr-v3-zstd-S/slice` | 16,380 | 52,381 | 54,566 | 3 | 240 | 738,092 | 6,570,532 | 10,483,470 | — | — |
| `zarr-v3-zstd-S/scrub` | 131,040 | 414,719 | 416,904 | 10 | 1,021 | 738,092 | 50,628,402 | 77,717,412 | — | — |
| `zarr-v3-zstd-S/place` | 0 | 613 | 2,798 | 2 | 168 | 17,437 | 310,300 | 473,086 | — | — |
| `zarr-v3-zstd-S/warp` | 16,380 | 0 | 0 | 0 | 5 | 212,948 | 7,020,167 | 9,710,552 | — | — |
| `zarr-v3-zstd-S/palette` | 0 | 0 | 0 | 0 | 1 | 1,576 | 126,969 | 178,068 | — | — |
| `zarr-v3-zstd-S/render` | 16,380 | 0 | 0 | 0 | 3 | 196,560 | 1,794,331 | 2,409,307 | — | — |
| `zarr-v3-zstd-S/contours` | 16,380 | 0 | 0 | 0 | 49 | 323,856 | 3,990,960 | 4,891,576 | — | — |
| `zarr-v3-zstd-S/codec` | 65,520 | — | — | — | 31 | 258,467 | 4,085,962 | 6,008,740 | — | — |
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
| `grib1-second-order/open` | 4 | 4 | holds |
| `grib1-second-order/decode` | 21 | 21 | holds |
| `grib1-second-order/place` | 4 | 4 | holds |
| `grib1-simple/open` | 4 | 4 | holds |
| `grib1-simple/decode` | 16 | 16 | holds |
| `grib1-simple/place` | 4 | 4 | holds |
| `grib1-spectral-complex/open` | 4 | 4 | holds |
| `grib1-spectral-complex/decode` | 23 | 23 | holds |
| `grib1-spectral-complex/place` | 5 | 5 | holds |
| `grib1-spectral-simple/open` | 4 | 4 | holds |
| `grib1-spectral-simple/decode` | 22 | 22 | holds |
| `grib1-spectral-simple/place` | 5 | 5 | holds |
| `grib2-5.0/open` | 4 | 4 | holds |
| `grib2-5.0/decode` | 17 | 17 | holds |
| `grib2-5.0/place` | 5 | 5 | holds |
| `grib2-5.0/warp` | 5 | 5 | holds |
| `grib2-5.0/palette` | 1 | 1 | holds |
| `grib2-5.0/render` | 3 | 3 | holds |
| `grib2-5.0/contours` | 50 | 55 | grows +5 |
| `grib2-5.3/open` | 4 | 4 | holds |
| `grib2-5.3/decode` | 19 | 19 | holds |
| `grib2-5.3/place` | 5 | 5 | holds |
| `grib2-5.40/open` | 4 | 4 | holds |
| `grib2-5.40/decode` | 430 | 568 | grows +138 |
| `grib2-5.40/place` | 5 | 5 | holds |
| `grib2-5.40/codec` | 415 | 553 | grows +138 |
| `grib2-5.41/open` | 4 | 4 | holds |
| `grib2-5.41/decode` | 26 | 26 | holds |
| `grib2-5.41/place` | 5 | 5 | holds |
| `grib2-5.41/codec` | 11 | 11 | holds |
| `grib2-5.42/open` | 4 | 4 | holds |
| `grib2-5.42/decode` | 530 | 2,055 | grows +1,525 |
| `grib2-5.42/place` | 5 | 5 | holds |
| `grib2-5.42/codec` | 515 | 2,040 | grows +1,525 |
| `netcdf-classic/open` | 103 | 103 | holds |
| `netcdf-classic/variables` | 42 | 42 | holds |
| `netcdf-classic/slice` | 120 | 120 | holds |
| `netcdf-classic/scrub` | 610 | 610 | holds |
| `netcdf-classic/place` | 96 | 96 | holds |
| `netcdf4-zlib/open` | 379 | 379 | holds |
| `netcdf4-zlib/variables` | 42 | 42 | holds |
| `netcdf4-zlib/slice` | 274 | 282 | grows +8 |
| `netcdf4-zlib/scrub` | 1,247 | 1,311 | grows +64 |
| `netcdf4-zlib/place` | 168 | 168 | holds |
| `netcdf4-zlib/codec` | 4 | 5 | grows +1 |
| `zarr-v2-blosc/open` | 320 | 320 | holds |
| `zarr-v2-blosc/variables` | 36 | 36 | holds |
| `zarr-v2-blosc/slice` | 171 | 174 | grows +3 |
| `zarr-v2-blosc/scrub` | 773 | 783 | grows +10 |
| `zarr-v2-blosc/place` | 122 | 124 | grows +2 |
| `zarr-v2-blosc/codec` | 7 | 8 | grows +1 |
| `zarr-v3-sharded/open` | 380 | 380 | holds |
| `zarr-v3-sharded/variables` | 36 | 36 | holds |
| `zarr-v3-sharded/slice` | 397 | 551 | grows +154 |
| `zarr-v3-sharded/scrub` | 2,259 | 3,483 | grows +1,224 |
| `zarr-v3-sharded/place` | 168 | 172 | grows +4 |
| `zarr-v3-zstd/open` | 337 | 337 | holds |
| `zarr-v3-zstd/variables` | 36 | 36 | holds |
| `zarr-v3-zstd/slice` | 240 | 258 | grows +18 |
| `zarr-v3-zstd/scrub` | 1,021 | 1,110 | grows +89 |
| `zarr-v3-zstd/place` | 168 | 172 | grows +4 |
| `zarr-v3-zstd/codec` | 31 | 45 | grows +14 |
| `zarr-v3-zstd/warp` | 5 | 5 | holds |
| `zarr-v3-zstd/palette` | 1 | 1 | holds |
| `zarr-v3-zstd/render` | 3 | 3 | holds |
| `zarr-v3-zstd/contours` | 49 | 56 | grows +7 |

#### Peak heap per cell

`B` is the slope between `S` and `L`: `(peak(L) − peak(S)) / (cells(L) − cells(S))`,
so a fixed overhead `C` cancels out. The floor is the least any implementation
could hold per cell for that operation.

| Operation | `B` today | `C` today | Floor `B` | Gap | Floor is |
|---|---:|---:|---:|---:|---|
| `grib1-second-order/decode` | 32.3 B | 3,383 B | 9 B | 3.6× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib1-simple/decode` | 29.0 B | 26 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib2-5.0/decode` | 29.0 B | 18 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib2-5.0/warp` | 13.0 B | 8 B | 5 B | 2.6× | f32 output (4) + mask (1) per output pixel |
| `grib2-5.0/render` | 12.0 B | 0 B | 4 B | 3.0× | one RGBA pixel |
| `grib2-5.0/contours` | 17.1 B | 52,088 B | 0 B | +17.1 B | marching squares needs a row of state, not a cell's |
| `grib2-5.3/decode` | 29.0 B | 18 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib2-5.40/decode` | 29.0 B | 18 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib2-5.41/decode` | 28.4 B | 45,801 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `grib2-5.42/decode` | 29.0 B | 18 B | 9 B | 3.2× | f64 output value (8) + mask byte (1); simple unpacking needs no buffer |
| `netcdf-classic/slice` | 144.0 B | 859 B | 5 B | 28.8× | f32 output (4) + mask (1); the plane is read in place |
| `netcdf-classic/scrub` | 18.0 B | 1,232 B | 5 B | 3.6× | f32 output (4) + mask (1); the plane is read in place |
| `netcdf4-zlib/slice` | 160.0 B | 1,555 B | 9 B | 17.8× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `netcdf4-zlib/scrub` | 20.0 B | 1,928 B | 9 B | 2.2× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/slice` | 45.0 B | 992 B | 9 B | 5.0× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v2-blosc/scrub` | 5.6 B | 992 B | 9 B | 0.6× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/slice` | 120.0 B | 1,515 B | 9 B | 13.3× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-sharded/scrub` | 15.0 B | 1,888 B | 9 B | 1.7× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/slice` | 45.0 B | 992 B | 9 B | 5.0× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/scrub` | 5.6 B | 992 B | 9 B | 0.6× | f32 output (4) + mask (1) + one decompressed f32 chunk element (4) |
| `zarr-v3-zstd/warp` | 13.0 B | 8 B | 5 B | 2.6× | f32 output (4) + mask (1) per output pixel |
| `zarr-v3-zstd/render` | 12.0 B | 0 B | 4 B | 3.0× | one RGBA pixel |
| `zarr-v3-zstd/contours` | 17.6 B | 35,643 B | 0 B | +17.6 B | marching squares needs a row of state, not a cell's |

#### Work against the variable, not the plane

A slice and a scrub at `D` (four times the variable, the same plane) against
`S`. The bound is a ratio of 1: the planes nobody asked for cost nothing.

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
| `grib1-second-order/decode` | 3,421,572 | 12,241,679 | 3.58 |
| `grib1-simple/decode` | 3,301,816 | 12,874,502 | 3.90 |
| `grib1-spectral-complex/decode` | 135,161,499 | 303,913,338 | 2.25 |
| `grib1-spectral-simple/decode` | 135,074,801 | 303,715,299 | 2.25 |
| `grib2-5.0/decode` | 3,301,237 | 12,874,568 | 3.90 |
| `grib2-5.0/warp` | 7,029,949 | 27,727,674 | 3.94 |
| `grib2-5.0/render` | 1,804,286 | 6,641,384 | 3.68 |
| `grib2-5.0/contours` | 4,016,006 | 15,302,545 | 3.81 |
| `grib2-5.3/decode` | 4,125,952 | 15,515,340 | 3.76 |
| `grib2-5.40/decode` | 34,512,583 | 114,251,134 | 3.31 |
| `grib2-5.40/codec` | 33,115,250 | 108,724,384 | 3.28 |
| `grib2-5.41/decode` | 3,139,865 | 11,590,046 | 3.69 |
| `grib2-5.41/codec` | 1,603,620 | 5,479,306 | 3.42 |
| `grib2-5.42/decode` | 8,053,655 | 30,573,400 | 3.80 |
| `grib2-5.42/codec` | 6,303,877 | 23,764,691 | 3.77 |
| `netcdf-classic/slice` | 6,276,282 | 24,496,026 | 3.90 |
| `netcdf-classic/scrub` | 49,370,910 | 195,094,494 | 3.95 |
| `netcdf4-zlib/slice` | 47,220,709 | 180,863,924 | 3.83 |
| `netcdf4-zlib/scrub` | 378,016,251 | 1,456,872,997 | 3.85 |
| `netcdf4-zlib/codec` | 1,240,233 | 3,995,906 | 3.22 |
| `zarr-v2-blosc/slice` | 4,212,988 | 16,234,806 | 3.85 |
| `zarr-v2-blosc/scrub` | 32,667,640 | 128,333,833 | 3.93 |
| `zarr-v2-blosc/codec` | 1,506,523 | 5,796,452 | 3.85 |
| `zarr-v3-sharded/slice` | 20,566,349 | 84,245,651 | 4.10 |
| `zarr-v3-sharded/scrub` | 162,074,206 | 695,274,314 | 4.29 |
| `zarr-v3-zstd/slice` | 6,570,532 | 25,159,156 | 3.83 |
| `zarr-v3-zstd/scrub` | 50,628,402 | 200,092,357 | 3.95 |
| `zarr-v3-zstd/codec` | 4,085,962 | 15,887,968 | 3.89 |
| `zarr-v3-zstd/warp` | 7,020,167 | 27,718,147 | 3.95 |
| `zarr-v3-zstd/render` | 1,794,331 | 6,758,654 | 3.77 |
| `zarr-v3-zstd/contours` | 3,990,960 | 15,298,940 | 3.83 |

#### Decode above its codec

The share of a decode's instructions spent outside the decompressor, over
the same bytes. What is left once the codec's ceiling is taken out.

| Input | Decode | Codec alone | Outside the codec |
|---|---:|---:|---:|
| `grib2-5.40-L` | 114,251,134 | 108,724,384 | 5% |
| `grib2-5.40-S` | 34,512,583 | 33,115,250 | 4% |
| `grib2-5.41-L` | 11,590,046 | 5,479,306 | 53% |
| `grib2-5.41-S` | 3,139,865 | 1,603,620 | 49% |
| `grib2-5.42-L` | 30,573,400 | 23,764,691 | 22% |
| `grib2-5.42-S` | 8,053,655 | 6,303,877 | 22% |
| `netcdf4-zlib-D` | 183,714,668 | 1,246,813 | 99% |
| `netcdf4-zlib-L` | 180,863,924 | 3,995,906 | 98% |
| `netcdf4-zlib-S` | 47,220,709 | 1,240,233 | 97% |
| `zarr-v2-blosc-D` | 4,213,629 | 1,507,744 | 64% |
| `zarr-v2-blosc-L` | 16,234,806 | 5,796,452 | 64% |
| `zarr-v2-blosc-S` | 4,212,988 | 1,506,523 | 64% |
| `zarr-v3-zstd-D` | 6,572,673 | 4,070,495 | 38% |
| `zarr-v3-zstd-L` | 25,159,156 | 15,887,968 | 37% |
| `zarr-v3-zstd-S` | 6,570,532 | 4,085,962 | 38% |
<!-- perf-gate:bounds:end -->
