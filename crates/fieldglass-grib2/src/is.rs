use fieldglass_core::FieldglassError;

/// Length, in bytes, of the GRIB2 Indicator Section (Section 0).
pub const INDICATOR_SECTION_LEN: usize = 16;

/// Trailing 4-byte End Section "7777" marker.
pub const END_SECTION_LEN: usize = 4;

/// GRIB edition number for GRIB2.
pub const GRIB2_EDITION: u8 = 2;

/// Parsed contents of the GRIB2 Indicator Section (Section 0, 16 bytes).
///
/// Per WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2):
///
/// | Octet  | Field                                              |
/// |--------|----------------------------------------------------|
/// | 1–4    | "GRIB" magic                                       |
/// | 5–6    | reserved                                           |
/// | 7      | discipline (Code Table 0.0)                        |
/// | 8      | edition number (always 2)                          |
/// | 9–16   | total length of message in bytes (64-bit unsigned) |
#[derive(Debug, Clone, Copy)]
pub struct IndicatorSection {
    /// WMO Code Table 0.0 discipline (e.g. 0 = Meteorological products).
    pub discipline: u8,
    /// GRIB edition number — always 2 for a valid GRIB2 message.
    pub edition: u8,
    /// Total length of the GRIB2 message in bytes (IS + sections 1..7 + ES).
    pub total_length: u64,
}

/// Parse the 16-byte GRIB2 Indicator Section starting at `bytes[0]`.
///
/// Returns an error if the slice is shorter than 16 bytes, the magic bytes
/// don't match `GRIB`, or the edition byte isn't `2`.
pub fn parse_indicator(bytes: &[u8]) -> Result<IndicatorSection, FieldglassError> {
    if bytes.len() < INDICATOR_SECTION_LEN {
        return Err(FieldglassError::Parse(format!(
            "GRIB2 IS requires {INDICATOR_SECTION_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if &bytes[0..4] != b"GRIB" {
        return Err(FieldglassError::InvalidMagic);
    }
    let discipline = bytes[6];
    let edition = bytes[7];
    if edition != GRIB2_EDITION {
        return Err(FieldglassError::Parse(format!(
            "expected GRIB edition 2, got edition {edition}"
        )));
    }
    let total_length = u64::from_be_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    Ok(IndicatorSection {
        discipline,
        edition,
        total_length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_is(discipline: u8, edition: u8, total_length: u64) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(b"GRIB");
        buf[6] = discipline;
        buf[7] = edition;
        buf[8..16].copy_from_slice(&total_length.to_be_bytes());
        buf
    }

    #[test]
    fn parses_minimal_valid_is() {
        let bytes = make_is(0, 2, 16);
        let is = parse_indicator(&bytes).expect("parse");
        assert_eq!(is.discipline, 0);
        assert_eq!(is.edition, 2);
        assert_eq!(is.total_length, 16);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut bytes = make_is(0, 2, 16);
        bytes[0] = b'X';
        match parse_indicator(&bytes) {
            Err(FieldglassError::InvalidMagic) => {}
            other => panic!("expected InvalidMagic, got {other:?}"),
        }
    }

    #[test]
    fn rejects_short_buffer() {
        let bytes = [0u8; 8];
        assert!(parse_indicator(&bytes).is_err());
    }

    #[test]
    fn rejects_non_edition_2() {
        let bytes = make_is(0, 1, 100);
        match parse_indicator(&bytes) {
            Err(FieldglassError::Parse(msg)) => assert!(msg.contains("edition 1")),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn parses_64_bit_length() {
        // 5 GiB length — exceeds u32, must round-trip through u64.
        let big = 5_368_709_120u64;
        let bytes = make_is(2, 2, big);
        let is = parse_indicator(&bytes).expect("parse");
        assert_eq!(is.total_length, big);
        assert_eq!(is.discipline, 2);
    }
}
