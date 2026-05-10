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
    // unused_trailing is attacker-controlled (single octet, 0..=255) and
    // bitmap_bytes can be empty when section_len==6, so the naive
    // `len*8 - unused_trailing` underflows. Use checked arithmetic so a
    // crafted file produces a parse error rather than a panic / wrap.
    let total_bits = bitmap_bytes
        .len()
        .checked_mul(8)
        .and_then(|t| t.checked_sub(unused_trailing as usize))
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "BMS unused_trailing {unused_trailing} exceeds bitmap body of {} bytes",
                bitmap_bytes.len()
            ))
        })?;
    let take = total_bits.min(expected_count);

    let mut bits = Vec::with_capacity(take);
    for i in 0..take {
        let byte = bitmap_bytes[i / 8];
        let mask = 0x80u8 >> (i % 8);
        bits.push(byte & mask != 0);
    }

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
    fn expected_count_larger_than_bitmap_takes_what_is_available() {
        // 8 bits of data, but caller claims 16 grid points. Implementation
        // takes the minimum so we never index past the buffer; downstream
        // decode_grid relies on this to surface the discrepancy as a length
        // mismatch rather than as a panic.
        let bms = build_bms(0, &[0xFF]);
        let bm = parse_bitmap(&bms, 16).unwrap();
        assert_eq!(bm.bits.len(), 8, "should clamp to available bitmap bits");
        assert!(bm.bits.iter().all(|b| *b));
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
