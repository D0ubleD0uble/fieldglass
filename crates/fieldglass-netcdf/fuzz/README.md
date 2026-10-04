# fieldglass-netcdf fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the NetCDF
parse path. `fieldglass-netcdf` parses attacker-controllable bytes; the classic
(CDF-1/2/5) path walks the dim_list / gatt_list / var_list with offset- and
length-driven reads, so this drives `NetcdfReader::from_bytes` against arbitrary
input and asserts it never panics, over-reads, or hangs.

For NetCDF-4 / HDF5 input the target also drives `NetcdfReader::hdf5_metadata`,
the on-demand deep walk — object headers, group and link tables, dense-attribute
fractal heaps and B-tree v2 indexes, and the filter pipeline — so the bounded,
fail-safe traversal hardened under #33 is fuzzed alongside the classic header
parser, not just the eager superblock probe. It then decodes the values of the
first few variables, which reads every chunk through the filter pipeline
(deflate, shuffle, fletcher32, zstd and szip) and the chunk indexes that locate
them.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-netcdf/fuzz
cargo +nightly fuzz run parse
```

The seed corpus under `corpus/parse/` is some of the crate's NetCDF test fixtures,
including the three szip files (#421), plus `oom_large_fill_dataset.h5`. That one
is the 13 KB input on which the time-boxed CI run reported out-of-memory: a
mutated HDF5 file whose second dataset declares a chunked 9,175,044 × 16 shape
of four-byte elements. It is inside the reader's
whole-variable cap and its decode needs about 2.9 GB, past libFuzzer's 2 GB RSS
limit, so the target reads each variable's shape first and skips decoding a
large one (see `MAX_FUZZ_DECODE_ELEMENTS`). The seed keeps that skip in place. CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
