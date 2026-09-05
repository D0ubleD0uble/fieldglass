//! GRIB1 BDS "classic" (pre-ECMWF-extended) second-order packing decoders:
//! `grid_second_order_row_by_row`, `grid_second_order_constant_width`, and
//! `grid_second_order_general_grib1`.
//!
//! These are the original WMO second-order grid-point packings (WMO No. 306,
//! Manual on Codes Vol I.2, FM 92 GRIB edition 1; see also ECMWF's
//! <https://codes.ecmwf.int/grib/format/grib1/packing/3/>). They predate — and
//! are structurally distinct from — the ECMWF "general-extended" family in
//! [`super::second_order`]: there is **no spatial differencing (SPD)** and no
//! `widthOfWidths`/`widthOfLengths`/`NL` group-descriptor block. Instead the
//! grid is split into groups, each with a first-order reference value, and the
//! second-order residual within a group is packed at the group's bit width
//! (width 0 ⇒ run of constant values — the WMO run-length case).
//!
//! eccodes dispatches these three via the BDS extended-flag bits (octet 14;
//! `grib1/section.4.def`), all with `generalExtended2ordr = 0`:
//!
//! | packingType                       | secondaryBitmapPresent | secondOrderOfDifferentWidth |
//! |-----------------------------------|:----------------------:|:---------------------------:|
//! | `grid_second_order_row_by_row`    | 0 (implied)            | 1 (per-group widths)        |
//! | `grid_second_order_constant_width`| 1                      | 0 (single shared width)     |
//! | `grid_second_order_general_grib1` | 1                      | 1 (per-group widths)        |
//!
//! Wire layout common to all three (0-indexed byte offsets within the BDS;
//! `widthOfFirstOrderValues` is the octet-11 bits-per-value field; `N1`/`N2`
//! are the octet-12-13 / octet-15-16 pointers parsed in [`crate::bds`]):
//!
//! ```text
//! 0..2    section_len
//! 3       flag (complex + extra-flags bits)
//! 4..5    binaryScaleFactor E
//! 6..9    referenceValue R (IBM float)
//! 10      widthOfFirstOrderValues
//! 11..12  N1   (octet pointer to first-order values)
//! 13      extendedFlag
//! 14..15  N2   (octet pointer to second-order values)
//! 16..17  codedNumberOfFirstOrderPackedValues  (numberOfGroups, sans extraValues)
//! 18..19  numberOfSecondOrderPackedValues (P2)
//! 20      extraValues  (numberOfGroups = coded + 65536·extraValues)
//! 21..    groupWidth(s): constant_width has ONE octet here; row_by_row and
//!         general_grib1 have `numberOfGroups` octets (one width per group).
//! ↑ offsetBeforeData = 22 (constant_width) or 21 + numberOfGroups (others)
//!         constant_width / general_grib1: secondary bitmap — P2 bits marking
//!           where each new group begins, padded to a whole octet.
//!         (row_by_row omits it; the implied bitmap sets one group per row.)
//!         first-order values: numberOfGroups @ widthOfFirstOrderValues bits,
//!           padded to a whole octet.
//!         second-order values: per group, its points at the group's width.
//! ```
//!
//! Reconstruction mirrors eccodes' `DataG1SecondOrder{RowByRow,ConstantWidth,
//! General}Packing::unpack`: read the descriptors, expand each group to
//! `firstOrderValues[g] + residual` (or just `firstOrderValues[g]` for a
//! zero-width group), then `value = (R + X·2^E) / 10^D`.

use fieldglass_core::{
    FieldglassError, StoredRuns,
    bits::{BitReader, bits_to_bytes},
};

use crate::bds::BdsHeader;

/// Byte offset within the BDS where the group descriptors begin (octet 22,
/// 1-indexed), i.e. just past the 21-byte classic second-order header. For
/// constant_width this holds one `groupWidth` octet; for row_by_row and
/// general_grib1 it holds `numberOfGroups` per-group width octets.
const GROUP_DESCRIPTORS_START: usize = 21;

/// Maximum supported bit width for any packed field (eccodes' `long` widths
/// never exceed this for GRIB1, and it bounds our per-value reads).
const MAX_BIT_WIDTH: u8 = 32;

/// Reject a bit width wider than [`MAX_BIT_WIDTH`], naming the offending field.
fn check_width(width: u8, what: &str) -> Result<(), FieldglassError> {
    if width > MAX_BIT_WIDTH {
        return Err(FieldglassError::Parse(format!(
            "BDS {what}={width} > {MAX_BIT_WIDTH}"
        )));
    }
    Ok(())
}

/// The header fields shared by all three classic second-order layouts, parsed
/// and bounds-checked once by [`common_header`].
struct ClassicHeader {
    /// `codedNumberOfFirstOrderPackedValues + 65536·extraValues`.
    num_groups: usize,
    /// `numberOfSecondOrderPackedValues` (octets 19-20).
    p2: usize,
    /// `widthOfFirstOrderValues` (the octet-11 bits-per-value field).
    width_of_first: u8,
}

/// Parse and validate the header fields common to every classic second-order
/// layout (numberOfGroups, P2, widthOfFirstOrderValues).
fn common_header(
    bds: &[u8],
    header: &BdsHeader,
    expected_count: usize,
) -> Result<ClassicHeader, FieldglassError> {
    let bds_len = header.section_len as usize;
    if bds.len() < bds_len {
        return Err(FieldglassError::Parse(format!(
            "BDS body shorter than declared section_len {bds_len}"
        )));
    }
    if bds_len < 22 {
        return Err(FieldglassError::Parse(format!(
            "BDS too short ({bds_len}) for classic second-order header"
        )));
    }
    let coded = u16::from_be_bytes([bds[16], bds[17]]) as usize;
    let p2 = u16::from_be_bytes([bds[18], bds[19]]) as usize;
    let extra = bds[20] as usize;
    let num_groups = coded
        .checked_add(65536usize.saturating_mul(extra))
        .ok_or_else(|| FieldglassError::Parse("BDS num_groups overflows usize".into()))?;
    if num_groups == 0 {
        return Err(FieldglassError::Parse(
            "BDS reports zero groups for second-order packing".into(),
        ));
    }
    if num_groups > expected_count {
        return Err(FieldglassError::Parse(format!(
            "BDS reports {num_groups} groups but grid only has {expected_count} points"
        )));
    }
    let width_of_first = header.bits_per_value;
    check_width(width_of_first, "widthOfFirstOrderValues")?;
    Ok(ClassicHeader {
        num_groups,
        p2,
        width_of_first,
    })
}

/// Read `count` unsigned values of `width` bits from `bds[start..]` as a
/// byte-aligned block, returning the values and the byte offset just past it.
fn read_block(
    bds: &[u8],
    start: usize,
    count: usize,
    width: u8,
    what: &str,
) -> Result<(Vec<u32>, usize), FieldglassError> {
    let nbytes = bits_to_bytes(count, width as usize).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "BDS {what} byte length overflows ({count}×{width} bits)"
        ))
    })?;
    let end = start
        .checked_add(nbytes)
        .ok_or_else(|| FieldglassError::Parse(format!("BDS {what} offset overflows usize")))?;
    let slot = bds
        .get(start..end)
        .ok_or_else(|| FieldglassError::Parse(format!("BDS too short for {what} section")))?;
    let mut reader = BitReader::new(slot);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(reader.read_bits(width)?);
    }
    Ok((out, end))
}

/// Read the `numberOfGroups` per-group width octets at [`GROUP_DESCRIPTORS_START`]
/// (row_by_row / general_grib1), validating each width and returning the slice
/// plus the byte offset just past it (= `offsetBeforeData`).
fn read_group_widths<'a>(
    bds: &'a [u8],
    num_groups: usize,
    what: &str,
) -> Result<(&'a [u8], usize), FieldglassError> {
    let end = GROUP_DESCRIPTORS_START
        .checked_add(num_groups)
        .ok_or_else(|| {
            FieldglassError::Parse(format!("BDS {what} groupWidths offset overflows"))
        })?;
    let widths = bds
        .get(GROUP_DESCRIPTORS_START..end)
        .ok_or_else(|| FieldglassError::Parse(format!("BDS too short for {what} groupWidths")))?;
    for &w in widths {
        check_width(w, &format!("{what} groupWidth"))?;
    }
    Ok((widths, end))
}

/// Expand one group into `x`: a zero-width group is a run of `len` copies of
/// its first-order reference; otherwise read `len` residuals at `width` bits
/// from `so` and add the reference to each (the WMO run-length scheme).
fn expand_group(
    x: &mut Vec<i64>,
    so: &mut BitReader,
    width: u8,
    len: usize,
    ref_val: i64,
) -> Result<(), FieldglassError> {
    if width == 0 {
        x.resize(x.len() + len, ref_val);
    } else {
        for _ in 0..len {
            let raw = so.read_bits(width)? as i64;
            x.push(ref_val.wrapping_add(raw));
        }
    }
    Ok(())
}

/// `grid_second_order_row_by_row` (implied secondary bitmap: one group per
/// row, `numberOfGroups == numberOfRows`; per-group widths). No SPD, no stored
/// secondary bitmap.
pub fn decode_row_by_row(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    runs: StoredRuns<'_>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("row_by_row decoder without complex_extended".into())
    })?;
    let ClassicHeader {
        num_groups,
        width_of_first,
        // P2 is unused here: the point count is derived from rows × columns.
        ..
    } = common_header(bds, header, expected_count)?;

    // One group per row ⇒ numberOfGroups rows of `cols` points each. A reduced
    // grid has no single width, and eccodes' row-by-row accessor sizes group
    // `j` by `pl[j]` for one — but nothing here can check that against an
    // oracle, since eccodes 2.34.1 cannot encode this packing and no committed
    // fixture pairs it with a reduced grid. Say so and refuse, rather than
    // guess a layout (#605).
    let Some(cols) = runs.uniform_width() else {
        return Err(FieldglassError::UnsupportedSection(
            "BDS uses `grid_second_order_row_by_row` on a reduced grid, whose rows \
             differ in width; this decoder sizes its groups by a single column count"
                .into(),
        ));
    };
    if cols == 0 {
        return Err(FieldglassError::Parse(
            "row_by_row needs a non-zero column count".into(),
        ));
    }
    if num_groups.checked_mul(cols) != Some(expected_count) {
        return Err(FieldglassError::Parse(format!(
            "row_by_row: numberOfGroups {num_groups} × cols {cols} != {expected_count} points"
        )));
    }

    let (group_widths, fo_start) = read_group_widths(bds, num_groups, "row_by_row")?;

    // offsetBeforeData = 21 + numberOfGroups: first-order values, then the
    // per-group second-order residual stream.
    let (first_order, x_start) = read_block(
        bds,
        fo_start,
        num_groups,
        width_of_first,
        "firstOrderValues",
    )?;

    let mut so = BitReader::new(&bds[x_start..]);
    let mut x: Vec<i64> = Vec::with_capacity(expected_count);
    for (&w, &ref_raw) in group_widths.iter().zip(&first_order) {
        expand_group(&mut x, &mut so, w, cols, ref_raw as i64)?;
    }

    super::finalize_second_order(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        runs,
        bitmap,
        expected_count,
    )
}

/// `grid_second_order_constant_width` — an explicit secondary bitmap (P2 bits,
/// a 1 marking each new group) plus a **single** `groupWidth` octet shared by
/// every group. Mirrors eccodes' `DataG1SecondOrderConstantWidthPacking::unpack`.
pub fn decode_constant_width(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    runs: StoredRuns<'_>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("constant_width decoder without complex_extended".into())
    })?;
    let ClassicHeader {
        num_groups,
        p2,
        width_of_first,
    } = common_header(bds, header, expected_count)?;

    // numberOfSecondOrderPackedValues is the full point count for this packing.
    if p2 != expected_count {
        return Err(FieldglassError::Parse(format!(
            "constant_width: P2={p2} != grid points {expected_count}"
        )));
    }

    // A single shared group width occupies the one descriptor octet (octet 22).
    let group_width = bds[GROUP_DESCRIPTORS_START];
    check_width(group_width, "constant_width groupWidth")?;

    // offsetBeforeData = byte 22: secondary bitmap (P2 × 1 bit, byte-aligned),
    // then first-order values, then a flat second-order residual stream (one
    // residual per point, since the width is constant — no per-group lengths).
    let data_start = GROUP_DESCRIPTORS_START + 1;
    let (sec_bitmap, after_sec) = read_block(bds, data_start, p2, 1, "secondaryBitmap")?;
    let (first_order, after_fo) = read_block(
        bds,
        after_sec,
        num_groups,
        width_of_first,
        "firstOrderValues",
    )?;

    let deltas = if group_width > 0 {
        read_block(bds, after_fo, p2, group_width, "secondOrderValues")?.0
    } else {
        Vec::new()
    };

    // Walk the points, mirroring eccodes' `i += secondaryBitmap[n]`: the group
    // index starts at -1 and each 1 bit advances it to the next group's
    // first-order reference. eccodes uses 0 when the index runs out of range
    // (ECC-1703), so mirror that rather than erroring.
    let mut x: Vec<i64> = Vec::with_capacity(p2);
    let mut group: isize = -1;
    for (n, &bit) in sec_bitmap.iter().enumerate() {
        // `sec_bitmap` holds one-bit values, so the widening is exact.
        group += bit as isize;
        let ref_val = usize::try_from(group)
            .ok()
            .and_then(|g| first_order.get(g))
            .map_or(0, |&fo| fo as i64);
        let val = if group_width > 0 {
            ref_val.wrapping_add(deltas[n] as i64)
        } else {
            ref_val
        };
        x.push(val);
    }

    super::finalize_second_order(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        runs,
        bitmap,
        expected_count,
    )
}

/// `grid_second_order_general_grib1` — an explicit secondary bitmap whose 1
/// bits delimit **variable-length** groups, combined with per-group widths
/// (`groupWidths[numberOfGroups]`). Mirrors eccodes'
/// `DataG1SecondOrderGeneralPacking::unpack`: each group's length is the run
/// from its leading 1 bit up to the next one (a sentinel 1 closes the last
/// group); a zero-width group is a run of its constant first-order value.
pub fn decode_general(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    runs: StoredRuns<'_>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("general_grib1 decoder without complex_extended".into())
    })?;
    let ClassicHeader {
        num_groups,
        p2,
        width_of_first,
    } = common_header(bds, header, expected_count)?;

    if p2 != expected_count {
        return Err(FieldglassError::Parse(format!(
            "general_grib1: P2={p2} != grid points {expected_count}"
        )));
    }

    let (group_widths, off) = read_group_widths(bds, num_groups, "general_grib1")?;

    // offsetBeforeData = 21 + numberOfGroups: secondary bitmap, first-order
    // values, then the per-group residual stream.
    let (sec_bitmap, after_sec) = read_block(bds, off, p2, 1, "secondaryBitmap")?;
    let (first_order, x_start) = read_block(
        bds,
        after_sec,
        num_groups,
        width_of_first,
        "firstOrderValues",
    )?;

    // Group starts are the 1 bits; group g spans starts[g]..starts[g+1] (P2
    // closes the last). A well-formed message has exactly numberOfGroups of
    // them, the first at index 0.
    let starts: Vec<usize> = sec_bitmap
        .iter()
        .enumerate()
        .filter_map(|(n, &b)| (b == 1).then_some(n))
        .collect();
    if starts.len() != num_groups {
        return Err(FieldglassError::Parse(format!(
            "general_grib1: secondary bitmap has {} group starts but numberOfGroups={num_groups}",
            starts.len()
        )));
    }
    if starts.first() != Some(&0) {
        return Err(FieldglassError::Parse(
            "general_grib1: secondary bitmap does not start a group at index 0".into(),
        ));
    }

    let mut so = BitReader::new(&bds[x_start..]);
    let mut x: Vec<i64> = Vec::with_capacity(p2);
    for (g, &start) in starts.iter().enumerate() {
        let end = starts.get(g + 1).copied().unwrap_or(p2);
        let len = end - start;
        expand_group(&mut x, &mut so, group_widths[g], len, first_order[g] as i64)?;
    }

    super::finalize_second_order(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        runs,
        bitmap,
        expected_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bds::ComplexExtendedHeader;

    /// A 22-octet `row_by_row` section: two groups, 8-bit first-order values,
    /// and nothing after the header. Enough to reach the layout check and no
    /// further, which is what these two assertions are about.
    fn stub_row_by_row() -> (Vec<u8>, BdsHeader) {
        let mut bds = vec![0u8; 22];
        bds[0..3].copy_from_slice(&[0, 0, 22]);
        bds[17] = 2; // codedNumberOfGroups = 2
        let header = BdsHeader {
            section_len: 22,
            is_spherical_harmonic: false,
            is_complex_packing: true,
            is_integer_data: false,
            has_extra_flags: false,
            unused_trailing_bits: 0,
            binary_scale_factor: 0,
            reference_value: 0.0,
            bits_per_value: 8,
            spherical_extended: None,
            complex_extended: Some(ComplexExtendedHeader {
                n1: 0,
                // secondOrderOfDifferentWidth, no secondary bitmap, no
                // general-extended: `grid_second_order_row_by_row`.
                extended_flag: 0x10,
            }),
        };
        (bds, header)
    }

    /// `row_by_row` sizes its groups by one column count, so a reduced grid —
    /// whose rows differ in width — is refused by name rather than decoded on a
    /// guessed layout. eccodes sizes group `j` by `pl[j]` for one, but it
    /// cannot encode this packing, so there is no oracle here to check that
    /// against; #611 tracks closing the gap with a hand-built fixture.
    #[test]
    fn row_by_row_refuses_a_reduced_grid() {
        let (bds, header) = stub_row_by_row();
        let err = decode_row_by_row(&bds, &header, 0, None, 6, StoredRuns::Ragged(&[2, 4]))
            .expect_err("a reduced grid has no single column count");
        match err {
            FieldglassError::UnsupportedSection(msg) => assert!(
                msg.contains("row_by_row") && msg.contains("reduced grid"),
                "error should name the packing and the grid, got: {msg}"
            ),
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    /// The same stub with a uniform layout gets *past* that check and fails
    /// later, on group descriptors the section does not carry — so the refusal
    /// above is about the grid's shape, not about the stub being a stub.
    #[test]
    fn the_same_section_with_a_uniform_layout_reaches_the_group_descriptors() {
        let (bds, header) = stub_row_by_row();
        let err = decode_row_by_row(&bds, &header, 0, None, 6, StoredRuns::Uniform(3))
            .expect_err("the stub carries no group widths");
        assert!(
            matches!(err, FieldglassError::Parse(_)),
            "expected a Parse error past the layout check, got {err:?}"
        );
    }
}
