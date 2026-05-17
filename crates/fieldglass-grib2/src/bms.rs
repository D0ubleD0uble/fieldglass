//! GRIB2 Bit-Map Section (§6).
//!
//! §6 carries an optional per-grid-point presence mask. The "bit-map
//! indicator" (octet 6) tells the decoder how to obtain the bitmap:
//!
//! - `0` — bitmap is included in this section (octets 7+, MSB-first packed
//!   bits).
//! - `1..=253` — predefined bitmap (rare; not used by NCEP / ECMWF
//!   operational products).
//! - `254` — reuse the bitmap from a previously defined message in the
//!   same file. Requires cross-message state; surfaced as unsupported.
//! - `255` — no bitmap; every grid point is present.
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 6 + Code Table 6.0.

use crate::section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
use fieldglass_core::FieldglassError;

/// Section number for the Bit-Map Section.
pub const BMS_SECTION_NUMBER: u8 = 6;

/// "A bit-map is present and applies to this product" — bitmap follows
/// inline at octet 7.
pub const BMS_INDICATOR_PRESENT: u8 = 0;

/// "A bit-map previously defined in the same GRIB2 message is used."
/// Requires cross-message state; not currently supported.
pub const BMS_INDICATOR_PREVIOUS: u8 = 254;

/// "A bit-map is not present" — every grid point in §7 is real.
pub const BMS_INDICATOR_NONE: u8 = 255;

/// Minimum byte length of a BMS — header (5) + indicator (1).
const BMS_MIN_LEN: usize = SECTION_HEADER_LEN + 1;

/// Parsed contents of the Bit-Map Section.
#[derive(Debug, Clone)]
pub struct BitMapSection {
    pub section_length: u32,
    /// Bit-map indicator (WMO Code Table 6.0). `Some(bitmap)` only when
    /// `indicator == 0`; for indicator 1..=254 the `bitmap` field is empty
    /// (predefined / reuse-previous handling lives outside this section).
    pub indicator: u8,
    /// Decoded per-grid-point presence flags, populated only when
    /// `indicator == 0`. Length equals the expected grid-point count
    /// passed at parse time; trailing pad bits in the last byte are
    /// discarded.
    pub bitmap: Vec<bool>,
}

impl BitMapSection {
    /// `true` when the section carries an inline bitmap that callers
    /// should consult per grid point.
    pub fn has_inline_bitmap(&self) -> bool {
        self.indicator == BMS_INDICATOR_PRESENT
    }
}

/// Parse the Bit-Map Section starting at `bytes[0]`. `grid_points` is the
/// number of points the bitmap is expected to cover (taken from §3 GDS) —
/// trailing pad bits in the last byte are discarded.
pub fn parse_bit_map(bytes: &[u8], grid_points: usize) -> Result<BitMapSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_bit_map_with_header(bytes, header, grid_points)
}

/// Variant for callers that have already read the section header.
pub fn parse_bit_map_with_header(
    bytes: &[u8],
    header: SectionHeader,
    grid_points: usize,
) -> Result<BitMapSection, FieldglassError> {
    if header.number != BMS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected BMS (section {BMS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if len < BMS_MIN_LEN {
        return Err(FieldglassError::Parse(format!(
            "BMS section length {len} is below the {BMS_MIN_LEN}-byte minimum"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "BMS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }

    let indicator = bytes[5];
    let bitmap = match indicator {
        BMS_INDICATOR_PRESENT => decode_inline_bitmap(&bytes[6..len], grid_points)?,
        BMS_INDICATOR_NONE => Vec::new(),
        BMS_INDICATOR_PREVIOUS => {
            return Err(FieldglassError::UnsupportedSection(format!(
                "BMS indicator {indicator} (reuse previous bitmap) is not supported"
            )));
        }
        // 1..=253: predefined bitmap indexes. Surface as unsupported with
        // the code in the message so callers can act on it specifically.
        n => {
            return Err(FieldglassError::UnsupportedSection(format!(
                "BMS indicator {n} (predefined bitmap) is not supported"
            )));
        }
    };

    Ok(BitMapSection {
        section_length: header.length,
        indicator,
        bitmap,
    })
}

/// Unpack the MSB-first inline bitmap bytes into one `bool` per grid point.
/// Errors when the byte slice is shorter than `ceil(grid_points / 8)`.
fn decode_inline_bitmap(bytes: &[u8], grid_points: usize) -> Result<Vec<bool>, FieldglassError> {
    let needed_bytes = grid_points.div_ceil(8);
    if bytes.len() < needed_bytes {
        return Err(FieldglassError::Parse(format!(
            "BMS bitmap needs {needed_bytes} bytes for {grid_points} points, got {}",
            bytes.len()
        )));
    }
    let mut bits = Vec::with_capacity(grid_points);
    for i in 0..grid_points {
        let byte = bytes[i / 8];
        let bit = (byte >> (7 - (i % 8))) & 1;
        bits.push(bit != 0);
    }
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a BMS with indicator 0 and an inline bitmap whose bits match
    /// the given `flags` (one bool per grid point).
    fn build_bms_inline(flags: &[bool]) -> Vec<u8> {
        let bitmap_bytes = flags.len().div_ceil(8);
        let len = SECTION_HEADER_LEN as u32 + 1 + bitmap_bytes as u32;
        let mut buf = Vec::with_capacity(len as usize);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(BMS_SECTION_NUMBER);
        buf.push(BMS_INDICATOR_PRESENT);
        let mut packed = vec![0u8; bitmap_bytes];
        for (i, &b) in flags.iter().enumerate() {
            if b {
                packed[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        buf.extend_from_slice(&packed);
        buf
    }

    #[test]
    fn parses_inline_bitmap_round_trip() {
        let flags = [
            true, false, true, true, false, false, true, false, true, true,
        ];
        let bytes = build_bms_inline(&flags);
        let bms = parse_bit_map(&bytes, flags.len()).expect("parse");
        assert_eq!(bms.indicator, BMS_INDICATOR_PRESENT);
        assert!(bms.has_inline_bitmap());
        assert_eq!(bms.bitmap, flags);
    }

    #[test]
    fn parses_no_bitmap_indicator() {
        let mut buf: Vec<u8> = Vec::new();
        let len = (SECTION_HEADER_LEN + 1) as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(BMS_SECTION_NUMBER);
        buf.push(BMS_INDICATOR_NONE);
        let bms = parse_bit_map(&buf, 1024).expect("parse");
        assert_eq!(bms.indicator, BMS_INDICATOR_NONE);
        assert!(!bms.has_inline_bitmap());
        assert!(bms.bitmap.is_empty());
    }

    #[test]
    fn rejects_reuse_previous_indicator() {
        let mut buf: Vec<u8> = Vec::new();
        let len = (SECTION_HEADER_LEN + 1) as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(BMS_SECTION_NUMBER);
        buf.push(BMS_INDICATOR_PREVIOUS);
        let err = parse_bit_map(&buf, 1024).expect_err("must reject");
        assert!(
            err.to_string().contains("reuse previous"),
            "names reuse-previous, got: {err}",
        );
    }

    #[test]
    fn rejects_predefined_indicator() {
        let mut buf: Vec<u8> = Vec::new();
        let len = (SECTION_HEADER_LEN + 1) as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(BMS_SECTION_NUMBER);
        buf.push(42); // predefined bitmap id 42
        let err = parse_bit_map(&buf, 1024).expect_err("must reject");
        assert!(
            err.to_string().contains("predefined bitmap"),
            "names predefined, got: {err}",
        );
    }

    #[test]
    fn rejects_truncated_inline_bitmap() {
        // Declare the section length too short to hold the bitmap bytes
        // for the requested grid-point count.
        let mut buf: Vec<u8> = Vec::new();
        let len = (SECTION_HEADER_LEN + 1 + 1) as u32; // 1 byte = 8 points max
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(BMS_SECTION_NUMBER);
        buf.push(BMS_INDICATOR_PRESENT);
        buf.push(0xFF);
        let err = parse_bit_map(&buf, 100).expect_err("must reject");
        assert!(
            err.to_string().contains("BMS bitmap needs"),
            "names bitmap-size shortfall, got: {err}",
        );
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_bms_inline(&[true, false]);
        bytes[4] = 5;
        assert!(parse_bit_map(&bytes, 2).is_err());
    }

    #[test]
    fn drops_pad_bits_in_last_byte() {
        // 10-bit bitmap encoded in 2 bytes — only the first 10 bits matter,
        // the remaining 6 pad bits should be ignored.
        let flags = [true; 10];
        let mut bytes = build_bms_inline(&flags);
        // Flip the pad bits to 1s — the decoder must still report 10 trues.
        *bytes.last_mut().unwrap() |= 0b0011_1111;
        let bms = parse_bit_map(&bytes, 10).expect("parse");
        assert_eq!(bms.bitmap.len(), 10);
        assert!(bms.bitmap.iter().all(|b| *b));
    }
}
