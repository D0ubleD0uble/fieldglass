//! What a stream was encoded with: sample width, block size, reference sample
//! interval and the option flags, validated exactly as libaec 1.1.7's
//! `aec_decode_init` validates them (`decode.c:692-754`).

use core::fmt;
use core::ops::{BitOr, BitOrAssign};

use crate::AecError;

/// The option flags of a stream, libaec's `AEC_*` bits.
///
/// The bit values are libaec's, and GRIB2's `ccsdsFlags` (template 5.42)
/// uses the same ones, so a GRIB2 reader can turn the octet into `Flags` with
/// [`Flags::from_bits_truncate`]. That gives libaec's reading of the octet,
/// not a GRIB2 reader's: ADR-0012 decision 5 has the reader clear
/// [`Flags::PAD_RSI`] before decoding (D1) and take a SIGNED sample as its
/// n-bit pattern (Q5), because that is what eccodes' encoder wrote.
///
/// ```
/// use fieldglass_aec::Flags;
///
/// let flags = Flags::from_bits_truncate(46).difference(Flags::PAD_RSI);
/// assert_eq!(flags, Flags::from_bits_truncate(14));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(u8);

impl Flags {
    /// Samples are two's-complement signed (`AEC_DATA_SIGNED`).
    pub const SIGNED: Flags = Flags(1);
    /// Samples of 17 to 24 bits are stored in three bytes rather than four
    /// (`AEC_DATA_3BYTE`). Ignored at other widths.
    pub const THREE_BYTE: Flags = Flags(2);
    /// Output bytes are most significant first (`AEC_DATA_MSB`). This is the
    /// byte order of the decoded samples, not of the host.
    pub const MSB: Flags = Flags(4);
    /// The stream was preprocessed: a reference sample opens each interval
    /// and the rest are mapped prediction errors (`AEC_DATA_PREPROCESS`).
    pub const PREPROCESS: Flags = Flags(8);
    /// The restricted code option set for 1 to 4 bits per sample
    /// (`AEC_RESTRICTED`).
    pub const RESTRICTED: Flags = Flags(16);
    /// Each reference sample interval starts on a byte boundary
    /// (`AEC_PAD_RSI`). Outside the standard, but used by some CCSDS sample
    /// data.
    pub const PAD_RSI: Flags = Flags(32);

    /// Every bit that changes how a stream decodes.
    const KNOWN: u8 = 0x3F;

    /// No flags: unsigned, least significant byte first, not preprocessed.
    pub const fn empty() -> Self {
        Flags(0)
    }

    /// The flags in `bits`, with every other bit dropped.
    ///
    /// Dropped bits include libaec's `AEC_NOT_ENFORCE` (64), which only
    /// relaxes the *encoder's* block-size check and has no effect on decoding.
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Flags(bits & Self::KNOWN)
    }

    /// The raw bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every flag in `other` is set in `self`.
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags in `self` that are not in `other`.
    pub const fn difference(self, other: Flags) -> Self {
        Flags(self.0 & !other.0)
    }

    /// The flags set in both `self` and `other`.
    pub const fn intersection(self, other: Flags) -> Self {
        Flags(self.0 & other.0)
    }

    /// Clear every flag in `other`.
    pub fn remove(&mut self, other: Flags) {
        *self = self.difference(other);
    }

    /// Set every flag in `other`.
    pub fn insert(&mut self, other: Flags) {
        *self |= other;
    }
}

impl BitOr for Flags {
    type Output = Flags;

    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

impl BitOrAssign for Flags {
    fn bitor_assign(&mut self, rhs: Flags) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(Flags, &str); 6] = [
            (Flags::SIGNED, "SIGNED"),
            (Flags::THREE_BYTE, "THREE_BYTE"),
            (Flags::MSB, "MSB"),
            (Flags::PREPROCESS, "PREPROCESS"),
            (Flags::RESTRICTED, "RESTRICTED"),
            (Flags::PAD_RSI, "PAD_RSI"),
        ];
        f.write_str("Flags(")?;
        let mut first = true;
        for (flag, name) in NAMES {
            if self.contains(flag) {
                if !first {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("empty")?;
        }
        f.write_str(")")
    }
}

/// A validated parameter set.
///
/// [`Params::new`] accepts exactly the sets libaec 1.1.7's `aec_decode_init`
/// accepts, and refuses the rest with the reason. The conformance test holds
/// that against every row of a grid libaec itself judged
/// (`tests/fixtures/manifest.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Params {
    bits_per_sample: u8,
    block_size: u16,
    rsi: u16,
    flags: Flags,
}

impl Params {
    /// Validate a parameter set.
    ///
    /// * `bits_per_sample`: 1 to 32.
    /// * `block_size`: samples per block, any even number from 2 to 256. The
    ///   standard allows 8, 16, 32 and 64 only, but libaec decodes the rest
    ///   and HDF5's szip filter writes them.
    /// * `rsi`: blocks per reference sample interval, 1 to 4096.
    /// * `flags`: [`Flags::RESTRICTED`] is refused from 5 to 8 bits and
    ///   ignored above 8, as libaec does. No other flag is ever refused.
    ///
    /// # Errors
    ///
    /// The first check that fails, in the order above.
    ///
    /// ```
    /// use fieldglass_aec::{AecError, Flags, Params};
    ///
    /// // GRIB2 template 5.42's usual settings.
    /// let params = Params::new(16, 32, 128, Flags::PREPROCESS | Flags::MSB)?;
    /// assert_eq!(params.bytes_per_sample(), 2);
    ///
    /// // HDF5 writes block sizes the standard does not name.
    /// assert!(Params::new(16, 10, 64, Flags::PREPROCESS).is_ok());
    /// assert_eq!(Params::new(16, 7, 64, Flags::empty()), Err(AecError::BlockSize(7)));
    /// # Ok::<(), AecError>(())
    /// ```
    pub fn new(
        bits_per_sample: u8,
        block_size: u16,
        rsi: u16,
        flags: Flags,
    ) -> Result<Self, AecError> {
        if !(1..=32).contains(&bits_per_sample) {
            return Err(AecError::BitsPerSample(bits_per_sample));
        }
        if block_size == 0 || !block_size.is_multiple_of(2) || block_size > 256 {
            return Err(AecError::BlockSize(block_size));
        }
        if !(1..=4096).contains(&rsi) {
            return Err(AecError::Rsi(rsi));
        }
        if flags.contains(Flags::RESTRICTED) && (5..=8).contains(&bits_per_sample) {
            return Err(AecError::Restricted(bits_per_sample));
        }
        Ok(Params {
            bits_per_sample,
            block_size,
            rsi,
            flags,
        })
    }

    /// Bits per sample, 1 to 32.
    pub const fn bits_per_sample(&self) -> u8 {
        self.bits_per_sample
    }

    /// Samples per block, an even number from 2 to 256.
    pub const fn block_size(&self) -> u16 {
        self.block_size
    }

    /// Blocks per reference sample interval, 1 to 4096.
    pub const fn rsi(&self) -> u16 {
        self.rsi
    }

    /// The flags, as given.
    pub const fn flags(&self) -> Flags {
        self.flags
    }

    /// Bytes per decoded sample in libaec's output layout: 1 up to 8 bits, 2
    /// up to 16, 3 from 17 to 24 bits with [`Flags::THREE_BYTE`], and 4
    /// otherwise.
    pub const fn bytes_per_sample(&self) -> usize {
        match self.bits_per_sample {
            0..=8 => 1,
            9..=16 => 2,
            17..=24 if self.flags.contains(Flags::THREE_BYTE) => 3,
            _ => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bits_truncate_drops_not_enforce_and_the_top_bit() {
        assert_eq!(Flags::from_bits_truncate(64), Flags::empty());
        assert_eq!(Flags::from_bits_truncate(0xFF).bits(), 0x3F);
        assert_eq!(
            Flags::from_bits_truncate(14),
            Flags::THREE_BYTE | Flags::MSB | Flags::PREPROCESS
        );
    }

    #[test]
    fn bit_values_are_libaecs() {
        let bits: Vec<u8> = [
            Flags::SIGNED,
            Flags::THREE_BYTE,
            Flags::MSB,
            Flags::PREPROCESS,
            Flags::RESTRICTED,
            Flags::PAD_RSI,
        ]
        .iter()
        .map(|f| f.bits())
        .collect();
        assert_eq!(bits, [1, 2, 4, 8, 16, 32]);
    }

    #[test]
    fn set_operations_stay_within_the_known_bits() {
        let grib2 = Flags::from_bits_truncate(46); // PAD_RSI | PP | 3BYTE | MSB
        assert!(grib2.contains(Flags::PAD_RSI));

        let cleared = grib2.difference(Flags::PAD_RSI);
        assert!(!cleared.contains(Flags::PAD_RSI));
        assert_eq!(cleared.bits(), 14);

        let mut f = grib2;
        f.remove(Flags::PAD_RSI | Flags::MSB);
        assert_eq!(f, Flags::PREPROCESS | Flags::THREE_BYTE);
        f.remove(Flags::SIGNED); // not set: no change
        assert_eq!(f.bits(), 10);
        f.insert(Flags::SIGNED);
        assert_eq!(f.bits(), 11);

        assert_eq!(grib2.intersection(Flags::MSB | Flags::SIGNED), Flags::MSB);
        assert!(Flags::empty().is_empty());
        assert!(!Flags::SIGNED.is_empty());
        assert!(grib2.difference(grib2).is_empty());
        // `difference` never sets a bit `from_bits_truncate` would drop.
        assert_eq!(Flags::empty().difference(Flags::SIGNED).bits(), 0);
    }

    #[test]
    fn debug_names_the_flags() {
        assert_eq!(format!("{:?}", Flags::empty()), "Flags(empty)");
        assert_eq!(
            format!("{:?}", Flags::SIGNED | Flags::PAD_RSI),
            "Flags(SIGNED | PAD_RSI)"
        );
        let mut f = Flags::MSB;
        f |= Flags::PREPROCESS;
        assert_eq!(format!("{f:?}"), "Flags(MSB | PREPROCESS)");
    }

    #[test]
    fn each_refusal_names_its_reason() {
        let ok = Flags::empty();
        assert_eq!(Params::new(0, 16, 128, ok), Err(AecError::BitsPerSample(0)));
        assert_eq!(
            Params::new(33, 16, 128, ok),
            Err(AecError::BitsPerSample(33))
        );
        assert_eq!(Params::new(8, 0, 128, ok), Err(AecError::BlockSize(0)));
        assert_eq!(Params::new(8, 258, 128, ok), Err(AecError::BlockSize(258)));
        assert_eq!(Params::new(8, 16, 0, ok), Err(AecError::Rsi(0)));
        assert_eq!(Params::new(8, 16, 4097, ok), Err(AecError::Rsi(4097)));
        assert_eq!(
            Params::new(5, 16, 128, Flags::RESTRICTED),
            Err(AecError::Restricted(5))
        );
    }

    #[test]
    fn accessors_return_what_was_validated() {
        let flags = Flags::PREPROCESS | Flags::THREE_BYTE;
        let p = Params::new(24, 64, 4096, flags).unwrap();
        assert_eq!(
            (p.bits_per_sample(), p.block_size(), p.rsi(), p.flags()),
            (24, 64, 4096, flags)
        );
        assert_eq!(p.bytes_per_sample(), 3);
    }

    #[test]
    fn bytes_per_sample_follows_libaecs_layout() {
        let width = |bps, flags| Params::new(bps, 16, 1, flags).unwrap().bytes_per_sample();
        let three = Flags::THREE_BYTE;
        assert_eq!(width(1, Flags::empty()), 1);
        assert_eq!(width(8, Flags::empty()), 1);
        assert_eq!(width(9, Flags::empty()), 2);
        assert_eq!(width(16, three), 2);
        assert_eq!(width(17, Flags::empty()), 4);
        assert_eq!(width(17, three), 3);
        assert_eq!(width(24, three), 3);
        assert_eq!(width(25, three), 4);
        assert_eq!(width(32, Flags::empty()), 4);
    }
}
