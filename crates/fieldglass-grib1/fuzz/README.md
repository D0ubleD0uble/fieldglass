# fieldglass-grib1 fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the GRIB1
decode path. `fieldglass-grib1` parses attacker-controllable bytes
(IS/PDS/GDS/BMS/BDS), so this drives the full scan-plus-decode pipeline against
arbitrary input and asserts it never panics, over-reads, or hangs.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-grib1/fuzz
cargo +nightly fuzz run decode
```

The seed corpus under `corpus/decode/` is the crate's GRIB1 test fixtures. CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
