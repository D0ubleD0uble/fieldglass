//! Storage order of a scanned field — the layouts a decoder regularises before
//! anyone indexes the result as a raster.
//!
//! A GRIB message says which way it walked the grid while packing it, and one
//! of those choices changes which axis runs fastest: GRIB1's GDS octet 28 bit 3
//! and GRIB2's §3 Flag Table 3.4 bit 3 (`jPointsAreConsecutive`) both say the
//! message stores meridians rather than parallels. A consumer addressing the
//! field as `raster[j·ni + i]` needs it put back, and both format crates do
//! that at their own decode boundary — from here, so the two editions cannot
//! drift apart on what the flag means (#542, #602).
//!
//! The other choice is *boustrophedonic* ordering — alternate stored runs
//! written backwards. Two independent flags ask for it (GRIB2 §3 Flag Table 3.4
//! bit 4 on the grid, and the `boustrophedonicOrdering` bit of second-order
//! packing in both editions), and all of them undo it the same way:
//! [`reverse_alternate_runs`] over the [`StoredRuns`] the message stored.
//!
//! The *directions* the scanning mode also carries — whether `i` runs east or
//! west, whether `j` runs north or south — are not this module's business. They
//! move where a row starts, which the grid geometry already accounts for; only
//! the storage order is normalised here.

/// The runs a field was stored in — what a boustrophedonic reversal steps by.
///
/// A run is one pass of the scan: a row of a regular grid, a meridian under
/// `j`-consecutive scanning, a reduced grid's `PL[j]` points. The two shapes
/// exist because a reduced grid has no single width, which is the reason its
/// runs were left un-reversed for as long as they were (#605).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredRuns<'a> {
    /// Every run is `width` points long. `0` means the layout has no runs to
    /// speak of (a grid with no declared width, a coefficient list), and every
    /// operation on it is a no-op.
    Uniform(usize),
    /// Run `j` holds `points_per_row[j]` points — a reduced grid, whose rows
    /// narrow towards the poles.
    Ragged(&'a [u32]),
}

impl StoredRuns<'_> {
    /// The width every run shares, or `None` for a ragged layout.
    ///
    /// For callers that need a run length *before* seeing the field — GRIB1's
    /// `row_by_row` second-order packing sizes its groups by it — rather than
    /// to walk one, which [`reverse_alternate_runs`] does without asking.
    pub fn uniform_width(&self) -> Option<usize> {
        match self {
            Self::Uniform(width) => Some(*width),
            Self::Ragged(_) => None,
        }
    }
}

/// Reverse every second stored run in place, undoing boustrophedonic ordering.
///
/// Run 0 scans in the nominal direction, so the odd-indexed runs (1, 3, 5, …)
/// are the ones stored backwards. `values` must be the field in *storage*
/// order and one entry per stored point: reversing a reduced grid's row after
/// it has been widened to a raster is a different operation, because expansion
/// maps columns by longitude and the flipped row would land half a cell off.
///
/// Generic in the element so a decoder can reverse whichever array it holds at
/// that point — GRIB2 reverses the `Option<f64>` grid after the bitmap has
/// spread the present points into it, GRIB1 the plain `f64` stream before,
/// because the two editions' eccodes templates compose the two steps in
/// opposite orders.
///
/// The reversal is an involution, which is what lets the two flags that ask for
/// it compose: a message setting both the grid's alternate-row bit and the
/// packing's boustrophedonic bit gets reversed twice and comes back as stored,
/// which is what eccodes produces for the same message.
///
/// Anything the runs do not cover is left alone, and a run that does not fit
/// entirely is not covered: a [`StoredRuns::Uniform`] width of `0`, a trailing
/// partial run, and a [`StoredRuns::Ragged`] list that overruns `values` all
/// leave the tail exactly as it arrived. Reversing *part* of a run would be a
/// reordering nothing wrote, which is worse than leaving a short field short.
/// The readers cross-check `sum(PL)` against the section's own point count
/// before getting here, so this is the guard rather than a reachable layout.
pub fn reverse_alternate_runs<T>(values: &mut [T], runs: StoredRuns<'_>) {
    match runs {
        StoredRuns::Uniform(width) => {
            if width == 0 {
                return;
            }
            // Checked throughout: a run width is a `u32` widened to `usize`, so
            // on a 32-bit target a declared width near `u32::MAX` makes
            // `start + width` wrap where a 64-bit build is comfortable. The
            // point cap upstream bounds `ni · nj`, which does not bound `ni`
            // when `nj` is zero.
            let mut start = width; // first odd run
            while let Some(end) = start.checked_add(width) {
                if end > values.len() {
                    break;
                }
                values[start..end].reverse();
                let Some(next) = end.checked_add(width) else {
                    break;
                };
                start = next; // next odd run
            }
        }
        StoredRuns::Ragged(widths) => {
            let mut start = 0usize;
            for (run, &width) in widths.iter().enumerate() {
                let Some(end) = start.checked_add(width as usize) else {
                    break;
                };
                if end > values.len() {
                    break; // a run that does not fit was never written backwards
                }
                if run % 2 == 1 {
                    values[start..end].reverse();
                }
                start = end;
            }
        }
    }
}

/// Reorder a `j`-consecutive (column-major) field of `ni · nj` points into the
/// row-major raster `out[j·ni + i] = values[i·nj + j]`.
///
/// `ni` is points along a parallel and `nj` is rows, as the message declares
/// them — the flag changes which of the two runs fastest in storage, not what
/// either one counts.
///
/// Anything that is not exactly `ni · nj` long comes back untouched: both
/// readers cap and cross-check the two counts before getting here, so a
/// mismatch is a grid this cannot describe, and returning it unchanged is the
/// same answer a grid without the flag gets.
pub fn transpose_j_consecutive(values: &[Option<f64>], ni: usize, nj: usize) -> Vec<Option<f64>> {
    if ni.checked_mul(nj) != Some(values.len()) {
        return values.to_vec();
    }
    let mut out = Vec::with_capacity(values.len());
    for j in 0..nj {
        for i in 0..ni {
            out.push(values[i * nj + j]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(n: usize) -> Vec<Option<f64>> {
        (0..n).map(|k| Some(k as f64)).collect()
    }

    /// Run 0 scans forwards, so only the odd runs come back reversed.
    #[test]
    fn uniform_runs_reverse_only_the_odd_ones() {
        let mut v = field(12);
        reverse_alternate_runs(&mut v, StoredRuns::Uniform(4));
        let want: Vec<Option<f64>> = [0.0, 1.0, 2.0, 3.0, 7.0, 6.0, 5.0, 4.0, 8.0, 9.0, 10.0, 11.0]
            .map(Some)
            .into();
        assert_eq!(v, want);
    }

    /// The ragged case is the one a reduced grid needs: row `j` is `PL[j]`
    /// wide, so the reversal boundaries move down the field (#605).
    #[test]
    fn ragged_runs_reverse_each_odd_row_over_its_own_width() {
        let mut v = field(9); // rows of 2, 3, 4
        reverse_alternate_runs(&mut v, StoredRuns::Ragged(&[2, 3, 4]));
        let want: Vec<Option<f64>> = [0.0, 1.0, 4.0, 3.0, 2.0, 5.0, 6.0, 7.0, 8.0]
            .map(Some)
            .into();
        assert_eq!(v, want);
    }

    /// Reversing twice restores the field, which is what lets a message
    /// setting both boustrophedonic flags come back as stored.
    #[test]
    fn reversing_twice_is_the_identity() {
        for runs in [
            StoredRuns::Uniform(5),
            StoredRuns::Ragged(&[3, 7, 5, 9, 11]),
        ] {
            let mut v = field(35);
            reverse_alternate_runs(&mut v, runs);
            assert_ne!(v, field(35), "{runs:?} reordered nothing");
            reverse_alternate_runs(&mut v, runs);
            assert_eq!(v, field(35));
        }
    }

    /// A masked point travels with its slot rather than being filled in.
    #[test]
    fn a_masked_point_moves_with_its_run() {
        let mut v = field(6);
        v[3] = None; // row 1 = [3, 4, 5] -> [5, 4, 3]
        reverse_alternate_runs(&mut v, StoredRuns::Ragged(&[3, 3]));
        assert_eq!(v[5], None);
    }

    /// A width of zero has no runs, and a trailing partial run was never
    /// reversed on the way in — both are left exactly as they arrived.
    #[test]
    fn a_run_that_was_never_written_backwards_is_untouched() {
        let mut v = field(6);
        reverse_alternate_runs(&mut v, StoredRuns::Uniform(0));
        assert_eq!(v, field(6));
        // 2 + 2 + a 2-long tail that is not a whole 4-wide run.
        let mut v = field(6);
        reverse_alternate_runs(&mut v, StoredRuns::Uniform(4));
        assert_eq!(v, field(6));
    }

    /// A ragged run that runs off the end of the field is left alone, the same
    /// answer the uniform shape gives a trailing partial run — reversing part
    /// of a run would be a reordering nothing wrote. The readers cross-check
    /// `sum(PL)` against the section's own point count first, so this is the
    /// guard rather than a reachable layout; an out-of-bounds slice is the
    /// failure it would otherwise be.
    #[test]
    fn a_ragged_run_past_the_end_of_the_field_is_untouched() {
        // Row 0 (2 wide) fits and is even; row 1 claims 9 of the 3 points left.
        let mut v = field(5);
        reverse_alternate_runs(&mut v, StoredRuns::Ragged(&[2, 9, 4]));
        assert_eq!(v, field(5));
        // The odd row before it is still reversed: only the overrun stops.
        let mut v = field(5);
        reverse_alternate_runs(&mut v, StoredRuns::Ragged(&[1, 2, 9]));
        let want: Vec<Option<f64>> = [0.0, 2.0, 1.0, 3.0, 4.0].map(Some).into();
        assert_eq!(v, want);
        // And a width that overflows when added stops rather than wrapping.
        let mut v = field(3);
        reverse_alternate_runs(&mut v, StoredRuns::Ragged(&[1, u32::MAX]));
        assert_eq!(v, field(3));
    }

    /// A width that overflows when doubled must stop, not wrap. Widths are
    /// `u32` from the message widened to `usize`; the decoders' cap bounds
    /// `ni · nj`, which says nothing about `ni` when `nj` is zero.
    #[test]
    fn a_huge_uniform_width_does_not_overflow() {
        let mut v = field(3);
        reverse_alternate_runs(&mut v, StoredRuns::Uniform(usize::MAX));
        assert_eq!(v, field(3));
        reverse_alternate_runs(&mut v, StoredRuns::Uniform(usize::MAX / 2 + 1));
        assert_eq!(v, field(3));
    }

    /// Only the uniform shape can name a width; a reduced grid has none, which
    /// is exactly what its callers must branch on.
    #[test]
    fn only_a_uniform_layout_has_one_width() {
        assert_eq!(StoredRuns::Uniform(7).uniform_width(), Some(7));
        assert_eq!(StoredRuns::Ragged(&[2, 3]).uniform_width(), None);
    }

    /// `ni = 2`, `nj = 3`: the stored columns `[0,1,2]` and `[3,4,5]` become
    /// the rows `[0,3]`, `[1,4]`, `[2,5]`.
    #[test]
    fn columns_become_rows() {
        let out = transpose_j_consecutive(&field(6), 2, 3);
        let want: Vec<Option<f64>> = [0.0, 3.0, 1.0, 4.0, 2.0, 5.0].map(Some).into();
        assert_eq!(out, want);
    }

    /// Transposing twice with the axes swapped is the identity, which is the
    /// property the raster path relies on: nothing is dropped or duplicated.
    #[test]
    fn transposing_back_is_the_identity() {
        let original = field(35);
        let once = transpose_j_consecutive(&original, 5, 7);
        assert_eq!(transpose_j_consecutive(&once, 7, 5), original);
    }

    /// A masked point travels with its position rather than being filled in.
    #[test]
    fn a_masked_point_moves_with_its_neighbours() {
        let mut values = field(6);
        values[4] = None; // stored index 4 = column 1, row 1 = raster index 3.
        assert_eq!(transpose_j_consecutive(&values, 2, 3)[3], None);
    }

    /// A count that is not `ni · nj` describes no rectangle, so there is
    /// nothing to transpose and the field comes back as it went in. The
    /// callers cap and cross-check both counts, so this is the guard rather
    /// than a reachable layout — but an out-of-bounds index is the failure it
    /// would otherwise be.
    #[test]
    fn a_field_that_is_not_the_rectangle_is_untouched() {
        assert_eq!(transpose_j_consecutive(&field(5), 2, 3), field(5));
        assert_eq!(transpose_j_consecutive(&field(0), usize::MAX, 2), field(0));
    }
}
