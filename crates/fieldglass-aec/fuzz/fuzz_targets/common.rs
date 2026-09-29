//! What both targets share: the input layout and the parameter mapping.
//!
//! An input is a 7-byte header followed by the stream:
//!
//! | byte | meaning |
//! | --- | --- |
//! | 0 | bits per sample - 1, modulo 32 |
//! | 1 | block size / 2 - 1, modulo 128 |
//! | 2-3 | reference sample interval - 1, little endian, modulo 4096 |
//! | 4 | flags, the low six bits |
//! | 5-6 | sample count, little endian |
//!
//! Every header maps to a parameter set `Params::new` accepts, so the fuzzer
//! spends its time in the decoder and not in the validator (the validator is
//! held against libaec's own verdicts by the crate's `tests/params.rs`). The
//! one adjustment: the restricted option set is refused from 5 to 8 bits, so
//! the flag is dropped there.
//! `tools/build_aec_fuzz_seeds.py` writes the seeds in this layout.

use fieldglass_aec::{Flags, Params};

/// Header length in bytes.
pub const HEADER: usize = 7;

/// The parameters, the requested sample count and the stream, or `None` when
/// the input is shorter than the header.
pub fn split(data: &[u8]) -> Option<(Params, usize, &[u8])> {
    let (head, stream) = data.split_at_checked(HEADER)?;
    let bits = head[0] % 32 + 1;
    let block = u16::from(head[1] % 128 + 1) * 2;
    let rsi = u16::from_le_bytes([head[2], head[3]]) % 4096 + 1;
    let mut flags = Flags::from_bits_truncate(head[4]);
    if (5..=8).contains(&bits) {
        flags.remove(Flags::RESTRICTED);
    }
    let count = usize::from(u16::from_le_bytes([head[5], head[6]]));
    let params = Params::new(bits, block, rsi, flags)
        .expect("the header mapping only produces parameters libaec accepts");
    Some((params, count, stream))
}
