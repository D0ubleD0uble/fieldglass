//! GRIB2 Data Representation Section (§5).
//!
//! Implements simple packing (template 5.0) — the GRIB1 `grid_simple`
//! analogue — IEEE floating-point packing (template 5.4, the GRIB2
//! counterpart to GRIB1 `grid_ieee`), complex packing (template 5.2, the
//! analogue of GRIB1 second-order packing), and complex packing plus
//! spatial differencing (template 5.3, the analogue of the GRIB1 SPD
//! orders). Other packing templates (JPEG 2000 5.40, PNG 5.41, CCSDS 5.42)
//! parse as [`DataRepresentationTemplate::Unsupported`] so message
//! enumeration still works.
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 5 layout + Templates 5.0 / 5.2 / 5.3 / 5.4.

use crate::section::{SectionHeader, parse_section_header};
use fieldglass_core::{FieldglassError, bits::sign_magnitude_i16};

/// Section number for the Data Representation Section.
pub const DRS_SECTION_NUMBER: u8 = 5;

/// Minimum byte length of a DRS — header (5) + num_data_points (4) +
/// template number (2). Real templates push this much higher; this is the
/// "can we read the template number safely" floor.
const DRS_MIN_LEN: usize = 11;

/// Template 5.0 payload length — octets 12..=21 of the section, 10 bytes.
const TEMPLATE_5_0_PAYLOAD_LEN: usize = 10;

/// Template 5.2 payload length — octets 12..=47 of the section, 36 bytes.
const TEMPLATE_5_2_PAYLOAD_LEN: usize = 36;

/// Template 5.3 payload length — the 36-byte template-5.2 block plus the two
/// spatial-differencing descriptor octets (48 + 49), 38 bytes total.
const TEMPLATE_5_3_PAYLOAD_LEN: usize = 38;

/// Template 5.4 payload length — a single octet (12), the precision code.
const TEMPLATE_5_4_PAYLOAD_LEN: usize = 1;

/// Template 5.0 — simple grid-point packing.
///
/// The unpacked value at each grid point is
/// `R + X · 2^E · 10^-D`, where `X` is the [`bits_per_value`]-wide unsigned
/// integer read from §7. `bits_per_value == 0` is the constant-field special
/// case: every present point equals `R · 10^-D`.
///
/// [`bits_per_value`]: SimplePackingTemplate::bits_per_value
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimplePackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15 of the section).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20).
    pub bits_per_value: u8,
    /// Type of original field values (octet 21) — WMO Code Table 5.1,
    /// `0` = floating point, `1` = integer.
    pub original_field_type: u8,
}

/// Template 5.4 — grid-point IEEE 754 floating-point packing.
///
/// Each grid point stores its value verbatim as a big-endian IEEE float;
/// there is no reference / binary-scale / decimal-scale transform. The only
/// payload field is the precision (WMO Code Table 5.7): `1` = 32-bit, `2` =
/// 64-bit, `3` = 128-bit. Mirrors GRIB1 `grid_ieee`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IeeePackingTemplate {
    /// Precision code (octet 12) — WMO Code Table 5.7.
    pub precision: u8,
}

/// Template 5.2 — complex grid-point packing.
///
/// The field is split into `num_groups` groups of consecutive points. Each
/// group carries its own reference value (its minimum), bit width, and
/// length; §7 then stores, as one continuous MSB-first bitstream, the group
/// references, the group widths, the group lengths, and finally the packed
/// per-point offsets. The unpacked value at a point in group `g` is
/// `R + (group_ref[g] + X) · 2^E · 10^-D`, where `X` is the point's
/// [`bits_per_value`]-style group-width-wide offset — the same global
/// `R`/`E`/`D` transform as simple packing, applied to the per-group
/// scaled integer. This mirrors GRIB1 second-order packing.
///
/// All template fields are parsed so the metadata view is complete; the §7
/// decoder (see `ds.rs`) currently handles the common envelope (general
/// group splitting, no inline missing-value management).
///
/// [`bits_per_value`]: ComplexPackingTemplate::bits_per_value
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each group reference value (octet 20).
    pub bits_per_value: u8,
    /// Type of original field values (octet 21) — WMO Code Table 5.1.
    pub original_field_type: u8,
    /// Group splitting method used (octet 22) — WMO Code Table 5.4
    /// (`0` = row by row, `1` = general group splitting).
    pub group_splitting_method: u8,
    /// Missing value management used (octet 23) — WMO Code Table 5.5
    /// (`0` = none, `1` = primary, `2` = primary + secondary).
    pub missing_value_management: u8,
    /// Primary missing value substitute (octets 24–27, raw bit pattern;
    /// interpretation follows [`original_field_type`]).
    ///
    /// [`original_field_type`]: ComplexPackingTemplate::original_field_type
    pub primary_missing_value: u32,
    /// Secondary missing value substitute (octets 28–31, raw bit pattern).
    pub secondary_missing_value: u32,
    /// NG — number of groups the field is split into (octets 32–35).
    pub num_groups: u32,
    /// Reference for the group widths (octet 36); each stored group width
    /// is this plus the value read at [`group_width_bits`].
    ///
    /// [`group_width_bits`]: ComplexPackingTemplate::group_width_bits
    pub group_width_reference: u8,
    /// Number of bits used for each (referenced) group width (octet 37).
    pub group_width_bits: u8,
    /// Reference for the group lengths (octets 38–41).
    pub group_length_reference: u32,
    /// Length increment for the group lengths (octet 42).
    pub group_length_increment: u8,
    /// True length of the last group (octets 43–46).
    pub group_length_last: u32,
    /// Number of bits used for each (scaled) group length (octet 47).
    pub group_length_bits: u8,
}

/// Template 5.3 — complex grid-point packing with spatial differencing.
///
/// Identical to template 5.2 (carried verbatim in [`complex`]) but the packed
/// integers are spatial *differences* of the original scaled field rather than
/// the scaled field itself, exactly like the GRIB1 second-order SPD orders.
/// Before grouping, the encoder takes 1st- or 2nd-order differences, then
/// subtracts the overall minimum difference so the grouped values are
/// non-negative. §7 therefore opens with the spatial-differencing *extra
/// descriptors* — the first `order` original values and the (signed) overall
/// minimum, each stored in [`extra_descriptor_octets`] octets — ahead of the
/// usual group-reference / width / length / data blocks. The §7 decoder (see
/// `ds.rs`) reads those descriptors, expands the groups to the differenced
/// integers, then inverts the differencing before applying the `R`/`E`/`D`
/// transform.
///
/// [`complex`]: ComplexSpatialDiffTemplate::complex
/// [`extra_descriptor_octets`]: ComplexSpatialDiffTemplate::extra_descriptor_octets
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexSpatialDiffTemplate {
    /// The underlying complex-packing parameters (octets 12–47), shared
    /// verbatim with template 5.2.
    pub complex: ComplexPackingTemplate,
    /// Order of spatial differencing (octet 48) — WMO Code Table 5.6
    /// (`1` = first-order, `2` = second-order).
    pub spatial_diff_order: u8,
    /// Number of octets used in §7 for each spatial-differencing extra
    /// descriptor — the first original value(s) and the overall minimum of
    /// the differences (octet 49).
    pub extra_descriptor_octets: u8,
}

/// Decoded template payload. Templates outside the supported set surface as
/// [`DataRepresentationTemplate::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataRepresentationTemplate {
    Simple(SimplePackingTemplate),
    Complex(ComplexPackingTemplate),
    ComplexSpatialDiff(ComplexSpatialDiffTemplate),
    Ieee(IeeePackingTemplate),
    Unsupported(u16),
}

/// Parsed contents of the Data Representation Section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataRepresentationSection {
    pub section_length: u32,
    /// Number of data values for which the §7 payload carries packed
    /// values. Equals the GDS grid-point count unless a §6 bitmap drops
    /// some points, in which case it equals the count of present points.
    pub num_data_points: u32,
    pub template_number: u16,
    pub template: DataRepresentationTemplate,
}

impl DataRepresentationSection {
    /// Short human-readable name of the template (`"simple"`,
    /// `"unsupported(5.N)"`).
    pub fn template_name(&self) -> String {
        match &self.template {
            DataRepresentationTemplate::Simple(_) => "simple".to_string(),
            DataRepresentationTemplate::Complex(_) => "complex".to_string(),
            DataRepresentationTemplate::ComplexSpatialDiff(_) => "complex_spatial_diff".to_string(),
            DataRepresentationTemplate::Ieee(_) => "ieee".to_string(),
            DataRepresentationTemplate::Unsupported(n) => format!("unsupported(5.{n})"),
        }
    }

    /// Borrow the simple-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn simple(&self) -> Option<&SimplePackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Simple(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the complex-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn complex(&self) -> Option<&ComplexPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Complex(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the complex + spatial-differencing template if that's what the
    /// section carries. Other templates return `None`.
    pub fn complex_spatial_diff(&self) -> Option<&ComplexSpatialDiffTemplate> {
        match &self.template {
            DataRepresentationTemplate::ComplexSpatialDiff(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the IEEE-packing template if that's what the section carries.
    /// Other templates return `None`.
    pub fn ieee(&self) -> Option<&IeeePackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Ieee(t) => Some(t),
            _ => None,
        }
    }
}

/// Parse the Data Representation Section starting at `bytes[0]`.
pub fn parse_data_representation(
    bytes: &[u8],
) -> Result<DataRepresentationSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_data_representation_with_header(bytes, header)
}

/// Variant for callers that have already read the section header.
pub fn parse_data_representation_with_header(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<DataRepresentationSection, FieldglassError> {
    if header.number != DRS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected DRS (section {DRS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if len < DRS_MIN_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS section length {len} is below the {DRS_MIN_LEN}-byte minimum"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "DRS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }

    let num_data_points = u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
    let template_number = u16::from_be_bytes([bytes[9], bytes[10]]);

    // Template payload starts at section octet 12 (= byte index 11).
    let payload = &bytes[11..len];
    let template = match template_number {
        0 => DataRepresentationTemplate::Simple(parse_template_5_0(payload)?),
        2 => DataRepresentationTemplate::Complex(parse_template_5_2(payload)?),
        3 => DataRepresentationTemplate::ComplexSpatialDiff(parse_template_5_3(payload)?),
        4 => DataRepresentationTemplate::Ieee(parse_template_5_4(payload)?),
        other => DataRepresentationTemplate::Unsupported(other),
    };

    Ok(DataRepresentationSection {
        section_length: header.length,
        num_data_points,
        template_number,
        template,
    })
}

fn parse_template_5_0(payload: &[u8]) -> Result<SimplePackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_0_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.0 needs {TEMPLATE_5_0_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    Ok(SimplePackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        original_field_type: payload[9],
    })
}

fn parse_template_5_2(payload: &[u8]) -> Result<ComplexPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_2_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.2 needs {TEMPLATE_5_2_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    Ok(ComplexPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        original_field_type: payload[9],
        group_splitting_method: payload[10],
        missing_value_management: payload[11],
        primary_missing_value: u32::from_be_bytes([
            payload[12],
            payload[13],
            payload[14],
            payload[15],
        ]),
        secondary_missing_value: u32::from_be_bytes([
            payload[16],
            payload[17],
            payload[18],
            payload[19],
        ]),
        num_groups: u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]),
        group_width_reference: payload[24],
        group_width_bits: payload[25],
        group_length_reference: u32::from_be_bytes([
            payload[26],
            payload[27],
            payload[28],
            payload[29],
        ]),
        group_length_increment: payload[30],
        group_length_last: u32::from_be_bytes([payload[31], payload[32], payload[33], payload[34]]),
        group_length_bits: payload[35],
    })
}

fn parse_template_5_3(payload: &[u8]) -> Result<ComplexSpatialDiffTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_3_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.3 needs {TEMPLATE_5_3_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–47 are a verbatim template-5.2 block; 48–49 (payload[36..=37])
    // carry the spatial-differencing order and extra-descriptor octet width.
    let complex = parse_template_5_2(payload)?;
    Ok(ComplexSpatialDiffTemplate {
        complex,
        spatial_diff_order: payload[36],
        extra_descriptor_octets: payload[37],
    })
}

fn parse_template_5_4(payload: &[u8]) -> Result<IeeePackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_4_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.4 needs {TEMPLATE_5_4_PAYLOAD_LEN} byte of payload, got {}",
            payload.len()
        )));
    }
    Ok(IeeePackingTemplate {
        precision: payload[0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal §5 with template 5.0 — 21-byte section, reference
    /// value `300.5`, E = 0, D = 1, 16 bits/value, original field type 0.
    fn build_drs_5_0() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 21;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&1024u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&0u16.to_be_bytes()); // template 5.0
        buf.extend_from_slice(&300.5_f32.to_be_bytes()); // R
        buf.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        buf.extend_from_slice(&1u16.to_be_bytes()); // D = 1
        buf.push(16); // bits per value
        buf.push(0); // original field type
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_0_round_trips_synthesized_payload() {
        let bytes = build_drs_5_0();
        let drs = parse_data_representation(&bytes).expect("parse 5.0");
        assert_eq!(drs.template_number, 0);
        assert_eq!(drs.num_data_points, 1024);

        let t = drs.simple().expect("5.0 has simple template");
        assert!((t.reference_value - 300.5).abs() < 1e-6);
        assert_eq!(t.binary_scale_factor, 0);
        assert_eq!(t.decimal_scale_factor, 1);
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.original_field_type, 0);
        assert_eq!(drs.template_name(), "simple");
    }

    #[test]
    fn sign_magnitude_scale_factors_decode_negatives() {
        let mut bytes = build_drs_5_0();
        // E lives at octets 16–17 = bytes 15–16; set sign-magnitude −3.
        bytes[15..17].copy_from_slice(&(0x8000u16 | 3).to_be_bytes());
        // D lives at octets 18–19 = bytes 17–18; set sign-magnitude −2.
        bytes[17..19].copy_from_slice(&(0x8000u16 | 2).to_be_bytes());
        let drs = parse_data_representation(&bytes).expect("parse");
        let t = drs.simple().unwrap();
        assert_eq!(t.binary_scale_factor, -3);
        assert_eq!(t.decimal_scale_factor, -2);
    }

    #[test]
    fn rejects_short_section() {
        let bytes = [0u8; 10];
        assert!(parse_data_representation(&bytes).is_err());
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_drs_5_0();
        bytes[4] = 4; // claim §4
        assert!(parse_data_representation(&bytes).is_err());
    }

    #[test]
    fn rejects_length_below_minimum() {
        let mut bytes = build_drs_5_0();
        bytes[0..4].copy_from_slice(&10u32.to_be_bytes());
        assert!(parse_data_representation(&bytes).is_err());
    }

    #[test]
    fn rejects_length_exceeding_buffer() {
        let mut bytes = build_drs_5_0();
        bytes[0..4].copy_from_slice(&100u32.to_be_bytes());
        assert!(parse_data_representation(&bytes).is_err());
    }

    #[test]
    fn rejects_5_0_when_payload_truncated() {
        // Declare a section length of 11 (just past the template number)
        // so the template-5.0 payload check fires.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&11u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // template 5.0
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.0 needs"),
            "error names template-5.0 shortfall, got: {err}",
        );
    }

    /// Build a minimal §5 with template 5.4 — 12-byte section, the given
    /// precision code in octet 12.
    fn build_drs_5_4(precision: u8) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 12;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&4u16.to_be_bytes()); // template 5.4
        buf.push(precision);
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_4_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_4(2)).expect("parse 5.4");
        assert_eq!(drs.template_number, 4);
        assert_eq!(drs.num_data_points, 496);
        assert_eq!(drs.template_name(), "ieee");
        let t = drs.ieee().expect("5.4 has ieee template");
        assert_eq!(t.precision, 2);
        assert!(drs.simple().is_none());
    }

    #[test]
    fn template_5_4_preserves_each_precision_code() {
        for p in [1u8, 2, 3] {
            let drs = parse_data_representation(&build_drs_5_4(p)).expect("parse");
            assert_eq!(drs.ieee().unwrap().precision, p);
        }
    }

    #[test]
    fn rejects_5_4_when_payload_truncated() {
        // Declare length 11 (just past the template number) so the 5.4
        // payload check fires on the missing precision octet.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&11u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes()); // template 5.4
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.4 needs"),
            "error names template-5.4 shortfall, got: {err}",
        );
    }

    /// Build a minimal §5 with template 5.2 — 47-byte section. Field values
    /// are arbitrary but distinct so the parser's octet mapping is pinned.
    fn build_drs_5_2() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 47;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&50u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&2u16.to_be_bytes()); // template 5.2
        buf.extend_from_slice(&12.5_f32.to_be_bytes()); // R (octets 12–15)
        buf.extend_from_slice(&1u16.to_be_bytes()); // E = 1 (16–17)
        buf.extend_from_slice(&2u16.to_be_bytes()); // D = 2 (18–19)
        buf.push(8); // bits per group reference value (20)
        buf.push(0); // original field type (21)
        buf.push(1); // group splitting method = general (22)
        buf.push(0); // missing value management = none (23)
        buf.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes()); // primary missing (24–27)
        buf.extend_from_slice(&0x0BAD_F00Du32.to_be_bytes()); // secondary missing (28–31)
        buf.extend_from_slice(&7u32.to_be_bytes()); // NG = 7 (32–35)
        buf.push(3); // group width reference (36)
        buf.push(4); // bits for group widths (37)
        buf.extend_from_slice(&5u32.to_be_bytes()); // group length reference (38–41)
        buf.push(1); // group length increment (42)
        buf.extend_from_slice(&9u32.to_be_bytes()); // true length of last group (43–46)
        buf.push(6); // bits for group lengths (47)
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_2_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_2()).expect("parse 5.2");
        assert_eq!(drs.template_number, 2);
        assert_eq!(drs.num_data_points, 50);
        assert_eq!(drs.template_name(), "complex");
        assert!(drs.simple().is_none());

        let t = drs.complex().expect("5.2 has complex template");
        assert!((t.reference_value - 12.5).abs() < 1e-6);
        assert_eq!(t.binary_scale_factor, 1);
        assert_eq!(t.decimal_scale_factor, 2);
        assert_eq!(t.bits_per_value, 8);
        assert_eq!(t.original_field_type, 0);
        assert_eq!(t.group_splitting_method, 1);
        assert_eq!(t.missing_value_management, 0);
        assert_eq!(t.primary_missing_value, 0xDEAD_BEEF);
        assert_eq!(t.secondary_missing_value, 0x0BAD_F00D);
        assert_eq!(t.num_groups, 7);
        assert_eq!(t.group_width_reference, 3);
        assert_eq!(t.group_width_bits, 4);
        assert_eq!(t.group_length_reference, 5);
        assert_eq!(t.group_length_increment, 1);
        assert_eq!(t.group_length_last, 9);
        assert_eq!(t.group_length_bits, 6);
    }

    #[test]
    fn template_5_2_decodes_negative_scale_factors() {
        let mut bytes = build_drs_5_2();
        // E at octets 16–17 = bytes 15–16; sign-magnitude −1.
        bytes[15..17].copy_from_slice(&(0x8000u16 | 1).to_be_bytes());
        // D at octets 18–19 = bytes 17–18; sign-magnitude −2.
        bytes[17..19].copy_from_slice(&(0x8000u16 | 2).to_be_bytes());
        let t = *parse_data_representation(&bytes)
            .unwrap()
            .complex()
            .unwrap();
        assert_eq!(t.binary_scale_factor, -1);
        assert_eq!(t.decimal_scale_factor, -2);
    }

    #[test]
    fn rejects_5_2_when_payload_truncated() {
        // Declare length 11 (just past the template number) so the 5.2
        // payload check fires on the missing 36-byte block.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&11u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&2u16.to_be_bytes()); // template 5.2
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.2 needs"),
            "error names template-5.2 shortfall, got: {err}",
        );
    }

    /// Build a minimal §5 with template 5.3 — the 5.2 block plus the order
    /// and extra-descriptor octets. Reuses [`build_drs_5_2`]'s field values so
    /// the embedded complex template is pinned to the same expectations.
    fn build_drs_5_3(order: u8, extra_octets: u8) -> Vec<u8> {
        let mut buf = build_drs_5_2();
        // Re-stamp the section length (49) and template number (3); append the
        // two spatial-differencing octets.
        buf[0..4].copy_from_slice(&49u32.to_be_bytes());
        buf[9..11].copy_from_slice(&3u16.to_be_bytes());
        buf.push(order); // order of spatial differencing (octet 48)
        buf.push(extra_octets); // extra-descriptor octets (octet 49)
        assert_eq!(buf.len() as u32, 49);
        buf
    }

    #[test]
    fn template_5_3_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_3(2, 2)).expect("parse 5.3");
        assert_eq!(drs.template_number, 3);
        assert_eq!(drs.num_data_points, 50);
        assert_eq!(drs.template_name(), "complex_spatial_diff");
        assert!(drs.complex().is_none(), "5.3 is not a bare 5.2");

        let t = drs
            .complex_spatial_diff()
            .expect("5.3 has spatial-diff template");
        assert_eq!(t.spatial_diff_order, 2);
        assert_eq!(t.extra_descriptor_octets, 2);
        // The embedded 5.2 block parses exactly as the standalone 5.2 fixture.
        assert!((t.complex.reference_value - 12.5).abs() < 1e-6);
        assert_eq!(t.complex.bits_per_value, 8);
        assert_eq!(t.complex.num_groups, 7);
        assert_eq!(t.complex.group_length_last, 9);
    }

    #[test]
    fn template_5_3_preserves_order_and_octet_width() {
        for (order, octets) in [(1u8, 3u8), (2, 1)] {
            let t = *parse_data_representation(&build_drs_5_3(order, octets))
                .unwrap()
                .complex_spatial_diff()
                .unwrap();
            assert_eq!(t.spatial_diff_order, order);
            assert_eq!(t.extra_descriptor_octets, octets);
        }
    }

    #[test]
    fn rejects_5_3_when_payload_truncated() {
        // Declare length 11 (just past the template number) so the 5.3
        // payload check fires on the missing 38-byte block.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&11u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&3u16.to_be_bytes()); // template 5.3
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.3 needs"),
            "error names template-5.3 shortfall, got: {err}",
        );
    }

    #[test]
    fn unsupported_template_round_trips_with_label() {
        let mut bytes = build_drs_5_0();
        // Template number lives at section octets 10–11 = bytes 9–10.
        bytes[9..11].copy_from_slice(&40u16.to_be_bytes()); // JPEG 2000
        let drs = parse_data_representation(&bytes).expect("parse");
        assert!(matches!(
            drs.template,
            DataRepresentationTemplate::Unsupported(40)
        ));
        assert_eq!(drs.template_name(), "unsupported(5.40)");
        assert!(drs.simple().is_none());
    }
}
