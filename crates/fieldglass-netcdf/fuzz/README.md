# fieldglass-netcdf fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the NetCDF
header parse path. `fieldglass-netcdf` parses attacker-controllable bytes; the
classic (CDF-1/2/5) path walks the dim_list / gatt_list / var_list with
offset- and length-driven reads, so this drives `NetcdfReader::from_bytes`
against arbitrary input and asserts it never panics, over-reads, or hangs.

HDF5 input only reaches the lightweight superblock probe today, so the classic
header parser is the substance of what this exercises. Revisit an HDF5 target
once that path does real decode rather than just probing.

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
