# fieldglass-aec fuzzing

[`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) targets for the AEC
decoder. GRIB2 template 5.42 payloads and HDF5 szip chunks are
attacker-controlled bytes, so the decoder is fuzzed before either reader uses it.

- `decode` decodes arbitrary bytes under arbitrary valid parameters. It asserts
  that nothing panics or overflows, that the sink never receives more than the
  requested count, that `Ok` means exactly that count arrived, that `Truncated`
  reports what the sink saw, and that a stream without preprocessing yields
  only n-bit values.
- `differential` runs `decode` and `decode_to_bytes` on the same input and
  asserts they agree on the verdict and, on success, on every byte. The byte
  layout it compares against is written out in the target, not taken from the
  crate.
- `sz` runs `sz::decompress` on arbitrary bytes under arbitrary valid szip
  parameters and output lengths, against a reference written in the target the
  way libsz does it: decode the padded stream into a buffer, drop each
  scanline's pads, then deinterleave byte planes. It asserts the two agree on
  the verdict and, on success, on every byte, and that a length that is not a
  whole number of pixels is refused by name.

Each input is a 7-byte header, then the stream. The header maps to a parameter
set the crate accepts (bits per sample, block size, reference sample interval,
flags, sample count); `fuzz_targets/common.rs` has the layout. The `sz`
target reads an 8-byte szip header instead (mask, bits per pixel, pixels per
block and per scanline, output length); `fuzz_targets/sz.rs` has that layout.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates never try to build the nightly-only libFuzzer targets.

## Run

```sh
# from crates/fieldglass-aec/fuzz
cargo +nightly fuzz run decode
cargo +nightly fuzz run differential
cargo +nightly fuzz run sz
```

The seed corpus under `corpus/` is a small subset of the conformance streams in
`../tests/fixtures/`, each behind its parameter header, and every szip stream
for `sz`. Regenerate it with `python3 tools/build_aec_fuzz_seeds.py`. CI runs
every target time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
