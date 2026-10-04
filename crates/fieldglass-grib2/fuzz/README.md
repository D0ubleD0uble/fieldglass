# fieldglass-grib2 fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the GRIB2
decode path. `fieldglass-grib2` parses attacker-controllable bytes
(IS/IDS/LUS/GDS/PDS/DRS/BMS/DS), so this drives the full scan-plus-decode
pipeline against arbitrary input and asserts it never panics, over-reads, or
hangs. The §5 DRS templates each carry their own length/offset-driven bit
unpacking, the same hazard class GRIB1 fuzzing surfaced.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-grib2/fuzz
cargo +nightly fuzz run decode
```

The seed corpus under `corpus/decode/` is the crate's GRIB2 test fixtures, plus
`constant_field_8192x8192.grib2`: `regular_latlon_surface.grib2` with Ni = Nj =
8192, the §3 and §5 point counts set to 8192², zero bits per value and §7 cut to
its 5-byte header, 196 bytes in all. It is a constant field, so it needs no
data, and it decodes to a gigabyte, about a second of writes per message; a
dozen of them in one input passed the time-boxed run's ten-second timeout. The
target skips a decode that size (see `FUZZ_MAX_FIELD_POINTS`), and the seed
keeps that skip in place. CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
