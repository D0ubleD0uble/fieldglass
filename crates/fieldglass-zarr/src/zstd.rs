//! Bounded zstd decode, for the `zstd` codec and for blosc's zstd blocks.
//!
//! Two ceilings, because a crafted frame has two ways to ask for memory. The
//! window is a back-reference buffer sized from the frame *header*, before any
//! output exists, so an output ceiling cannot see it; the output ceiling bounds
//! what the frame actually produces. A small window producing enormous output
//! is precisely what a decompression bomb is.
//!
//! `fieldglass-netcdf` carries the same pair of bounds for the HDF5 zstd
//! filter, and they are deliberately not shared. The only place both crates
//! could reach is `fieldglass-core`, and a module there would have to be behind
//! a feature — `miniz_oxide` and `ruzstd` are dead weight in the GRIB-only
//! browser build that `core` is also the floor of, and the bundle-size gate
//! measures it. But `tools/check_parsing_surface.py` refuses a *gated* core
//! module to a format crate library by construction, because the parsing
//! surface's whole claim is that none of it is gated. So the choice is a second
//! fifteen-line wrapper or a weaker gate, and this is the fifteen lines. What
//! the two do share is how the output buffer grows, [`capped`], which depends
//! on nothing (#981).

use fieldglass_core::FieldglassError;
use fieldglass_core::array::ArrayError;
use fieldglass_core::capped;
use ruzstd::decoding::errors::FrameDecoderError;

/// Ceiling on the window a frame may ask the decoder to buffer.
///
/// Tighter than `ruzstd`'s 128 MiB default on purpose: the guarantee should be
/// ours and stated, rather than inherited from an upstream default that a
/// version bump could quietly widen. A window this large already implies a
/// single chunk of 64 MiB, which no Zarr writer produces — chunks are sized for
/// a cache, and the convention's own guidance is single-digit megabytes.
///
/// Each frame is held to less when its chunk is smaller: the window may be
/// the chunk's length, or 8 MiB if that is more ([`capped::zstd_window_limit`],
/// #989). A frame past that is refused as `ArrayError::WindowTooLarge`, which
/// says the file is fine, unless it is single-segment and so decodes past the
/// chunk.
const MAX_WINDOW: u64 = 64 << 20;

/// Decompress one zstd frame, refusing anything that decodes past `limit`.
pub fn decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    decompress_into(data, data.len().saturating_mul(2), limit)
}

/// [`decompress`] into a buffer that starts at `hint` bytes. A caller that
/// knows the exact output length passes it, bounded by
/// [`capped::unzstd_bound`] when something other than the frame states it,
/// so a valid frame fills one allocation, nothing is copied, and a short
/// frame is not handed more than it could fill (#986).
pub(crate) fn decompress_into(
    data: &[u8],
    hint: usize,
    limit: usize,
) -> Result<Vec<u8>, FieldglassError> {
    // The window follows the output limit, so a frame cannot make the decoder
    // hold far more than the chunk before the limit sees any output (#989).
    let window = capped::zstd_window_limit(limit, MAX_WINDOW);
    let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(data, window)
        .map_err(|e| match e {
            FrameDecoderError::WindowSizeTooBig { requested, .. } => {
                // A single-segment frame's window is its content size, so one
                // past the limit decodes past it: the same chunk the output
                // bound refuses, refused before it is decoded.
                if capped::zstd_single_segment(data)
                    && requested > u64::try_from(limit).unwrap_or(u64::MAX)
                {
                    past(limit)
                } else {
                    ArrayError::WindowTooLarge {
                        codec: "zstd",
                        window: requested,
                        limit: window,
                    }
                    .into()
                }
            }
            e => FieldglassError::Parse(format!("zstd frame header: {e}")),
        })?;
    // Not `read_to_end`, whose amortised growth can reserve nearly twice the
    // limit (#981): this buffer never grows past it.
    capped::read_to_end_within(decoder, hint, limit)
        .map_err(|e| FieldglassError::Parse(format!("zstd decompress failed: {e}")))?
        .ok_or_else(|| past(limit))
}

/// The error for a frame that decompresses past `limit`.
fn past(limit: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "zstd stream decompresses past the {limit}-byte ceiling"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_frame_and_refuses_one_past_the_ceiling() {
        let big = vec![7u8; 4096];
        let frame = ruzstd::encoding::compress_to_vec(
            big.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(frame.len() < big.len(), "the vector must actually expand");
        assert_eq!(decompress(&frame, big.len()).unwrap(), big);
        let err = decompress(&frame, 64).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("ceiling")),
            "got {err:?}"
        );
    }

    /// A valid frame of one raw 4-byte block, `AA BB CC DD`, behind a frame
    /// header stating no content size and the window `descriptor`
    /// (`exponent << 3 | mantissa`, RFC 8878 section 3.1.1.1.2).
    fn windowed(descriptor: u8) -> [u8; 13] {
        [
            0x28, 0xB5, 0x2F, 0xFD,       // magic
            0x00,       // frame header descriptor: no content size, no dictionary
            descriptor, // window descriptor
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD,
        ]
    }

    /// The window bound is ours, not the decoder's default: this frame is
    /// otherwise valid and declares 72 MiB, inside `ruzstd`'s 128 MiB default
    /// and outside the 64 MiB set here, even for a chunk past 64 MiB. A bump
    /// that widened the upstream default could not pass this unnoticed.
    #[test]
    fn refuses_a_frame_demanding_a_window_past_our_own_bound() {
        // Exponent 16, mantissa 1: 72 MiB.
        let err = decompress(&windowed(0x81), 256 << 20).unwrap_err();
        assert!(
            matches!(
                &err,
                FieldglassError::Array(ArrayError::WindowTooLarge {
                    codec: "zstd",
                    window: 0x480_0000,
                    limit: 0x400_0000,
                })
            ),
            "got {err:?}"
        );
    }

    /// The window follows the chunk (#989): a 4-byte chunk accepts a frame
    /// stating 8 MiB, the RFC's recommended minimum, and refuses one stating
    /// 9 MiB as a window too large, which says the file is fine.
    #[test]
    fn a_small_chunk_takes_an_8_mib_window_and_no_more() {
        // Exponent 13: 8 MiB, and with mantissa 1, 9 MiB.
        assert_eq!(
            decompress(&windowed(0x68), 4).unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD]
        );
        let err = decompress(&windowed(0x69), 4).unwrap_err();
        assert!(
            matches!(
                &err,
                FieldglassError::Array(ArrayError::WindowTooLarge {
                    window: 0x90_0000,
                    limit: 0x80_0000,
                    ..
                })
            ),
            "got {err:?}"
        );
        // A chunk of 9 MiB takes it.
        assert!(decompress(&windowed(0x69), 9 << 20).is_ok());
    }

    /// A single-segment frame's window is its content size, so one stating
    /// more than the chunk is the chunk the output bound refuses, refused from
    /// its header with the same error.
    #[test]
    fn a_single_segment_frame_past_the_chunk_is_refused_as_past_it() {
        let frame = [
            0x28u8, 0xB5, 0x2F, 0xFD, // magic
            0xA0, // single-segment, 4-byte content size
            0x00, 0x00, 0x00, 0x01, // Frame_Content_Size: 16 MiB
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        let err = decompress(&frame, 4096).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("past the 4096-byte ceiling")),
            "got {err:?}"
        );
    }

    /// The densest frames `ruzstd` writes stay inside
    /// [`capped::unzstd_bound`], so a valid frame whose length is stated
    /// always starts at that length (#986).
    #[test]
    fn the_densest_frames_stay_within_the_unzstd_bound() {
        let zeros = vec![0u8; 8 << 20];
        for level in [
            ruzstd::encoding::CompressionLevel::Uncompressed,
            ruzstd::encoding::CompressionLevel::Fastest,
        ] {
            let frame = ruzstd::encoding::compress_to_vec(zeros.as_slice(), level);
            assert!(
                zeros.len() <= capped::unzstd_bound(frame.len()),
                "{} bytes from {}",
                zeros.len(),
                frame.len()
            );
        }
    }

    #[test]
    fn refuses_bytes_that_are_not_a_frame() {
        assert!(decompress(&[0u8; 32], 1 << 20).is_err());
    }
}
