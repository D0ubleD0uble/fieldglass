use fieldglass_core::FieldglassError;
use fieldglass_core::bits::{ibm_float_to_f64, sign_magnitude_i16};

/// Header of the Binary Data Section. Does not own the packed data.
#[derive(Debug)]
pub struct BdsHeader {
    pub section_len: u32,
    /// True = spherical-harmonic coefficients (unsupported here).
    pub is_spherical_harmonic: bool,
    /// True = complex / second-order packing (unsupported here).
    pub is_complex_packing: bool,
    /// True = integer values; false = floating point.
    pub is_integer_data: bool,
    /// True if extra flag octets follow (complex packing only).
    pub has_extra_flags: bool,
    /// Number of unused bits at the end of the packed data stream.
    pub unused_trailing_bits: u8,
    /// Binary scale factor E (sign-magnitude i16 in the wire format).
    pub binary_scale_factor: i16,
    /// Reference value R, decoded from IBM single-precision float.
    pub reference_value: f64,
    /// Bits per packed value N. Zero means a constant field.
    pub bits_per_value: u8,
}

/// Offset (within the BDS) at which packed data values begin.
pub const BDS_DATA_OFFSET: usize = 11;

/// Parse the 11-byte BDS header. `bytes` should begin at the start of the BDS.
pub fn parse_bds_header(bytes: &[u8]) -> Result<BdsHeader, FieldglassError> {
    if bytes.len() < BDS_DATA_OFFSET {
        return Err(FieldglassError::Parse(format!(
            "BDS header requires {BDS_DATA_OFFSET} bytes, got {}",
            bytes.len()
        )));
    }

    let section_len = read_u24(&bytes[0..3]);
    if (section_len as usize) < BDS_DATA_OFFSET {
        return Err(FieldglassError::Parse(format!(
            "BDS section_len {section_len} below minimum of {BDS_DATA_OFFSET}"
        )));
    }
    if bytes.len() < section_len as usize {
        return Err(FieldglassError::Parse(format!(
            "BDS section_len {section_len} exceeds available bytes {}",
            bytes.len()
        )));
    }

    let flag = bytes[3];
    Ok(BdsHeader {
        section_len,
        is_spherical_harmonic: flag & 0x80 != 0,
        is_complex_packing: flag & 0x40 != 0,
        is_integer_data: flag & 0x20 != 0,
        has_extra_flags: flag & 0x10 != 0,
        unused_trailing_bits: flag & 0x0F,
        binary_scale_factor: sign_magnitude_i16(u16::from_be_bytes([bytes[4], bytes[5]])),
        reference_value: ibm_float_to_f64(u32::from_be_bytes([
            bytes[6], bytes[7], bytes[8], bytes[9],
        ])),
        bits_per_value: bytes[10],
    })
}

/// Decode a BDS into floating-point values.
///
/// `bds` is the full Binary Data Section starting at its length octets;
/// `header` is the parsed header for `bds`; `decimal_scale` is the PDS
/// `decimal_scale_factor` (D); `bitmap` is the BMS bitmap if one was
/// present; `expected_count` is the total number of grid points (from the
/// GDS).
///
/// Returns one `Option<f64>` per grid point: `None` for points masked out
/// by the bitmap, `Some(value)` otherwise. The actual decoding is
/// delegated to a [`crate::packing::Grib1Packing`] implementation chosen
/// by the BDS flag bits — only simple grid-point packing is implemented
/// today; complex/second-order and spherical-harmonic packings surface
/// [`FieldglassError::UnsupportedSection`] with a message naming the
/// specific packing mode.
pub fn decode_values(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    crate::packing::decoder_for(header).decode(bds, header, decimal_scale, bitmap, expected_count)
}

fn read_u24(b: &[u8]) -> u32 {
    u32::from_be_bytes([0, b[0], b[1], b[2]])
}

// ---------------------------------------------------------------------------
// Tests — exercise the public `decode_values` API end-to-end. Bit-utility
// unit tests live alongside the utilities themselves in
// `fieldglass_core::bits`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_constant_field() {
        // bits_per_value = 0 → all points equal R / 10^D.
        let header = BdsHeader {
            section_len: BDS_DATA_OFFSET as u32,
            is_spherical_harmonic: false,
            is_complex_packing: false,
            is_integer_data: false,
            has_extra_flags: false,
            unused_trailing_bits: 0,
            binary_scale_factor: 0,
            reference_value: 42.0,
            bits_per_value: 0,
        };
        let bds = vec![0u8; BDS_DATA_OFFSET];
        let out = decode_values(&bds, &header, 0, None, 4).unwrap();
        assert_eq!(out, vec![Some(42.0); 4]);
    }

    #[test]
    fn decode_simple_packing_round_trip() {
        // 4 values packed at 8 bits each, R=0, E=0, D=0 → identity.
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds.extend_from_slice(&[1, 2, 3, 4]);
        let section_len = bds.len() as u32;
        bds[0..3].copy_from_slice(&[
            (section_len >> 16) as u8,
            (section_len >> 8) as u8,
            section_len as u8,
        ]);
        bds[10] = 8; // N
        let header = parse_bds_header(&bds).unwrap();
        let out = decode_values(&bds, &header, 0, None, 4).unwrap();
        assert_eq!(out, vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    }

    #[test]
    fn decode_with_bitmap_inserts_none() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds.extend_from_slice(&[7, 9]);
        let section_len = bds.len() as u32;
        bds[0..3].copy_from_slice(&[
            (section_len >> 16) as u8,
            (section_len >> 8) as u8,
            section_len as u8,
        ]);
        bds[10] = 8;
        let header = parse_bds_header(&bds).unwrap();
        let bitmap = [true, false, true, false];
        let out = decode_values(&bds, &header, 0, Some(&bitmap), 4).unwrap();
        assert_eq!(out, vec![Some(7.0), None, Some(9.0), None]);
    }

    #[test]
    fn rejects_complex_packing() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        bds[3] = 0x40; // complex packing flag
        let header = parse_bds_header(&bds).unwrap();
        assert!(matches!(
            decode_values(&bds, &header, 0, None, 1).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
    }

    #[test]
    fn rejects_spherical_harmonic_packing() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        bds[3] = 0x80; // spherical-harmonic flag
        let header = parse_bds_header(&bds).unwrap();
        let err = decode_values(&bds, &header, 0, None, 1).unwrap_err();
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("spherical-harmonic"), "msg = {msg:?}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }
}
