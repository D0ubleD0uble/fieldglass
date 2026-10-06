//! GRIB2 matrix-of-values decode — template 5.1 with `matrixBitmapsPresent = 1`.
//!
//! An `NR × NC` matrix at every grid point, delimited by secondary bitmaps.
//! Stock eccodes cannot handle this variant — it divides by zero and crashes
//! on the WMO secondary-bitmap sizing — so, following the same GRIBEX
//! interpretation the GRIB1 `grid_simple_matrix` decoder uses, §7 is laid out
//! as `[N·datum secondary bits, byte-aligned][simple-packed coded values]`,
//! where `N` is the count of present grid points (from the §6 primary bitmap)
//! and `datum = NR·NC`. Each set secondary bit consumes one packed value; the
//! reshape into the flattened `expected_count · datum` field is the shared
//! [`fieldglass_core::matrix::expand_matrix`], the same code GRIB1 uses.
//!
//! Not one value per grid point, so this has its own entry point
//! (`Grib2Reader::decode_matrix_message`) and the scalar `decode_message_values`
//! path rejects it — mirroring the GRIB1 matrix path.

use crate::drs::{MatrixSimplePackingTemplate, packing_scaling};
use fieldglass_core::FieldglassError;
use fieldglass_core::bitmap::{count_present, unpack_bitmap};
use fieldglass_core::scaling::unpack_simple;

/// Decode the §7 payload of a template-5.1 `matrixBitmapsPresent = 1` message
/// into the flattened `expected_count · (NR·NC)` matrix field. `bitmap` is the
/// decoded §6 primary bitmap (present grid points), or `None` when every point
/// is present.
pub fn decode_matrix_of_values(
    ds_payload: &[u8],
    t: &MatrixSimplePackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    // Match the GRIB1 true-matrix decoder, which requires 1..=32 (a constant
    // field, bits == 0, is not a defined layout here) — the two editions must
    // decode the same input domain identically, and check it in the same
    // order: bits per value, then NR·NC, then the bitmap (#846).
    if t.bits_per_value == 0 || t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix bits_per_value {} is unsupported (expected 1..=32)",
            t.bits_per_value
        )));
    }
    let datum = (t.nr as usize)
        .checked_mul(t.nc as usize)
        .filter(|d| *d > 0)
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "grid_simple_matrix datum size NR×NC = {}×{} is zero or overflows",
                t.nr, t.nc
            ))
        })?;
    // A public entry point, so the bitmap is a caller's as much as the file's:
    // one bit per grid point, or a parse error with the wording every other
    // decoder in both editions uses, at the step GRIB1's matrix decoder checks
    // it (#824). It was a `debug_assert!`, a panic in a debug build.
    if let Some(b) = bitmap
        && b.len() != expected_count
    {
        return Err(FieldglassError::Parse(format!(
            "bitmap length {} != grid-point count {expected_count}",
            b.len()
        )));
    }
    // The flattened output has `expected_count · datum` cells (a `None` even for
    // masked cells / absent points), and `NR`/`NC` are file-declared `u16`s: a
    // §6 bitmap could leave `present` tiny while `datum` is huge. Bound it now,
    // before the secondary bitmaps are unpacked, by the rule `expand_matrix`
    // applies again before it allocates — the same step GRIB1 checks it at.
    fieldglass_core::matrix::matrix_cell_count(expected_count, datum)?;

    // Present grid points drive the secondary-bitmap length.
    let present = match bitmap {
        Some(b) => count_present(b),
        None => expected_count,
    };
    let sec_count = present.checked_mul(datum).ok_or_else(|| {
        FieldglassError::Parse("grid_simple_matrix secondary-bitmap count overflows".into())
    })?;
    let sec_bytes = sec_count.div_ceil(8);
    if ds_payload.len() < sec_bytes {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix secondary bitmaps ({sec_bytes} bytes) overrun the {}-byte §7",
            ds_payload.len()
        )));
    }

    // Secondary bitmaps: N·datum bits, then the coded values start byte-aligned.
    let secondary = unpack_bitmap(&ds_payload[..sec_bytes], sec_count).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "grid_simple_matrix secondary bitmaps: {sec_bytes} bytes hold fewer than {sec_count} bits"
        ))
    })?;
    let coded_count = count_present(&secondary);
    // §5.1 declares numberOfCodedValues — the §7 packed count. It must equal the
    // set-bit total, or the header and the secondary bitmaps disagree. (GRIB1
    // makes the analogous cross-check of its redundant present-point count.)
    if t.number_of_coded_values as usize != coded_count {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix declares numberOfCodedValues={} but the secondary bitmaps set \
             {coded_count} cells",
            t.number_of_coded_values
        )));
    }

    // One simple-packed value per set secondary bit: `(R + X·2^E)·10^-D`.
    let coded_bytes = &ds_payload[sec_bytes..];
    let scaling = packing_scaling(
        t.reference_value,
        t.binary_scale_factor,
        t.decimal_scale_factor,
    );
    let available_bits = coded_bytes.len().saturating_mul(8);
    let required_bits = coded_count.saturating_mul(t.bits_per_value as usize);
    if required_bits > available_bits {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix needs {required_bits} coded bits but §7 holds only {available_bits}"
        )));
    }
    let coded = unpack_simple(coded_bytes, t.bits_per_value, &scaling, coded_count)?;

    fieldglass_core::matrix::expand_matrix(&secondary, coded, bitmap, expected_count, datum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn template(
        nr: u16,
        nc: u16,
        r: f32,
        e: i16,
        d: i16,
        bits: u8,
        coded: u32,
    ) -> MatrixSimplePackingTemplate {
        MatrixSimplePackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            matrix_bitmaps_present: 1,
            number_of_coded_values: coded,
            nr,
            nc,
            first_dim_coordinate_definition: 0,
            second_dim_coordinate_definition: 0,
            first_dim_physical_significance: 0,
            second_dim_physical_significance: 0,
            coefficients_first: vec![],
            coefficients_second: vec![],
        }
    }

    /// Build a §7 payload: `secondary` bits (byte-aligned), then each set bit's
    /// value packed at `bits` MSB-first (values as raw scaled integers X).
    fn ds_payload(secondary: &[bool], coded_x: &[u32], bits: u8) -> Vec<u8> {
        let mut out = vec![0u8; secondary.len().div_ceil(8)];
        for (i, &b) in secondary.iter().enumerate() {
            if b {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let base = out.len();
        out.resize(base + (coded_x.len() * bits as usize).div_ceil(8), 0);
        let mut bit = base * 8;
        for &x in coded_x {
            for k in (0..bits).rev() {
                if (x >> k) & 1 != 0 {
                    out[bit / 8] |= 0x80 >> (bit % 8);
                }
                bit += 1;
            }
        }
        out
    }

    #[test]
    fn all_present_matrix_reshapes_in_order() {
        // 2 grid points, NR=1×NC=2 (datum 2), every cell present, R=0/E=0/D=0,
        // 8-bit. Coded X = [10,20,30,40] → value == X.
        let t = template(1, 2, 0.0, 0, 0, 8, 4);
        let ds = ds_payload(&[true; 4], &[10, 20, 30, 40], 8);
        let out = decode_matrix_of_values(&ds, &t, None, 2).unwrap();
        assert_eq!(out, vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0)]);
    }

    #[test]
    fn masked_cells_and_absent_point() {
        // 3 points, datum 2. Primary bitmap: point 1 absent. Secondary for the 2
        // present points: [1,0] then [1,1]. Coded X = [100,200,300].
        let t = template(2, 1, 0.0, 0, 0, 12, 3);
        let ds = ds_payload(&[true, false, true, true], &[100, 200, 300], 12);
        let primary = [true, false, true];
        let out = decode_matrix_of_values(&ds, &t, Some(&primary), 3).unwrap();
        assert_eq!(
            out,
            vec![Some(100.0), None, None, None, Some(200.0), Some(300.0)]
        );
    }

    #[test]
    fn reference_and_scale_applied() {
        // R=10, E=1, D=1 → value = (10 + X·2)·10^-1. X=5 → 2.0.
        let t = template(1, 1, 10.0, 1, 1, 8, 1);
        let ds = ds_payload(&[true], &[5], 8);
        let out = decode_matrix_of_values(&ds, &t, None, 1).unwrap();
        assert!((out[0].unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_bits_zero() {
        // Match GRIB1: bits_per_value == 0 is not a defined true-matrix layout.
        let t = template(1, 2, 7.0, 0, 0, 0, 2);
        let ds = ds_payload(&[true, true], &[], 0);
        let err = decode_matrix_of_values(&ds, &t, None, 1).unwrap_err();
        assert!(format!("{err:?}").contains("1..=32"), "got {err:?}");
    }

    #[test]
    fn rejects_coded_count_mismatch() {
        // numberOfCodedValues disagrees with the set-bit total.
        let t = template(1, 2, 0.0, 0, 0, 8, 99);
        let ds = ds_payload(&[true; 4], &[10, 20, 30, 40], 8);
        let err = decode_matrix_of_values(&ds, &t, None, 2).unwrap_err();
        assert!(
            format!("{err:?}").contains("numberOfCodedValues"),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_short_coded_stream() {
        // 4 set cells but only 2 coded values' worth of bytes.
        let t = template(1, 2, 0.0, 0, 0, 8, 4);
        let mut ds = ds_payload(&[true; 4], &[1, 2, 3, 4], 8);
        ds.truncate(ds.len() - 2); // drop 2 coded bytes
        assert!(decode_matrix_of_values(&ds, &t, None, 2).is_err());
    }

    #[test]
    fn rejects_zero_datum() {
        let t = template(0, 2, 0.0, 0, 0, 8, 0);
        assert!(decode_matrix_of_values(&[0u8; 8], &t, None, 2).is_err());
    }

    /// NR = 0 with 0 bits per value breaks two rules, and both editions report
    /// the same one: bits per value is checked first, then NR·NC (#846). The
    /// GRIB1 half is `tests/decode_matrix.rs` in `fieldglass-grib1`.
    #[test]
    fn checks_bits_per_value_before_the_datum_as_grib1_does() {
        let both = template(0, 2, 0.0, 0, 0, 0, 0);
        let err = decode_matrix_of_values(&[0u8; 8], &both, None, 2).unwrap_err();
        assert_eq!(
            err.to_string(),
            FieldglassError::Parse(
                "grid_simple_matrix bits_per_value 0 is unsupported (expected 1..=32)".into()
            )
            .to_string()
        );
        let datum_only = template(0, 2, 0.0, 0, 0, 8, 0);
        let err = decode_matrix_of_values(&[0u8; 8], &datum_only, None, 2).unwrap_err();
        assert!(err.to_string().contains("datum size NR×NC = 0×2"), "{err}");
    }

    /// A bitmap that is not one bit per grid point is a parse error with the
    /// wording every other decoder in both editions uses (#824). It used to be
    /// a `debug_assert!`: a panic in a debug build, and in a release build a
    /// secondary-bitmap count sized from the wrong bitmap.
    #[test]
    fn a_bitmap_that_is_not_one_bit_per_point_is_refused() {
        let t = template(1, 2, 0.0, 0, 0, 8, 0);
        for len in [1, 3] {
            let bitmap = vec![true; len];
            let err = decode_matrix_of_values(&[0u8; 8], &t, Some(&bitmap), 2).unwrap_err();
            assert!(
                matches!(&err, FieldglassError::Parse(m)
                    if *m == format!("bitmap length {len} != grid-point count 2")),
                "got {err:?}"
            );
        }
    }

    /// The bitmap is checked where GRIB1 checks it, before the cell cap, so a
    /// wrong-length bitmap on an oversized matrix names the bitmap in both
    /// editions.
    #[test]
    fn a_wrong_length_bitmap_is_named_before_the_cell_cap() {
        let t = template(u16::MAX, u16::MAX, 0.0, 0, 0, 8, 0);
        let err = decode_matrix_of_values(&[0u8; 8], &t, Some(&[true; 3]), 2).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m)
                if m == "bitmap length 3 != grid-point count 2"),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_oversized_matrix_before_allocating() {
        // NR·NC = 65535² ≈ 4.3e9; even 2 grid points blow past the cell cap. A
        // §6 bitmap making `present` tiny must not let this reach the big alloc.
        let t = template(u16::MAX, u16::MAX, 0.0, 0, 0, 8, 0);
        let present = [true, false]; // 1 present point
        let err = decode_matrix_of_values(&[0u8; 8], &t, Some(&present), 2).unwrap_err();
        assert!(format!("{err:?}").contains("cell cap"), "got {err:?}");
    }
}
