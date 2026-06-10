//! Matrix-of-values packing — `grid_simple_matrix`.
//!
//! Selected by BDS octet-4 flag bits `complexPacking = 0`,
//! `integerPointValues = 0`, `additionalFlagPresent = 1` (see
//! [`crate::packing::decoder_for`]). The WMO Manual on Codes provisions a
//! *matrix* of values at each grid point (e.g. ECMWF 2-D wave spectra); the
//! wire layout is eccodes' `grib1/data.grid_simple_matrix.def`.
//!
//! After the standard 11-octet BDS header the matrix sub-header is:
//!
//! | octet(s) | field |
//! |---|---|
//! | 11 | `bitsPerValue` (already in [`BdsHeader`]) |
//! | 12–13 | `octetAtWichPackedDataBegins` (u16) — repurposed as `N` when `matrixOfValues = 1` |
//! | 14 | `extendedFlag` |
//! | 15–16 | `NR` (first dimension) |
//! | 17–18 | `NC` (second dimension) |
//! | 19 | `coordinateFlag1` |
//! | 20 | `NC1` |
//! | 21 | `coordinateFlag2` |
//! | 22 | `NC2` |
//! | 23 | `physicalFlag1` |
//! | 24 | `physicalFlag2` |
//! | … | `coefsFirst[NC1]`, `coefsSecond[NC2]` — IBM floats (4 bytes each) |
//!
//! The value stream begins right after the coefficient arrays, i.e. at byte
//! offset `24 + 4·(NC1 + NC2)`.
//!
//! `extendedFlag` bit numbering in this (`grid_simple_matrix`) context differs
//! from the complex-packing context's [`crate::bds::ComplexExtendedHeader`].
//! eccodes' `data.grid_simple_matrix.def` reads `matrixOfValues` =
//! `flagbit(extendedFlag, 3)`, `secondaryBitmapPresent` =
//! `flagbit(extendedFlag, 2)`, `secondOrderOfDifferentWidth` =
//! `flagbit(extendedFlag, 1)`, where `flagbit(x, n)` tests mask `1 << n`.
//!
//! - `matrixOfValues = 0`: the body is plain simple packing (one datum per grid
//!   point) sitting behind the matrix header. This is what eccodes emits when
//!   you `grib_set packingType=grid_simple_matrix`, and is fully decoded +
//!   rendered here.
//! - `matrixOfValues = 1`: a genuine `NR×NC` matrix per grid point, delimited
//!   by secondary bitmaps. Decoded by [`decode_matrix_of_values`] and surfaced
//!   through `Grib1Reader::decode_matrix_message`; the per-grid-point scalar
//!   path ([`MatrixPacking::decode`]) rejects it since it isn't one value per
//!   point.

use fieldglass_core::{FieldglassError, bits::BitReader};

use crate::bds::BdsHeader;

use super::{Grib1Packing, interleave_with_bitmap, present_count, unpack_simple_values};

/// `matrixOfValues` — bit `1 << 3` of the matrix-context `extendedFlag`.
const MATRIX_OF_VALUES: u8 = 0x08;
/// `secondaryBitmapPresent` — bit `1 << 2`.
const SECONDARY_BITMAP_PRESENT: u8 = 0x04;
/// `secondOrderOfDifferentWidth` — bit `1 << 1`.
const SECOND_ORDER_DIFFERENT_WIDTH: u8 = 0x02;

/// Fixed length of the matrix sub-header up to (but excluding) the coefficient
/// arrays: octets 12–24 sit at byte offsets 11..=23, so the coefficients begin
/// at offset 24.
const MATRIX_HEADER_FIXED_END: usize = 24;
/// IBM single-precision float width, used for the `coefsFirst`/`coefsSecond`
/// coordinate arrays.
const COEF_WIDTH: usize = 4;

/// Parsed `grid_simple_matrix` sub-header (the octets following the standard
/// 11-octet BDS header).
#[derive(Debug, Clone, Copy)]
struct MatrixHeader {
    /// `octetAtWichPackedDataBegins`; repurposed as the secondary-bitmap count
    /// `N` (number of present grid points) when `matrix_of_values` is set.
    packed_data_begins: u16,
    extended_flag: u8,
    /// First matrix dimension.
    nr: u16,
    /// Second matrix dimension.
    nc: u16,
    /// Byte offset (within the BDS) at which the packed value stream begins
    /// (just past the `coefsFirst`/`coefsSecond` coordinate arrays).
    data_offset: usize,
}

impl MatrixHeader {
    fn matrix_of_values(self) -> bool {
        self.extended_flag & MATRIX_OF_VALUES != 0
    }
    fn secondary_bitmap_present(self) -> bool {
        self.extended_flag & SECONDARY_BITMAP_PRESENT != 0
    }
    #[allow(dead_code)] // documents the third extendedFlag bit; not needed to decode
    fn second_order_of_different_width(self) -> bool {
        self.extended_flag & SECOND_ORDER_DIFFERENT_WIDTH != 0
    }
}

/// Parse the matrix sub-header from a BDS slice. `bds` starts at the BDS length
/// octets; `section_len` is the BDS's declared length. All offsets are bounded
/// by `section_len` (not by `bds.len()`, which may run past the declared
/// section), so callers can slice `[..section_len]` without a `start > end`
/// panic. Callers must ensure `bds.len() >= section_len` beforehand.
fn parse_matrix_header(bds: &[u8], section_len: usize) -> Result<MatrixHeader, FieldglassError> {
    if section_len < MATRIX_HEADER_FIXED_END {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix section_len {section_len} below matrix-header minimum \
             {MATRIX_HEADER_FIXED_END}"
        )));
    }
    // `bds.len() >= section_len >= MATRIX_HEADER_FIXED_END` (caller invariant),
    // so octets 11..24 are in range.
    let packed_data_begins = u16::from_be_bytes([bds[11], bds[12]]);
    let extended_flag = bds[13];
    let nr = u16::from_be_bytes([bds[14], bds[15]]);
    let nc = u16::from_be_bytes([bds[16], bds[17]]);
    let nc1 = bds[19];
    let nc2 = bds[21];

    let data_offset = MATRIX_HEADER_FIXED_END + COEF_WIDTH * (nc1 as usize + nc2 as usize);
    if section_len < data_offset {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix coefficient arrays (NC1={nc1}, NC2={nc2}) overrun the \
             {section_len}-octet section"
        )));
    }

    Ok(MatrixHeader {
        packed_data_begins,
        extended_flag,
        nr,
        nc,
        data_offset,
    })
}

pub struct MatrixPacking;

impl Grib1Packing for MatrixPacking {
    fn decode(
        &self,
        bds: &[u8],
        header: &BdsHeader,
        decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        _cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let section_len = header.section_len as usize;
        if bds.len() < section_len {
            return Err(FieldglassError::Parse(format!(
                "grid_simple_matrix BDS shorter than declared section_len {section_len}"
            )));
        }
        let matrix = parse_matrix_header(bds, section_len)?;

        if matrix.matrix_of_values() {
            // A genuine NR×NC matrix at every grid point yields `NR·NC` values
            // per point, so it cannot satisfy this "one Option<f64> per grid
            // point" contract or render as a single 2-D panel. The full matrix
            // is decoded through the dedicated [`decode_matrix_of_values`] entry
            // (surfaced as `Grib1Reader::decode_matrix_message`); reject it here
            // so the scalar path stays sound.
            return Err(FieldglassError::UnsupportedSection(format!(
                "BDS uses grid_simple_matrix with matrixOfValues set \
                 (NR={}, NC={}): a per-grid-point {}×{} matrix is not a single \
                 2-D field — decode it via Grib1Reader::decode_matrix_message.",
                matrix.nr, matrix.nc, matrix.nr, matrix.nc
            )));
        }

        // matrixOfValues = 0: the body is plain simple packing behind the
        // matrix header. Constant field (bits_per_value == 0) still applies.
        let d_scale = 10f64.powi(-(decimal_scale as i32));
        let r = header.reference_value;
        let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);
        let present = present_count(bitmap, expected_count);

        if header.bits_per_value == 0 {
            let constant = r * d_scale;
            let decoded = vec![constant; present];
            return Ok(interleave_with_bitmap(decoded, bitmap, expected_count));
        }
        if header.bits_per_value > 32 {
            return Err(FieldglassError::Parse(format!(
                "grid_simple_matrix bits_per_value {} exceeds 32",
                header.bits_per_value
            )));
        }

        let packed = &bds[matrix.data_offset..section_len];
        let stored_count = (packed.len() * 8) / header.bits_per_value as usize;
        if stored_count < present {
            return Err(FieldglassError::Parse(format!(
                "grid_simple_matrix holds {stored_count} values but {present} are required"
            )));
        }

        let decoded = unpack_simple_values(
            packed,
            header.bits_per_value,
            r,
            two_pow_e,
            d_scale,
            present,
        )?;
        Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
    }
}

/// A decoded `matrixOfValues = 1` field: an `nr × nc` matrix at every grid
/// point. `values` is `expected_count · nr · nc` long, laid out grid-point
/// major (scan order) with the `nr·nc` matrix cells of each point stored
/// consecutively in their on-the-wire order; `None` marks a bitmap-masked
/// cell or grid point.
#[derive(Debug, Clone)]
pub struct MatrixValues {
    pub nr: usize,
    pub nc: usize,
    pub values: Vec<Option<f64>>,
}

/// Decode a `grid_simple_matrix` BDS with `matrixOfValues = 1`.
///
/// Mirrors eccodes' `grib1/data.grid_simple_matrix.def` matrix branch and the
/// `DataG1SecondaryBitmap`/`data_apply_bitmap` accessors. The wire layout after
/// the matrix sub-header (see [`MatrixHeader`]) is:
///
/// 1. **secondary bitmaps** — `N · (NR·NC)` bits, where `N`
///    (`octetAtWichPackedDataBegins`) is the number of present grid points.
///    For each present grid point these `NR·NC` bits mark which matrix cells
///    carry a value.
/// 2. **coded values** — simple-packed, one per set secondary-bitmap bit.
///
/// Reconstruction walks the `expected_count` grid points in scan order: a point
/// present in the primary (`bitmap`/BMS) consumes its `NR·NC` secondary bits —
/// each set bit pulls the next coded value, each clear bit yields `None` — while
/// an absent point contributes `NR·NC` `None`s and consumes no secondary bits
/// (matching eccodes' zero-fill). `N` must equal the number of present primary
/// points.
///
/// > Note: eccodes 2.34.1 can neither encode nor decode this variant (it
/// > asserts out), so there is no `grib_get_data` oracle; the decoder is
/// > validated against the eccodes *definition*/accessor source and a
/// > hand-computed fixture. See `tests/fixtures/NOTICE.md`.
pub(crate) fn decode_matrix_of_values(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<MatrixValues, FieldglassError> {
    // Confirm the message is actually grid_simple_matrix before reading the
    // matrix sub-header. Other packings repurpose octets 11–24 differently —
    // notably general-extended second-order packing, whose complex-context
    // extendedFlag bit 0x08 (`generalExtended2ordr`) collides with this
    // context's `matrixOfValues` mask — so without this guard a non-matrix
    // message could be silently mis-read as a matrix. The flag combination is
    // the same one `decoder_for` routes to `MatrixPacking`.
    if header.is_spherical_harmonic
        || header.is_complex_packing
        || header.is_integer_data
        || !header.has_extra_flags
    {
        return Err(FieldglassError::Parse(
            "message is not grid_simple_matrix packing (BDS flags: expected \
             complexPacking=0, integerPointValues=0, additionalFlagPresent=1); \
             decode_matrix_message only applies to matrix-of-values fields."
                .into(),
        ));
    }

    let section_len = header.section_len as usize;
    if bds.len() < section_len {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix BDS shorter than declared section_len {section_len}"
        )));
    }
    let matrix = parse_matrix_header(bds, section_len)?;
    if !matrix.matrix_of_values() {
        return Err(FieldglassError::Parse(
            "grid_simple_matrix message has matrixOfValues = 0 (a scalar field); \
             decode it with decode_message_values, not as a matrix."
                .into(),
        ));
    }
    // eccodes' data.grid_simple_matrix.def treats matrixOfValues = 1 as always
    // carrying secondary bitmaps (its `secondaryBitmapPresent == 0` guard is a
    // `not_implemented` error). We rely on that layout, so require the bit.
    if !matrix.secondary_bitmap_present() {
        return Err(FieldglassError::UnsupportedSection(
            "grid_simple_matrix with matrixOfValues = 1 but secondaryBitmapPresent = 0 \
             is not implemented (no defined layout in eccodes' GRIB1 templates)."
                .into(),
        ));
    }
    if header.bits_per_value == 0 || header.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix bits_per_value {} is unsupported (expected 1..=32)",
            header.bits_per_value
        )));
    }

    let datum = (matrix.nr as usize)
        .checked_mul(matrix.nc as usize)
        .filter(|d| *d > 0)
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "grid_simple_matrix datum size NR×NC = {}×{} is zero or overflows",
                matrix.nr, matrix.nc
            ))
        })?;

    // `octetAtWichPackedDataBegins` is repurposed as N, the count of present
    // grid points; it must agree with the primary bitmap.
    let n = matrix.packed_data_begins as usize;
    let present_primary = present_count(bitmap, expected_count);
    if n != present_primary {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix N (octetAtWichPackedDataBegins = {n}) disagrees with \
             {present_primary} present grid points"
        )));
    }

    // Secondary bitmaps: N·datum bits immediately after the matrix sub-header.
    let sec_count = n.checked_mul(datum).ok_or_else(|| {
        FieldglassError::Parse("grid_simple_matrix secondary-bitmap count overflows".into())
    })?;
    let sec_bytes = sec_count.div_ceil(8);
    let sec_end = matrix.data_offset.checked_add(sec_bytes).ok_or_else(|| {
        FieldglassError::Parse("grid_simple_matrix secondary-bitmap offset overflows".into())
    })?;
    if section_len < sec_end {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix secondary bitmaps ({sec_bytes} bytes) overrun the section"
        )));
    }
    let mut reader = BitReader::new(&bds[matrix.data_offset..sec_end]);
    let mut secondary = Vec::with_capacity(sec_count);
    for _ in 0..sec_count {
        secondary.push(reader.read_bits(1)? != 0);
    }

    // Coded values: one simple-packed value per set secondary bit.
    let coded_count = secondary.iter().filter(|b| **b).count();
    let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);
    let d_scale = 10f64.powi(-(decimal_scale as i32));
    let coded = unpack_simple_values(
        &bds[sec_end..section_len],
        header.bits_per_value,
        header.reference_value,
        two_pow_e,
        d_scale,
        coded_count,
    )?;

    let values = expand_matrix(&secondary, coded, bitmap, expected_count, datum)?;
    Ok(MatrixValues {
        nr: matrix.nr as usize,
        nc: matrix.nc as usize,
        values,
    })
}

/// Expand the per-present-point secondary bitmap into the full
/// `expected_count · datum` value grid, pulling `coded` values where a cell is
/// present. Factored out for direct unit testing of the bitmap walk.
///
/// Errors if the coded stream runs short of the set secondary bits: every set
/// bit must consume exactly one coded value, so a shortfall means the declared
/// bitmap and the packed data disagree. Silently substituting `None` there
/// would misreport a present cell as missing and shift every later value.
fn expand_matrix(
    secondary: &[bool],
    coded: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    datum: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let mut out = Vec::with_capacity(expected_count.saturating_mul(datum));
    let mut sec = secondary.iter();
    let mut vals = coded.into_iter();
    for p in 0..expected_count {
        let point_present = match bitmap {
            Some(b) => b.get(p).copied().unwrap_or(false),
            None => true,
        };
        for _ in 0..datum {
            if point_present {
                // Present points were sized to consume exactly `datum`
                // secondary bits each (validated via N above).
                let cell_present = sec.next().copied().unwrap_or(false);
                if cell_present {
                    let v = vals.next().ok_or_else(|| {
                        FieldglassError::Parse(
                            "grid_simple_matrix coded values exhausted before the secondary \
                             bitmap: packed data shorter than the bitmap declares"
                                .into(),
                        )
                    })?;
                    out.push(Some(v));
                } else {
                    out.push(None);
                }
            } else {
                out.push(None);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_all_present_is_sequential() {
        // 3 grid points, 2×1 matrix, every cell present → values == coded order.
        let secondary = vec![true; 6];
        let coded = vec![10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
        let out = expand_matrix(&secondary, coded, None, 3, 2).unwrap();
        assert_eq!(
            out,
            vec![
                Some(10.0),
                Some(11.0),
                Some(12.0),
                Some(13.0),
                Some(14.0),
                Some(15.0)
            ]
        );
    }

    #[test]
    fn expand_masks_clear_cells_and_absent_points() {
        // 3 points, datum 2. Primary: point 1 absent. Secondary (for the 2
        // present points, 2 cells each): present point 0 → [1,0], present
        // point 2 → [1,1]. Absent point 1 → two None, no secondary consumed.
        let secondary = vec![true, false, true, true];
        let coded = vec![100.0, 200.0, 300.0];
        let primary = [true, false, true];
        let out = expand_matrix(&secondary, coded, Some(&primary), 3, 2).unwrap();
        assert_eq!(
            out,
            vec![
                Some(100.0), // point 0, cell 0 (secondary 1)
                None,        // point 0, cell 1 (secondary 0)
                None,        // point 1 absent
                None,        // point 1 absent
                Some(200.0), // point 2, cell 0 (secondary 1)
                Some(300.0), // point 2, cell 1 (secondary 1)
            ]
        );
    }

    /// A set secondary bit with no coded value behind it means the packed data
    /// is shorter than the bitmap claims — that must error, not silently fill
    /// `None` and shift every later value by one.
    #[test]
    fn expand_errors_when_coded_runs_short() {
        // 4 set cells, but only 3 coded values supplied.
        let secondary = vec![true, true, true, true];
        let coded = vec![1.0, 2.0, 3.0];
        let err = expand_matrix(&secondary, coded, None, 2, 2).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    /// A declared section shorter than the coefficient arrays must error, not
    /// drive a `&bds[data_offset..section_len]` slice with `start > end`.
    #[test]
    fn parse_rejects_data_offset_past_section() {
        // NC1 = 4 ⇒ data_offset = 24 + 4·4 = 40, past a 24-octet section. The
        // backing slice is longer (as it can be when the IS span exceeds the
        // declared BDS length), so only the section_len bound catches it.
        let mut bds = vec![0u8; 64];
        bds[19] = 4; // NC1
        let err = parse_matrix_header(&bds, 24).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn parse_rejects_section_below_fixed_header() {
        let bds = vec![0u8; 64];
        let err = parse_matrix_header(&bds, 20).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    /// `decode_matrix_of_values` must reject a non-matrix message up front —
    /// e.g. complex/second-order packing, whose complex-context extendedFlag
    /// bit 0x08 would otherwise be misread as `matrixOfValues`.
    #[test]
    fn decode_rejects_non_matrix_packing() {
        let header = BdsHeader {
            section_len: 32,
            is_spherical_harmonic: false,
            is_complex_packing: true, // second-order, not grid_simple_matrix
            is_integer_data: false,
            has_extra_flags: true,
            unused_trailing_bits: 0,
            binary_scale_factor: 0,
            reference_value: 0.0,
            bits_per_value: 8,
            complex_extended: None,
        };
        let bds = vec![0u8; 32];
        let err = decode_matrix_of_values(&bds, &header, 0, None, 4).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }
}
