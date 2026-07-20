//! Matrix-of-values reshape — shared by the GRIB1 (`grid_simple_matrix`,
//! `matrixOfValues = 1`) and GRIB2 (§5.1, `matrixBitmapsPresent = 1`) matrix
//! decoders.
//!
//! Both carry an `NR × NC` matrix at every grid point delimited by *secondary
//! bitmaps*: for each present grid point, `datum = NR·NC` secondary bits mark
//! which matrix cells hold a value, and the §7/BDS payload holds one packed
//! value per set bit. The two editions differ only in where those bytes sit and
//! how the header is parsed; the reshape from `(secondary bits, packed values,
//! primary bitmap)` to the flattened `expected_count · datum` field is identical,
//! and follows the GRIBEX interpretation (the WMO secondary-bitmap sizing is
//! unusable — stock eccodes divides by zero and crashes on the GRIB2 form).

use crate::error::FieldglassError;

/// Expand a per-present-point secondary bitmap into the full
/// `expected_count · datum` value grid, pulling `coded` values where a cell is
/// present.
///
/// Walks the `expected_count` grid points in scan order: a point present in the
/// primary `bitmap` consumes its `datum` secondary bits — each set bit pulls the
/// next `coded` value, each clear bit yields `None` — while an absent point
/// contributes `datum` `None`s and consumes no secondary bits.
///
/// Errors if the coded stream runs short of the set secondary bits: every set
/// bit must consume exactly one coded value, so a shortfall means the declared
/// bitmap and the packed data disagree. Silently substituting `None` there would
/// misreport a present cell as missing and shift every later value.
pub fn expand_matrix(
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
                // Present points were sized to consume exactly `datum` secondary
                // bits each (the caller validates the secondary-bit count).
                let cell_present = sec.next().copied().unwrap_or(false);
                if cell_present {
                    let v = vals.next().ok_or_else(|| {
                        FieldglassError::Parse(
                            "matrix-of-values coded values exhausted before the secondary \
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
}
