use fieldglass_core::bits::{ibm_float_to_f64, sign_magnitude_i16};
use fieldglass_core::{FieldglassError, StoredRuns};

/// Header of the Binary Data Section. Does not own the packed data.
#[derive(Debug)]
pub struct BdsHeader {
    /// Length of the section in bytes, from its own 3-octet length prefix.
    pub section_len: u32,
    /// True = spherical-harmonic coefficients (unsupported here).
    pub is_spherical_harmonic: bool,
    /// True = complex / second-order packing (unsupported here).
    pub is_complex_packing: bool,
    /// True = integer values; false = floating point.
    pub is_integer_data: bool,
    /// Octet 4 bit 4, WMO's "additional flags present". It selects `grid_ieee`
    /// or `grid_simple_matrix` among the *simple* packings; it does **not**
    /// gate the complex-packing extended header, which follows every
    /// non-spherical complex section whatever this says (#605).
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
    /// Present when `is_spherical_harmonic`. Holds the spectral follow-on
    /// header (the field mean, or the sub-truncation + Laplacian exponent).
    pub spherical_extended: Option<SphericalExtendedHeader>,
    /// Present for every non-spherical complex-packed section. Holds N1 + the
    /// extended flag byte (octets 12-14) so [`crate::packing`] decoders can
    /// branch on the precise variant without re-parsing the section header.
    /// Not gated on `has_extra_flags` — see [`parse_bds_header`].
    pub complex_extended: Option<ComplexExtendedHeader>,
}

/// The 3-octet header that follows the standard 11-octet BDS header on every
/// non-spherical complex-packed section. See WMO Manual on Codes Vol I.2,
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

impl BdsHeader {
    /// eccodes-style `packingType` identifier for this BDS, covering every
    /// variant [`crate::packing::decoder_for`] dispatches on. Mirrors that
    /// dispatch order exactly, so the label names the decoder that will run.
    /// Surfaced as metadata (the friendly form ships in the message table) and
    /// kept in step with the README GRIB1 packing-modes table.
    pub fn packing_type_label(&self) -> &'static str {
        if self.is_spherical_harmonic {
            // The complex bit selects the variant, as in eccodes'
            // `grib1/section.4.def` concept dispatch.
            return if self.is_complex_packing {
                "spectral_complex"
            } else {
                "spectral_simple"
            };
        }
        if self.is_complex_packing {
            // `parse_bds_header` fills `complex_extended` for every section it
            // accepts here, so the fallback is only for a header built by hand.
            return match self.complex_extended {
                Some(ext) => ext.packing_type_label(),
                None => "grid_second_order",
            };
        }
        if self.has_extra_flags {
            if self.is_integer_data {
                return "grid_ieee";
            }
            return "grid_simple_matrix";
        }
        "grid_simple"
    }
}

/// Offset (within the BDS) at which packed data values begin.
pub const BDS_DATA_OFFSET: usize = 11;

/// Shortest BDS that can hold the complex-packing extended header: the standard
/// 11 octets plus N1 (12-13) and the extended flag (14).
pub const COMPLEX_EXTENDED_LEN: usize = 14;

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

    // Spherical-harmonic packing has its own follow-on layout. `spectral_simple`
    // (no complex bit) carries the (0, 0) coefficient's real part as a bare IBM
    // float; `spectral_complex` carries the sub-truncation and the Laplacian
    // exponent instead. Both are read here so the decoder never re-parses.
    let spherical_extended = if is_spherical_harmonic {
        Some(parse_spherical_extended(bytes, is_complex_packing)?)
    } else {
        None
    };

    // Octets 12-14 are present (and meaningful) for every non-spherical
    // complex-packed section. Spherical-harmonic packing sets the same complex
    // bit but lays its octets out differently (see above), so it is excluded
    // here.
    //
    // Deliberately *not* also gated on `has_extra_flags` (octet 4 bit 4).
    // eccodes' `grib1/section.4.def` reads this block under
    // `if (complexPacking && sphericalHarmonics==0)` alone, and none of its
    // `grid_second_order*` concepts constrain `additionalFlagPresent` — so a
    // second-order section with the bit clear is an ordinary one to eccodes.
    // Its own encoder writes exactly that (`grib_set -r -s
    // packingType=grid_second_order` on a GRIB1 message leaves octet 4 = 0x47,
    // complex set and bit 4 clear) and then decodes it back, while requiring
    // the bit here refused the message with "complex packing without
    // extra-flags octet". Found while building the reduced-grid fixture for
    // #605, which is such a message.
    //
    // The cost is that a *truncated* complex section — one that declares the
    // packing in fewer than 14 octets — is now a parse error rather than a
    // header whose flags were never read. `packing_label` and `message_kind`
    // then report `None` / `Unsupported` for it instead of naming a packing on
    // the strength of octets that are not there, so such a message drops out of
    // the message table. That is the honest answer for a section this cannot
    // read, and it is unreachable for any message a producer wrote.
    let complex_extended = if is_complex_packing && !is_spherical_harmonic {
        if bytes.len() < COMPLEX_EXTENDED_LEN {
            return Err(FieldglassError::Parse(format!(
                "BDS complex extended header requires {COMPLEX_EXTENDED_LEN} bytes, got {}",
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
        spherical_extended,
    })
}

/// Follow-on header of a spherical-harmonic BDS, after `bitsPerValue` (octet
/// 11). The two spectral packings lay these octets out differently, so the
/// variant is decided by the complex-packing flag — exactly as eccodes'
/// `grib1/data.spectral_{simple,complex}.def` do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SphericalExtendedHeader {
    /// `spectral_simple`: octets 12-15 hold the real part of the `(0, 0)`
    /// coefficient — the field mean — as a bare IBM float, lifted out of the
    /// packed stream so its magnitude doesn't swamp the quantisation of every
    /// other coefficient. Data begins at octet 16.
    Simple {
        /// The `(0, 0)` coefficient's real part — the field mean.
        real_part: f64,
    },
    /// `spectral_complex`: octets 12-13 `N`, 14-15 `P`, 16 `JS`, 17 `KS`,
    /// 18 `MS`. Data begins at octet 19.
    Complex {
        /// Laplacian exponent, as stored (thousandths). The operator applied to
        /// each coefficient is `(n·(n+1))^(P/1000)`.
        p: i16,
        /// Sub-truncation `JS` / `KS` / `MS`. eccodes asserts all three are
        /// equal (`DataComplexPacking.cc`), so a message that disagrees is
        /// malformed rather than a variant to support.
        js: u8,
        /// Sub-truncation `KS`; see `js`.
        ks: u8,
        /// Sub-truncation `MS`; see `js`.
        ms: u8,
    },
}

/// Octet at which a spectral BDS's data begins, counted from the start of the
/// section. `spectral_simple` ends after the 4-byte real part; `spectral_complex`
/// ends after `MS`.
pub const SPECTRAL_SIMPLE_DATA_OFFSET: usize = 15;
/// Octet at which a `spectral_complex` BDS's data begins — after `MS`.
pub const SPECTRAL_COMPLEX_DATA_OFFSET: usize = 18;

fn parse_spherical_extended(
    bytes: &[u8],
    is_complex_packing: bool,
) -> Result<SphericalExtendedHeader, FieldglassError> {
    if is_complex_packing {
        if bytes.len() < SPECTRAL_COMPLEX_DATA_OFFSET {
            return Err(FieldglassError::Parse(format!(
                "spectral_complex BDS header requires {SPECTRAL_COMPLEX_DATA_OFFSET} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(SphericalExtendedHeader::Complex {
            // `N` (octets 12-13) is deliberately not read: eccodes writes it as
            // an offset from the start of the *message* rather than the section
            // and its own decoder ignores it, deriving the packed-data offset
            // from `KS` instead. We do the same rather than trust it.
            p: sign_magnitude_i16(u16::from_be_bytes([bytes[13], bytes[14]])),
            js: bytes[15],
            ks: bytes[16],
            ms: bytes[17],
        })
    } else {
        if bytes.len() < SPECTRAL_SIMPLE_DATA_OFFSET {
            return Err(FieldglassError::Parse(format!(
                "spectral_simple BDS header requires {SPECTRAL_SIMPLE_DATA_OFFSET} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(SphericalExtendedHeader::Simple {
            real_part: ibm_float_to_f64(u32::from_be_bytes([
                bytes[11], bytes[12], bytes[13], bytes[14],
            ])),
        })
    }
}

/// Decode a BDS into floating-point values.
///
/// `bds` is the full Binary Data Section starting at its length octets;
/// `header` is the parsed header for `bds`; `decimal_scale` is the PDS
/// `decimal_scale_factor` (D); `bitmap` is the BMS bitmap if one was
/// present; `expected_count` is the total number of grid points (from the
/// GDS); `runs` is the grid's stored run layout (used by complex/second-order
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
    runs: StoredRuns<'_>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    crate::packing::decoder_for(header).decode(
        bds,
        header,
        decimal_scale,
        bitmap,
        expected_count,
        runs,
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
            decode_values(bds, &header, 0, None, expected, StoredRuns::Uniform(ni)).is_ok(),
            "row_by_row BDS should decode without a bit-map"
        );

        // Inject a BMS bit-map that masks the final point. Pre-fix, the
        // row_by_row decoder still read `cols` residuals per row and produced a
        // full-length, value-shifted result (silent misdecode). It must now be
        // rejected with a clear error instead.
        let mut bitmap = vec![true; expected];
        *bitmap.last_mut().unwrap() = false;
        let err = decode_values(
            bds,
            &header,
            0,
            Some(&bitmap),
            expected,
            StoredRuns::Uniform(ni),
        )
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
            spherical_extended: None,
            complex_extended: None,
        };
        let bds = vec![0u8; BDS_DATA_OFFSET];
        let out = decode_values(&bds, &header, 0, None, 4, StoredRuns::Uniform(0)).unwrap();
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
        let out = decode_values(&bds, &header, 0, None, 4, StoredRuns::Uniform(0)).unwrap();
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
        let out =
            decode_values(&bds, &header, 0, Some(&bitmap), 4, StoredRuns::Uniform(0)).unwrap();
        assert_eq!(out, vec![Some(7.0), None, Some(9.0), None]);
    }

    /// A complex-packed section whose extended flags name no known variant is
    /// refused rather than guessed at. All-zero flags are that case: no
    /// secondary bitmap, constant second-order width and no general-extended
    /// bit is a combination eccodes' own `packingType` concept does not list.
    #[test]
    fn rejects_complex_packing() {
        let mut bds = vec![0u8; COMPLEX_EXTENDED_LEN];
        bds[0..3].copy_from_slice(&[0, 0, COMPLEX_EXTENDED_LEN as u8]);
        bds[3] = 0x40; // complex packing flag
        let header = parse_bds_header(&bds).unwrap();
        assert_eq!(header.packing_type_label(), "grid_second_order_unknown");
        assert!(matches!(
            decode_values(&bds, &header, 0, None, 1, StoredRuns::Uniform(0)).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
    }

    /// The extended-flag octet is read for every non-spherical complex section,
    /// not only when octet 4 bit 4 is set — eccodes' `grib1/section.4.def`
    /// gates that block on the complex bit alone, and its own encoder leaves
    /// bit 4 clear when it writes GRIB1 second-order packing (#605). A section
    /// too short to hold octets 12-14 is then malformed, and says so here
    /// rather than reporting a packing whose flags it never read.
    #[test]
    fn a_complex_section_carries_the_extended_flags_without_the_additional_flag_bit() {
        let mut bds = vec![0u8; COMPLEX_EXTENDED_LEN];
        bds[0..3].copy_from_slice(&[0, 0, COMPLEX_EXTENDED_LEN as u8]);
        bds[3] = 0x40; // complex, and *not* 0x10
        bds[13] = 0x1A; // the flags eccodes writes for grid_second_order
        let header = parse_bds_header(&bds).unwrap();
        assert!(!header.has_extra_flags);
        let ext = header.complex_extended.expect("extended flags are read");
        assert!(ext.general_extended_2ordr());
        assert!(!ext.boustrophedonic());
        assert_eq!(header.packing_type_label(), "grid_second_order");

        let mut short = vec![0u8; BDS_DATA_OFFSET];
        short[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        short[3] = 0x40;
        assert!(matches!(
            parse_bds_header(&short).unwrap_err(),
            FieldglassError::Parse(_)
        ));
    }

    #[test]
    fn spherical_harmonic_refuses_the_scalar_grid_path() {
        // Coefficients are not one scalar per grid point, so the scalar decoder
        // must refuse — and name the call that does decode them, rather than
        // leaving the caller to guess. (`decode_spectral_message` is the entry
        // point; see `packing::spherical`.)
        let mut bds = vec![0u8; SPECTRAL_SIMPLE_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, SPECTRAL_SIMPLE_DATA_OFFSET as u8]);
        bds[3] = 0x80; // spherical-harmonic flag, complex bit clear → simple
        let header = parse_bds_header(&bds).unwrap();
        assert!(header.is_spherical_harmonic);
        assert!(matches!(
            header.spherical_extended,
            Some(SphericalExtendedHeader::Simple { .. })
        ));
        assert_eq!(header.packing_type_label(), "spectral_simple");

        let err = decode_values(&bds, &header, 0, None, 1, StoredRuns::Uniform(0)).unwrap_err();
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("decode_spectral_message"), "msg = {msg:?}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn spectral_complex_header_reads_the_sub_truncation() {
        // Octets 12-13 N, 14-15 P, 16 JS, 17 KS, 18 MS. `N` is deliberately
        // ignored (eccodes writes it relative to the message and ignores it too).
        let mut bds = vec![0u8; SPECTRAL_COMPLEX_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, SPECTRAL_COMPLEX_DATA_OFFSET as u8]);
        bds[3] = 0xC0; // spherical-harmonic + complex
        bds[13..15].copy_from_slice(&2000u16.to_be_bytes()); // P = +2.000
        bds[15] = 20; // JS
        bds[16] = 20; // KS
        bds[17] = 20; // MS
        let header = parse_bds_header(&bds).unwrap();
        assert_eq!(header.packing_type_label(), "spectral_complex");
        assert_eq!(
            header.spherical_extended,
            Some(SphericalExtendedHeader::Complex {
                p: 2000,
                js: 20,
                ks: 20,
                ms: 20
            })
        );
    }
}
