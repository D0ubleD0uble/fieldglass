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
keeps that skip in place. Four more seeds are the same kind of message for the
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
- `constant_field_bifourier_ellipse_wide.grib2` (145 bytes): the message above
  with §3 cut to its bi-Fourier head (N = 16,783,359, M = 0, an ellipse
  truncation), a minimal §4 (template 4.0), §6 = 255, an empty §7 and a §5
  count of 80. The reader used to build and walk the 134 MB truncation layout
  before it found §5 disagrees, about 0.3 s. It now counts the layout first
  and refuses at once (#849), so the target gates bi-Fourier on the larger of
  §5's count and the rows the count walks, and runs this seed.

`jpeg2000_codestream_8192x8192_on_1x1.grib2` (300 bytes) is
`jpeg2000_regular_latlon.grib2` with a 1 × 1 grid, both counts 1, no bitmap,
and §7 replaced by a 102-byte single-tile codestream whose SIZ states an
8192 × 8192 image (5 decomposition levels, empty packets). The reader used to
decode the whole codestream before it compared the sample count with the
field's, about 14 s. It now reads SIZ first and refuses the image at once
(#848). The target still skips an image past `FUZZ_MAX_J2K_SAMPLES`, since a
codestream that does match its field costs far more per sample than the other
packings.

`jpeg2000_codestream_128x128_max_passes.grib2` (401 bytes) is the worst case
measured among the codestreams the gate admits (#838). I varied the coded
bytes (0x00, 0x7F, 0x80, 0xAA, 0xFE, 0xFF, random), the code-block size (16 × 16
to 64 × 64) and the QCD exponent (two settings); the pass count was always the
maximum, and tiles, precincts and layers were not varied. It is a 128 × 128
single-component image
(5 levels, 64 × 64 code-blocks, one layer) in which every code-block declares
164 passes in two segments of one byte each, every coded byte is 0xFF, and QCD
sets 30 bit-planes, so the decoder runs all 88 coding passes over each sample
from 203 bytes of codestream. On the fuzz build that is about 25 µs per sample
(random coded bytes cost a third of that), so one message takes 0.41 s and the
eight an input decodes take 3.3 s. The target does not run the HEALPix resample
on a JPEG 2000 message, since that decodes it a second time. The same codestream at 512 × 512 took 6.5 s
(26 s at 1024 × 1024 by scaling), which is why `FUZZ_MAX_J2K_SAMPLES` is 2^14
rather than 2^20. Rebuild it with `tools/build_grib2_j2k_fuzz_seed.py`.

The target also decodes at most eight messages per input
(`MAX_DECODED_MESSAGES`): each gate bounds one message, but the run's timeout
bounds a whole input. At the budget a bi-Fourier message at 1023 × 1023 costs
about 280 ms and a HEALPix message at Nside 591 about 150 ms, and 68 of the
latter in one 10 KB input took 9.5 s; with the cap, 70 take 1.2 s.

CI
runs this target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
