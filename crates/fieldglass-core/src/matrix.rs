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

use std::borrow::Cow;

use crate::bitmap::{count_present, interleave_with_bitmap};
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
/// That is one bitmap over the cells: the primary bit of each point widened to
/// its `datum` cells, and the secondary bits written into the cells of the
/// present points. The values are then spread over it by
/// [`interleave_with_bitmap`], the proved rule every other GRIB decoder
/// spreads its values with.
///
/// Errors unless the lengths agree: `bitmap` has `expected_count` bits,
/// `secondary` has `datum` bits per present point, and `coded` has one value
/// per set secondary bit. A shortfall means the declared bitmaps and the packed
/// data disagree, and silently substituting `None` there would misreport a
/// present cell as missing and shift every later value.
pub fn expand_matrix(
    secondary: &[bool],
    coded: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    datum: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_points = match bitmap {
        Some(b) if b.len() != expected_count => {
            return Err(FieldglassError::Parse(format!(
                "matrix-of-values: primary bitmap has {} bits for {expected_count} grid points",
                b.len()
            )));
        }
        Some(b) => count_present(b),
        None => expected_count,
    };
    if present_points.checked_mul(datum) != Some(secondary.len()) {
        return Err(FieldglassError::Parse(format!(
            "matrix-of-values: {} secondary bits for {present_points} present points of \
             {datum} cells each",
            secondary.len()
        )));
    }
    let cells: Cow<'_, [bool]> = match bitmap {
        None => Cow::Borrowed(secondary),
        Some(b) => {
            let mut cells = Vec::with_capacity(expected_count.saturating_mul(datum));
            let mut next = 0;
            for &point_present in b {
                if point_present {
                    // `secondary` holds `datum` bits per present point (checked
                    // above), so this stays in bounds.
                    cells.extend_from_slice(&secondary[next..next + datum]);
                    next += datum;
                } else {
                    cells.resize(cells.len() + datum, false);
                }
            }
            Cow::Owned(cells)
        }
    };
    interleave_with_bitmap(&coded, &cells).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "matrix-of-values: {} coded values but the secondary bitmaps set {} cells",
            coded.len(),
            count_present(&cells)
        ))
    })
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

    /// One coded value per set secondary bit: a surplus is the same
    /// disagreement as a shortfall, not values to drop.
    #[test]
    fn expand_errors_when_coded_runs_long() {
        let secondary = vec![true, false, true, true];
        let coded = vec![1.0, 2.0, 3.0, 4.0];
        let primary = [true, false, true];
        let err = expand_matrix(&secondary, coded, Some(&primary), 3, 2).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    /// `datum` secondary bits per present point, and one primary bit per grid
    /// point: any other length is an error rather than cells read as absent.
    #[test]
    fn expand_errors_when_a_bitmap_has_the_wrong_length() {
        let primary = [true, false, true];
        let short = vec![true, true, true];
        let err = expand_matrix(&short, vec![1.0; 3], Some(&primary), 3, 2).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
        let err = expand_matrix(&[true; 4], vec![1.0; 4], Some(&primary[..2]), 3, 2).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
        let err = expand_matrix(&[true; 5], vec![1.0; 5], None, 3, 2).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }
}
