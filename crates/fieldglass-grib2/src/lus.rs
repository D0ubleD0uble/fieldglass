//! GRIB2 Local Use Section (§2).
//!
//! §2 is **optional**. Centres use it for opaque, centre-specific metadata;
//! the WMO spec deliberately does not define its contents. We surface the
//! byte range so callers (and future centre-specific decoders) can pull the
//! raw bytes without re-walking sections.

use crate::section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
use fieldglass_core::FieldglassError;

/// Section number for the Local Use Section.
pub const LUS_SECTION_NUMBER: u8 = 2;

/// Parsed contents of the Local Use Section.
///
/// The body is opaque — `local_data_len` is the byte length of the centre-
/// specific payload (`section_length - 5`).
#[derive(Debug, Clone, Copy)]
pub struct LocalUseSection {
    pub section_length: u32,
    pub local_data_len: usize,
}

/// Parse the Local Use Section starting at `bytes[0]`.
pub fn parse_local_use(bytes: &[u8]) -> Result<LocalUseSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_local_use_with_header(bytes, header)
}

/// Variant for callers that have already read the section header.
pub fn parse_local_use_with_header(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<LocalUseSection, FieldglassError> {
    if header.number != LUS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected LUS (section {LUS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "LUS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }
    let local_data_len = len - SECTION_HEADER_LEN;
    Ok(LocalUseSection {
        section_length: header.length,
        local_data_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_lus(local_data: &[u8]) -> Vec<u8> {
        let len = (SECTION_HEADER_LEN + local_data.len()) as u32;
        let mut buf = Vec::with_capacity(len as usize);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(LUS_SECTION_NUMBER);
        buf.extend_from_slice(local_data);
        buf
    }

    #[test]
    fn parses_lus_with_payload() {
        let bytes = build_lus(&[0xAA, 0xBB, 0xCC, 0xDD]);
        let lus = parse_local_use(&bytes).expect("parse");
        assert_eq!(lus.section_length, 9);
        assert_eq!(lus.local_data_len, 4);
    }

    #[test]
    fn parses_empty_payload() {
        let bytes = build_lus(&[]);
        let lus = parse_local_use(&bytes).expect("parse");
        assert_eq!(lus.section_length, 5);
        assert_eq!(lus.local_data_len, 0);
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_lus(&[0xAA]);
        bytes[4] = 1; // claim to be IDS
        let err = parse_local_use(&bytes).expect_err("must reject");
        assert!(err.to_string().contains("section"));
    }

    #[test]
    fn rejects_length_exceeding_buffer() {
        let mut bytes = build_lus(&[0xAA, 0xBB]);
        bytes[0..4].copy_from_slice(&100u32.to_be_bytes());
        assert!(parse_local_use(&bytes).is_err());
    }
}
