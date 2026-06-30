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
parser, not just the eager superblock probe.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-netcdf/fuzz
cargo +nightly fuzz run parse
```

The seed corpus under `corpus/parse/` is the crate's NetCDF test fixtures. CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
