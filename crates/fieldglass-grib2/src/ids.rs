//! GRIB2 Identification Section (§1).

use crate::section::{SectionHeader, parse_section_header};
use fieldglass_core::FieldglassError;

/// Section number for the Identification Section.
pub const IDS_SECTION_NUMBER: u8 = 1;

/// Minimum byte length of the IDS, per WMO Manual on Codes Vol I.2
/// (FM 92 GRIB Edition 2, Table 1). Octets 1..=21 are required; octet 22+
/// are reserved/optional.
pub const IDS_MIN_LEN: usize = 21;

/// Parsed contents of the Identification Section.
///
/// | Octet  | Field                                                    |
/// |--------|----------------------------------------------------------|
/// | 1–4    | section length                                           |
/// | 5      | section number = 1                                       |
/// | 6–7    | originating/generating centre (Common Code Table C-1)    |
/// | 8–9    | originating/generating sub-centre                        |
/// | 10     | GRIB master tables version (Code Table 1.0)              |
/// | 11     | local tables version (Code Table 1.1)                    |
/// | 12     | significance of reference time (Code Table 1.2)          |
/// | 13–14  | year (4 digits)                                          |
/// | 15     | month                                                    |
/// | 16     | day                                                      |
/// | 17     | hour                                                     |
/// | 18     | minute                                                   |
/// | 19     | second                                                   |
/// | 20     | production status of processed data (Code Table 1.3)     |
/// | 21     | type of processed data (Code Table 1.4)                  |
#[derive(Debug, Clone, Copy)]
pub struct IdentificationSection {
    pub section_length: u32,
    pub centre: u16,
    pub sub_centre: u16,
    pub master_tables_version: u8,
    pub local_tables_version: u8,
    pub reference_time_significance: u8,
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub production_status: u8,
    pub data_type: u8,
}

impl IdentificationSection {
    /// Render the reference time as ISO-8601 (`YYYY-MM-DDTHH:MM:SSZ`).
    /// Bytes are emitted verbatim — no validation of calendar legality.
    pub fn reference_time_iso8601(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Parse the Identification Section starting at `bytes[0]`.
///
/// `bytes` must begin at the section header; the slice may extend past the
/// section — only the declared `section_length` bytes are consumed.
pub fn parse_identification(bytes: &[u8]) -> Result<IdentificationSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_identification_with_header(bytes, header)
}

/// Variant for callers that have already read the section header — avoids
/// re-parsing the first 5 bytes when walking the message section-by-section.
pub fn parse_identification_with_header(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<IdentificationSection, FieldglassError> {
    if header.number != IDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected IDS (section {IDS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if len < IDS_MIN_LEN {
        return Err(FieldglassError::Parse(format!(
            "IDS section length {len} is below required minimum {IDS_MIN_LEN}"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "IDS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }
    // Octet indices below are 1-based per the WMO spec; subtract 1 for the slice.
    let centre = u16::from_be_bytes([bytes[5], bytes[6]]);
    let sub_centre = u16::from_be_bytes([bytes[7], bytes[8]]);
    let year = u16::from_be_bytes([bytes[12], bytes[13]]);

    Ok(IdentificationSection {
        section_length: header.length,
        centre,
        sub_centre,
        master_tables_version: bytes[9],
        local_tables_version: bytes[10],
        reference_time_significance: bytes[11],
        year,
        month: bytes[14],
        day: bytes[15],
        hour: bytes[16],
        minute: bytes[17],
        second: bytes[18],
        production_status: bytes[19],
        data_type: bytes[20],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimum-valid 21-byte IDS for testing.
    #[allow(clippy::too_many_arguments)]
    fn build_ids(
        centre: u16,
        sub_centre: u16,
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        production_status: u8,
        data_type: u8,
    ) -> [u8; 21] {
        let mut buf = [0u8; 21];
        buf[0..4].copy_from_slice(&21u32.to_be_bytes());
        buf[4] = IDS_SECTION_NUMBER;
        buf[5..7].copy_from_slice(&centre.to_be_bytes());
        buf[7..9].copy_from_slice(&sub_centre.to_be_bytes());
        buf[9] = 5; // master tables
        buf[10] = 0; // local tables
        buf[11] = 1; // ref-time significance
        buf[12..14].copy_from_slice(&year.to_be_bytes());
        buf[14] = month;
        buf[15] = day;
        buf[16] = hour;
        buf[17] = 0;
        buf[18] = 0;
        buf[19] = production_status;
        buf[20] = data_type;
        buf
    }

    #[test]
    fn parses_minimal_valid_ids() {
        let bytes = build_ids(98, 0, 2008, 2, 6, 12, 0, 1);
        let ids = parse_identification(&bytes).expect("parse");
        assert_eq!(ids.section_length, 21);
        assert_eq!(ids.centre, 98);
        assert_eq!(ids.sub_centre, 0);
        assert_eq!(ids.master_tables_version, 5);
        assert_eq!(ids.year, 2008);
        assert_eq!(ids.month, 2);
        assert_eq!(ids.day, 6);
        assert_eq!(ids.hour, 12);
        assert_eq!(ids.production_status, 0);
        assert_eq!(ids.data_type, 1);
    }

    #[test]
    fn formats_reference_time_iso8601() {
        let bytes = build_ids(98, 0, 2008, 2, 6, 12, 0, 0);
        let ids = parse_identification(&bytes).expect("parse");
        assert_eq!(ids.reference_time_iso8601(), "2008-02-06T12:00:00Z");
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_ids(98, 0, 2008, 2, 6, 12, 0, 0);
        bytes[4] = 3; // claim to be GDS
        let err = parse_identification(&bytes).expect_err("must reject");
        assert!(err.to_string().contains("section"));
    }

    #[test]
    fn rejects_length_below_minimum() {
        let mut bytes = build_ids(98, 0, 2008, 2, 6, 12, 0, 0);
        bytes[0..4].copy_from_slice(&20u32.to_be_bytes());
        assert!(parse_identification(&bytes).is_err());
    }

    #[test]
    fn rejects_length_exceeding_buffer() {
        let mut bytes = build_ids(98, 0, 2008, 2, 6, 12, 0, 0);
        bytes[0..4].copy_from_slice(&100u32.to_be_bytes());
        assert!(parse_identification(&bytes).is_err());
    }

    #[test]
    fn parses_max_year() {
        let bytes = build_ids(0, 0, 9999, 12, 31, 23, 0, 0);
        let ids = parse_identification(&bytes).expect("parse");
        assert_eq!(ids.year, 9999);
        assert_eq!(ids.reference_time_iso8601(), "9999-12-31T23:00:00Z");
    }

    #[test]
    fn parses_two_byte_centre() {
        // 0x1234 = 4660 — verifies octets 6..=7 are read big-endian, not just octet 7.
        let bytes = build_ids(0x1234, 0x00FF, 2024, 1, 1, 0, 0, 0);
        let ids = parse_identification(&bytes).expect("parse");
        assert_eq!(ids.centre, 0x1234);
        assert_eq!(ids.sub_centre, 0x00FF);
    }
}
