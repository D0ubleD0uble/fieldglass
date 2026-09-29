//! The bit reader: most significant bit first, over a whole input slice.
//!
//! A `u64` accumulator holds the next bits of the stream left-aligned. While at
//! least eight bytes remain it refills with one `u64::from_be_bytes`; the last
//! few bytes go in one at a time. Running out of input is [`Eof`], never a
//! zero-filled read: libaec's fast path pads with zeros, and a decoder that did
//! the same would turn a truncated stream into plausible-looking samples.

/// The input ended before the bits a read asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Eof;

/// A big-endian bit reader over `input`.
#[derive(Debug)]
pub(crate) struct BitReader<'a> {
    input: &'a [u8],
    /// The next byte of `input` not yet counted into `bits`.
    pos: usize,
    /// The next unread bits, left-aligned. The top `bits` bits are counted.
    /// Bits below them are either zero or the stream's own next bits, read
    /// early by an eight-byte refill; a later refill ORs the same bits into
    /// the same places, so neither is ever wrong.
    acc: u64,
    /// How many of `acc`'s top bits are unread stream bits, 0 to 64.
    bits: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        BitReader {
            input,
            pos: 0,
            acc: 0,
            bits: 0,
        }
    }

    /// Top up `acc` to at least 57 counted bits, or to the end of the input.
    /// Needs `bits <= 56`.
    #[inline]
    fn refill(&mut self) {
        debug_assert!(self.bits <= 56);
        let rest = self.input.get(self.pos..).unwrap_or_default();
        if let Some(chunk) = rest.first_chunk::<8>() {
            self.acc |= u64::from_be_bytes(*chunk) >> self.bits;
            // Whole bytes that fit below the counted bits: 7 when bits is 0,
            // 1 when it is 56. `bits` ends between 57 and 64.
            let bytes = (63 - self.bits) >> 3;
            self.pos += bytes as usize;
            self.bits += bytes << 3;
        } else {
            for &byte in rest {
                if self.bits > 56 {
                    break;
                }
                self.acc |= u64::from(byte) << (56 - self.bits);
                self.pos += 1;
                self.bits += 8;
            }
        }
    }

    /// The next `n` bits as an unsigned number, for `1 <= n <= 32`.
    #[inline]
    pub(crate) fn bits(&mut self, n: u32) -> Result<u32, Eof> {
        debug_assert!((1..=32).contains(&n));
        if self.bits < n {
            self.refill();
            if self.bits < n {
                return Err(Eof);
            }
        }
        let value = self.acc >> (64 - n);
        self.acc <<= n;
        self.bits -= n;
        // `value` has at most 32 significant bits.
        Ok(u32::try_from(value).unwrap_or(u32::MAX))
    }

    /// A fundamental sequence: the number of `0` bits before the next `1`,
    /// which is consumed too (CCSDS 121.0-B-3 §3.2).
    ///
    /// No cap on the count: every zero consumes input, so the count is bounded
    /// by the input's length.
    #[inline(always)]
    pub(crate) fn fs(&mut self) -> Result<u64, Eof> {
        // The common case, inline: the sequence ends within the counted bits.
        let lead = self.acc.leading_zeros();
        if lead < self.bits {
            self.acc = (self.acc << lead) << 1;
            self.bits -= lead + 1;
            return Ok(u64::from(lead));
        }
        self.fs_across_refills()
    }

    /// [`fs`](Self::fs) when the counted bits run out first.
    #[inline(never)]
    fn fs_across_refills(&mut self) -> Result<u64, Eof> {
        let mut zeros: u64 = 0;
        loop {
            // A one among the counted bits ends the sequence.
            let lead = self.acc.leading_zeros();
            if lead < self.bits {
                // `lead + 1 <= 64`: shift in two steps so 64 is not an overflow.
                self.acc = (self.acc << lead) << 1;
                self.bits -= lead + 1;
                return Ok(zeros + u64::from(lead));
            }
            // Every counted bit is a zero.
            zeros += u64::from(self.bits);
            self.acc = self.acc.checked_shl(self.bits).unwrap_or(0);
            self.bits = 0;
            self.refill();
            if self.bits == 0 {
                return Err(Eof);
            }
        }
    }

    /// Skip to the next byte boundary of the input (libaec's `AEC_PAD_RSI`).
    pub(crate) fn align_to_byte(&mut self) {
        let drop = self.bits % 8;
        self.acc <<= drop;
        self.bits -= drop;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields_across_refills_and_the_tail() {
        // 0xA5 = 1010_0101, repeated; 11 bytes so the tail path runs too.
        let input = [0xA5u8; 11];
        let mut r = BitReader::new(&input);
        let mut got = Vec::new();
        for _ in 0..(11 * 8 / 4) {
            got.push(r.bits(4).unwrap());
        }
        assert!(got.chunks(2).all(|p| p == [0xA, 0x5]));
        assert_eq!(r.bits(1), Err(Eof));
    }

    #[test]
    fn reads_32_bit_fields_at_every_alignment() {
        let input: Vec<u8> = (0u8..64).collect();
        for skip in 0..8 {
            let mut r = BitReader::new(&input);
            if skip > 0 {
                r.bits(skip).unwrap();
            }
            let expected = u64::from_be_bytes(input[..8].try_into().unwrap());
            let want = u32::try_from((expected << skip) >> 32).unwrap();
            assert_eq!(r.bits(32).unwrap(), want, "skip {skip}");
        }
    }

    #[test]
    fn counts_fundamental_sequences_longer_than_the_accumulator() {
        // 100 zeros then a one, then `101`.
        let mut input = vec![0u8; 12];
        input.push(0b0000_1101);
        input.push(0b0000_0000);
        let mut r = BitReader::new(&input);
        assert_eq!(r.fs().unwrap(), 100);
        assert_eq!(r.bits(3).unwrap(), 0b101);
        assert_eq!(r.fs(), Err(Eof));
    }

    #[test]
    fn a_sequence_ending_on_the_last_bit_of_the_accumulator() {
        // 63 zeros then a one fill exactly eight bytes.
        let mut input = vec![0u8; 7];
        input.push(1);
        input.push(0x80);
        let mut r = BitReader::new(&input);
        assert_eq!(r.fs().unwrap(), 63);
        assert_eq!(r.fs().unwrap(), 0);
    }

    #[test]
    fn truncation_is_an_error_not_zeros() {
        let mut r = BitReader::new(&[0xFF]);
        assert_eq!(r.bits(8).unwrap(), 0xFF);
        assert_eq!(r.bits(1), Err(Eof));
        let mut r = BitReader::new(&[0x00, 0x00]);
        assert_eq!(r.fs(), Err(Eof));
        let mut r = BitReader::new(&[]);
        assert_eq!(r.bits(3), Err(Eof));
    }

    #[test]
    fn align_skips_to_the_next_byte() {
        let mut r = BitReader::new(&[0b1110_0000, 0x5A]);
        assert_eq!(r.bits(3).unwrap(), 0b111);
        r.align_to_byte();
        assert_eq!(r.bits(8).unwrap(), 0x5A);
        r.align_to_byte();
        assert_eq!(r.bits(1), Err(Eof));
    }
}
