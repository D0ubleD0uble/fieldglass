# fieldglass-aec

A pure-Rust decoder for CCSDS 121.0-B Adaptive Entropy Coding (AEC), the
lossless coder behind GRIB2 template 5.42 (CCSDS packing) and the HDF5 szip
filter.

The goal is to decode correctly per the standard, CCSDS 121.0-B-3.
[libaec](https://github.com/MathisRosenhauer/libaec) 1.1.7 is the reference it
is checked against: the crate accepts exactly the parameter sets libaec's
decoder accepts, and its output matches libaec's byte for byte wherever libaec
is correct, over a committed corpus of streams that libaec's encoder wrote.
Where libaec disagrees with
the standard, or accepts a stream no valid encoder writes, the crate follows
the standard (see below). It depends on nothing but `thiserror`, contains no
`unsafe`, and allocates nothing while decoding.

**Status:** the decoder and the libsz-compatible szip layer are here.
`fieldglass-grib2` decodes GRIB2 template 5.42 with the decoder, and
`fieldglass-netcdf` decodes the HDF5 szip filter with the szip layer.

```rust
use fieldglass_aec::{AecError, Flags, Params, decode_to_bytes};

// A GRIB2 5.42 message's `ccsdsFlags` octet maps straight onto `Flags`.
let params = Params::new(16, 32, 128, Flags::from_bits_truncate(14))?;
assert_eq!(params.bytes_per_sample(), 2);

// HDF5 writes block sizes the standard does not name, and libaec decodes them.
assert!(Params::new(16, 10, 64, Flags::PREPROCESS).is_ok());
assert_eq!(Params::new(16, 7, 64, Flags::empty()), Err(AecError::BlockSize(7)));

// Decode into a caller-owned buffer: 2 bytes per sample here.
let stream: &[u8] = &[0b1111_0000, 0x10, 0x20, 0x30, 0x40];
let params = Params::new(16, 2, 1, Flags::MSB)?;
let mut out = [0u8; 4];
decode_to_bytes(stream, &params, &mut out)?;
assert_eq!(out, [1, 2, 3, 4]);
# Ok::<(), AecError>(())
```

`decode` hands samples to a `Sink` instead, one block or one run of zero blocks
at a time, so a consumer can convert them straight into its own output.

## szip

`sz::decompress` does what libsz's `SZ_BufftoBuffDecompress` does, the call
behind the HDF5 szip filter: scanlines padded to whole blocks, 32- and 64-bit
pixels coded as byte planes, and the option mask, of which only `NN` and `MSB`
change a decode. libsz decodes into a padded copy of up to 32 times the output
and copies it again to reorder byte planes. This crate skips pad samples as
they arrive and writes each byte straight to its place, so it allocates
nothing here either.

```rust
use fieldglass_aec::sz::{self, NN_OPTION_MASK, SzParams};

// SzParams is in libsz's SZ_com_t order: mask, bits per pixel, pixels per
// block, pixels per scanline. HDF5's cd_values swap the middle two.
let params = SzParams::new(NN_OPTION_MASK, 16, 32, 20)?;
assert_eq!(params.bytes_per_pixel(), 2);

// The output length is the uncompressed size, and must be filled exactly.
let mut out = [0u8; 4];
assert!(sz::decompress(&[], &params, &mut out).is_err());
# Ok::<(), fieldglass_aec::AecError>(())
```

Parameters are checked as libsz checks them: 1 to 32 or 64 bits per pixel, an
even number of pixels per block up to 256, and 1 to 4096 pixels per scanline.

## What it accepts

The complement of libaec 1.1.7's `aec_decode_init` refusals:

- 1 to 32 bits per sample;
- any even block size from 2 to 256 (the standard names only 8, 16, 32 and 64);
- a reference sample interval of 1 to 4096 blocks;
- the restricted code option set up to 4 bits per sample. It is refused from 5
  to 8 bits and ignored above 8, as libaec does.

## Where it differs from libaec

- Second-extension codewords with a pair sum above 12 are decoded. libaec's
  decoder refuses them, though its encoder writes them; the standard sets no
  bound.
- Truncated input is an error. libaec returns success with short output.
- A value of 2^n or more before postprocessing is an error. libaec wraps it.
- A second-extension pair beside a reference sample must start with the 0 the
  standard puts there. libaec ignores that value.
- A zero-block run longer than 63 blocks is an error: the standard's table of
  run codes ends there. libaec accepts any length that fits the interval.
- Decoding stops at the requested count, so trailing fill that libaec misreads
  after the last sample is never an error.
- An szip stream that runs out before the output is full is an error. libsz
  returns success, with a shorter length or, when scanlines are padded, the
  full length and uninitialised bytes in the part it could not decode.
- szip decoding stops at the last pixel asked for, so a bad code after it is
  never read. libsz decodes whole scanlines and fails on one.
- An szip output of 32- or 64-bit pixels that is not a whole number of pixels
  is an error. libsz returns success with the bytes out of place and the last
  few unwritten.

ADR-0012 in the repository gives the evidence for each.

## How it is checked

`tests/fixtures/` holds the conformance corpus. `tools/build_aec_fixtures.py`
in the repository downloads the libaec v1.1.7 tag tarball, checks its SHA-256,
builds it twice with cmake (the default build, and one with
`ENABLE_RSI_PADDING`, the only build whose encoder writes RSI padding), and
drives it through `ctypes`. Every stream in the corpus comes from libaec's
encoder, and libaec's decoder produced every expected output, except where
libaec's decoder refuses a valid stream: there the expected output is the field
the encoder was given. The tests need none of that: the manifest records each
stream's parameters, libaec's status and the SHA-256 of its output. See
`tests/fixtures/NOTICE.md` for the provenance.

A CI job rebuilds libaec, regenerates the corpus and fails if it differs from
the committed one. It also decodes a 4,819-case matrix too large to commit and
the 66 CCSDS 121.0-B-2 sample streams shipped with libaec, and fails on any
result that is neither libaec's nor one of the differences listed above.

## Licence

MIT OR Apache-2.0, like the rest of Fieldglass. The corpus inputs are ported
from libaec's test suite, so the crate also carries libaec's BSD-2-Clause
notice, in `NOTICE`.
