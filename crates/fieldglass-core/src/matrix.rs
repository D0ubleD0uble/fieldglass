//! Functions that reshape fields holding a small matrix of values at every
//! grid point.
//!
//! Some GRIB fields store an `NR × NC` matrix of values at each grid point
//! rather than one value, such as a wave spectrum with one value per direction
//! and frequency. GRIB1 calls this matrix of values, and GRIB2 uses template
//! 5.1. Not every cell of a matrix has a value, so each present grid point
//! carries a secondary bitmap of `NR·NC` bits marking its filled cells, and
//! the data holds one packed value per filled cell. The editions store the
//! headers differently but lay the values out the same way, so both readers
//! share this code.
//!
//! [`expand_matrix`] lays the values out as one long list, every cell of every
//! point's matrix in order, with `None` wherever there is no value. It spreads
//! the values with the same proved rule every other GRIB decoder uses,
//! [`interleave_with_bitmap`]. If the bitmaps and the number of values don't
//! agree, it fails rather than shifting later values into the wrong cells.
//!
//! [`matrix_cell_count`] works out how many cells that list will have, and
//! fails past [`MAX_FIELD_POINTS`]. A damaged file can claim a huge matrix at
//! every point with every point missing, which passes every other size check. This keeps a few kilobytes of file from asking for
//! terabytes of memory.
//!
//! The layout follows ECMWF's GRIBEX. The WMO's own rule for sizing the
//! secondary bitmaps can't be used as written: eccodes divides by zero and
//! crashes on the GRIB2 form.

use std::borrow::Cow;

use crate::array::MAX_FIELD_POINTS;
use crate::bitmap::{count_present, interleave_with_bitmap};
use crate::error::FieldglassError;

/// The number of cells in a flattened `expected_count · datum` matrix field,
/// or an error when it overflows or exceeds [`MAX_FIELD_POINTS`].
///
/// The flattened field holds a cell (a `None` at least) for every point of
/// every matrix, present or not, so this is what the decode allocates. Both
/// factors come from the file: `datum = NR·NC` is two `u16`s, and a primary
/// bitmap can mark every point absent, which leaves the secondary bitmaps and
/// the packed stream empty while `datum` stays huge. Every size check on the
/// packed bytes then passes, so this cap is the only thing between a
/// kilobyte-sized message and a multi-terabyte allocation.
///
/// [`expand_matrix`] checks it before it allocates. A decoder should also call
/// it as soon as it knows `datum`, before it unpacks the secondary bitmaps, so
/// the two editions reject the same messages at the same step.
pub fn matrix_cell_count(expected_count: usize, datum: usize) -> Result<usize, FieldglassError> {
    expected_count
        .checked_mul(datum)
        .filter(|&n| n <= MAX_FIELD_POINTS)
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "matrix-of-values: {expected_count} grid points × {datum} cells (NR·NC) \
                 exceeds the {MAX_FIELD_POINTS}-cell cap"
            ))
        })
}

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
/// Errors when the flattened field would exceed [`matrix_cell_count`]'s cap,
/// before anything is allocated, and unless the lengths agree: `bitmap` has `expected_count` bits,
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
    let cell_count = matrix_cell_count(expected_count, datum)?;
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
            let mut cells = Vec::with_capacity(cell_count);
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

    /// An all-absent primary bitmap leaves the secondary bitmaps and the coded
    /// stream empty, so every length check passes while `datum` is huge. The
    /// cap must reject that before the cell bitmap is allocated (#802): without
    /// it this call asks for about 4 TB and aborts the process.
    #[test]
    fn expand_rejects_a_field_past_the_cap_before_allocating() {
        let datum = usize::from(u16::MAX) * usize::from(u16::MAX);
        let primary = vec![false; 1_000];
        let err = expand_matrix(&[], Vec::new(), Some(&primary), primary.len(), datum).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("cell cap")),
            "got {err:?}"
        );
    }

    /// The cap is inclusive: a field of exactly `MAX_FIELD_POINTS` cells is
    /// accepted, one more is not, and an overflowing product is an error rather
    /// than a wrapped count.
    #[test]
    fn matrix_cell_count_is_bounded_by_the_field_cap() {
        assert_eq!(
            matrix_cell_count(MAX_FIELD_POINTS / 4, 4).unwrap(),
            MAX_FIELD_POINTS
        );
        assert!(matrix_cell_count(MAX_FIELD_POINTS + 1, 1).is_err());
        assert!(matrix_cell_count(usize::MAX, 2).is_err());
        assert_eq!(matrix_cell_count(0, usize::MAX).unwrap(), 0);
    }
}
