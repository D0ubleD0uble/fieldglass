//! The crate's one error type.

/// Why a parameter set or a stream was refused.
///
/// `#[non_exhaustive]`: a later release may add a reason, as the szip layer
/// did, so a `match` on it needs a wildcard arm.
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

    /// The input ended before `requested` samples were decoded.
    ///
    /// `decoded` samples had already reached the sink. libaec returns success
    /// with short output here; this crate never pads the rest with zeros.
    #[error("the input ended after {decoded} of {requested} samples")]
    Truncated {
        /// Samples handed to the sink before the input ran out.
        decoded: usize,
        /// Samples the caller asked for.
        requested: usize,
    },

    /// A code no valid stream contains, in the block that starts at sample
    /// `sample`.
    ///
    /// One of:
    ///
    /// - a value of 2^n or more before postprocessing, which CCSDS 121.0-B-3
    ///   rules out and libaec silently wraps;
    /// - a second-extension pair beside a reference sample whose first value
    ///   is not the 0 the standard puts there, which libaec ignores;
    /// - a zero-block run longer than the 63 blocks of CCSDS 121.0-B-3
    ///   Table 3-2, which libaec accepts if it fits the interval;
    /// - a zero-block run that passes the end of its reference sample
    ///   interval, which libaec also refuses.
    #[error("invalid code in the block starting at sample {sample}: {reason}")]
    InvalidCode {
        /// Index of the first sample of the block holding the code.
        sample: usize,
        /// What was wrong with it.
        reason: &'static str,
    },

    /// An output buffer whose length is not a whole number of samples.
    ///
    /// From [`sz::decompress`](crate::sz::decompress), `bytes_per_sample` is
    /// the pixel width, [`SzParams::bytes_per_pixel`](crate::sz::SzParams::bytes_per_pixel).
    #[error("an output of {len} bytes is not a whole number of {bytes_per_sample}-byte samples")]
    OutputLength {
        /// The buffer's length in bytes.
        len: usize,
        /// Bytes per sample for the parameter set.
        bytes_per_sample: usize,
    },

    /// szip bits per pixel outside 1 to 32 and not 64 (`sz_compat.c:233-235`).
    #[error("szip bits per pixel must be 1 to 32 or 64, got {0}")]
    BitsPerPixel(u32),

    /// szip pixels per block that is zero, odd, or above 256.
    ///
    /// libsz checks for zero and odd (`sz_compat.c:231-232`); libaec's decoder
    /// refuses a block above 256 behind it.
    #[error("szip pixels per block must be an even number from 2 to 256, got {0}")]
    PixelsPerBlock(u32),

    /// szip pixels per scanline outside 1 to 4096 (`sz_compat.c:229-230`).
    #[error("szip pixels per scanline must be 1 to 4096, got {0}")]
    PixelsPerScanline(u32),

    /// An szip output of 32- or 64-bit pixels that stops short of its stream.
    ///
    /// Those pixels are coded as byte planes whose length is the output's
    /// pixel count, so an output shorter than the stream puts every byte
    /// after the first plane in the wrong place. libsz returns `SZ_OK` with
    /// those bytes. This is raised when a whole byte of input is left after
    /// the last scanline the output reaches, which libsz always encodes
    /// whole. A shortfall inside that scanline cannot be seen. `unread` is
    /// the whole bytes left.
    #[error(
        "an szip output of {len} bytes is shorter than its stream of 32- or 64-bit pixels: \
         {unread} bytes are left after the last pixel"
    )]
    TrailingInput {
        /// The buffer's length in bytes.
        len: usize,
        /// Whole bytes of input left after the last pixel's code.
        unread: usize,
    },

    /// An szip output whose padded stream has more samples than `usize` can
    /// count. Only a 32-bit target can reach it: one pixel per scanline in
    /// blocks of 256 decodes 256 stream samples per pixel.
    #[error("an szip output of {len} bytes needs more padded samples than this target can count")]
    OutputTooLarge {
        /// The buffer's length in bytes.
        len: usize,
    },
}
