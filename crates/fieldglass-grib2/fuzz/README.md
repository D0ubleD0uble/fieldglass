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
keeps that skip in place. Three more seeds are the same kind of message for the
other decode entry points, each zero bits per value with §7 cut to its header:

- `constant_field_healpix_nside2364.grib2` (149 bytes): `healpix_n2_ring.grib2`
  with Nside = 2364 and both counts 12·2364² = 67,061,952. The HEALPix resample
  decodes the values first, about a second.
- `constant_field_spectral_t8191.grib2` (155 bytes): `spectral_simple_t63.grib2`
  with J = K = M = 8191, 67,117,056 coefficients (537 MB) in about 0.4 s. The
  spectral decode is sized by J, so the target skips it past
  `FUZZ_MAX_TRUNCATION`.
- `constant_field_bifourier_4095.grib2` (1,294 bytes):
  `bifourier_rectangle_keepaxes.grib2` with both truncations 4095, so §5 counts
  4·4096² = 67,108,864 coefficients, about 4.5 s.

CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
