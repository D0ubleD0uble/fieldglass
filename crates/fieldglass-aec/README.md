# fieldglass-aec

A pure-Rust decoder for CCSDS 121.0-B Adaptive Entropy Coding (AEC), the
lossless coder behind GRIB2 template 5.42 (CCSDS packing) and the HDF5 szip
filter.

The reference is [libaec](https://github.com/MathisRosenhauer/libaec) 1.1.7.
The crate accepts exactly the parameter sets libaec's decoder accepts, and is
checked against libaec's own output byte for byte over a committed corpus of
streams that libaec's encoder wrote. It depends on nothing but `thiserror` and
contains no `unsafe`.

**Status:** this release carries the parameter surface (`Params`, `Flags`) and
the error type (`AecError`). The decoder and the libsz-compatible szip layer
follow. The crate is not on crates.io yet.

```rust
use fieldglass_aec::{AecError, Flags, Params};

// A GRIB2 5.42 message's `ccsdsFlags` octet maps straight onto `Flags`.
let params = Params::new(16, 32, 128, Flags::from_bits_truncate(14))?;
assert_eq!(params.bytes_per_sample(), 2);

// HDF5 writes block sizes the standard does not name, and libaec decodes them.
assert!(Params::new(16, 10, 64, Flags::PREPROCESS).is_ok());
assert_eq!(Params::new(16, 7, 64, Flags::empty()), Err(AecError::BlockSize(7)));
# Ok::<(), AecError>(())
```

## What it accepts

The complement of libaec 1.1.7's `aec_decode_init` refusals:

- 1 to 32 bits per sample;
- any even block size from 2 to 256 (the standard names only 8, 16, 32 and 64);
- a reference sample interval of 1 to 4096 blocks;
- the restricted code option set up to 4 bits per sample. It is refused from 5
  to 8 bits and ignored above 8, as libaec does.

## How it is checked

`tests/fixtures/` holds the conformance corpus. `tools/build_aec_fixtures.py`
in the repository downloads the libaec v1.1.7 tag tarball, checks its SHA-256,
builds it twice with cmake (the default build, and one with
`ENABLE_RSI_PADDING`, the only build whose encoder writes RSI padding), and
drives it through `ctypes`. Every stream in the corpus comes from libaec's
encoder, and libaec's decoder produced every expected output. The tests need
none of that: the manifest records each stream's parameters, libaec's status and
the SHA-256 of its output. See `tests/fixtures/NOTICE.md` for the provenance.

## Licence

MIT OR Apache-2.0, like the rest of Fieldglass. The corpus inputs are ported
from libaec's test suite, so the crate also carries libaec's BSD-2-Clause
notice, in `NOTICE`.
