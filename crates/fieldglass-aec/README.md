# fieldglass-aec

Decodes [CCSDS adaptive entropy coding](https://public.ccsds.org/Pubs/121x0b3.pdf) (AEC, also called szip) in pure Rust,
checked against libaec. This crate is usable on its own, and is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

You only need it directly if you're decoding AEC or szip data yourself.
[`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) already uses
it for GRIB2's [CCSDS packing](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml), and
[`fieldglass-netcdf`](https://crates.io/crates/fieldglass-netcdf) for the HDF5
szip filter, so to read those files, use one of them or
[`fieldglass`](https://crates.io/crates/fieldglass) instead.

## What it decodes

Satellite and weather data often store whole numbers that change only a
little from one to the next. Adaptive entropy coding packs them tightly and
without loss. It splits the numbers into small blocks, and for each block it
picks whichever of a few coding methods makes that block shortest. The
standard that defines it is [CCSDS 121.0-B](https://public.ccsds.org/Pubs/121x0b3.pdf), and this crate decodes it as
described in version 3 of that standard. GRIB2 uses it for one of its packings.
HDF5, and so NetCDF-4, uses it as the szip filter, which wraps the same coder
in libsz's own framing.

[libaec](https://github.com/MathisRosenhauer/libaec) 1.1.7 is the reference
it's checked against. The crate accepts exactly the settings libaec's decoder
accepts, and its output matches libaec's byte for byte wherever libaec is
correct. Where libaec disagrees with the standard, or accepts a stream no valid
encoder writes, the crate follows the standard (see
[Where it differs from libaec](#where-it-differs-from-libaec)). It depends on
nothing but `thiserror`, contains no `unsafe`, and allocates nothing while
decoding.

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

`decode_to_bytes` writes into a buffer you own. `decode` hands the samples to
a `Sink` instead, one block or one run of zero blocks at a time, so you can
convert them straight into your own output.

It accepts these settings and refuses the rest:

- 1 to 32 bits per sample.
- Any even block size from 2 to 256. The standard names only 8, 16, 32 and 64,
  but HDF5 writes others.
- A reference sample interval of 1 to 4096 blocks.
- The restricted set of coding methods up to 4 bits per sample. Like libaec,
  it refuses that set from 5 to 8 bits and ignores it above 8.

## szip

`sz::decompress` does what libsz's `SZ_BufftoBuffDecompress` does, the call
behind the HDF5 szip filter. That covers scanlines padded to whole blocks,
32- and 64-bit pixels coded as separate byte planes, and the option mask, of
which only `NN` and `MSB` change a decode. libsz decodes into a padded copy of
up to 32 times the output, then copies it again to put the byte planes in
order. This crate skips the padding as it arrives and writes each byte
straight to its place, so it allocates nothing here either.

```rust
use fieldglass_aec::sz::{self, NN_OPTION_MASK, SzParams};

// SzParams is in libsz's SZ_com_t order: mask, bits per pixel, pixels per
// block, pixels per scanline. HDF5's cd_values swap the middle two.
let params = SzParams::new(NN_OPTION_MASK, 16, 32, 20)?;
assert_eq!(params.bytes_per_pixel(), 2);

// The output length is the uncompressed size, and must be filled exactly.
let mut out = [0u8; 4];
assert!(sz::decompress(&[], &params, &mut out).is_err());

// A length stated outside the stream, such as HDF5's size prefix, can be
// checked against what the stream can hold before the output is allocated.
assert!(params.max_decompressed_len(16) < 1 << 20);
# Ok::<(), fieldglass_aec::AecError>(())
```

Because you own the output, you choose its size, and a damaged file shouldn't
be able to make you allocate a huge one. `max_decompressed_len` (and
`Params::max_samples` for the plain coder) is the most a stream of a given
length can decode to under the standard, whatever wrote it, so a stated length
past it can be refused before anything is allocated. `max_compressed_len` goes
the other way, for an encoder that picks each block's method as libsz does:
it's the longest stream that encoder writes for a given output.

The szip settings are checked as libsz checks them, plus libaec's cap of 256
pixels per block: 1 to 32 or 64 bits per pixel, an even number of pixels per
block up to 256, and 1 to 4096 pixels per scanline.

## Where it differs from libaec

In each of these cases, the crate follows the standard. ADR-0012 in the
repository gives the evidence for each.

- It decodes second-extension codewords with a pair sum above 12. libaec's
  decoder refuses them, though its own encoder writes them, and the standard
  sets no bound.
- Truncated input is an error. libaec returns success with short output.
- A value of 2^n or more before postprocessing is an error. libaec wraps it.
- A second-extension pair beside a reference sample must start with the 0 the
  standard puts there. libaec ignores that value.
- A run of more than 63 zero blocks is an error, because the standard's table
  of run codes ends there. libaec accepts any length that fits the interval.
- Decoding stops at the count you asked for, so trailing fill after the last
  sample is never an error, even where libaec misreads it as a run of zero
  blocks that overruns the interval.
- An szip stream that runs out before the output is full is an error. libsz
  returns success, either with a shorter length or, when scanlines are padded,
  with the full length and uninitialized bytes in the part it couldn't decode.
- A bad code after the last pixel you asked for is never an szip error. libsz
  decodes whole scanlines when they are padded, and fails on such a code.
- An szip output of 32- or 64-bit pixels that isn't a whole number of pixels is
  an error. libsz returns success with the bytes out of place and the last few
  unwritten.
- An szip output of 32- or 64-bit pixels shorter than its stream is an error.
  Those pixels are coded as byte planes laid out by the output's length, so a
  shorter output isn't a prefix of a longer one. The decoder reads to the end
  of the stream that the output's length implies, and refuses a whole byte
  left after it. libsz returns success with the bytes out of place. Below 32
  bits, a shorter output is a valid prefix and decodes.

## What it is not

It's a decoder only. It doesn't encode, and it doesn't know about GRIB or HDF5
beyond the settings they pass in. Reading the surrounding file is the format
crates' job.

## How it is checked

Every stream in the committed test corpus was written by libaec's own
encoder, built from the checksummed v1.1.7 release both in its default form
and with `ENABLE_RSI_PADDING`, the only build whose encoder writes reference
sample padding. libaec's decoder produced every expected output, except where
it refuses a valid stream; there the expected output is the field the encoder
was given. `tools/build_aec_fixtures.py` in the repository builds the corpus,
and the tests themselves need no libaec, since a manifest records each
stream's settings, libaec's result and the SHA-256 of its output.
`tests/fixtures/NOTICE.md` gives the provenance.

A CI job rebuilds libaec, regenerates the corpus and fails if it differs from
the committed one. It also decodes a 4,819-case matrix too large to commit and
the 66 [CCSDS 121.0-B-2](https://public.ccsds.org/Pubs/121x0b2ec1s.pdf) sample streams that ship with libaec, and fails on any
result that is neither libaec's nor one of the differences above.

## Related crates

| Crate | Use it to |
|---|---|
| [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) | Read GRIB2, including its [CCSDS packing](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) |
| [`fieldglass-netcdf`](https://crates.io/crates/fieldglass-netcdf) | Read NetCDF-4, including szip-compressed variables |
| [`fieldglass`](https://crates.io/crates/fieldglass) | Open any supported file and draw it |

## License

MIT OR Apache-2.0, like the rest of Fieldglass. The test corpus's inputs are
ported from libaec's test suite, so the crate also carries libaec's
BSD-2-Clause notice, in `NOTICE`.

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries are
  used to check against the resulting behaviors. If behaviors differ between
  Fieldglass and a reference, Fieldglass follows the specification and records
  why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-aec`, every stream is written by libaec's own encoder, and its
  output is checked against libaec's decoder wherever libaec accepts it.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, which would be fine
  if it made the docs more readable but instead it's hard to understand. I am
  undertaking efforts to rewrite the user-facing documentation with my own
  words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and
  larger-picture dependency graphs, I haven't been reading over every line of
  code. As the shape of things starts to become a bit more stable, I hope to
  review more of the code closely, but to be totally transparent this is more
  of a 'desire' than a reality currently.
