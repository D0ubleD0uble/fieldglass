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
//! fifteen-line wrapper or a weaker gate, and this is the fifteen lines.

use fieldglass_core::FieldglassError;
use std::io::Read;

/// Ceiling on the window a frame may ask the decoder to buffer.
///
/// Tighter than `ruzstd`'s 100 MiB default on purpose: the guarantee should be
/// ours and stated, rather than inherited from an upstream default that a
/// version bump could quietly widen. A window this large already implies a
/// single chunk of 64 MiB, which no Zarr writer produces — chunks are sized for
/// a cache, and the convention's own guidance is single-digit megabytes.
const MAX_WINDOW: u64 = 64 << 20;

/// Decompress one zstd frame, refusing anything that decodes past `limit`.
pub fn decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    let mut decoder =
        ruzstd::decoding::StreamingDecoder::new_with_max_window_size(data, MAX_WINDOW)
            .map_err(|e| FieldglassError::Parse(format!("zstd frame header: {e}")))?;
    let mut out = Vec::new();
    // One past the ceiling, so a frame that decodes to exactly the limit is
    // distinguishable from one that merely stops being read there.
    decoder
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| FieldglassError::Parse(format!("zstd decompress failed: {e}")))?;
    if out.len() > limit {
        return Err(FieldglassError::Parse(format!(
            "zstd stream decompresses past the {limit}-byte ceiling"
        )));
    }
    Ok(out)
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

    /// The window bound is ours, not the decoder's default: this frame is
    /// otherwise valid and declares 72 MiB, inside `ruzstd`'s 100 MiB default
    /// and outside the 64 MiB set here. A bump that widened the upstream
    /// default could not pass this unnoticed.
    #[test]
    fn refuses_a_frame_demanding_a_window_past_our_own_bound() {
        let valid_but_wide = [
            0x28u8, 0xB5, 0x2F, 0xFD, // magic
            0x00, // frame header descriptor: no content size, no dictionary
            0x81, // window descriptor: exponent 16, mantissa 1 => 72 MiB
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        let err = decompress(&valid_but_wide, 1 << 20).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("frame header")),
            "got {err:?}"
        );
    }

    #[test]
    fn refuses_bytes_that_are_not_a_frame() {
        assert!(decompress(&[0u8; 32], 1 << 20).is_err());
    }
}
