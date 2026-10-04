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

The seed corpus under `corpus/decode/` is the crate's GRIB1 test fixtures, plus
`hand_matrix_of_values_all_absent.grib1`: `hand_matrix_of_values.grib1` with the
BMS body zeroed, N (BDS octets 12-13) set to 0 and NR = NC = 0xFFFF (octets
15-18). It is the #802 message, built the same way by the
`an_all_absent_bitmap_with_a_huge_matrix_is_refused_before_allocating` test.
`constant_field_8192x8192.grib1` is `ecmwf_lfpw_msg0.grib1`'s PDS with a GDS
stating Ni = Nj = 8192 and a 12-byte BDS at zero bits per value, 84 bytes in
all. A constant field needs no data, so it decodes to a gigabyte, about a
second of writes per message, and a dozen in one input passed the time-boxed
run's ten-second timeout. The target skips a decode that size (see
`FUZZ_MAX_FIELD_POINTS`), and the seed keeps that skip in place.
`hand_matrix_of_values_all_absent_367x367.grib1` is the all-absent matrix seed
above with NR = NC = 367 instead of 0xFFFF: its 496 points times 367² cells are
66,805,744, inside the cap, so it decodes to a gigabyte in about 1.5 s. The
target multiplies the grid by the `NR·NC` it reads from BDS octets 15-18 and
skips it. The target also decodes at most eight messages per input
(`MAX_DECODED_MESSAGES`): each gate bounds one message, but the run's timeout
bounds a whole input, and 200 copies of `constant_field_8192x8192.grib1` with
Ni = Nj = 2048, each inside the gate, took 11.8 s without it. That input is not
a seed, since each mutant of it would pay the cap's full cost. CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
