use fieldglass_core::FieldglassError;
use fieldglass_core::bitmap::{bitmap_bit_len, unpack_bitmap};

/// Parsed Bit Map Section. The bitmap has one boolean per grid point in
/// scan order: `true` means the corresponding value is present in the BDS,
/// `false` means it is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    /// Length of the section in bytes, from its own 3-octet length prefix.
    pub section_len: u32,
    /// Predefined bitmap indicator (0 = bitmap follows in this section).
    pub predefined_indicator: u16,
    /// One flag per grid point in scan order, exactly the GDS point count:
    /// `true` when the BDS carries a value for that point.
    pub bits: Vec<bool>,
}

/// Parse a Bit Map Section. `bytes` must begin at the BMS length octets.
/// `expected_count` is the total number of grid points (from the GDS); the
/// returned `bits` has exactly that length. A bitmap holding fewer bits than
/// that, after the unused trailing bits octet 4 declares, is an error; bits
/// past the last point are ignored.
pub fn parse_bitmap(bytes: &[u8], expected_count: usize) -> Result<Bitmap, FieldglassError> {
    if bytes.len() < 6 {
        return Err(FieldglassError::Parse(format!(
            "BMS too short for header: {} bytes",
            bytes.len()
        )));
    }

    let section_len = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
    if (section_len as usize) < 6 {
        return Err(FieldglassError::Parse(format!(
            "BMS section_len {section_len} below minimum of 6"
        )));
    }
    if bytes.len() < section_len as usize {
        return Err(FieldglassError::Parse(format!(
            "BMS section_len {section_len} exceeds available bytes {}",
            bytes.len()
        )));
    }

    let unused_trailing = bytes[3];
    let predefined_indicator = u16::from_be_bytes([bytes[4], bytes[5]]);

    // A non-zero predefined indicator means the bitmap is referenced by id and
    // is not embedded in the section. We don't carry a registry of predefined
    // bitmaps, so we surface this as unsupported rather than silently returning
    // an all-present mask.
    if predefined_indicator != 0 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "BMS references predefined bitmap id {predefined_indicator} \
             (this build does not carry a registry of predefined bitmaps)"
        )));
    }

    let bitmap_bytes = &bytes[6..section_len as usize];
    // An empty body with unused_trailing > 0 would underflow `len * 8 - unused`;
    // `bitmap_bit_len` is proved to return `None` there instead.
    let total_bits = bitmap_bit_len(bitmap_bytes.len(), unused_trailing).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "BMS unused_trailing {unused_trailing} exceeds bitmap body of {} bytes",
            bitmap_bytes.len()
        ))
    })?;
    // One bit per grid point (Section 3, octet 7 onwards: "contiguous bits with
    // a bit to data point correspondence"). A bitmap with fewer bits cannot say
    // which points are present. More is fine: sections are padded to an even
    // length, and not every encoder counts that padding in octet 4.
    if total_bits < expected_count {
        return Err(FieldglassError::Parse(format!(
            "BMS holds {total_bits} bits but the grid has {expected_count} points"
        )));
    }
    // `total_bits` is at most the body's bit length, so this is `Some`.
    let bits = unpack_bitmap(bitmap_bytes, expected_count).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "BMS body of {} bytes is short of {expected_count} bits",
            bitmap_bytes.len()
        ))
    })?;

    Ok(Bitmap {
        section_len,
        predefined_indicator,
        bits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bms(unused_trailing: u8, bitmap: &[u8]) -> Vec<u8> {
        let len = (6 + bitmap.len()) as u32;
        let mut bytes = vec![
            (len >> 16) as u8,
            (len >> 8) as u8,
            len as u8,
            unused_trailing,
            0,
            0, // predefined indicator = 0 (bitmap embedded)
        ];
        bytes.extend_from_slice(bitmap);
        bytes
    }

    #[test]
    fn parses_full_byte_bitmap() {
        // 0b1010_1010 → present, missing, present, missing, …
        let bms = build_bms(0, &[0b1010_1010]);
        let bm = parse_bitmap(&bms, 8).unwrap();
        assert_eq!(
            bm.bits,
            vec![true, false, true, false, true, false, true, false]
        );
    }

    #[test]
    fn truncates_to_expected_count() {
        let bms = build_bms(0, &[0xFF]);
        let bm = parse_bitmap(&bms, 5).unwrap();
        assert_eq!(bm.bits.len(), 5);
        assert!(bm.bits.iter().all(|b| *b));
    }

    #[test]
    fn honours_unused_trailing_bits() {
        // 0b1111_1100 with 2 trailing unused → 6 bits, all present.
        let bms = build_bms(2, &[0b1111_1100]);
        let bm = parse_bitmap(&bms, 6).unwrap();
        assert_eq!(bm.bits, vec![true; 6]);
    }

    #[test]
    fn rejects_predefined_bitmap() {
        let mut bms = build_bms(0, &[0xFF]);
        bms[4] = 0;
        bms[5] = 1; // non-zero predefined indicator
        assert!(matches!(
            parse_bitmap(&bms, 8).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
    }

    #[test]
    fn parses_multi_byte_bitmap_in_scan_order() {
        // 24 bits across 3 bytes: alternating present-byte / missing-byte / mixed.
        // Verifies the i/8, 0x80 >> i%8 traversal works across byte boundaries.
        let bms = build_bms(0, &[0xFF, 0x00, 0b1100_0011]);
        let bm = parse_bitmap(&bms, 24).unwrap();
        let expected: Vec<bool> = [true; 8]
            .iter()
            .copied()
            .chain([false; 8].iter().copied())
            .chain([true, true, false, false, false, false, true, true])
            .collect();
        assert_eq!(bm.bits, expected);
    }

    #[test]
    fn all_missing_bitmap_yields_all_false() {
        let bms = build_bms(0, &[0x00, 0x00]);
        let bm = parse_bitmap(&bms, 16).unwrap();
        assert_eq!(bm.bits.len(), 16);
        assert!(bm.bits.iter().all(|b| !*b));
    }

    #[test]
    fn expected_count_larger_than_bitmap_is_an_error() {
        // 8 bits of data, but the grid has 16 points. This used to return the
        // 8 flags there were, on the expectation that a later length check
        // would catch it; none did, and the reader returned a short field.
        let bms = build_bms(0, &[0xFF]);
        let err = parse_bitmap(&bms, 16).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("holds 8 bits")),
            "got {err:?}"
        );
    }

    #[test]
    fn unused_trailing_bits_count_against_the_grid() {
        // 16 bits, the last 2 unused, leaves 14: one short of 15 points.
        let bms = build_bms(2, &[0xFF, 0xFC]);
        assert!(parse_bitmap(&bms, 14).is_ok());
        assert!(parse_bitmap(&bms, 15).is_err());
    }

    #[test]
    fn rejects_section_too_short_for_header() {
        let too_short = vec![0, 0, 5, 0]; // claims length 5 but no body
        assert!(matches!(
            parse_bitmap(&too_short, 0).unwrap_err(),
            FieldglassError::Parse(_)
        ));
    }

    #[test]
    fn rejects_section_len_exceeding_buffer() {
        // section_len declares 12 but only 8 bytes provided.
        let mut bms = vec![0, 0, 12, 0, 0, 0];
        bms.extend_from_slice(&[0xFF, 0xFF]); // 8 bytes total
        assert!(matches!(
            parse_bitmap(&bms, 16).unwrap_err(),
            FieldglassError::Parse(_)
        ));
    }
}
