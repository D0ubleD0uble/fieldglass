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

use crate::drs::{MatrixSimplePackingTemplate, red_scale};
use fieldglass_core::{FieldglassError, bits::BitReader};

/// Upper bound on the total matrix-cell count (`Ni·Nj·NR·NC`) the decoder will
/// allocate. `NR`/`NC` are attacker-controlled `u16`s, and a §6 bitmap can drop
/// most grid points while leaving a huge `datum = NR·NC`, so the flattened
/// output (which still holds a `None` per masked cell) is capped here — matching
/// the grid-point envelope the scalar reader accepts. Real wave-spectra matrices
/// are orders of magnitude below this.
const MAX_MATRIX_CELLS: usize = 200_000_000;

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
    debug_assert!(
        bitmap.is_none_or(|b| b.len() == expected_count),
        "primary bitmap length must equal the grid-point count"
    );
    let datum = (t.nr as usize)
        .checked_mul(t.nc as usize)
        .filter(|d| *d > 0)
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "grid_simple_matrix datum size NR×NC = {}×{} is zero or overflows",
                t.nr, t.nc
            ))
        })?;
    // Match the GRIB1 true-matrix decoder, which requires 1..=32 (a constant
    // field, bits == 0, is not a defined layout here) — the two editions must
    // decode the same input domain identically.
    if t.bits_per_value == 0 || t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix bits_per_value {} is unsupported (expected 1..=32)",
            t.bits_per_value
        )));
    }
    // The flattened output has `expected_count · datum` cells (a `None` even for
    // masked cells / absent points), so bound it before `expand_matrix`
    // allocates — a §6 bitmap could leave `present` tiny while `datum` is huge.
    if expected_count
        .checked_mul(datum)
        .filter(|&n| n <= MAX_MATRIX_CELLS)
        .is_none()
    {
        return Err(FieldglassError::Parse(format!(
            "grid_simple_matrix field {expected_count}×(NR·NC={datum}) exceeds the \
             {MAX_MATRIX_CELLS}-cell cap"
        )));
    }

    // Present grid points drive the secondary-bitmap length.
    let present = match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
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
    let mut reader = BitReader::new(&ds_payload[..sec_bytes]);
    let mut secondary = Vec::with_capacity(sec_count);
    for _ in 0..sec_count {
        secondary.push(reader.read_bits(1)? != 0);
    }
    let coded_count = secondary.iter().filter(|b| **b).count();
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
    let (r, two_pow_e, d_inv) = red_scale(
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
    let mut cr = BitReader::new(coded_bytes);
    let mut coded = Vec::with_capacity(coded_count);
    for _ in 0..coded_count {
        let x = cr.read_bits(t.bits_per_value)? as f64;
        coded.push((r + x * two_pow_e) * d_inv);
    }

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
