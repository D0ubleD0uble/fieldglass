use fieldglass_core::FieldglassError;

/// Parsed Bit Map Section. The bitmap has one boolean per grid point in
/// scan order: `true` means the corresponding value is present in the BDS,
/// `false` means it is missing.
#[derive(Debug)]
pub struct Bitmap {
    pub section_len: u32,
    /// Predefined bitmap indicator (0 = bitmap follows in this section).
    pub predefined_indicator: u16,
    pub bits: Vec<bool>,
}

/// Parse a Bit Map Section. `bytes` must begin at the BMS length octets.
/// `expected_count` is the total number of grid points (from the GDS); the
/// returned `bits` is truncated to that length.
pub fn parse_bitmap(
    bytes: &[u8],
    expected_count: usize,
) -> Result<Bitmap, FieldglassError> {
    if bytes.len() < 6 {
        return Err(FieldglassError::Parse(format!(
            "BMS too short for header: {} bytes", bytes.len()
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
        return Err(FieldglassError::UnsupportedSection);
    }

    let bitmap_bytes = &bytes[6..section_len as usize];
    let total_bits = bitmap_bytes.len() * 8 - unused_trailing as usize;
    let take = total_bits.min(expected_count);

    let mut bits = Vec::with_capacity(take);
    for i in 0..take {
        let byte = bitmap_bytes[i / 8];
        let mask = 0x80u8 >> (i % 8);
        bits.push(byte & mask != 0);
    }

    Ok(Bitmap { section_len, predefined_indicator, bits })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bms(unused_trailing: u8, bitmap: &[u8]) -> Vec<u8> {
        let len = (6 + bitmap.len()) as u32;
        let mut bytes = vec![
            (len >> 16) as u8, (len >> 8) as u8, len as u8,
            unused_trailing,
            0, 0, // predefined indicator = 0 (bitmap embedded)
        ];
        bytes.extend_from_slice(bitmap);
        bytes
    }

    #[test]
    fn parses_full_byte_bitmap() {
        // 0b1010_1010 → present, missing, present, missing, …
        let bms = build_bms(0, &[0b1010_1010]);
        let bm = parse_bitmap(&bms, 8).unwrap();
        assert_eq!(bm.bits, vec![true, false, true, false, true, false, true, false]);
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
        bms[4] = 0; bms[5] = 1; // non-zero predefined indicator
        assert!(matches!(
            parse_bitmap(&bms, 8).unwrap_err(),
            FieldglassError::UnsupportedSection
        ));
    }
}
