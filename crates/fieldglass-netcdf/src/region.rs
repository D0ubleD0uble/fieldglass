//! The arithmetic of reading part of a variable (#939): checking a region
//! against a shape, and the contiguous runs a region occupies in a C-order
//! array.
//!
//! Both backings use it. A classic variable and an HDF5 contiguous dataset are
//! C-order arrays at an offset in the file, so a region of either is a list of
//! runs; a chunked HDF5 dataset is read by chunk instead, and needs only the
//! check.

use std::ops::Range;

use fieldglass_core::{FieldglassError, MAX_FIELD_POINTS};

/// The element count of `region` over an array of `shape`, after checking the
/// region against it: one range per axis, each inside its axis.
///
/// The count is refused past [`MAX_FIELD_POINTS`], the most one field holds,
/// which is the bound a Zarr store's region read has too: a region read
/// allocates its result, sixteen bytes an element, and the shape it is checked
/// against is a file's own numbers. A whole-variable read is bounded by its own
/// budget instead (`fieldglass_core::MAX_VARIABLE_ELEMENTS`).
///
/// A region with an empty range on any axis has no elements and is not an
/// error.
pub(crate) fn element_count(
    shape: &[u64],
    region: &[Range<u64>],
) -> Result<usize, FieldglassError> {
    if region.len() != shape.len() {
        return Err(FieldglassError::Parse(format!(
            "the variable has {} dimensions, and the region states {}",
            shape.len(),
            region.len()
        )));
    }
    let mut count = 1u64;
    for (axis, (range, &extent)) in region.iter().zip(shape).enumerate() {
        if range.start > range.end || range.end > extent {
            return Err(FieldglassError::Parse(format!(
                "region {}..{} is outside dimension {axis} of the variable (length {extent})",
                range.start, range.end
            )));
        }
        count = count.saturating_mul(range.end - range.start);
    }
    if count > MAX_FIELD_POINTS as u64 {
        return Err(FieldglassError::Parse(format!(
            "a region of {count} elements is more than the {MAX_FIELD_POINTS} one read will hold"
        )));
    }
    // Under `MAX_FIELD_POINTS`, which is a `usize`, so this is exact.
    Ok(count as usize)
}

/// The runs of `region` in a C-order array of `shape`, in the region's own C
/// order: each is `(offset, length)` in elements from the array's first.
///
/// Trailing axes the region covers whole merge into the run, so a plane of a
/// `(time, lat, lon)` array is one run and a whole array is one run. The
/// region must have passed [`element_count`], and `shape`'s element count must
/// fit a `u64` — both backings check that before they get here — so no offset
/// overflows; the arithmetic is checked regardless and an overflow is an
/// error, never a wrapped offset. An empty region has no runs.
pub(crate) fn runs(
    shape: &[u64],
    region: &[Range<u64>],
) -> Result<Vec<(u64, u64)>, FieldglassError> {
    let overflow = || FieldglassError::Parse("a region offset overflows u64".into());
    if region.iter().any(|r| r.start >= r.end) {
        return Ok(Vec::new());
    }
    let rank = shape.len();
    if rank == 0 {
        return Ok(vec![(0, 1)]);
    }
    // Row-major strides of the array.
    let mut strides = vec![1u64; rank];
    for d in (0..rank - 1).rev() {
        strides[d] = strides[d + 1]
            .checked_mul(shape[d + 1])
            .ok_or_else(overflow)?;
    }
    // The outermost axis the runs are taken along: every axis inside it is
    // covered whole, so it and they are one contiguous stretch.
    let mut inner = rank - 1;
    while inner > 0 && region[inner].start == 0 && region[inner].end == shape[inner] {
        inner -= 1;
    }
    let run = (region[inner].end - region[inner].start)
        .checked_mul(strides[inner])
        .ok_or_else(overflow)?;
    let outer = &region[..inner];
    let count: u64 = outer.iter().map(|r| r.end - r.start).product();
    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    let mut at: Vec<u64> = outer.iter().map(|r| r.start).collect();
    let base = region[inner]
        .start
        .checked_mul(strides[inner])
        .ok_or_else(overflow)?;
    loop {
        let mut offset = base;
        for (d, &a) in at.iter().enumerate() {
            offset = a
                .checked_mul(strides[d])
                .and_then(|o| offset.checked_add(o))
                .ok_or_else(overflow)?;
        }
        out.push((offset, run));
        // Next outer coordinate; done once every one wraps.
        let mut axis = at.len();
        loop {
            if axis == 0 {
                return Ok(out);
            }
            axis -= 1;
            at[axis] += 1;
            if at[axis] < outer[axis].end {
                break;
            }
            at[axis] = outer[axis].start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every element `runs` names, in order, is every element of the region in
    /// C order — checked against a walk of the region one element at a time.
    #[test]
    fn runs_name_the_region_in_c_order() {
        let shapes: &[&[u64]] = &[&[7], &[3, 4], &[2, 3, 5], &[4, 1, 3, 2]];
        for shape in shapes {
            let rank = shape.len();
            // Every region whose ranges come from a small set per axis.
            let options: Vec<Vec<Range<u64>>> = shape
                .iter()
                .map(|&n| {
                    let mut v = vec![0..n, 0..1, n - 1..n, 0..0];
                    if n > 2 {
                        v.push(1..n - 1);
                    }
                    v
                })
                .collect();
            let mut pick = vec![0usize; rank];
            loop {
                let region: Vec<Range<u64>> =
                    (0..rank).map(|d| options[d][pick[d]].clone()).collect();
                let got: Vec<u64> = runs(shape, &region)
                    .unwrap()
                    .into_iter()
                    .flat_map(|(o, n)| o..o + n)
                    .collect();
                let want = walk(shape, &region);
                assert_eq!(got, want, "shape {shape:?} region {region:?}");
                assert_eq!(
                    element_count(shape, &region).unwrap(),
                    want.len(),
                    "shape {shape:?} region {region:?}"
                );
                let mut d = rank;
                loop {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                    pick[d] += 1;
                    if pick[d] < options[d].len() {
                        break;
                    }
                    pick[d] = 0;
                }
                if pick.iter().all(|&p| p == 0) {
                    break;
                }
            }
        }
    }

    /// The region's elements, one at a time, as C-order offsets into `shape`.
    fn walk(shape: &[u64], region: &[Range<u64>]) -> Vec<u64> {
        let total: u64 = shape.iter().product();
        (0..total)
            .filter(|&i| {
                let mut rest = i;
                (0..shape.len()).rev().all(|d| {
                    let c = rest % shape[d];
                    rest /= shape[d];
                    region[d].contains(&c)
                })
            })
            .collect()
    }

    #[test]
    fn a_whole_array_and_a_plane_are_one_run() {
        assert_eq!(runs(&[4, 5, 6], &[0..4, 0..5, 0..6]).unwrap(), [(0, 120)]);
        assert_eq!(runs(&[4, 5, 6], &[2..3, 0..5, 0..6]).unwrap(), [(60, 30)]);
        assert_eq!(runs(&[], &[]).unwrap(), [(0, 1)]);
    }

    // `&[0..4]` is a region of one axis, not a mistyped `Vec` of its values.
    #[allow(clippy::single_range_in_vec_init)]
    #[test]
    fn a_region_outside_its_shape_is_refused() {
        assert!(element_count(&[4, 5], &[0..4]).is_err());
        assert!(element_count(&[4, 5], &[0..4, 0..6]).is_err());
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = [3..2, 0..5];
        assert!(element_count(&[4, 5], &backwards).is_err());
        let huge = MAX_FIELD_POINTS as u64 + 1;
        assert!(element_count(&[huge], &[0..huge]).is_err());
        assert_eq!(element_count(&[huge], &[5..9]).unwrap(), 4);
    }
}
