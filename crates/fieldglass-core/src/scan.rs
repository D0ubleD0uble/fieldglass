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
//! The *directions* the scanning mode also carries — whether `i` runs east or
//! west, whether `j` runs north or south — are not this module's business. They
//! move where a row starts, which the grid geometry already accounts for; only
//! the storage order is normalised here.

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
