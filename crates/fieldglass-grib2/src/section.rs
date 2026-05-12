//! Common GRIB2 section header.
//!
//! Every section after the Indicator Section (§1 through §7) starts with the
//! same 5-byte preamble: a 4-byte big-endian length followed by a 1-byte
//! section number. Centralising the parse keeps each per-section module
//! focused on its own template fields and lets the message walker dispatch
//! on `number` without re-reading the header.

use fieldglass_core::FieldglassError;

/// Length, in bytes, of the common section header (length + section number).
pub const SECTION_HEADER_LEN: usize = 5;

/// Header of a non-indicator GRIB2 section.
#[derive(Debug, Clone, Copy)]
pub struct SectionHeader {
    /// Total length of the section in bytes, including the 5-byte header.
    pub length: u32,
    /// Section number (1 = IDS, 2 = LUS, 3 = GDS, …, 7 = DS).
    pub number: u8,
}

/// Parse the common 5-byte section header at the start of `bytes`.
///
/// Errors if the slice is shorter than 5 bytes or if the declared length is
/// itself shorter than the header.
pub fn parse_section_header(bytes: &[u8]) -> Result<SectionHeader, FieldglassError> {
    if bytes.len() < SECTION_HEADER_LEN {
        return Err(FieldglassError::Parse(format!(
            "section header requires {SECTION_HEADER_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    let length = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if (length as usize) < SECTION_HEADER_LEN {
        return Err(FieldglassError::Parse(format!(
            "section length {length} is below the {SECTION_HEADER_LEN}-byte header minimum"
        )));
    }
    Ok(SectionHeader {
        length,
        number: bytes[4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimum_header() {
        let bytes = [0x00, 0x00, 0x00, 0x05, 0x07];
        let h = parse_section_header(&bytes).expect("parse");
        assert_eq!(h.length, 5);
        assert_eq!(h.number, 7);
    }

    #[test]
    fn parses_typical_ids_header() {
        // 21-byte IDS, section number 1.
        let bytes = [0x00, 0x00, 0x00, 0x15, 0x01];
        let h = parse_section_header(&bytes).expect("parse");
        assert_eq!(h.length, 21);
        assert_eq!(h.number, 1);
    }

    #[test]
    fn rejects_short_buffer() {
        let bytes = [0x00, 0x00, 0x00];
        assert!(parse_section_header(&bytes).is_err());
    }

    #[test]
    fn rejects_length_below_header() {
        // Declared length < 5 is structurally impossible.
        let bytes = [0x00, 0x00, 0x00, 0x04, 0x01];
        assert!(parse_section_header(&bytes).is_err());
    }
}
