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
    /// Bits per packed value N. For simple packing this is the per-point
    /// width; for complex packing this same octet (octet 11) is repurposed
    /// as `widthOfFirstOrderValues`. Zero means a constant field.
    pub bits_per_value: u8,
    /// Present when `is_complex_packing && has_extra_flags`. Holds N1 + the
    /// extended flag byte (octets 12-14) so [`crate::packing`] decoders can
    /// branch on the precise variant without re-parsing the section header.
    pub complex_extended: Option<ComplexExtendedHeader>,
}

/// The 3-octet header that follows the standard 11-octet BDS header when
/// `is_complex_packing && has_extra_flags`. See WMO Manual on Codes Vol I.2,
/// "GRIB1 BDS extended flag" (mirrored in eccodes' `grib1/section.4.def`).
#[derive(Debug, Clone, Copy)]
pub struct ComplexExtendedHeader {
    /// Octets 12-13. Byte offset (from start of BDS) to the first-order
    /// packed reference values.
    pub n1: u16,
    /// Octet 14. Bit positions follow the WMO numbering — bit 1 is the MSB.
    /// Use the named accessors below rather than touching this directly.
    pub extended_flag: u8,
}

impl ComplexExtendedHeader {
    /// Bit 2 (0x40). True = matrix of values per grid point.
    pub fn matrix_of_values(self) -> bool {
        self.extended_flag & 0x40 != 0
    }
    /// Bit 3 (0x20). True = secondary bitmap present.
    pub fn secondary_bitmap_present(self) -> bool {
        self.extended_flag & 0x20 != 0
    }
    /// Bit 4 (0x10). True = each group has a different width;
    /// false = all groups share one constant width.
    pub fn second_order_of_different_width(self) -> bool {
        self.extended_flag & 0x10 != 0
    }
    /// Bit 5 (0x08). True = "general extended" second-order packing
    /// (ECMWF's most common encoding).
    pub fn general_extended_2ordr(self) -> bool {
        self.extended_flag & 0x08 != 0
    }
    /// Bit 6 (0x04). True = boustrophedonic (zig-zag) row scan.
    pub fn boustrophedonic(self) -> bool {
        self.extended_flag & 0x04 != 0
    }
    /// Bit 7 (0x02). High bit of the 2-bit `orderOfSPD` field.
    pub fn two_orders_of_spd(self) -> bool {
        self.extended_flag & 0x02 != 0
    }
    /// Bit 8 (0x01). Low bit of the 2-bit `orderOfSPD` field.
    pub fn plus_one_in_orders_of_spd(self) -> bool {
        self.extended_flag & 0x01 != 0
    }
    /// Order of spatial differencing (0..=3). 0 = none, 1/2/3 = first/second/
    /// third-order predictor encoding. ECMWF's default `grid_second_order`
    /// variant uses order 2.
    pub fn order_of_spd(self) -> u8 {
        u8::from(self.plus_one_in_orders_of_spd()) + 2 * u8::from(self.two_orders_of_spd())
    }
    /// Map the extended-flag bits to eccodes' `packingType` label. Mirrors
    /// the concept dispatch in `grib1/section.4.def` so error messages and
    /// (future) decoders can route on the same name eccodes prints.
    pub fn packing_type_label(self) -> &'static str {
        match (
            self.secondary_bitmap_present(),
            self.second_order_of_different_width(),
            self.general_extended_2ordr(),
            self.order_of_spd(),
        ) {
            (false, true, true, 0) => "grid_second_order_no_SPD",
            (false, true, true, 1) => "grid_second_order_SPD1",
            (false, true, true, 2) => "grid_second_order",
            (false, true, true, 3) => "grid_second_order_SPD3",
            (false, true, false, _) => "grid_second_order_row_by_row",
            (true, false, false, _) => "grid_second_order_constant_width",
            (true, true, false, _) => "grid_second_order_general_grib1",
            _ => "grid_second_order_unknown",
        }
    }
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
    let is_spherical_harmonic = flag & 0x80 != 0;
    let is_complex_packing = flag & 0x40 != 0;
    let has_extra_flags = flag & 0x10 != 0;

    // Octets 12-14 are only present (and meaningful) for complex packing
    // with the extra-flags bit set. They are not used by spherical-harmonic
    // packing, which has its own follow-on layout we don't decode today.
    let complex_extended = if is_complex_packing && !is_spherical_harmonic && has_extra_flags {
        if bytes.len() < 14 {
            return Err(FieldglassError::Parse(format!(
                "BDS complex extended header requires 14 bytes, got {}",
                bytes.len()
            )));
        }
        Some(ComplexExtendedHeader {
            n1: u16::from_be_bytes([bytes[11], bytes[12]]),
            extended_flag: bytes[13],
        })
    } else {
        None
    };

    Ok(BdsHeader {
        section_len,
        is_spherical_harmonic,
        is_complex_packing,
        is_integer_data: flag & 0x20 != 0,
        has_extra_flags,
        unused_trailing_bits: flag & 0x0F,
        binary_scale_factor: sign_magnitude_i16(u16::from_be_bytes([bytes[4], bytes[5]])),
        reference_value: ibm_float_to_f64(u32::from_be_bytes([
            bytes[6], bytes[7], bytes[8], bytes[9],
        ])),
        bits_per_value: bytes[10],
        complex_extended,
    })
}

/// Decode a BDS into floating-point values.
///
/// `bds` is the full Binary Data Section starting at its length octets;
/// `header` is the parsed header for `bds`; `decimal_scale` is the PDS
/// `decimal_scale_factor` (D); `bitmap` is the BMS bitmap if one was
/// present; `expected_count` is the total number of grid points (from the
/// GDS); `cols` is the GDS column count (used by complex/second-order
/// decoders to undo boustrophedonic row-scan — simple packing ignores it).
///
/// Returns one `Option<f64>` per grid point: `None` for points masked out
/// by the bitmap, `Some(value)` otherwise. The actual decoding is
/// delegated to a [`crate::packing::Grib1Packing`] implementation chosen
/// by the BDS flag bits.
pub fn decode_values(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    cols: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    crate::packing::decoder_for(header).decode(
        bds,
        header,
        decimal_scale,
        bitmap,
        expected_count,
        cols,
    )
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

    /// A real `grid_second_order_row_by_row` BDS (240×121, no bit-map) that
    /// decodes correctly on its own — reused here to prove that *adding* a
    /// masking bit-map is rejected rather than silently misdecoded.
    const ROW_BY_ROW: &[u8] =
        include_bytes!("../tests/fixtures/hand_second_order_row_by_row.grib1");

    #[test]
    fn second_order_packing_with_masking_bitmap_is_rejected() {
        use crate::reader::Grib1Reader;
        let reader = Grib1Reader::from_bytes(ROW_BY_ROW.to_vec()).expect("fixture parses");
        let (s, e) = reader.messages[0].bds_range;
        let bds = &ROW_BY_ROW[s..e];
        let header = parse_bds_header(bds).expect("BDS header parses");
        let (ni, nj) = (240usize, 121usize);
        let expected = ni * nj;

        // Baseline: with no bit-map this exact BDS decodes the full grid.
        assert!(
            decode_values(bds, &header, 0, None, expected, ni).is_ok(),
            "row_by_row BDS should decode without a bit-map"
        );

        // Inject a BMS bit-map that masks the final point. Pre-fix, the
        // row_by_row decoder still read `cols` residuals per row and produced a
        // full-length, value-shifted result (silent misdecode). It must now be
        // rejected with a clear error instead.
        let mut bitmap = vec![true; expected];
        *bitmap.last_mut().unwrap() = false;
        let err = decode_values(bds, &header, 0, Some(&bitmap), expected, ni)
            .expect_err("second-order packing + masking bit-map must be rejected");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("bit-map"), "unexpected message: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

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
            complex_extended: None,
        };
        let bds = vec![0u8; BDS_DATA_OFFSET];
        let out = decode_values(&bds, &header, 0, None, 4, 0).unwrap();
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
        let out = decode_values(&bds, &header, 0, None, 4, 0).unwrap();
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
        let out = decode_values(&bds, &header, 0, Some(&bitmap), 4, 0).unwrap();
        assert_eq!(out, vec![Some(7.0), None, Some(9.0), None]);
    }

    #[test]
    fn rejects_complex_packing() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        bds[3] = 0x40; // complex packing flag
        let header = parse_bds_header(&bds).unwrap();
        assert!(matches!(
            decode_values(&bds, &header, 0, None, 1, 0).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
    }

    #[test]
    fn rejects_spherical_harmonic_packing() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        bds[3] = 0x80; // spherical-harmonic flag
        let header = parse_bds_header(&bds).unwrap();
        let err = decode_values(&bds, &header, 0, None, 1, 0).unwrap_err();
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("spherical-harmonic"), "msg = {msg:?}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }
}
