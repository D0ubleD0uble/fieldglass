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
    FieldglassError,
    bits::{BitReader, bits_to_bytes},
};

use crate::bds::BdsHeader;

/// Shared scaling + boustrophedonic-undo + bitmap-interleave tail, identical
/// to the general-extended decoder's. `x` is the reconstructed integer grid in
/// storage order; the result is one `Option<f64>` per grid point.
fn finalize(
    x: Vec<i64>,
    header: &BdsHeader,
    decimal_scale: i16,
    boustrophedonic: bool,
    cols: usize,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);
    let d_scale = 10f64.powi(-(decimal_scale as i32));
    let r = header.reference_value;

    let mut scaled: Vec<f64> = x
        .iter()
        .map(|v| (r + (*v as f64) * two_pow_e) * d_scale)
        .collect();

    // Undo boustrophedonic ordering (odd rows stored right-to-left) before the
    // bitmap interleave, which maps the storage stream.
    if boustrophedonic && cols > 0 {
        let n = scaled.len();
        let rows = n / cols;
        for row in (1..rows).step_by(2) {
            let start = row * cols;
            let end = start + cols;
            scaled[start..end].reverse();
        }
    }

    let result = match bitmap {
        None => {
            if scaled.len() != expected_count {
                return Err(FieldglassError::Parse(format!(
                    "second-order decoded {} values but {} expected",
                    scaled.len(),
                    expected_count
                )));
            }
            scaled.into_iter().map(Some).collect()
        }
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = scaled.into_iter();
            for present in b.iter().take(expected_count) {
                out.push(if *present { iter.next() } else { None });
            }
            out
        }
    };
    Ok(result)
}

/// Parse the shared header fields (numberOfGroups, P2, widthOfFirstOrder) and
/// validate the common bounds. Returns `(num_groups, p2, width_of_first)`.
fn common_header(
    bds: &[u8],
    header: &BdsHeader,
    expected_count: usize,
) -> Result<(usize, usize, u8), FieldglassError> {
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
    if width_of_first > 32 {
        return Err(FieldglassError::Parse(format!(
            "BDS widthOfFirstOrderValues={width_of_first} > 32"
        )));
    }
    Ok((num_groups, p2, width_of_first))
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
    if bds.len() < start + nbytes {
        return Err(FieldglassError::Parse(format!(
            "BDS too short for {what} section"
        )));
    }
    let mut reader = BitReader::new(&bds[start..start + nbytes]);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(reader.read_bits(width)?);
    }
    Ok((out, start + nbytes))
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
    cols: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("row_by_row decoder without complex_extended".into())
    })?;
    let (num_groups, _p2, width_of_first) = common_header(bds, header, expected_count)?;
    let bds_len = header.section_len as usize;

    // One group per row ⇒ numberOfGroups rows of `cols` points each.
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

    // groupWidths: numberOfGroups octets starting at byte 21.
    let gw_start = 21usize;
    if bds_len < gw_start + num_groups {
        return Err(FieldglassError::Parse(
            "BDS too short for row_by_row groupWidths".into(),
        ));
    }
    let group_widths = &bds[gw_start..gw_start + num_groups];
    for &w in group_widths {
        if w > 32 {
            return Err(FieldglassError::Parse(format!(
                "row_by_row groupWidth={w} > 32"
            )));
        }
    }

    // offsetBeforeData = 21 + numberOfGroups: first-order values, then the
    // per-group second-order residual stream.
    let fo_start = gw_start + num_groups;
    let (first_order, x_start) = read_block(
        bds,
        fo_start,
        num_groups,
        width_of_first,
        "firstOrderValues",
    )?;

    let mut so = BitReader::new(&bds[x_start..]);
    let mut x: Vec<i64> = Vec::with_capacity(expected_count);
    for g in 0..num_groups {
        let w = group_widths[g];
        let ref_val = first_order[g] as i64;
        if w == 0 {
            for _ in 0..cols {
                x.push(ref_val);
            }
        } else {
            for _ in 0..cols {
                let raw = so.read_bits(w)? as i64;
                x.push(ref_val.wrapping_add(raw));
            }
        }
    }

    finalize(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        cols,
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
    cols: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("constant_width decoder without complex_extended".into())
    })?;
    let (num_groups, p2, width_of_first) = common_header(bds, header, expected_count)?;

    // numberOfSecondOrderPackedValues is the full point count for this packing.
    if p2 != expected_count {
        return Err(FieldglassError::Parse(format!(
            "constant_width: P2={p2} != grid points {expected_count}"
        )));
    }

    // Single shared group width at octet 22 (byte 21).
    let group_width = bds[21];
    if group_width > 32 {
        return Err(FieldglassError::Parse(format!(
            "constant_width groupWidth={group_width} > 32"
        )));
    }

    // offsetBeforeData = byte 22: secondary bitmap (P2 × 1 bit, byte-aligned),
    // then first-order values, then the second-order residual stream.
    let (sec_bitmap, after_sec) = read_block(bds, 22, p2, 1, "secondaryBitmap")?;
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

    // Walk the points: each 1 bit advances to the next group's first-order
    // reference. (eccodes uses 0 when the group index runs out of range —
    // ECC-1703 — so mirror that rather than erroring.)
    let mut x: Vec<i64> = Vec::with_capacity(p2);
    let mut g: isize = -1;
    for (n, &bit) in sec_bitmap.iter().enumerate() {
        g += bit as isize;
        let ref_val = if g >= 0 && (g as usize) < num_groups {
            first_order[g as usize] as i64
        } else {
            0
        };
        let val = if group_width > 0 {
            ref_val.wrapping_add(deltas[n] as i64)
        } else {
            ref_val
        };
        x.push(val);
    }

    finalize(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        cols,
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
    cols: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse("general_grib1 decoder without complex_extended".into())
    })?;
    let (num_groups, p2, width_of_first) = common_header(bds, header, expected_count)?;
    let bds_len = header.section_len as usize;

    if p2 != expected_count {
        return Err(FieldglassError::Parse(format!(
            "general_grib1: P2={p2} != grid points {expected_count}"
        )));
    }

    // groupWidths: numberOfGroups octets starting at byte 21.
    let gw_start = 21usize;
    if bds_len < gw_start + num_groups {
        return Err(FieldglassError::Parse(
            "BDS too short for general_grib1 groupWidths".into(),
        ));
    }
    let group_widths = &bds[gw_start..gw_start + num_groups];
    for &w in group_widths {
        if w > 32 {
            return Err(FieldglassError::Parse(format!(
                "general_grib1 groupWidth={w} > 32"
            )));
        }
    }

    // offsetBeforeData = 21 + numberOfGroups: secondary bitmap, first-order
    // values, then the per-group residual stream.
    let off = gw_start + num_groups;
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
    for g in 0..num_groups {
        let start = starts[g];
        let end = if g + 1 < num_groups {
            starts[g + 1]
        } else {
            p2
        };
        let len = end - start;
        let w = group_widths[g];
        let ref_val = first_order[g] as i64;
        if w == 0 {
            for _ in 0..len {
                x.push(ref_val);
            }
        } else {
            for _ in 0..len {
                let raw = so.read_bits(w)? as i64;
                x.push(ref_val.wrapping_add(raw));
            }
        }
    }

    finalize(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        cols,
        bitmap,
        expected_count,
    )
}
