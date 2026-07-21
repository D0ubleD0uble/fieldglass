//! GRIB2 Data Representation Section (§5).
//!
//! Implements simple packing (template 5.0) — the GRIB1 `grid_simple`
//! analogue — IEEE floating-point packing (template 5.4, the GRIB2
//! counterpart to GRIB1 `grid_ieee`), complex packing (template 5.2, the
//! analogue of GRIB1 second-order packing), complex packing plus
//! spatial differencing (template 5.3, the analogue of the GRIB1 SPD
//! orders), JPEG 2000 packing (template 5.40, whose §7 wraps the integer
//! grid in a JPEG 2000 codestream, decoded with the pure-Rust `rust-j2k`
//! crate), PNG packing (template 5.41, whose §7 wraps the integer grid in
//! a PNG image), CCSDS / AEC packing (template 5.42, whose §7 wraps the
//! integer grid in a libaec-compatible adaptive-entropy-coding stream),
//! run-length packing (template 5.200, whose §7 is a run-length-encoded stream
//! of quantised level indices resolved through a level → value table — JMA
//! radar and nowcast products), simple packing with logarithmic
//! pre-processing (template 5.61), spectral (spherical-harmonic) packing
//! (templates 5.50 simple and 5.51 complex, whose §7 decodes to coefficients),
//! and second-order (general-extended) packing (templates 5.50001 and 5.50002,
//! the GRIB1 `grid_second_order` codec carried into GRIB2). The pre-standard
//! local templates 5.40000 (JPEG 2000) and 5.40010 (PNG) alias onto the
//! 5.40 / 5.41 codecs. Templates outside this set parse as
//! [`DataRepresentationTemplate::Unsupported`] so message enumeration still
//! works.
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 5 layout + Templates 5.0 / 5.2 / 5.3 / 5.4 / 5.40 / 5.41 / 5.42 /
//! 5.50 / 5.51 / 5.61 / 5.200 (plus local 5.40000 / 5.40010 / 5.50001 / 5.50002).

use crate::section::{SectionHeader, parse_section_header};
use fieldglass_core::{
    FieldglassError,
    bits::{BitReader, bits_to_bytes, sign_magnitude_i16, sign_magnitude_to_i64},
};

/// Section number for the Data Representation Section.
pub const DRS_SECTION_NUMBER: u8 = 5;

/// Minimum byte length of a DRS — header (5) + num_data_points (4) +
/// template number (2). Real templates push this much higher; this is the
/// "can we read the template number safely" floor.
const DRS_MIN_LEN: usize = 11;

/// Template 5.0 payload length — octets 12..=21 of the section, 10 bytes.
const TEMPLATE_5_0_PAYLOAD_LEN: usize = 10;

/// Template 5.1 fixed payload length up to (but excluding) the coordinate
/// coefficient arrays — octets 12..=35, 24 bytes: the 9-byte common packing
/// block (R / E / D / bits, no type octet), then matrixBitmapsPresent (1),
/// numberOfCodedValues (4), NR (2), NC (2), the two coordinate-value-definition
/// octets and their NC1 / NC2 counts (4), and the two physical-significance
/// octets (2). The `coefsFirst[NC1]` / `coefsSecond[NC2]` IEEE-float arrays
/// follow.
const TEMPLATE_5_1_FIXED_PAYLOAD_LEN: usize = 24;

/// Template 5.2 payload length — octets 12..=47 of the section, 36 bytes.
const TEMPLATE_5_2_PAYLOAD_LEN: usize = 36;

/// Template 5.3 payload length — the 36-byte template-5.2 block plus the two
/// spatial-differencing descriptor octets (48 + 49), 38 bytes total.
const TEMPLATE_5_3_PAYLOAD_LEN: usize = 38;

/// Template 5.4 payload length — a single octet (12), the precision code.
const TEMPLATE_5_4_PAYLOAD_LEN: usize = 1;

/// Template 5.41 payload length — octets 12..=21, identical to template 5.0
/// (R / E / D / bits-per-value / original-field-type). The compressed grid
/// lives in §7 as a PNG image rather than a bit-packed stream.
const TEMPLATE_5_41_PAYLOAD_LEN: usize = 10;

/// Template 5.42 payload length — octets 12..=25, 14 bytes: the 10-byte
/// simple-packing block (R / E / D / bits-per-value / original-field-type)
/// followed by the three CCSDS / AEC descriptors — flags (octet 22), block
/// size (octet 23), and the 2-octet reference sample interval (24–25).
const TEMPLATE_5_42_PAYLOAD_LEN: usize = 14;

/// Template 5.40 payload length — octets 12..=23, 12 bytes: the 10-byte
/// simple-packing block (R / E / D / bits-per-value / original-field-type)
/// followed by the two JPEG 2000 descriptors — type-of-compression-used
/// (octet 22) and target-compression-ratio (octet 23).
const TEMPLATE_5_40_PAYLOAD_LEN: usize = 12;

/// Template 5.61 payload length — octets 12..=24, 13 bytes: the simple-packing
/// block *without* the type-of-original-values octet (R / E / D / bits, per
/// eccodes' shared `template.5.packing.def`, 9 bytes) followed by the 4-byte
/// IEEE `preProcessingParameter`. Note this is one octet shorter than the 5.0
/// block, which appends the type octet that 5.61 omits.
const TEMPLATE_5_61_PAYLOAD_LEN: usize = 13;

/// Template 5.50 payload length — octets 12..=24, 13 bytes: the simple-packing
/// block (R / E / D / bits, 9 bytes, no type-of-original-values octet) plus the
/// 4-byte IEEE `realPartOf00` (the (0,0) coefficient, stored out of band).
const TEMPLATE_5_50_PAYLOAD_LEN: usize = 13;

/// Template 5.51 payload length — octets 12..=35, 24 bytes: the 9-byte
/// simple-packing block (R / E / D / bits, no type octet), the 4-byte
/// Laplacian scaling factor, the three 2-byte sub-truncation parameters
/// (JS / KS / MS), the 4-byte sub-truncation value count (TS), and the 1-byte
/// unpacked-subset precision.
const TEMPLATE_5_51_PAYLOAD_LEN: usize = 24;

/// Template 5.53 payload length — octets 12..=35, 24 bytes: the 9-byte
/// simple-packing block (R / E / D / bits, no type octet), the
/// biFourierSubTruncationType (1) and biFourierPackingModeForAxes (1) octets,
/// the 4-byte sign-magnitude Laplacian scaling factor, the two 2-byte
/// sub-truncation resolutions (NS / MS), the 4-byte unpacked-subset value count
/// (TS), and the 1-byte unpacked-subset precision.
const TEMPLATE_5_53_PAYLOAD_LEN: usize = 24;

/// Template 5.50001 fixed payload length up to (but not including) the SPD
/// block — octets 12..=32, 21 bytes: the R / E / D block (8), then
/// bits-per-value (1), width-of-first-order (1), number-of-groups (4),
/// number-of-second-order-packed-values (4), width-of-widths (1),
/// width-of-lengths (1), and order-of-SPD (1). When order-of-SPD > 0 a
/// width-of-SPD octet (1) and the SPD seed block (variable) follow; both are
/// absent from the wire otherwise (eccodes gates them behind `if (orderOfSPD)`).
/// 5.50002 inserts a one-octet `secondOrderFlags` field before order-of-SPD, so
/// its fixed prefix is one byte longer.
const TEMPLATE_5_50001_FIXED_PAYLOAD_LEN: usize = 21;

/// Template 5.200 fixed payload length — octets 12..=17, 6 bytes:
/// bits-per-value (12), max-level-value (13–14), number-of-level-values
/// (15–16), and decimal-scale-factor (17). The variable-length level-value
/// list (2 bytes each) follows, so a full section is this plus
/// `2 · number_of_level_values`.
const TEMPLATE_5_200_FIXED_PAYLOAD_LEN: usize = 6;

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

/// Template 5.1 — matrix values at grid point, simple packing
/// (`grid_simple_matrix`). WMO provisions an `NR × NC` matrix at each grid
/// point (e.g. ECMWF 2-D wave spectra); it is flagged experimental.
///
/// §7 is plain simple packing (`data_g2simple_packing`). When
/// [`matrix_bitmaps_present`](Self::matrix_bitmaps_present) is `0`, §7 holds one
/// value per grid point and the field decodes exactly like template 5.0 — the
/// `NR`/`NC` and coordinate arrays are descriptive metadata. When it is `1`,
/// secondary bitmaps delimit a genuine per-point matrix (the eccodes-unsupported
/// variant); that reshape is not decoded here.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixSimplePackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20).
    pub bits_per_value: u8,
    /// Secondary (matrix) bitmaps present (octet 21). `0` = one value per grid
    /// point (flat, like 5.0); `1` = secondary bitmaps delimit a per-point
    /// matrix.
    pub matrix_bitmaps_present: u8,
    /// Number of data values encoded in §7 (octets 22–25).
    pub number_of_coded_values: u32,
    /// `NR` — first matrix dimension / rows (octets 26–27).
    pub nr: u16,
    /// `NC` — second matrix dimension / columns (octets 28–29).
    pub nc: u16,
    /// First-dimension coordinate-value definition (octet 30) — Code Table 5.2.
    pub first_dim_coordinate_definition: u8,
    /// Second-dimension coordinate-value definition (octet 32) — Code Table 5.2.
    pub second_dim_coordinate_definition: u8,
    /// First-dimension physical significance (octet 34) — Code Table 5.3.
    pub first_dim_physical_significance: u8,
    /// Second-dimension physical significance (octet 35) — Code Table 5.3.
    pub second_dim_physical_significance: u8,
    /// `NC1` coefficients defining the first-dimension coordinate function
    /// (IEEE 32-bit floats, following the fixed header).
    pub coefficients_first: Vec<f32>,
    /// `NC2` coefficients defining the second-dimension coordinate function.
    pub coefficients_second: Vec<f32>,
}

/// Template 5.41 — PNG packing.
///
/// The §5 payload is identical to simple packing (5.0): the same `R` / `E` /
/// `D` / [`bits_per_value`] / original-field-type fields. The difference is in
/// §7, which carries a complete PNG image whose pixels are the packed integers
/// `X` rather than a raw bit-packed stream. After the PNG is decoded back to
/// integers, the value transform is the simple-packing formula
/// `R + X · 2^E · 10^-D`, so `bits_per_value == 0` is the same constant-field
/// special case (every present point equals `R · 10^-D`, with no PNG present).
///
/// [`bits_per_value`]: PngPackingTemplate::bits_per_value
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PngPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15 of the section).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20). Drives the PNG
    /// sample depth eccodes chose: ≤8 → 8-bit grayscale, ≤16 → 16-bit
    /// grayscale, ≤24 → 8-bit RGB, ≤32 → 8-bit RGBA.
    pub bits_per_value: u8,
    /// Type of original field values (octet 21) — WMO Code Table 5.1,
    /// `0` = floating point, `1` = integer.
    pub original_field_type: u8,
}

/// Template 5.42 — CCSDS / AEC packing.
///
/// Like PNG (5.41), the first ten payload octets mirror simple packing (5.0):
/// `R` / `E` / `D` / [`bits_per_value`] / original-field-type, and the value
/// transform after decompression is the simple-packing formula
/// `R + X · 2^E · 10^-D`. The difference is §7, which carries a
/// CCSDS-121.0-B AEC (libaec-compatible) bitstream whose decoded samples are
/// the packed integers `X`. The three extra octets parameterise that codec.
///
/// `bits_per_value == 0` is the constant-field special case: §7 is empty and
/// every present point equals `R` (matching eccodes' `grid_ccsds` unpack,
/// which returns the reference value verbatim — see [`super::ds`]).
///
/// [`bits_per_value`]: CcsdsPackingTemplate::bits_per_value
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CcsdsPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15 of the section).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20) — the AEC sample
    /// width. `0` is the constant-field special case (no §7 stream).
    pub bits_per_value: u8,
    /// Type of original field values (octet 21) — WMO Code Table 5.1,
    /// `0` = floating point, `1` = integer.
    pub original_field_type: u8,
    /// CCSDS compression-options mask (octet 22). Bitfield matching libaec's
    /// `aec_stream.flags` (bit 0 signed, 1 three-byte, 2 MSB, 3 preprocess,
    /// 4 restricted, 5 pad-RSI).
    pub ccsds_flags: u8,
    /// CCSDS block size (octet 23) — the AEC coding block length `J`
    /// (typically 32).
    pub block_size: u8,
    /// Reference sample interval (octets 24–25) — how often the AEC stream
    /// restarts with a verbatim reference sample (typically 128).
    pub reference_sample_interval: u16,
}

/// Template 5.40 — JPEG 2000 packing.
///
/// Like PNG (5.41) and CCSDS (5.42), the first ten payload octets mirror simple
/// packing (5.0): `R` / `E` / `D` / [`bits_per_value`] / original-field-type,
/// and the value transform after decompression is the simple-packing formula
/// `R + X · 2^E · 10^-D`. The difference is §7, which carries a JPEG 2000
/// codestream (ISO/IEC 15444-1 Annex A, no JP2 boxes) whose decoded
/// single-component samples are the packed integers `X`. The two extra octets
/// describe the compression.
///
/// `bits_per_value == 0` is the constant-field special case: §7 is empty and
/// every present point equals the reference value verbatim (matching eccodes'
/// `grid_jpeg` unpack — see [`super::ds`]).
///
/// [`bits_per_value`]: Jpeg2000PackingTemplate::bits_per_value
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jpeg2000PackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15 of the section).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20) — the JPEG 2000
    /// component bit depth. `0` is the constant-field special case (no §7
    /// codestream).
    pub bits_per_value: u8,
    /// Type of original field values (octet 21) — WMO Code Table 5.1,
    /// `0` = floating point, `1` = integer.
    pub original_field_type: u8,
    /// Type of compression used (octet 22) — WMO Code Table 5.40,
    /// `0` = lossless, `1` = lossy. The wavelet transform (reversible 5/3 vs
    /// irreversible 9/7) is selected by the codestream's COD marker, so decode
    /// does not branch on this field; it is parsed for metadata completeness.
    pub type_of_compression_used: u8,
    /// Target compression ratio `M:1` (octet 23), meaningful only for lossy
    /// compression. `255` (missing) for lossless, as eccodes writes.
    pub target_compression_ratio: u8,
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
/// decoder (see `ds.rs`) handles both splitting methods and inline
/// missing-value management 0, 1, and 2.
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

/// Template 5.61 — simple packing with logarithmic pre-processing.
///
/// §7 carries a simple-packed integer stream exactly like template 5.0, but
/// the packed values are the *natural logarithm* of the field (shifted by
/// [`pre_processing_parameter`] `B` so the log's argument stays positive). The
/// decode is therefore simple unpacking followed by the inverse transform
/// `Y = exp(X) - B`, where `X = (R + packed · 2^E) · 10^-D` is the ordinary
/// simple-packing value. `B == 0` (the encoder's choice for an all-positive
/// field) reduces this to `Y = exp(X)`.
///
/// The template is experimental (WMO flags it "not validated … bilateral
/// tests only") and has no known operational producer; it is decoded for
/// census completeness. Unlike 5.0, the §5 block has no type-of-original-values
/// octet — the pre-processing parameter takes octets 21–24 directly after
/// `bits_per_value`.
///
/// [`pre_processing_parameter`]: LogPreprocessingPackingTemplate::pre_processing_parameter
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogPreprocessingPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed (log-domain) value (octet 20).
    pub bits_per_value: u8,
    /// Pre-processing parameter `B` (IEEE 32-bit float, octets 21–24): the
    /// shift added inside the logarithm at encode time and subtracted after
    /// the exponential at decode time. `0` when the source field was strictly
    /// positive.
    pub pre_processing_parameter: f32,
}

/// Template 5.50 — spectral (spherical-harmonic) simple packing.
///
/// The message carries spherical-harmonic coefficients, not gridded values. The
/// (0,0) coefficient's real part is stored out of band as [`real_part_of_00`];
/// every other coefficient part is simple-packed in §7 with the usual
/// `R` / `E` / `D` transform. The decode produces coefficients (see the
/// `spectral` module), so it does not ride [`decode_values`](crate::ds::decode_values);
/// like GRIB1 spectral messages, these do not render as a 2-D field until an
/// inverse spherical-harmonic transform lands.
///
/// [`real_part_of_00`]: SpectralSimplePackingTemplate::real_part_of_00
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectralSimplePackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed coefficient part (octet 20).
    pub bits_per_value: u8,
    /// Real part of the `(0, 0)` coefficient (IEEE 32-bit float, octets
    /// 21–24), stored out of band and copied through unscaled as the first
    /// decoded value.
    pub real_part_of_00: f32,
}

/// Template 5.51 — spectral (spherical-harmonic) complex packing.
///
/// Like 5.50 the message carries spherical-harmonic coefficients, but with a
/// two-part §7: coefficients up to the sub-truncation `(JS, KS, MS)` are stored
/// as raw IEEE floats, and the rest are simple-packed after division by a
/// Laplacian operator `(n·(n+1))^P` (with `P` the [`laplacian_scaling_factor`]
/// divided by 10⁶) — flattening the steep spectral fall-off before packing.
/// The decode produces coefficients (see the `spectral` module), so it does not
/// ride [`decode_values`](crate::ds::decode_values); like GRIB1 spectral
/// messages, these do not render as a 2-D field until an inverse
/// spherical-harmonic transform lands.
///
/// [`laplacian_scaling_factor`]: SpectralComplexPackingTemplate::laplacian_scaling_factor
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectralComplexPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed coefficient part (octet 20).
    pub bits_per_value: u8,
    /// Laplacian scaling factor `P·10⁶` (sign-magnitude `i32`, octets 21–24);
    /// the operator exponent `P` is this divided by 10⁶.
    pub laplacian_scaling_factor: i32,
    /// Sub-truncation `JS` — the pentagonal resolution of the unpacked block
    /// (octets 25–26).
    pub js: u16,
    /// Sub-truncation `KS` (octets 27–28).
    pub ks: u16,
    /// Sub-truncation `MS` (octets 29–30).
    pub ms: u16,
    /// `TS` — the count of values in the unpacked sub-truncation block
    /// (octets 31–34), `(KS+1)·(KS+2)` for a triangular sub-truncation.
    pub ts: u32,
    /// Precision of the unpacked subset (octet 35) — WMO Code Table 5.7
    /// (`1` = IEEE 32-bit).
    pub unpacked_subset_precision: u8,
}

/// Template 5.200 — grid-point run-length packing with level values.
///
/// Used by JMA for radar, rain-gauge analysis, and nowcast products. §7 is a
/// stream of [`bits_per_value`]-wide unsigned codes, MSB-first. A code in
/// `0..=max_level_value` is a *level index*; a level of `0` marks a missing
/// point and a level `v` in `1..=number_of_level_values` resolves to
/// `level_values[v - 1] · 10^-decimal_scale_factor`. A code greater than
/// `max_level_value` is a *run-length digit*: the run for the preceding level
/// is `1 + Σ (digit_i - max_level_value - 1) · range^i`, where
/// `range = 2^bits_per_value - 1 - max_level_value` and the digits appear
/// least-significant first. There is no `R`/`E` transform — only the decimal
/// scale is applied. This is a distinct §7 codec, not a variant of the simple
/// `R`/`E`/`D` families, so it holds its own level-value table rather than a
/// reference value.
///
/// [`bits_per_value`]: RunLengthPackingTemplate::bits_per_value
#[derive(Debug, Clone, PartialEq)]
pub struct RunLengthPackingTemplate {
    /// Number of bits used for each §7 code (octet 12) — `V` above.
    pub bits_per_value: u8,
    /// `MV` — the largest code value that denotes a level rather than a
    /// run-length digit (octets 13–14). Codes above it are run-length digits.
    pub max_level_value: u16,
    /// `MVL` — the number of entries in the level-value table (octets 15–16).
    pub number_of_level_values: u16,
    /// Decimal scale factor `D` (octet 17), stored sign-magnitude in a single
    /// octet: a raw byte above 127 is negative (`-(raw - 128)`). The value at
    /// a point of level `v` is `level_values[v - 1] · 10^-D`.
    pub decimal_scale_factor: i16,
    /// The level → scaled-value table (octets 18…, 2 bytes each). Entry
    /// `i` (zero-based) is the scaled integer for level index `i + 1`; level
    /// `0` is reserved for missing and has no entry.
    pub level_values: Vec<u16>,
}

/// Template 5.53 — bi-Fourier spectral (`bifourier_complex`) packing.
///
/// The limited-area (ACCORD / ALADIN / AROME) analogue of spherical-harmonic
/// complex packing (5.51): the message carries bi-Fourier spectral
/// coefficients — 4-tuples per `(i, j)` wavenumber pair — not gridded values.
/// §7 has two parts: the coefficients inside the unpacked sub-truncation
/// `(sub_i, sub_j)` are stored as raw IEEE 32-bit floats, and the rest are
/// simple-packed after division by a Laplacian operator `(i²+j²)^P` (with `P`
/// the [`laplacian_scaling_factor`] divided by 10⁶). The decode produces
/// coefficients (see the `bifourier` module); like the spherical-harmonic
/// templates it does not ride [`decode_values`](crate::ds::decode_values), and
/// there is no inverse transform to render the field yet.
///
/// [`laplacian_scaling_factor`]: BiFourierPackingTemplate::laplacian_scaling_factor
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiFourierPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed coefficient part (octet 20).
    pub bits_per_value: u8,
    /// Type of bi-Fourier sub-truncation (octet 21) — Code Table 3.25
    /// (`77` = rectangle, `88` = ellipse, `99` = diamond). Shapes the unpacked
    /// subset `(sub_i, sub_j)`.
    pub sub_truncation_type: u8,
    /// Bi-Fourier packing mode for axes (octet 22) — Code Table 5.26. `1`
    /// (`keepaxes`) forces the `i == 0` / `j == 0` axes into the unpacked
    /// subset regardless of the sub-truncation.
    pub packing_mode_for_axes: u8,
    /// Laplacian scaling factor (**sign-magnitude** `signed[4]`, octets
    /// 23–26), in units of 10⁻⁶; the operator exponent `P` is this divided by
    /// 10⁶. The sentinel `-2_147_483_647` marks it unset. Like every GRIB
    /// signed integer (and the 5.51 factor), it is sign-magnitude, not
    /// two's-complement — see the parse site.
    pub laplacian_scaling_factor: i32,
    /// `NS` — bi-Fourier sub-truncation resolution in `i` (octets 27–28).
    pub sub_i: u16,
    /// `MS` — bi-Fourier sub-truncation resolution in `j` (octets 29–30).
    pub sub_j: u16,
    /// `TS` — total number of values in the unpacked subset (octets 31–34).
    pub total_values_in_unpacked_subset: u32,
    /// Precision of the unpacked subset (octet 35) — WMO Code Table 5.7
    /// (`1` = IEEE 32-bit, `2` = IEEE 64-bit; ECMWF/ALADIN fields use `2`).
    pub unpacked_subset_precision: u8,
}

/// Templates 5.50001 / 5.50002 — second-order (general-extended) packing.
///
/// The GRIB1 general-extended second-order codec (`grid_second_order_*`)
/// carried into GRIB2, split across §5 and §7. §5 (this struct) holds the
/// `R` / `E` / `D` transform, the group-descriptor bit widths, the number of
/// groups, and the spatial-predictor-differencing (SPD) seeds. §7 then carries
/// — as byte-aligned blocks — the per-group widths, per-group lengths, and
/// first-order (group reference) values, followed by the second-order packed
/// per-point offsets (see [`super::ds`]). The value at a point is
/// `(R + X · 2^E) · 10^-D`, where `X` is the SPD-reconstructed integer.
///
/// 5.50001 and 5.50002 share this layout; 5.50002 adds a `secondOrderFlags`
/// octet whose bit 7 requests boustrophedonic (alternating-row) ordering, and
/// 5.50001 (`grid_second_order_no_boustrophedonic`) omits it. The two templates
/// decode identically apart from that row reordering, which is undone after the
/// grid width is known (see [`super::ds::undo_second_order_boustrophedonic`]).
///
/// Not `Copy`: the SPD seeds are heap-allocated.
#[derive(Debug, Clone, PartialEq)]
pub struct SecondOrderPackingTemplate {
    /// Reference value `R` (IEEE 32-bit float, octets 12–15).
    pub reference_value: f32,
    /// Binary scale factor `E` (sign-magnitude `i16`, octets 16–17).
    pub binary_scale_factor: i16,
    /// Decimal scale factor `D` (sign-magnitude `i16`, octets 18–19).
    pub decimal_scale_factor: i16,
    /// Number of bits used for each packed value (octet 20). Unlike simple
    /// packing, second-order packing has no `bits_per_value == 0`
    /// constant-field convention: a constant field is signalled by
    /// `num_groups == 0` instead (see that field), and eccodes encodes such a
    /// field as simple packing (`grid_simple`) rather than second-order anyway.
    pub bits_per_value: u8,
    /// Width in bits of each first-order (group reference) value in §7
    /// (octet 21).
    pub width_of_first_order_values: u8,
    /// NG — number of groups the field is split into (octets 22–25). `0` marks
    /// a constant field: §7 carries no data and every present point equals
    /// `R · 10^-D` (see [`super::ds`]).
    pub num_groups: u32,
    /// Number of second-order packed values in §7 (octets 26–29) — the grid
    /// point count less the `order_of_spd` SPD seeds.
    pub num_second_order_packed_values: u32,
    /// Width in bits of each group width in §7 (octet 30).
    pub width_of_widths: u8,
    /// Width in bits of each group length in §7 (octet 31).
    pub width_of_lengths: u8,
    /// Boustrophedonic (alternating-row) ordering requested — 5.50002 octet 32
    /// bit 7. Always `false` for 5.50001, which has no `secondOrderFlags` octet.
    pub boustrophedonic: bool,
    /// Order of spatial-predictor differencing (WMO Code Table 5.6): `0`–`3`.
    /// eccodes' `grid_second_order` encoder emits `2`.
    pub order_of_spd: u8,
    /// Width in bits of each SPD value (the seeds and the bias). Present only
    /// when `order_of_spd > 0`; `0` otherwise.
    pub width_of_spd: u8,
    /// The `order_of_spd` SPD seeds — the first original scaled integers that
    /// prime the inverse-differencing recurrence (each `width_of_spd` bits,
    /// unsigned).
    pub spd_seeds: Vec<i64>,
    /// The SPD bias — the overall minimum difference added at each
    /// reconstruction step (`width_of_spd` bits, sign-magnitude).
    pub spd_bias: i64,
}

/// Decoded template payload. Templates outside the supported set surface as
/// [`DataRepresentationTemplate::Unsupported`].
///
/// Not `Copy`: [`RunLengthPackingTemplate`] carries a heap-allocated
/// level-value table.
#[derive(Debug, Clone, PartialEq)]
pub enum DataRepresentationTemplate {
    Simple(SimplePackingTemplate),
    MatrixSimple(MatrixSimplePackingTemplate),
    Complex(ComplexPackingTemplate),
    ComplexSpatialDiff(ComplexSpatialDiffTemplate),
    Ieee(IeeePackingTemplate),
    Png(PngPackingTemplate),
    Ccsds(CcsdsPackingTemplate),
    Jpeg2000(Jpeg2000PackingTemplate),
    RunLength(RunLengthPackingTemplate),
    LogPreprocessing(LogPreprocessingPackingTemplate),
    SpectralSimple(SpectralSimplePackingTemplate),
    SpectralComplex(SpectralComplexPackingTemplate),
    BiFourier(BiFourierPackingTemplate),
    SecondOrder(SecondOrderPackingTemplate),
    Unsupported(u16),
}

/// Parsed contents of the Data Representation Section.
#[derive(Debug, Clone, PartialEq)]
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
            DataRepresentationTemplate::MatrixSimple(_) => "grid_simple_matrix".to_string(),
            DataRepresentationTemplate::Complex(_) => "complex".to_string(),
            DataRepresentationTemplate::ComplexSpatialDiff(_) => "complex_spatial_diff".to_string(),
            DataRepresentationTemplate::Ieee(_) => "ieee".to_string(),
            DataRepresentationTemplate::Png(_) => "png".to_string(),
            DataRepresentationTemplate::Ccsds(_) => "ccsds".to_string(),
            DataRepresentationTemplate::Jpeg2000(_) => "jpeg".to_string(),
            DataRepresentationTemplate::RunLength(_) => "run_length".to_string(),
            DataRepresentationTemplate::LogPreprocessing(_) => {
                "simple_log_preprocessing".to_string()
            }
            DataRepresentationTemplate::SpectralSimple(_) => "spectral_simple".to_string(),
            DataRepresentationTemplate::SpectralComplex(_) => "spectral_complex".to_string(),
            DataRepresentationTemplate::BiFourier(_) => "bifourier_complex".to_string(),
            DataRepresentationTemplate::SecondOrder(_) => "second_order".to_string(),
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

    /// Borrow the matrix-simple-packing template (5.1) if that's what the
    /// section carries. Other templates return `None`.
    pub fn matrix_simple(&self) -> Option<&MatrixSimplePackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::MatrixSimple(t) => Some(t),
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

    /// Borrow the PNG-packing template if that's what the section carries.
    /// Other templates return `None`.
    pub fn png(&self) -> Option<&PngPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Png(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the CCSDS / AEC-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn ccsds(&self) -> Option<&CcsdsPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Ccsds(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the JPEG 2000-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn jpeg2000(&self) -> Option<&Jpeg2000PackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::Jpeg2000(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the run-length-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn run_length(&self) -> Option<&RunLengthPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::RunLength(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the log-preprocessing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn log_preprocessing(&self) -> Option<&LogPreprocessingPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::LogPreprocessing(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the spectral simple-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn spectral_simple(&self) -> Option<&SpectralSimplePackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::SpectralSimple(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the spectral complex-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn spectral_complex(&self) -> Option<&SpectralComplexPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::SpectralComplex(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the bi-Fourier spectral-packing template if that's what the
    /// section carries. Other templates return `None`.
    pub fn bifourier(&self) -> Option<&BiFourierPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::BiFourier(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the second-order-packing template if that's what the section
    /// carries. Other templates return `None`.
    pub fn second_order(&self) -> Option<&SecondOrderPackingTemplate> {
        match &self.template {
            DataRepresentationTemplate::SecondOrder(t) => Some(t),
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
        1 => DataRepresentationTemplate::MatrixSimple(parse_template_5_1(payload)?),
        2 => DataRepresentationTemplate::Complex(parse_template_5_2(payload)?),
        3 => DataRepresentationTemplate::ComplexSpatialDiff(parse_template_5_3(payload)?),
        4 => DataRepresentationTemplate::Ieee(parse_template_5_4(payload)?),
        40 => DataRepresentationTemplate::Jpeg2000(parse_template_5_40(payload)?),
        41 => DataRepresentationTemplate::Png(parse_template_5_41(payload)?),
        42 => DataRepresentationTemplate::Ccsds(parse_template_5_42(payload)?),
        61 => DataRepresentationTemplate::LogPreprocessing(parse_template_5_61(payload)?),
        50 => DataRepresentationTemplate::SpectralSimple(parse_template_5_50(payload)?),
        51 => DataRepresentationTemplate::SpectralComplex(parse_template_5_51(payload)?),
        53 => DataRepresentationTemplate::BiFourier(parse_template_5_53(payload)?),
        200 => DataRepresentationTemplate::RunLength(parse_template_5_200(payload)?),
        // Second-order (general-extended) packing — the GRIB1 `grid_second_order`
        // codec carried into GRIB2. 5.50001 (`grid_second_order_no_boustrophedonic`)
        // omits the `secondOrderFlags` octet that 5.50002 (`grid_second_order`)
        // carries; both otherwise share the §5 layout.
        50001 => DataRepresentationTemplate::SecondOrder(parse_template_5_50001(payload, false)?),
        50002 => DataRepresentationTemplate::SecondOrder(parse_template_5_50001(payload, true)?),
        // Local-use (pre-standard NCEP) templates whose §5 and §7 are identical
        // to a registered image packing, so they decode through the same codec.
        // 5.40000 predates the registered JPEG 2000 template 5.40; 5.40010
        // predates PNG template 5.41 (and eccodes has no definition for it, so
        // this is a genuinely exceed-eccodes decode). The `template_number`
        // field preserves the local number for the metadata view.
        40000 => DataRepresentationTemplate::Jpeg2000(parse_template_5_40(payload)?),
        40010 => DataRepresentationTemplate::Png(parse_template_5_41(payload)?),
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

fn parse_template_5_1(payload: &[u8]) -> Result<MatrixSimplePackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_1_FIXED_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.1 needs at least {TEMPLATE_5_1_FIXED_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    let matrix_bitmaps_present = payload[9];
    let number_of_coded_values =
        u32::from_be_bytes([payload[10], payload[11], payload[12], payload[13]]);
    let nr = u16::from_be_bytes([payload[14], payload[15]]);
    let nc = u16::from_be_bytes([payload[16], payload[17]]);
    let nc1 = payload[19] as usize;
    let nc2 = payload[21] as usize;

    // The coordinate-function coefficients (`coefsFirst[NC1]`, then
    // `coefsSecond[NC2]`) are IEEE 32-bit floats immediately after the fixed
    // header; bound their length before reading.
    let coef_bytes = nc1
        .checked_add(nc2)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| {
            FieldglassError::Parse("DRS template 5.1 coefficient byte count overflows".into())
        })?;
    let need = TEMPLATE_5_1_FIXED_PAYLOAD_LEN + coef_bytes;
    if payload.len() < need {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.1 declares NC1={nc1} + NC2={nc2} coefficients needing {need} payload \
             bytes, got {}",
            payload.len()
        )));
    }
    let read_coefs = |start: usize, count: usize| -> Vec<f32> {
        (0..count)
            .map(|i| {
                let o = start + i * 4;
                f32::from_be_bytes([payload[o], payload[o + 1], payload[o + 2], payload[o + 3]])
            })
            .collect()
    };
    let coefficients_first = read_coefs(TEMPLATE_5_1_FIXED_PAYLOAD_LEN, nc1);
    let coefficients_second = read_coefs(TEMPLATE_5_1_FIXED_PAYLOAD_LEN + nc1 * 4, nc2);

    Ok(MatrixSimplePackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        matrix_bitmaps_present,
        number_of_coded_values,
        nr,
        nc,
        first_dim_coordinate_definition: payload[18],
        second_dim_coordinate_definition: payload[20],
        first_dim_physical_significance: payload[22],
        second_dim_physical_significance: payload[23],
        coefficients_first,
        coefficients_second,
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

fn parse_template_5_41(payload: &[u8]) -> Result<PngPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_41_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.41 needs {TEMPLATE_5_41_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–21 mirror simple packing (5.0): R, E, D, bits-per-value, type.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    Ok(PngPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        original_field_type: payload[9],
    })
}

fn parse_template_5_40(payload: &[u8]) -> Result<Jpeg2000PackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_40_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.40 needs {TEMPLATE_5_40_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–21 mirror simple packing (5.0): R, E, D, bits-per-value, type.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    // Octets 22–23 are the JPEG 2000 descriptors.
    Ok(Jpeg2000PackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        original_field_type: payload[9],
        type_of_compression_used: payload[10],
        target_compression_ratio: payload[11],
    })
}

fn parse_template_5_42(payload: &[u8]) -> Result<CcsdsPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_42_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.42 needs {TEMPLATE_5_42_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–21 mirror simple packing (5.0): R, E, D, bits-per-value, type.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    // Octets 22–25 are the CCSDS / AEC descriptors.
    let reference_sample_interval = u16::from_be_bytes([payload[12], payload[13]]);
    Ok(CcsdsPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        original_field_type: payload[9],
        ccsds_flags: payload[10],
        block_size: payload[11],
        reference_sample_interval,
    })
}

fn parse_template_5_61(payload: &[u8]) -> Result<LogPreprocessingPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_61_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.61 needs {TEMPLATE_5_61_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–20 are the simple-packing block (R / E / D / bits); unlike 5.0
    // there is no type-of-original-values octet, so octets 21–24 carry the
    // IEEE pre-processing parameter directly.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    let pre_processing_parameter =
        f32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);
    Ok(LogPreprocessingPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        pre_processing_parameter,
    })
}

fn parse_template_5_50(payload: &[u8]) -> Result<SpectralSimplePackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_50_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.50 needs {TEMPLATE_5_50_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–20 are the simple-packing block (R / E / D / bits); like 5.61
    // there is no type-of-original-values octet, so octets 21–24 carry the
    // out-of-band (0, 0) coefficient directly.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    let real_part_of_00 = f32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]);
    Ok(SpectralSimplePackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        real_part_of_00,
    })
}

fn parse_template_5_51(payload: &[u8]) -> Result<SpectralComplexPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_51_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.51 needs {TEMPLATE_5_51_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–20: the R / E / D / bits simple-packing block (no type octet,
    // like 5.50). Then the Laplacian scaling factor and the sub-truncation
    // descriptors.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    let laplacian_scaling_factor = sign_magnitude_to_i64(
        u32::from_be_bytes([payload[9], payload[10], payload[11], payload[12]]),
        32,
    ) as i32;
    let js = u16::from_be_bytes([payload[13], payload[14]]);
    let ks = u16::from_be_bytes([payload[15], payload[16]]);
    let ms = u16::from_be_bytes([payload[17], payload[18]]);
    let ts = u32::from_be_bytes([payload[19], payload[20], payload[21], payload[22]]);
    Ok(SpectralComplexPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        laplacian_scaling_factor,
        js,
        ks,
        ms,
        ts,
        unpacked_subset_precision: payload[23],
    })
}

fn parse_template_5_53(payload: &[u8]) -> Result<BiFourierPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_53_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.53 needs {TEMPLATE_5_53_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    // Octets 12–20: the R / E / D / bits simple-packing block (no type octet,
    // like 5.50 / 5.51). Then the bi-Fourier sub-truncation descriptors.
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    // laplacianScalingFactor is eccodes' `signed[4]`, which — like every GRIB
    // signed integer, and like the 5.51 Laplacian factor — is sign-magnitude,
    // NOT two's-complement (verified against eccodes: a negative value encodes
    // with the sign bit set and a small magnitude). Units of 10^-6.
    let laplacian_scaling_factor = sign_magnitude_to_i64(
        u32::from_be_bytes([payload[11], payload[12], payload[13], payload[14]]),
        32,
    ) as i32;
    Ok(BiFourierPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value: payload[8],
        sub_truncation_type: payload[9],
        packing_mode_for_axes: payload[10],
        laplacian_scaling_factor,
        sub_i: u16::from_be_bytes([payload[15], payload[16]]),
        sub_j: u16::from_be_bytes([payload[17], payload[18]]),
        total_values_in_unpacked_subset: u32::from_be_bytes([
            payload[19],
            payload[20],
            payload[21],
            payload[22],
        ]),
        unpacked_subset_precision: payload[23],
    })
}

fn parse_template_5_200(payload: &[u8]) -> Result<RunLengthPackingTemplate, FieldglassError> {
    if payload.len() < TEMPLATE_5_200_FIXED_PAYLOAD_LEN {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.200 needs at least {TEMPLATE_5_200_FIXED_PAYLOAD_LEN} bytes of payload, got {}",
            payload.len()
        )));
    }
    let bits_per_value = payload[0];
    let max_level_value = u16::from_be_bytes([payload[1], payload[2]]);
    let number_of_level_values = u16::from_be_bytes([payload[3], payload[4]]);
    // Single-octet sign-magnitude: a raw byte above 127 encodes a negative
    // exponent (`-(raw - 128)`), matching eccodes' run-length accessor.
    let decimal_scale_factor = sign_magnitude_to_i64(payload[5] as u32, 8) as i16;

    // The level-value list is `number_of_level_values` big-endian u16s
    // starting at octet 18 (payload index 6).
    let list_len = number_of_level_values as usize * 2;
    let want = TEMPLATE_5_200_FIXED_PAYLOAD_LEN + list_len;
    if payload.len() < want {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.200 declares {number_of_level_values} level values (needs {want} payload bytes), got {}",
            payload.len()
        )));
    }
    let level_values = payload[TEMPLATE_5_200_FIXED_PAYLOAD_LEN..want]
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();

    Ok(RunLengthPackingTemplate {
        bits_per_value,
        max_level_value,
        number_of_level_values,
        decimal_scale_factor,
        level_values,
    })
}

/// Parse templates 5.50001 (`has_flags == false`) and 5.50002
/// (`has_flags == true`) — second-order (general-extended) packing. The two
/// share the §5 layout except that 5.50002 inserts a one-octet
/// `secondOrderFlags` field (bit 7 = boustrophedonic ordering) just before
/// `orderOfSPD`. All offsets below are payload-relative (payload octet 0 =
/// section octet 12).
fn parse_template_5_50001(
    payload: &[u8],
    has_flags: bool,
) -> Result<SecondOrderPackingTemplate, FieldglassError> {
    // The `secondOrderFlags` octet (5.50002 only) shifts everything after
    // `widthOfLengths` forward by one byte.
    let flags_len = usize::from(has_flags);
    let fixed_len = TEMPLATE_5_50001_FIXED_PAYLOAD_LEN + flags_len;
    if payload.len() < fixed_len {
        return Err(FieldglassError::Parse(format!(
            "DRS template 5.{} needs at least {fixed_len} bytes of payload before the SPD block, got {}",
            if has_flags { 50002 } else { 50001 },
            payload.len()
        )));
    }
    let reference_value = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let binary_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[4], payload[5]]));
    let decimal_scale_factor = sign_magnitude_i16(u16::from_be_bytes([payload[6], payload[7]]));
    let bits_per_value = payload[8];
    let width_of_first_order_values = payload[9];
    let num_groups = u32::from_be_bytes([payload[10], payload[11], payload[12], payload[13]]);
    let num_second_order_packed_values =
        u32::from_be_bytes([payload[14], payload[15], payload[16], payload[17]]);
    let width_of_widths = payload[18];
    let width_of_lengths = payload[19];
    // 5.50002: octet 20 is `secondOrderFlags`, boustrophedonic is bit 7 (0x80,
    // the MSB, matching eccodes' `flagbit(secondOrderFlags, 7)`). 5.50001 has no
    // such octet, so ordering is never boustrophedonic.
    let boustrophedonic = has_flags && (payload[20] & 0x80) != 0;
    // `order_of_spd` is the last byte of the fixed prefix (payload index
    // 20 + flags_len). `width_of_spd` and the SPD seed block follow it on the
    // wire ONLY when order_of_spd > 0 — eccodes gates them behind `if
    // (orderOfSPD)` in `template.5.5000{1,2}.def` — so a valid no-SPD field
    // (order_of_spd == 0) ends right here, with no width octet.
    let order_of_spd = payload[20 + flags_len];

    // The SPD block holds orderOfSPD unsigned seeds followed by one
    // sign-magnitude bias, each widthOfSPD bits, in a byte-aligned block of
    // ceil((orderOfSPD + 1) · widthOfSPD / 8) bytes.
    let mut width_of_spd = 0u8;
    let mut spd_seeds = Vec::new();
    let mut spd_bias = 0i64;
    if order_of_spd > 0 {
        // widthOfSPD sits immediately after the fixed prefix.
        if payload.len() <= fixed_len {
            return Err(FieldglassError::Parse(format!(
                "DRS template 5.{} declares orderOfSPD={order_of_spd} but has no widthOfSPD octet",
                if has_flags { 50002 } else { 50001 },
            )));
        }
        width_of_spd = payload[fixed_len];
        if width_of_spd == 0 || width_of_spd > 32 {
            return Err(FieldglassError::Parse(format!(
                "DRS template 5.{} widthOfSPD={width_of_spd} out of the supported 1..=32 range",
                if has_flags { 50002 } else { 50001 },
            )));
        }
        let spd_count = order_of_spd as usize + 1;
        let spd_bytes = bits_to_bytes(spd_count, width_of_spd as usize).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "DRS second-order SPD byte length overflows ({spd_count}×{width_of_spd} bits)"
            ))
        })?;
        let spd_start = fixed_len + 1;
        if payload.len() < spd_start + spd_bytes {
            return Err(FieldglassError::Parse(format!(
                "DRS template 5.{} declares orderOfSPD={order_of_spd} but the section is too short for the SPD block",
                if has_flags { 50002 } else { 50001 },
            )));
        }
        let mut reader = BitReader::new(&payload[spd_start..spd_start + spd_bytes]);
        for _ in 0..order_of_spd {
            spd_seeds.push(reader.read_bits(width_of_spd)? as i64);
        }
        spd_bias = sign_magnitude_to_i64(reader.read_bits(width_of_spd)?, width_of_spd);
    }

    Ok(SecondOrderPackingTemplate {
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value,
        width_of_first_order_values,
        num_groups,
        num_second_order_packed_values,
        width_of_widths,
        width_of_lengths,
        boustrophedonic,
        order_of_spd,
        width_of_spd,
        spd_seeds,
        spd_bias,
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

    fn build_drs_5_1(matrix_bitmaps: u8, nr: u16, nc: u16, nc1: u8, nc2: u8) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(&280.0_f32.to_be_bytes()); // R
        payload.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        payload.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        payload.push(12); // bitsPerValue
        payload.push(matrix_bitmaps); // matrixBitmapsPresent
        payload.extend_from_slice(&496u32.to_be_bytes()); // numberOfCodedValues
        payload.extend_from_slice(&nr.to_be_bytes()); // NR
        payload.extend_from_slice(&nc.to_be_bytes()); // NC
        payload.push(0); // firstDimensionCoordinateValueDefinition
        payload.push(nc1); // NC1
        payload.push(0); // secondDimensionCoordinateValueDefinition
        payload.push(nc2); // NC2
        payload.push(0); // firstDimensionPhysicalSignificance
        payload.push(0); // secondDimensionPhysicalSignificance
        for i in 0..nc1 {
            payload.extend_from_slice(&(1.0_f32 + i as f32).to_be_bytes());
        }
        for i in 0..nc2 {
            payload.extend_from_slice(&(10.0_f32 + i as f32).to_be_bytes());
        }

        let section_len = (11 + payload.len()) as u32;
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&1u16.to_be_bytes()); // template 5.1
        buf.extend_from_slice(&payload);
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_1_round_trips_with_coefficient_arrays() {
        let drs = parse_data_representation(&build_drs_5_1(0, 2, 3, 1, 2)).expect("parse 5.1");
        assert_eq!(drs.template_number, 1);
        assert_eq!(drs.template_name(), "grid_simple_matrix");
        let t = drs.matrix_simple().expect("5.1 has matrix template");
        assert!((t.reference_value - 280.0).abs() < 1e-6);
        assert_eq!(t.bits_per_value, 12);
        assert_eq!(t.matrix_bitmaps_present, 0);
        assert_eq!(t.number_of_coded_values, 496);
        assert_eq!((t.nr, t.nc), (2, 3));
        assert_eq!(t.coefficients_first, vec![1.0]);
        assert_eq!(t.coefficients_second, vec![10.0, 11.0]);
        assert!(drs.simple().is_none());
    }

    #[test]
    fn template_5_1_reads_matrix_bitmaps_flag() {
        let drs = parse_data_representation(&build_drs_5_1(1, 4, 5, 0, 0)).expect("parse 5.1");
        let t = drs.matrix_simple().expect("matrix template");
        assert_eq!(t.matrix_bitmaps_present, 1);
        assert_eq!((t.nr, t.nc), (4, 5));
        assert!(t.coefficients_first.is_empty());
        assert!(t.coefficients_second.is_empty());
    }

    #[test]
    fn rejects_5_1_when_coefficient_arrays_truncated() {
        // Declare NC1=4 coefficients but chop the section short of them.
        let mut buf = build_drs_5_1(0, 2, 3, 4, 0);
        let new_len = 35u32; // fixed header (11 + 24) only, no coefficient bytes
        buf.truncate(new_len as usize);
        buf[0..4].copy_from_slice(&new_len.to_be_bytes());
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.1"),
            "error names template 5.1, got: {err}",
        );
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

    /// Build a minimal §5 with template 5.41 — a 21-byte section whose payload
    /// mirrors simple packing: R = 97392.0, E = 0, D = 0, 13 bits/value,
    /// original field type 0 (the parameters of the `png_eta_lambert` fixture).
    fn build_drs_5_41() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 21;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&6045u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&41u16.to_be_bytes()); // template 5.41
        buf.extend_from_slice(&97392.0_f32.to_be_bytes()); // R
        buf.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(13); // bits per value
        buf.push(0); // original field type
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_41_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_41()).expect("parse 5.41");
        assert_eq!(drs.template_number, 41);
        assert_eq!(drs.num_data_points, 6045);
        assert_eq!(drs.template_name(), "png");

        let t = drs.png().expect("5.41 has png template");
        assert!((t.reference_value - 97392.0).abs() < 1e-3);
        assert_eq!(t.binary_scale_factor, 0);
        assert_eq!(t.decimal_scale_factor, 0);
        assert_eq!(t.bits_per_value, 13);
        assert_eq!(t.original_field_type, 0);
        // The accessors for the other templates must not claim a PNG section.
        assert!(drs.simple().is_none());
        assert!(drs.ieee().is_none());
    }

    #[test]
    fn rejects_5_41_when_payload_truncated() {
        // Declare length 11 (just past the template number) so the 5.41
        // payload check fires on the missing R/E/D/bits octets.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&11u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&41u16.to_be_bytes()); // template 5.41
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.41 needs"),
            "error names template-5.41 shortfall, got: {err}",
        );
    }

    /// Build a minimal §5 with template 5.42 — a 25-byte section whose payload
    /// carries the simple-packing block plus the three CCSDS descriptors, using
    /// the parameters of the `ccsds_regular_latlon` fixture: R = 270.467, E =
    /// -10, D = 0, 16 bits/value, type 0, flags 0x0e, block size 32, RSI 128.
    fn build_drs_5_42() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 25;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&42u16.to_be_bytes()); // template 5.42
        buf.extend_from_slice(&270.467_f32.to_be_bytes()); // R
        buf.extend_from_slice(&0x800a_u16.to_be_bytes()); // E = -10 (sign-magnitude)
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(16); // bits per value
        buf.push(0); // original field type
        buf.push(0x0e); // ccsds flags
        buf.push(32); // block size
        buf.extend_from_slice(&128u16.to_be_bytes()); // reference sample interval
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_42_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_42()).expect("parse 5.42");
        assert_eq!(drs.template_number, 42);
        assert_eq!(drs.num_data_points, 496);
        assert_eq!(drs.template_name(), "ccsds");

        let t = drs.ccsds().expect("5.42 has ccsds template");
        assert!((t.reference_value - 270.467).abs() < 1e-3);
        assert_eq!(t.binary_scale_factor, -10);
        assert_eq!(t.decimal_scale_factor, 0);
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.original_field_type, 0);
        assert_eq!(t.ccsds_flags, 0x0e);
        assert_eq!(t.block_size, 32);
        assert_eq!(t.reference_sample_interval, 128);
        // The accessors for the other templates must not claim a CCSDS section.
        assert!(drs.simple().is_none());
        assert!(drs.png().is_none());
    }

    #[test]
    fn rejects_5_42_when_payload_truncated() {
        // Declare length 21 (a full simple-packing block but no CCSDS
        // descriptors) so the 5.42 payload check fires on the missing
        // flags/block-size/RSI octets.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&21u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&42u16.to_be_bytes()); // template 5.42
        buf.extend_from_slice(&[0u8; 10]); // R/E/D/bits/type only
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.42 needs"),
            "error names template-5.42 shortfall, got: {err}",
        );
    }

    /// Build a §5 with template 5.200 (run-length): a 6-byte fixed block plus
    /// a `level_values` list. `decimal_scale_raw` is the raw octet (single-byte
    /// sign-magnitude), so 129 encodes D = -1.
    fn build_drs_5_200(
        bits: u8,
        max_level: u16,
        level_values: &[u16],
        decimal_scale_raw: u8,
    ) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len = 17 + level_values.len() * 2;
        buf.extend_from_slice(&(section_len as u32).to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&200u16.to_be_bytes()); // template 5.200
        buf.push(bits); // bits per value
        buf.extend_from_slice(&max_level.to_be_bytes()); // max level value
        buf.extend_from_slice(&(level_values.len() as u16).to_be_bytes()); // number of level values
        buf.push(decimal_scale_raw); // decimal scale factor (raw)
        for lv in level_values {
            buf.extend_from_slice(&lv.to_be_bytes());
        }
        assert_eq!(buf.len(), section_len);
        buf
    }

    #[test]
    fn template_5_200_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_200(8, 5, &[10, 20, 30, 40, 50], 1))
            .expect("parse 5.200");
        assert_eq!(drs.template_number, 200);
        assert_eq!(drs.num_data_points, 496);
        assert_eq!(drs.template_name(), "run_length");

        let t = drs.run_length().expect("5.200 has run-length template");
        assert_eq!(t.bits_per_value, 8);
        assert_eq!(t.max_level_value, 5);
        assert_eq!(t.number_of_level_values, 5);
        assert_eq!(t.decimal_scale_factor, 1);
        assert_eq!(t.level_values, vec![10, 20, 30, 40, 50]);
        // Other accessors must not claim a run-length section.
        assert!(drs.simple().is_none());
        assert!(drs.ccsds().is_none());
    }

    #[test]
    fn template_5_200_decodes_single_byte_sign_magnitude_decimal_scale() {
        // Raw octet 129 → −(129 − 128) = −1, matching eccodes' run-length
        // accessor. Octet 128 is negative zero → 0.
        let neg = parse_data_representation(&build_drs_5_200(4, 2, &[1, 2], 129))
            .expect("parse 5.200")
            .run_length()
            .expect("run-length")
            .decimal_scale_factor;
        assert_eq!(neg, -1);
        let neg_zero = parse_data_representation(&build_drs_5_200(4, 2, &[1, 2], 128))
            .expect("parse 5.200")
            .run_length()
            .expect("run-length")
            .decimal_scale_factor;
        assert_eq!(neg_zero, 0);
    }

    #[test]
    fn rejects_5_200_when_fixed_block_truncated() {
        // Declare a length that leaves fewer than the 6 fixed payload octets.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&14u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&200u16.to_be_bytes()); // template 5.200
        buf.extend_from_slice(&[0u8; 3]); // only 3 of the 6 fixed octets
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.200 needs"),
            "error names template-5.200 shortfall, got: {err}",
        );
    }

    #[test]
    fn rejects_5_200_when_level_list_truncated() {
        // numberOfLevelValues = 5 needs 10 list octets, but only 4 follow.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&21u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes());
        buf.extend_from_slice(&200u16.to_be_bytes());
        buf.push(8); // bits
        buf.extend_from_slice(&5u16.to_be_bytes()); // max level
        buf.extend_from_slice(&5u16.to_be_bytes()); // number of level values
        buf.push(0); // decimal scale
        buf.extend_from_slice(&[0u8; 4]); // only 2 of the 5 level values
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("level values"),
            "error names the level-value shortfall, got: {err}",
        );
    }

    /// Build a §5 with template 5.61 (log pre-processing): the 9-byte
    /// simple-packing block (no type octet) plus a 4-byte IEEE
    /// `preProcessingParameter`, 24 bytes total.
    fn build_drs_5_61(r: f32, e: i16, d: i16, bits: u8, ppp: f32) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 24;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&61u16.to_be_bytes()); // template 5.61
        buf.extend_from_slice(&r.to_be_bytes()); // R
        buf.extend_from_slice(&(e as u16).to_be_bytes()); // E (sign-magnitude via i16 round-trip)
        buf.extend_from_slice(&(d as u16).to_be_bytes()); // D
        buf.push(bits); // bits per value
        buf.extend_from_slice(&ppp.to_be_bytes()); // pre-processing parameter
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_61_round_trips_synthesized_payload() {
        // E/D encoded as plain positive values so the sign-magnitude round-trip
        // is the identity here; the sign path is covered by the simple-packing
        // tests that share `sign_magnitude_i16`.
        let drs =
            parse_data_representation(&build_drs_5_61(5.6, 0, 2, 16, 32.5)).expect("parse 5.61");
        assert_eq!(drs.template_number, 61);
        assert_eq!(drs.num_data_points, 496);
        assert_eq!(drs.template_name(), "simple_log_preprocessing");

        let t = drs
            .log_preprocessing()
            .expect("5.61 has log-preprocessing template");
        assert!((t.reference_value - 5.6).abs() < 1e-4);
        assert_eq!(t.binary_scale_factor, 0);
        assert_eq!(t.decimal_scale_factor, 2);
        assert_eq!(t.bits_per_value, 16);
        assert!((t.pre_processing_parameter - 32.5).abs() < 1e-4);
        // Other accessors must not claim a log-preprocessing section.
        assert!(drs.simple().is_none());
        assert!(drs.run_length().is_none());
    }

    #[test]
    fn rejects_5_61_when_payload_truncated() {
        // Declare length 21 (a full 5.0 block) so the 5.61 check fires on the
        // missing pre-processing-parameter octets.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&21u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&61u16.to_be_bytes()); // template 5.61
        buf.extend_from_slice(&[0u8; 10]); // only 10 payload octets
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.61 needs"),
            "error names template-5.61 shortfall, got: {err}",
        );
    }

    /// Build a §5 with template 5.50 (spectral simple): the 9-byte
    /// simple-packing block (no type octet) plus the 4-byte IEEE realPartOf00,
    /// 24 bytes total.
    fn build_drs_5_50(r: f32, bits: u8, real00: f32) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 24;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&4160u32.to_be_bytes()); // num data points (T63)
        buf.extend_from_slice(&50u16.to_be_bytes()); // template 5.50
        buf.extend_from_slice(&r.to_be_bytes()); // R
        buf.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(bits); // bits per value
        buf.extend_from_slice(&real00.to_be_bytes()); // realPartOf00
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_50_round_trips_synthesized_payload() {
        let drs =
            parse_data_representation(&build_drs_5_50(-12.765, 16, 288.234)).expect("parse 5.50");
        assert_eq!(drs.template_number, 50);
        assert_eq!(drs.template_name(), "spectral_simple");
        let t = drs.spectral_simple().expect("5.50 has spectral template");
        assert!((t.reference_value - (-12.765)).abs() < 1e-3);
        assert_eq!(t.bits_per_value, 16);
        assert!((t.real_part_of_00 - 288.234).abs() < 1e-3);
        assert!(drs.simple().is_none());
        assert!(drs.log_preprocessing().is_none());
    }

    #[test]
    fn rejects_5_50_when_payload_truncated() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&20u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&50u16.to_be_bytes()); // template 5.50
        buf.extend_from_slice(&[0u8; 9]); // only 9 of the 13 payload octets
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.50 needs"),
            "error names template-5.50 shortfall, got: {err}",
        );
    }

    /// Build a §5 with template 5.51 (spectral complex): the 24-byte payload of
    /// R / E / D / bits, the Laplacian scaling factor, JS / KS / MS, TS, and the
    /// unpacked-subset precision.
    fn build_drs_5_51(r: f32, bits: u8, laplacian: i32, ks: u16) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 35;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&4160u32.to_be_bytes()); // num data points (T63)
        buf.extend_from_slice(&51u16.to_be_bytes()); // template 5.51
        buf.extend_from_slice(&r.to_be_bytes()); // R
        buf.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(bits); // bits per value
        // Laplacian scaling factor, encoded sign-magnitude (sign bit + magnitude).
        let laplacian_raw = if laplacian < 0 {
            0x8000_0000u32 | laplacian.unsigned_abs()
        } else {
            laplacian as u32
        };
        buf.extend_from_slice(&laplacian_raw.to_be_bytes());
        buf.extend_from_slice(&ks.to_be_bytes()); // JS
        buf.extend_from_slice(&ks.to_be_bytes()); // KS
        buf.extend_from_slice(&ks.to_be_bytes()); // MS
        buf.extend_from_slice(&((ks as u32 + 1) * (ks as u32 + 2)).to_be_bytes()); // TS
        buf.push(1); // unpacked subset precision (IEEE 32-bit)
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_51_round_trips_synthesized_payload() {
        let drs =
            parse_data_representation(&build_drs_5_51(-12.0, 16, 722_000, 20)).expect("parse 5.51");
        assert_eq!(drs.template_number, 51);
        assert_eq!(drs.template_name(), "spectral_complex");
        let t = drs
            .spectral_complex()
            .expect("5.51 has spectral-complex template");
        assert!((t.reference_value - (-12.0)).abs() < 1e-3);
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.laplacian_scaling_factor, 722_000);
        assert_eq!((t.js, t.ks, t.ms), (20, 20, 20));
        assert_eq!(t.ts, 21 * 22);
        assert_eq!(t.unpacked_subset_precision, 1);
        assert!(drs.spectral_simple().is_none());
        assert!(drs.simple().is_none());
    }

    #[test]
    fn template_5_51_reads_negative_laplacian_sign_magnitude() {
        // The Laplacian scaling factor is a 4-byte sign-magnitude integer.
        let drs =
            parse_data_representation(&build_drs_5_51(0.0, 16, -1_500_000, 4)).expect("parse 5.51");
        let t = drs.spectral_complex().expect("spectral-complex");
        assert_eq!(t.laplacian_scaling_factor, -1_500_000);
    }

    #[test]
    fn rejects_5_51_when_payload_truncated() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&30u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&51u16.to_be_bytes()); // template 5.51
        buf.extend_from_slice(&[0u8; 19]); // only 19 of the 24 payload octets
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.51 needs"),
            "error names template-5.51 shortfall, got: {err}",
        );
    }

    fn build_drs_5_53(
        r: f32,
        bits: u8,
        sub_trunc: u8,
        keepaxes: u8,
        laplacian: i32,
        sub: u16,
        precision: u8,
    ) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let section_len: u32 = 35;
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&68u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&53u16.to_be_bytes()); // template 5.53
        buf.extend_from_slice(&r.to_be_bytes()); // R
        buf.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(bits); // bitsPerValue
        buf.push(sub_trunc); // biFourierSubTruncationType
        buf.push(keepaxes); // biFourierPackingModeForAxes
        // laplacianScalingFactor: sign-magnitude `signed[4]`.
        let laplacian_raw = if laplacian < 0 {
            0x8000_0000u32 | laplacian.unsigned_abs()
        } else {
            laplacian as u32
        };
        buf.extend_from_slice(&laplacian_raw.to_be_bytes());
        buf.extend_from_slice(&sub.to_be_bytes()); // NS
        buf.extend_from_slice(&sub.to_be_bytes()); // MS
        buf.extend_from_slice(&52u32.to_be_bytes()); // TS
        buf.push(precision); // unpackedSubsetPrecision
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_53_round_trips_synthesized_payload() {
        let drs = parse_data_representation(&build_drs_5_53(270.5, 12, 77, 1, 1_506_732, 2, 2))
            .expect("parse 5.53");
        assert_eq!(drs.template_number, 53);
        assert_eq!(drs.template_name(), "bifourier_complex");
        let t = drs.bifourier().expect("5.53 has bi-Fourier template");
        assert!((t.reference_value - 270.5).abs() < 1e-3);
        assert_eq!(t.bits_per_value, 12);
        assert_eq!(t.sub_truncation_type, 77);
        assert_eq!(t.packing_mode_for_axes, 1);
        assert_eq!(t.laplacian_scaling_factor, 1_506_732);
        assert_eq!((t.sub_i, t.sub_j), (2, 2));
        assert_eq!(t.total_values_in_unpacked_subset, 52);
        assert_eq!(t.unpacked_subset_precision, 2);
        assert!(drs.spectral_complex().is_none());
        assert!(drs.simple().is_none());
    }

    #[test]
    fn template_5_53_reads_negative_laplacian_sign_magnitude() {
        // The Laplacian scaling factor is sign-magnitude (like 5.51), not
        // two's-complement — a negative value must round-trip exactly.
        let drs = parse_data_representation(&build_drs_5_53(0.0, 12, 88, 0, -250_024, 3, 1))
            .expect("parse 5.53");
        let t = drs.bifourier().expect("bi-Fourier template");
        assert_eq!(t.laplacian_scaling_factor, -250_024);
        assert_eq!(t.unpacked_subset_precision, 1);
        assert_eq!(t.packing_mode_for_axes, 0);
    }

    #[test]
    fn rejects_5_53_when_payload_truncated() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&30u32.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&53u16.to_be_bytes()); // template 5.53
        buf.extend_from_slice(&[0u8; 19]); // only 19 of the 24 payload octets
        let err = parse_data_representation(&buf).expect_err("must reject");
        assert!(
            err.to_string().contains("template 5.53 needs"),
            "error names template-5.53 shortfall, got: {err}",
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
        // 99 is unassigned in WMO Code Table 5.0 — a genuinely unsupported
        // template (0/2/3/4/40/41/42/50/61/200 and the local 40000/40010 all
        // decode now).
        bytes[9..11].copy_from_slice(&99u16.to_be_bytes());
        let drs = parse_data_representation(&bytes).expect("parse");
        assert!(matches!(
            drs.template,
            DataRepresentationTemplate::Unsupported(99)
        ));
        assert_eq!(drs.template_name(), "unsupported(5.99)");
        assert!(drs.simple().is_none());
    }

    /// Build a minimal §5 with template 5.40 — a 23-byte section whose payload
    /// is the 10-byte simple-packing block plus the two JPEG 2000 descriptors.
    fn build_drs_5_40() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&23u32.to_be_bytes()); // section length
        buf.push(DRS_SECTION_NUMBER); // section number
        buf.extend_from_slice(&496u32.to_be_bytes()); // number of data points
        buf.extend_from_slice(&40u16.to_be_bytes()); // template 5.40
        buf.extend_from_slice(&270.467f32.to_be_bytes()); // R
        buf.extend_from_slice(&0x800au16.to_be_bytes()); // E = -10 (sign-magnitude)
        buf.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        buf.push(16); // bits per value
        buf.push(0); // original field type
        buf.push(0); // type of compression used (lossless)
        buf.push(255); // target compression ratio (missing)
        buf
    }

    #[test]
    fn parses_template_5_40() {
        let drs = parse_data_representation(&build_drs_5_40()).expect("parse 5.40");
        assert_eq!(drs.template_number, 40);
        assert_eq!(drs.template_name(), "jpeg");
        let t = drs.jpeg2000().expect("5.40 has jpeg2000 template");
        assert_eq!(t.reference_value, 270.467);
        assert_eq!(t.binary_scale_factor, -10);
        assert_eq!(t.decimal_scale_factor, 0);
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.original_field_type, 0);
        assert_eq!(t.type_of_compression_used, 0);
        assert_eq!(t.target_compression_ratio, 255);
        assert!(drs.simple().is_none());
        assert!(drs.png().is_none());
    }

    #[test]
    fn template_5_40_short_payload_is_rejected() {
        let mut buf = build_drs_5_40();
        // Declare length 22 (one octet short of the 12-byte payload) so the
        // 5.40 parser sees a truncated payload.
        buf[3] = 22;
        buf.truncate(22);
        let err = parse_data_representation(&buf).expect_err("must reject short 5.40");
        assert!(
            err.to_string().contains("template 5.40 needs"),
            "error names template-5.40 shortfall, got: {err}",
        );
    }

    /// Rewrite the §5 data-representation-template number (octets 10–11 =
    /// byte indices 9–10) in place, so an existing builder can stand in for a
    /// pre-standard local template with the same payload.
    fn relabel_template_number(buf: &mut [u8], number: u16) {
        buf[9..11].copy_from_slice(&number.to_be_bytes());
    }

    #[test]
    fn local_template_5_40010_parses_as_png() {
        // Pre-standard local PNG: §5 payload identical to 5.41, so it decodes
        // through the PNG codec. eccodes has no 5.40010 definition, so this is
        // an exceed-eccodes decode.
        let mut buf = build_drs_5_41();
        relabel_template_number(&mut buf, 40010);
        let drs = parse_data_representation(&buf).expect("parse 5.40010");
        assert_eq!(drs.template_number, 40010);
        assert_eq!(drs.template_name(), "png");
        let t = drs.png().expect("5.40010 decodes via the PNG template");
        assert_eq!(t.bits_per_value, 13);
        assert!((t.reference_value - 97392.0).abs() < 1e-3);
    }

    #[test]
    fn local_template_5_40000_parses_as_jpeg2000() {
        // Pre-standard local JPEG 2000: §5 payload identical to 5.40.
        let mut buf = build_drs_5_40();
        relabel_template_number(&mut buf, 40000);
        let drs = parse_data_representation(&buf).expect("parse 5.40000");
        assert_eq!(drs.template_number, 40000);
        assert_eq!(drs.template_name(), "jpeg");
        let t = drs
            .jpeg2000()
            .expect("5.40000 decodes via the JPEG 2000 template");
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.type_of_compression_used, 0);
    }

    /// Build a minimal §5 for a second-order template. `flags` is `Some(byte)`
    /// for 5.50002 (the `secondOrderFlags` octet) or `None` for 5.50001. SPD is
    /// orderOfSPD=2, widthOfSPD=8, seeds [5, 9], bias raw byte 0x83 (−3).
    fn build_drs_second_order(template: u16, flags: Option<u8>) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(&270.5_f32.to_be_bytes()); // R
        payload.extend_from_slice(&(0x8000u16 | 10).to_be_bytes()); // E = −10
        payload.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        payload.push(16); // bitsPerValue
        payload.push(14); // widthOfFirstOrderValues
        payload.extend_from_slice(&15u32.to_be_bytes()); // numberOfGroups
        payload.extend_from_slice(&494u32.to_be_bytes()); // numberOfSecondOrderPackedValues
        payload.push(4); // widthOfWidths
        payload.push(7); // widthOfLengths
        if let Some(f) = flags {
            payload.push(f); // secondOrderFlags
        }
        payload.push(2); // orderOfSPD
        payload.push(8); // widthOfSPD
        payload.extend_from_slice(&[5, 9, 0x83]); // SPD seeds [5, 9], bias −3

        let section_len = (11 + payload.len()) as u32;
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&template.to_be_bytes());
        buf.extend_from_slice(&payload);
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_5_50002_round_trips_synthesized_payload() {
        // secondOrderFlags = 0x80 → boustrophedonic bit set.
        let bytes = build_drs_second_order(50002, Some(0x80));
        let drs = parse_data_representation(&bytes).expect("parse 5.50002");
        assert_eq!(drs.template_number, 50002);
        assert_eq!(drs.template_name(), "second_order");

        let t = drs
            .second_order()
            .expect("5.50002 has second-order template");
        assert!((t.reference_value - 270.5).abs() < 1e-3);
        assert_eq!(t.binary_scale_factor, -10);
        assert_eq!(t.decimal_scale_factor, 0);
        assert_eq!(t.bits_per_value, 16);
        assert_eq!(t.width_of_first_order_values, 14);
        assert_eq!(t.num_groups, 15);
        assert_eq!(t.num_second_order_packed_values, 494);
        assert_eq!(t.width_of_widths, 4);
        assert_eq!(t.width_of_lengths, 7);
        assert!(t.boustrophedonic, "bit 7 of secondOrderFlags");
        assert_eq!(t.order_of_spd, 2);
        assert_eq!(t.width_of_spd, 8);
        assert_eq!(t.spd_seeds, vec![5, 9]);
        assert_eq!(t.spd_bias, -3);
    }

    #[test]
    fn template_5_50001_has_no_flags_octet() {
        // 5.50001 omits secondOrderFlags, so orderOfSPD sits one byte earlier
        // and boustrophedonic is always false.
        let bytes = build_drs_second_order(50001, None);
        let drs = parse_data_representation(&bytes).expect("parse 5.50001");
        assert_eq!(drs.template_number, 50001);
        let t = drs
            .second_order()
            .expect("5.50001 has second-order template");
        assert!(!t.boustrophedonic);
        assert_eq!(t.order_of_spd, 2);
        assert_eq!(t.width_of_spd, 8);
        assert_eq!(t.spd_seeds, vec![5, 9]);
        assert_eq!(t.spd_bias, -3);
        assert_eq!(t.num_groups, 15);
    }

    #[test]
    fn template_5_50002_clear_flag_is_not_boustrophedonic() {
        let bytes = build_drs_second_order(50002, Some(0x00));
        let drs = parse_data_representation(&bytes).expect("parse");
        assert!(!drs.second_order().unwrap().boustrophedonic);
    }

    #[test]
    fn template_5_50001_rejects_truncated_spd_block() {
        // Chop the SPD block off entirely; the parser must reject rather than
        // read past the section.
        let mut bytes = build_drs_second_order(50001, None);
        let new_len = (bytes.len() - 3) as u32;
        bytes.truncate(bytes.len() - 3);
        bytes[0..4].copy_from_slice(&new_len.to_be_bytes());
        let err = parse_data_representation(&bytes).expect_err("must reject");
        assert!(
            err.to_string().contains("SPD"),
            "error names the SPD shortfall, got: {err}",
        );
    }

    #[test]
    fn template_5_50001_order_of_spd_zero_has_no_width_octet() {
        // A valid no-SPD field: orderOfSPD == 0, so widthOfSPD and the SPD seed
        // block are absent from the wire and the §5 payload ends right after
        // orderOfSPD (21 bytes). eccodes gates both behind `if (orderOfSPD)`, so
        // the parser must accept the shorter section, not demand a width octet.
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(&270.5_f32.to_be_bytes()); // R
        payload.extend_from_slice(&0u16.to_be_bytes()); // E = 0
        payload.extend_from_slice(&0u16.to_be_bytes()); // D = 0
        payload.push(16); // bitsPerValue
        payload.push(14); // widthOfFirstOrderValues
        payload.extend_from_slice(&15u32.to_be_bytes()); // numberOfGroups
        payload.extend_from_slice(&496u32.to_be_bytes()); // numberOfSecondOrderPackedValues
        payload.push(4); // widthOfWidths
        payload.push(7); // widthOfLengths
        payload.push(0); // orderOfSPD = 0 → section ends here, no widthOfSPD
        assert_eq!(payload.len(), 21, "no-SPD 5.50001 prefix is 21 bytes");

        let section_len = (11 + payload.len()) as u32;
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&section_len.to_be_bytes());
        buf.push(DRS_SECTION_NUMBER);
        buf.extend_from_slice(&496u32.to_be_bytes()); // num data points
        buf.extend_from_slice(&50001u16.to_be_bytes());
        buf.extend_from_slice(&payload);

        let drs = parse_data_representation(&buf).expect("no-SPD 5.50001 parses");
        let t = drs.second_order().expect("second-order template");
        assert_eq!(t.order_of_spd, 0);
        assert_eq!(t.width_of_spd, 0, "absent widthOfSPD defaults to 0");
        assert!(t.spd_seeds.is_empty());
        assert_eq!(t.spd_bias, 0);
        assert_eq!(t.num_groups, 15);
    }

    #[test]
    fn second_order_accessor_is_none_for_other_templates() {
        let bytes = build_drs_5_0();
        let drs = parse_data_representation(&bytes).unwrap();
        assert!(drs.second_order().is_none());
    }
}
