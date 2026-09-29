//! The crate's one error type.

/// Why a parameter set or a stream was refused.
///
/// `#[non_exhaustive]`: the decoder adds the stream errors (truncated input,
/// an invalid code) to this same enum, so a `match` on it needs a wildcard arm.
///
/// Named `AecError` rather than `Error` so it cannot be confused with
/// `fieldglass::Error` in a consumer that uses both (ADR-0012 decision 1).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AecError {
    /// Bits per sample outside 1 to 32.
    #[error("bits per sample must be 1 to 32, got {0}")]
    BitsPerSample(u8),

    /// A block size that is zero, odd, or above 256.
    #[error("block size must be an even number from 2 to 256, got {0}")]
    BlockSize(u16),

    /// A reference sample interval outside 1 to 4096 blocks.
    #[error("reference sample interval must be 1 to 4096 blocks, got {0}")]
    Rsi(u16),

    /// The restricted code option set at 5 to 8 bits per sample.
    ///
    /// CCSDS 121.0-B-3 defines the restricted set only up to 4 bits, and libaec
    /// refuses it from 5 to 8. Above 8 bits libaec ignores the flag, and so does
    /// [`Params::new`](crate::Params::new).
    #[error("the restricted code option set is defined up to 4 bits per sample, got {0}")]
    Restricted(u8),
}
