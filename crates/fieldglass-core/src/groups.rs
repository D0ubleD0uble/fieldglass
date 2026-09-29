//! GRIB group expansion: a run of grid points stored as groups, each a
//! reference value plus one offset per point at the group's own bit width.
//!
//! Point `k` of a group is `reference + X_k`, where `X_k` is the `k`-th
//! `width`-bit field of the group's offsets; a group of width 0 stores no
//! offsets, and every point in it is the reference. That one rule serves every
//! grouped GRIB packing:
//!
//! - GRIB1 second-order packing, both the classic layouts (`row_by_row`,
//!   `general_grib1`) and the extended SPD one, and GRIB2 templates 5.50001 and
//!   5.50002, which share the extended codec: `expand_group_into` and
//!   `expand_groups_into`;
//! - GRIB2 complex packing (templates 5.2 and 5.3), whose group tables are laid
//!   out differently and which can mark a point missing with an all-ones
//!   offset: `expand_complex_groups`.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes and the `verus_keep_ghost`
//! items at the bottom, which a normal build never sees. What is proved, and
//! what is trusted rather than proved, is listed in `docs/verification.md`.
//!
//! The file names only items both crates provide (`crate::FieldglassError`,
//! `crate::bits::BitReader`, and, under Verus only, `crate::bits_model`), and
//! its docs use plain backticks rather than intra-doc links.
//! `tools/check_verified_kernels.py` fails if the verification crate stops
//! including it.

use crate::FieldglassError;
use crate::bits::BitReader;
#[cfg(verus_keep_ghost)]
use crate::bits_model::{align8, msb_bits, reader_bytes, reader_pos};
#[cfg(verus_keep_ghost)]
use vstd::arithmetic::power2::pow2;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Append one group's points to `out`: `len` copies of `reference` when
/// `width` is 0, else `reference + X` for each of the next `len` `width`-bit
/// fields of `reader`.
///
/// A width above 32 is an error whatever `len` is: the bit reader returns a
/// `u32`, so such a group is malformed. `reference` and each field are
/// unsigned 32-bit values, so their sum always fits an `i64`.
///
/// Proved (see the module docs): `Ok` exactly when the width is at most 32 and
/// the reader holds the group's fields; then `out` keeps what it held, gains
/// `len` values, the `k`-th being `reference + X_k`, and the reader has moved
/// past exactly `len · width` bits.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r is Ok <==> group_fits(
            reader_bytes(*old(reader)).len() as int,
            reader_pos(*old(reader)),
            width as int,
            len as int,
        ),
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + len * width,
        r is Ok ==> final(out)@.len() == old(out)@.len() + len,
        r is Ok ==> forall|i: int|
            0 <= i < old(out)@.len() ==> #[trigger] final(out)@[i] == old(out)@[i],
        r is Ok ==> forall|k: int|
            0 <= k < len ==> #[trigger] final(out)@[old(out)@.len() + k] == group_point(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)),
                width as int,
                reference as int,
                k,
            ),
))]
pub fn expand_group_into(
    reader: &mut BitReader,
    width: u8,
    len: usize,
    reference: u32,
    out: &mut Vec<i64>,
) -> Result<(), FieldglassError> {
    if width > 32 {
        return Err(group_too_wide(width));
    }
    let reference = reference as i64;
    if width == 0 {
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                out@.len() == old(out)@.len() + it.index@,
                forall|i: int| 0 <= i < old(out)@.len() ==> #[trigger] out@[i] == old(out)@[i],
                forall|k: int| 0 <= k < it.index@ ==> #[trigger] out@[old(out)@.len() + k] == reference,
        ))]
        for _ in 0..len {
            out.push(reference);
        }
    } else {
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                1 <= width <= 32,
                0 <= reference <= u32::MAX,
                reader_bytes(*reader) == reader_bytes(*old(reader)),
                reader_pos(*reader) == reader_pos(*old(reader)) + it.index@ * width,
                group_fits(
                    reader_bytes(*old(reader)).len() as int,
                    reader_pos(*old(reader)),
                    width as int,
                    it.index@ as int,
                ),
                out@.len() == old(out)@.len() + it.index@,
                forall|i: int| 0 <= i < old(out)@.len() ==> #[trigger] out@[i] == old(out)@[i],
                forall|k: int| 0 <= k < it.index@ ==> #[trigger] out@[old(out)@.len() + k] == group_point(
                    reader_bytes(*old(reader)),
                    reader_pos(*old(reader)),
                    width as int,
                    reference as int,
                    k,
                ),
        ))]
        for _ in 0..len {
            #[cfg(verus_keep_ghost)]
            proof! { lemma_next_field(out@.len() - old(out)@.len(), len as int, width as int); }
            let raw = reader.read_bits(width)? as i64;
            out.push(reference + raw);
        }
    }
    Ok(())
}

/// Read `count` stored group widths of `bits` bits each, the second-order
/// width block, failing on any wider than 32 rather than cutting it to fit a
/// byte.
///
/// Proved (see the module docs): on `Ok`, width `g` is the `g`-th `bits`-bit
/// field from the reader's cursor, it is at most 32, and the reader has moved
/// exactly `count · bits` bits.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r matches Ok(v) ==> v@.len() == count,
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + count * bits,
        r matches Ok(v) ==> forall|g: int|
            0 <= g < count ==> #[trigger] v@[g] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ) && v@[g] <= 32,
))]
pub fn read_group_widths(
    reader: &mut BitReader,
    count: usize,
    bits: u8,
) -> Result<Vec<u8>, FieldglassError> {
    let mut out: Vec<u8> = Vec::with_capacity(count);
    let mut g = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= g <= count,
            reader_bytes(*reader) == reader_bytes(*old(reader)),
            reader_pos(*reader) == reader_pos(*old(reader)) + g * bits,
            out@.len() == g,
            forall|h: int| 0 <= h < g ==> #[trigger] out@[h] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + h * bits,
                bits as int,
            ) && out@[h] <= 32,
        decreases count - g,
    ))]
    while g < count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_mul_succ(g as int, bits as int); }
        let width = reader.read_bits(bits)?;
        if width > 32 {
            return Err(second_order_group_too_wide(g, width as u64));
        }
        out.push(width as u8);
        g += 1;
    }
    Ok(out)
}

/// Append the points of a run of groups to `out`, group by group:
/// `expand_group_into` over `widths[g]`, `lengths[g]` and `references[g]`.
///
/// The three tables must be the same length, one entry per group; a mismatch
/// is an error rather than a panic.
///
/// Proved (see the module docs): on `Ok`, `out` gains exactly the sum of the
/// lengths, every width is at most 32, and point `k` of group `g` sits at
/// `old length + lengths[0] + … + lengths[g - 1] + k` and is `references[g]`
/// plus the `k`-th field of that group, whose fields start where the previous
/// group's end.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r is Ok ==> widths@.len() == lengths@.len() && widths@.len() == references@.len(),
        r is Ok ==> final(out)@.len() == old(out)@.len() + prefix_sum(
            u32_seq(lengths@),
            lengths@.len() as int,
        ),
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + prefix_sum(
            group_bits(widths@, lengths@),
            widths@.len() as int,
        ),
        r is Ok ==> forall|g: int| 0 <= g < widths@.len() ==> #[trigger] widths@[g] <= 32,
        r is Ok ==> forall|i: int|
            0 <= i < old(out)@.len() ==> #[trigger] final(out)@[i] == old(out)@[i],
        r is Ok ==> forall|g: int, k: int|
            0 <= g < widths@.len() && 0 <= k < lengths@[g] ==> #[trigger] final(out)@[old(out)@.len()
                + prefix_sum(u32_seq(lengths@), g) + k] == group_point(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + prefix_sum(group_bits(widths@, lengths@), g),
                widths@[g] as int,
                references@[g] as int,
                k,
            ),
))]
pub fn expand_groups_into(
    reader: &mut BitReader,
    widths: &[u8],
    lengths: &[u32],
    references: &[u32],
    out: &mut Vec<i64>,
) -> Result<(), FieldglassError> {
    let num_groups = widths.len();
    if lengths.len() != num_groups || references.len() != num_groups {
        return Err(parse_error(
            "grouped packing: group width, length and reference tables differ in length",
        ));
    }
    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost out0 = out@; }
    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost pos0 = reader_pos(*reader); }
    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost bytes = reader_bytes(*reader); }
    #[cfg(verus_keep_ghost)]
    proof! { lemma_groups_placed_none(out@, out@.len() as int, u32_seq(lengths@), so_points(bytes, pos0, widths@, lengths@, references@)); }
    let mut g = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= g <= num_groups,
            num_groups == widths@.len(),
            lengths@.len() == num_groups,
            references@.len() == num_groups,
            out0 == old(out)@,
            pos0 == reader_pos(*old(reader)),
            bytes == reader_bytes(*old(reader)),
            reader_bytes(*reader) == bytes,
            reader_pos(*reader) == pos0 + prefix_sum(group_bits(widths@, lengths@), g as int),
            out@.len() == out0.len() + prefix_sum(u32_seq(lengths@), g as int),
            forall|h: int| 0 <= h < g ==> #[trigger] widths@[h] <= 32,
            forall|i: int| 0 <= i < out0.len() ==> #[trigger] out@[i] == out0[i],
            groups_placed(
                out@,
                out0.len() as int,
                u32_seq(lengths@),
                so_points(bytes, pos0, widths@, lengths@, references@),
                g as int,
            ),
        decreases num_groups - g,
    ))]
    while g < num_groups {
        #[cfg(verus_keep_ghost)]
        proof_decl! { let ghost before = out@; }
        // `expand_group_into` rejects a wide group too; checking here first
        // lets the error name the group.
        if widths[g] > 32 {
            return Err(second_order_group_too_wide(g, widths[g] as u64));
        }
        expand_group_into(reader, widths[g], lengths[g] as usize, references[g], out)?;
        #[cfg(verus_keep_ghost)]
        proof! {
            let lens = u32_seq(lengths@);
            lemma_prefix_sum_step(lens, g as int);
            lemma_prefix_sum_step(group_bits(widths@, lengths@), g as int);
            lemma_u32_seq_nonneg(lengths@);
            lemma_prefix_sum_mono(lens, 0, g as int);
            let (w, l) = (widths@[g as int] as int, lengths@[g as int] as int);
            assert(l * w == w * l) by (nonlinear_arith);
            lemma_place_group(
                before,
                out@,
                out0.len() as int,
                lens,
                so_points(bytes, pos0, widths@, lengths@, references@),
                g as int,
            );
        }
        g += 1;
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        reveal(groups_placed);
        assert forall|h: int, k: int|
            0 <= h < num_groups && 0 <= k < lengths@[h] implies #[trigger] out@[out0.len()
                + prefix_sum(u32_seq(lengths@), h) + k] == group_point(
                bytes,
                pos0 + prefix_sum(group_bits(widths@, lengths@), h),
                widths@[h] as int,
                references@[h] as int,
                k,
            ) by {
            lemma_group_point_fits(
                bytes,
                pos0 + prefix_sum(group_bits(widths@, lengths@), h),
                widths@[h] as int,
                references@[h] as int,
                k,
            );
        }
    }
    Ok(())
}

/// The layout of a GRIB2 complex-packing (template 5.2 / 5.3) group table,
/// from the data representation section. Every field is the template's own
/// value, unscaled.
// `verus_verify` makes the type visible to the proofs.
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComplexGroupLayout {
    /// `NG`, the number of groups.
    pub num_groups: u32,
    /// Bits per group reference value (the template's bits per value).
    pub reference_bits: u8,
    /// Bits per stored group width.
    pub width_bits: u8,
    /// Added to every stored group width.
    pub width_reference: u8,
    /// Bits per stored group length.
    pub length_bits: u8,
    /// Added to every scaled stored group length.
    pub length_reference: u32,
    /// Every stored group length is multiplied by this.
    pub length_increment: u8,
    /// The true length of the last group, which overrides its stored one.
    pub length_last: u32,
    /// Code Table 5.5: 0 none, 1 primary, 2 primary and secondary. Any other
    /// value marks nothing missing; the caller rejects it first.
    pub missing_value_management: u8,
}

/// Expand the §7 group structure of GRIB2 complex packing into one scaled
/// integer (`group_ref[g] + X`) per present point, `None` for a point marked
/// missing. `reader`'s cursor must sit at the start of the group-reference
/// block.
///
/// §7 holds four blocks, each starting on an octet boundary: the `NG` group
/// references (`reference_bits` each), the `NG` stored widths (`width_bits`
/// each, plus `width_reference`), the `NG` stored lengths (`length_bits` each,
/// times `length_increment`, plus `length_reference`; the last group's length
/// is `length_last` instead), then each group's offsets at its width.
///
/// Missing points are flagged by all-ones values (eccodes
/// `DataG22OrderPacking::unpack`): in a zero-width group, a reference equal to
/// `2^reference_bits − 1` marks the whole group missing; in a wider group, an
/// offset equal to `2^width − 1` marks that point. Management 2 also treats
/// that value minus one as missing.
///
/// An error, never a panic, when: a field width is over 32; `NG` is 0 (the
/// constant-field case, which the caller handles) or exceeds `present_count`;
/// a group length overflows `usize`; the lengths do not sum to
/// `present_count`; a group's width is over 32; or the reader runs out.
///
/// Proved (see the module docs): on `Ok`, the group lengths sum to
/// `present_count` and the result has that many values; every group's width is
/// at most 32, so no read is truncated; and point `k` of group `g` is at index
/// `len[0] + … + len[g − 1] + k` and equals `cg_point`, the value the layout
/// above defines for it.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r matches Ok(v) ==> complex_groups_decoded(
            *layout,
            reader_bytes(*old(reader)),
            reader_pos(*old(reader)),
            present_count as int,
            v@,
        ),
        r is Ok ==> reader_pos(*final(reader)) == cg_data_start(*layout, reader_pos(*old(reader)))
            + prefix_sum(
            cg_bits(*layout, reader_bytes(*old(reader)), reader_pos(*old(reader))),
            layout.num_groups as int,
        ),
))]
pub fn expand_complex_groups(
    reader: &mut BitReader,
    layout: &ComplexGroupLayout,
    present_count: usize,
) -> Result<Vec<Option<i64>>, FieldglassError> {
    let reference_bits = layout.reference_bits;
    let width_bits = layout.width_bits;
    let length_bits = layout.length_bits;
    let width_reference = layout.width_reference;
    let length_reference = layout.length_reference;
    let length_increment = layout.length_increment;
    let length_last = layout.length_last;
    let mvm = layout.missing_value_management;
    // The bit-width fields feed `BitReader::read_bits`, whose contract tops
    // out at 32 bits; a wider field is malformed (and would silently truncate).
    if reference_bits > 32 {
        return Err(field_too_wide("group reference", reference_bits));
    }
    if width_bits > 32 {
        return Err(field_too_wide("group width", width_bits));
    }
    if length_bits > 32 {
        return Err(field_too_wide("group length", length_bits));
    }

    let num_groups = layout.num_groups as usize;
    // NG == 0 is the constant-field case (ECC-2095) and both callers
    // intercept it before any §7 read; reaching here with 0 groups is a
    // caller bug, kept as an error.
    if num_groups == 0 {
        return Err(parse_error(
            "complex packing: group expansion invoked with 0 groups",
        ));
    }
    // Every group covers at least one point, so NG can't legitimately exceed
    // the number of present points — this bounds the per-group allocations
    // below against a malformed huge NG.
    if num_groups > present_count {
        return Err(too_many_groups(num_groups, present_count));
    }

    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost bytes = reader_bytes(*reader); }
    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost p0 = reader_pos(*reader); }

    // The four §7 sub-blocks (group references, widths, lengths, then the data
    // values) each begin on an octet boundary, so we realign after every one.
    // eccodes does the same: each `buf_*` pointer is advanced by the previous
    // block's *byte* size, `ceil(bits / 8)` (DataG22OrderPacking::unpack).
    // Without this, a block whose bit length isn't a multiple of 8 leaves the
    // cursor mid-byte and every following block is misread.

    // Block 1: group reference values.
    let group_refs = read_references(reader, num_groups, reference_bits)?;
    reader.align_to_byte();

    // Block 2: group widths (stored value offset by the width reference).
    // Computed in u64 so the reference + a 32-bit stored value can't overflow
    // before the `> 32` range check in the data loop sees it.
    let group_widths = read_widths(reader, num_groups, width_bits, width_reference)?;
    reader.align_to_byte();

    // Block 3: group lengths. The stored value for every group is read (so
    // the bit cursor reaches the data block correctly), then the last group's
    // length is overridden by the explicit `length_last` field.
    let mut group_lengths = read_lengths(
        reader,
        num_groups,
        length_bits,
        length_increment,
        length_reference,
    )?;
    // num_groups >= 1 here, so the last element exists.
    group_lengths[num_groups - 1] = length_last as usize;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert forall|g: int| 0 <= g < num_groups implies #[trigger] group_refs@[g] as int
            == cg_ref(*layout, bytes, p0, g) by {}
        assert forall|g: int| 0 <= g < num_groups implies #[trigger] group_widths@[g] as int
            == cg_width(*layout, bytes, p0, g) by {}
        assert forall|g: int| 0 <= g < num_groups implies #[trigger] group_lengths@[g] as int
            == cg_len(*layout, bytes, p0, g) by {}
    }

    // The group lengths must account for exactly the present points; validate
    // before allocating so a malformed length can't drive a huge allocation.
    let total = match checked_total(&group_lengths) {
        Some(total) => total,
        None => {
            return Err(parse_error(
                "complex packing: group lengths sum overflows usize",
            ));
        }
    };
    #[cfg(verus_keep_ghost)]
    proof! {
        lemma_prefix_sum_ext(usize_seq(group_lengths@), cg_lens(*layout, bytes, p0), num_groups as int);
    }
    if total != present_count {
        return Err(length_sum_mismatch(total, present_count));
    }

    // Block 4: the per-point offsets, decoded group by group. Starts on the
    // octet boundary after the group-length block.
    reader.align_to_byte();
    #[cfg(verus_keep_ghost)]
    proof_decl! { let ghost ds = reader_pos(*reader); }

    let mut scaled: Vec<Option<i64>> = Vec::with_capacity(present_count);
    #[cfg(verus_keep_ghost)]
    proof! { lemma_groups_placed_none(scaled@, 0, cg_lens(*layout, bytes, p0), cg_points(*layout, bytes, p0)); }
    let mut g = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= g <= num_groups,
            1 <= num_groups <= present_count,
            present_count == prefix_sum(cg_lens(*layout, bytes, p0), num_groups as int),
            bytes == reader_bytes(*old(reader)),
            p0 == reader_pos(*old(reader)),
            ds == cg_data_start(*layout, p0),
            num_groups == layout.num_groups,
            reference_bits == layout.reference_bits,
            reference_bits <= 32,
            mvm == layout.missing_value_management,
            group_refs@.len() == num_groups,
            group_widths@.len() == num_groups,
            group_lengths@.len() == num_groups,
            forall|h: int| 0 <= h < num_groups ==> #[trigger] group_refs@[h] as int == cg_ref(*layout, bytes, p0, h),
            forall|h: int| 0 <= h < num_groups ==> #[trigger] group_widths@[h] as int == cg_width(*layout, bytes, p0, h),
            forall|h: int| 0 <= h < num_groups ==> #[trigger] group_lengths@[h] as int == cg_len(*layout, bytes, p0, h),
            reader_bytes(*reader) == bytes,
            reader_pos(*reader) == ds + prefix_sum(cg_bits(*layout, bytes, p0), g as int),
            scaled@.len() == prefix_sum(cg_lens(*layout, bytes, p0), g as int),
            forall|h: int| 0 <= h < g ==> #[trigger] cg_width(*layout, bytes, p0, h) <= 32,
            groups_placed(scaled@, 0, cg_lens(*layout, bytes, p0), cg_points(*layout, bytes, p0), g as int),
        decreases num_groups - g,
    ))]
    while g < num_groups {
        #[cfg(verus_keep_ghost)]
        proof_decl! { let ghost before = scaled@; }
        expand_complex_group(
            reader,
            g,
            group_widths[g],
            group_refs[g],
            group_lengths[g],
            reference_bits,
            mvm,
            &mut scaled,
        )?;
        #[cfg(verus_keep_ghost)]
        proof! {
            let lens = cg_lens(*layout, bytes, p0);
            let bits = cg_bits(*layout, bytes, p0);
            lemma_prefix_sum_step(lens, g as int);
            lemma_prefix_sum_step(bits, g as int);
            lemma_cg_lens_nonneg(*layout, bytes, p0);
            lemma_prefix_sum_mono(lens, 0, g as int);
            let (w, l) = (cg_width(*layout, bytes, p0, g as int), cg_len(*layout, bytes, p0, g as int));
            assert(l * w == w * l) by (nonlinear_arith);
            lemma_place_group(before, scaled@, 0, lens, cg_points(*layout, bytes, p0), g as int);
        }
        g += 1;
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        reveal(groups_placed);
        let lens = cg_lens(*layout, bytes, p0);
        assert forall|h: int, k: int|
            0 <= h < num_groups && 0 <= k < cg_len(*layout, bytes, p0, h) implies #[trigger] scaled@[prefix_sum(
                lens,
                h,
            ) + k] == cg_point(*layout, bytes, p0, h, k) by {
            assert(scaled@[0 + prefix_sum(lens, h) + k] == cg_points(*layout, bytes, p0)(h, k));
        }
    }

    Ok(scaled)
}

/// Read `count` consecutive `bits`-bit fields: a complex-packing group
/// reference block.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r matches Ok(v) ==> v@.len() == count,
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + count * bits,
        r matches Ok(v) ==> forall|g: int|
            0 <= g < count ==> #[trigger] v@[g] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ),
))]
fn read_references(
    reader: &mut BitReader,
    count: usize,
    bits: u8,
) -> Result<Vec<u32>, FieldglassError> {
    let mut out: Vec<u32> = Vec::with_capacity(count);
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            reader_bytes(*reader) == reader_bytes(*old(reader)),
            reader_pos(*reader) == reader_pos(*old(reader)) + it.index@ * bits,
            out@.len() == it.index@,
            forall|g: int| 0 <= g < it.index@ ==> #[trigger] out@[g] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ),
    ))]
    for _ in 0..count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_mul_succ(out@.len() as int, bits as int); }
        out.push(reader.read_bits(bits)?);
    }
    Ok(out)
}

/// Read `count` group widths: each a `bits`-bit stored value plus
/// `reference`, in `u64` so the sum cannot overflow.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r matches Ok(v) ==> v@.len() == count,
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + count * bits,
        r matches Ok(v) ==> forall|g: int|
            0 <= g < count ==> #[trigger] v@[g] as int == reference + msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ),
))]
fn read_widths(
    reader: &mut BitReader,
    count: usize,
    bits: u8,
    reference: u8,
) -> Result<Vec<u64>, FieldglassError> {
    let mut out: Vec<u64> = Vec::with_capacity(count);
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            reader_bytes(*reader) == reader_bytes(*old(reader)),
            reader_pos(*reader) == reader_pos(*old(reader)) + it.index@ * bits,
            out@.len() == it.index@,
            forall|g: int| 0 <= g < it.index@ ==> #[trigger] out@[g] as int == reference + msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ),
    ))]
    for _ in 0..count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_mul_succ(out@.len() as int, bits as int); }
        let stored = reader.read_bits(bits)?;
        out.push(reference as u64 + stored as u64);
    }
    Ok(out)
}

/// Read `count` group lengths: each a `bits`-bit stored value times
/// `increment` plus `reference`, an error when that overflows `usize`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r matches Ok(v) ==> v@.len() == count,
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + count * bits,
        r matches Ok(v) ==> forall|g: int|
            0 <= g < count ==> #[trigger] v@[g] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ) * increment + reference,
))]
fn read_lengths(
    reader: &mut BitReader,
    count: usize,
    bits: u8,
    increment: u8,
    reference: u32,
) -> Result<Vec<usize>, FieldglassError> {
    let mut out: Vec<usize> = Vec::with_capacity(count);
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            reader_bytes(*reader) == reader_bytes(*old(reader)),
            reader_pos(*reader) == reader_pos(*old(reader)) + it.index@ * bits,
            out@.len() == it.index@,
            forall|g: int| 0 <= g < it.index@ ==> #[trigger] out@[g] as int == msb_bits(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)) + g * bits,
                bits as int,
            ) * increment + reference,
    ))]
    for _ in 0..count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_mul_succ(out@.len() as int, bits as int); }
        let stored = reader.read_bits(bits)? as usize;
        let len = match stored.checked_mul(increment as usize) {
            Some(scaled) => match scaled.checked_add(reference as usize) {
                Some(len) => len,
                None => return Err(length_overflow()),
            },
            None => return Err(length_overflow()),
        };
        out.push(len);
    }
    Ok(out)
}

/// The sum of `lengths`, or `None` when it overflows `usize`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(t) ==> t == prefix_sum(usize_seq(lengths@), lengths@.len() as int),
        r is None ==> prefix_sum(usize_seq(lengths@), lengths@.len() as int) > usize::MAX,
))]
fn checked_total(lengths: &[usize]) -> Option<usize> {
    let mut total = 0usize;
    let mut g = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= g <= lengths@.len(),
            total == prefix_sum(usize_seq(lengths@), g as int),
        decreases lengths@.len() - g,
    ))]
    while g < lengths.len() {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_prefix_sum_step(usize_seq(lengths@), g as int);
            lemma_usize_seq_nonneg(lengths@);
            lemma_prefix_sum_mono(usize_seq(lengths@), g as int + 1, lengths@.len() as int);
        }
        total = total.checked_add(lengths[g])?;
        g += 1;
    }
    Some(total)
}

/// Append one complex-packing group's points to `scaled`: see
/// `expand_complex_groups`. `g` is the group's index, for the error message.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires
        reference_bits <= 32,
    ensures
        reader_bytes(*final(reader)) == reader_bytes(*old(reader)),
        r is Ok ==> width <= 32,
        r is Ok ==> reader_pos(*final(reader)) == reader_pos(*old(reader)) + len * width,
        r is Ok ==> final(scaled)@.len() == old(scaled)@.len() + len,
        r is Ok ==> forall|i: int|
            0 <= i < old(scaled)@.len() ==> #[trigger] final(scaled)@[i] == old(scaled)@[i],
        r is Ok ==> forall|k: int|
            0 <= k < len ==> #[trigger] final(scaled)@[old(scaled)@.len() + k]
                == complex_group_point(
                reader_bytes(*old(reader)),
                reader_pos(*old(reader)),
                width as int,
                reference as int,
                reference_bits as int,
                mvm,
                k,
            ),
))]
#[allow(clippy::too_many_arguments)]
fn expand_complex_group(
    reader: &mut BitReader,
    g: usize,
    width: u64,
    reference: u32,
    len: usize,
    reference_bits: u8,
    mvm: u8,
    scaled: &mut Vec<Option<i64>>,
) -> Result<(), FieldglassError> {
    let group_ref = reference as i64;
    if width == 0 {
        // Zero-width group: no per-point offsets are stored. Every point
        // equals the group reference — unless the reference is the missing
        // sentinel at `reference_bits`, which marks the whole group missing.
        let value = if is_missing(mvm, group_ref, reference_bits) {
            None
        } else {
            Some(group_ref)
        };
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                scaled@.len() == old(scaled)@.len() + it.index@,
                forall|i: int| 0 <= i < old(scaled)@.len() ==> #[trigger] scaled@[i] == old(scaled)@[i],
                forall|k: int| 0 <= k < it.index@ ==> #[trigger] scaled@[old(scaled)@.len() + k] == value,
        ))]
        for _ in 0..len {
            scaled.push(value);
        }
    } else {
        if width > 32 {
            // The actual width is `width_reference + stored`, which can
            // exceed 32 even when each field is individually in range;
            // `read_bits` only honours up to 32 bits per value.
            return Err(group_width_too_wide(g, width));
        }
        let w = width as u8;
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                1 <= w <= 32,
                w == width,
                0 <= group_ref <= u32::MAX,
                group_ref == reference,
                reader_bytes(*reader) == reader_bytes(*old(reader)),
                reader_pos(*reader) == reader_pos(*old(reader)) + it.index@ * w,
                scaled@.len() == old(scaled)@.len() + it.index@,
                forall|i: int| 0 <= i < old(scaled)@.len() ==> #[trigger] scaled@[i] == old(scaled)@[i],
                forall|k: int| 0 <= k < it.index@ ==> #[trigger] scaled@[old(scaled)@.len() + k]
                    == complex_group_point(
                    reader_bytes(*old(reader)),
                    reader_pos(*old(reader)),
                    width as int,
                    reference as int,
                    reference_bits as int,
                    mvm,
                    k,
                ),
        ))]
        for _ in 0..len {
            #[cfg(verus_keep_ghost)]
            proof! { lemma_mul_succ((scaled@.len() - old(scaled)@.len()) as int, w as int); }
            let x = reader.read_bits(w)? as i64;
            let value = if is_missing(mvm, x, w) {
                None
            } else {
                Some(group_ref + x)
            };
            scaled.push(value);
        }
    }
    Ok(())
}

/// Code Table 5.5's missing-value test: `raw` is the primary substitute (all
/// ones at `field_bits`) under management 1 or 2, or the secondary (all ones
/// minus one) under management 2.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    requires
        field_bits <= 32,
    ensures
        out == is_missing_spec(mvm, raw as int, field_bits as int),
))]
fn is_missing(mvm: u8, raw: i64, field_bits: u8) -> bool {
    #[cfg(verus_keep_ghost)]
    proof! {
        vstd::arithmetic::power2::lemma2_to64();
        vstd::arithmetic::power2::lemma_pow2_strictly_increases(field_bits as nat, 64);
        vstd::bits::lemma_u64_shl_is_mul(1, field_bits as u64);
    }
    let sentinel = ((1u64 << field_bits) - 1) as i64;
    match mvm {
        1 => raw == sentinel,
        2 => raw == sentinel || raw == sentinel - 1,
        _ => false,
    }
}

// Error constructors. `format!` has no Verus specification, so each message
// is built outside the proof; a proof only needs to know an error is returned.

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn group_too_wide(width: u8) -> FieldglassError {
    FieldglassError::Parse(format!(
        "grouped packing: group width {width} exceeds 32 bits"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn second_order_group_too_wide(g: usize, width: u64) -> FieldglassError {
    FieldglassError::Parse(format!(
        "second-order packing: group {g} width {width} exceeds 32 bits"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn field_too_wide(label: &str, bits: u8) -> FieldglassError {
    FieldglassError::Parse(format!(
        "complex packing: {label} field width {bits} exceeds 32 bits"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn too_many_groups(num_groups: usize, present_count: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "complex packing declares {num_groups} groups but only {present_count} \
         values are present"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn parse_error(message: &str) -> FieldglassError {
    FieldglassError::Parse(message.to_string())
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn length_overflow() -> FieldglassError {
    FieldglassError::Parse("complex packing: group length overflows usize".into())
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn length_sum_mismatch(total: usize, present_count: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "complex packing: group lengths sum to {total} but {present_count} values are required"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn group_width_too_wide(g: usize, width: u64) -> FieldglassError {
    FieldglassError::Parse(format!(
        "complex packing: group {g} width {width} exceeds 32 bits"
    ))
}

#[cfg(verus_keep_ghost)]
verus! {

/// Point `k` of a group whose offsets start at bit `start` of `bytes`:
/// `reference` plus the `k`-th `width`-bit field. A zero-width field is 0, so
/// every point of a zero-width group is `reference`.
pub open spec fn group_point(bytes: Seq<u8>, start: int, width: int, reference: int, k: int) -> int {
    reference + msb_bits(bytes, start + k * width, width)
}

/// A group point of width at most 32 over a `u32` reference fits an `i64`.
pub proof fn lemma_group_point_fits(bytes: Seq<u8>, start: int, width: int, reference: int, k: int)
    requires
        0 <= width <= 32,
        0 <= reference <= u32::MAX,
    ensures
        0 <= group_point(bytes, start, width, reference, k) <= 2 * (u32::MAX as int) + 1,
{
    lemma_msb_bits_bound(bytes, start + k * width, width);
    vstd::arithmetic::power2::lemma2_to64();
    if width < 32 {
        vstd::arithmetic::power2::lemma_pow2_strictly_increases(width as nat, 32);
    }
}

/// A `width`-bit field is below `2^width`.
pub proof fn lemma_msb_bits_bound(bytes: Seq<u8>, start: int, width: int)
    requires
        0 <= width,
    ensures
        msb_bits(bytes, start, width) < pow2(width as nat),
    decreases width,
{
    if width == 0 {
        vstd::arithmetic::power2::lemma2_to64();
    } else {
        lemma_msb_bits_bound(bytes, start, width - 1);
        vstd::arithmetic::power2::lemma_pow2_unfold(width as nat);
    }
}

/// Whether a group of `len` fields of `width` bits can be read from bit `pos`
/// of a `blen`-byte buffer: the width is at most 32, and either no bit is read
/// or every field ends inside the buffer.
pub open spec fn group_fits(blen: int, pos: int, width: int, len: int) -> bool {
    width <= 32 && (width == 0 || len == 0 || (pos + len * width <= blen * 8 && blen * 8
        <= usize::MAX))
}

/// Point `k` of group `g` of a second-order table whose offsets start at bit
/// `pos0`, as an `i64`.
pub open spec fn so_points(
    bytes: Seq<u8>,
    pos0: int,
    widths: Seq<u8>,
    lengths: Seq<u32>,
    references: Seq<u32>,
) -> spec_fn(int, int) -> i64 {
    |h: int, k: int|
        group_point(
            bytes,
            pos0 + prefix_sum(group_bits(widths, lengths), h),
            widths[h] as int,
            references[h] as int,
            k,
        ) as i64
}

/// Groups `0..g` are laid out back to back from index `base` of `v`: point `k`
/// of group `h` is at `base + lens(0) + … + lens(h − 1) + k` and equals
/// `pts(h, k)`. Opaque, so a loop carries it as one fact rather than a
/// quantifier the solver re-instantiates at every step.
#[verifier::opaque]
pub open spec fn groups_placed<T>(
    v: Seq<T>,
    base: int,
    lens: spec_fn(int) -> int,
    pts: spec_fn(int, int) -> T,
    g: int,
) -> bool {
    forall|h: int, k: int|
        0 <= h < g && 0 <= k < lens(h) ==> #[trigger] v[base + prefix_sum(lens, h) + k] == pts(h, k)
}

/// No group is placed yet.
pub proof fn lemma_groups_placed_none<T>(
    v: Seq<T>,
    base: int,
    lens: spec_fn(int) -> int,
    pts: spec_fn(int, int) -> T,
)
    ensures
        groups_placed(v, base, lens, pts, 0),
{
    reveal(groups_placed);
}

/// Appending group `g`'s points after groups `0..g` places groups `0..g + 1`.
/// The step every group-expansion loop takes, proved once.
pub proof fn lemma_place_group<T>(
    before: Seq<T>,
    after: Seq<T>,
    base: int,
    lens: spec_fn(int) -> int,
    pts: spec_fn(int, int) -> T,
    g: int,
)
    requires
        0 <= g,
        0 <= base,
        forall|i: int| 0 <= i < g ==> #[trigger] lens(i) >= 0,
        before.len() == base + prefix_sum(lens, g),
        after.len() == before.len() + lens(g),
        forall|i: int| 0 <= i < before.len() ==> #[trigger] after[i] == before[i],
        forall|k: int| 0 <= k < lens(g) ==> #[trigger] after[before.len() + k] == pts(g, k),
        groups_placed(before, base, lens, pts, g),
    ensures
        groups_placed(after, base, lens, pts, g + 1),
{
    reveal(groups_placed);
    assert forall|h: int, k: int| 0 <= h < g + 1 && 0 <= k < lens(h) implies #[trigger] after[base
        + prefix_sum(lens, h) + k] == pts(h, k) by {
        if h < g {
            lemma_prefix_sum_step(lens, h);
            lemma_prefix_sum_mono(lens, 0, h);
            lemma_prefix_sum_mono(lens, h + 1, g);
            assert(before[base + prefix_sum(lens, h) + k] == pts(h, k));
        } else {
            assert(after[before.len() + k] == pts(g, k));
        }
    }
}

/// `f(0) + … + f(g − 1)`.
pub open spec fn prefix_sum(f: spec_fn(int) -> int, g: int) -> int
    decreases g,
{
    if g <= 0 {
        0
    } else {
        prefix_sum(f, g - 1) + f(g - 1)
    }
}

/// The entries of a `u32` table, as a function of the index.
pub open spec fn u32_seq(s: Seq<u32>) -> spec_fn(int) -> int {
    |i: int| s[i] as int
}

/// The entries of a `usize` table, as a function of the index.
pub open spec fn usize_seq(s: Seq<usize>) -> spec_fn(int) -> int {
    |i: int| s[i] as int
}

/// The bits group `g` of a second-order table occupies: width times length.
pub open spec fn group_bits(widths: Seq<u8>, lengths: Seq<u32>) -> spec_fn(int) -> int {
    |i: int| widths[i] as int * lengths[i] as int
}

// ---- complex packing: the value the layout defines for each point ----

/// Group `g`'s reference value.
pub open spec fn cg_ref(l: ComplexGroupLayout, b: Seq<u8>, p0: int, g: int) -> int {
    msb_bits(b, p0 + g * l.reference_bits, l.reference_bits as int) as int
}

/// Where the width block starts: the octet after the reference block.
pub open spec fn cg_widths_start(l: ComplexGroupLayout, p0: int) -> int {
    align8(p0 + l.num_groups * l.reference_bits)
}

/// Group `g`'s width: the stored width plus the width reference.
pub open spec fn cg_width(l: ComplexGroupLayout, b: Seq<u8>, p0: int, g: int) -> int {
    l.width_reference + msb_bits(b, cg_widths_start(l, p0) + g * l.width_bits, l.width_bits as int)
}

/// Where the length block starts: the octet after the width block.
pub open spec fn cg_lengths_start(l: ComplexGroupLayout, p0: int) -> int {
    align8(cg_widths_start(l, p0) + l.num_groups * l.width_bits)
}

/// Group `g`'s stored length, scaled: `stored · increment + reference`.
pub open spec fn cg_stored_len(l: ComplexGroupLayout, b: Seq<u8>, p0: int, g: int) -> int {
    msb_bits(b, cg_lengths_start(l, p0) + g * l.length_bits, l.length_bits as int)
        * l.length_increment + l.length_reference
}

/// Group `g`'s length: the scaled stored length, except the last group's,
/// which is `length_last`.
pub open spec fn cg_len(l: ComplexGroupLayout, b: Seq<u8>, p0: int, g: int) -> int {
    if g == l.num_groups - 1 {
        l.length_last as int
    } else {
        cg_stored_len(l, b, p0, g)
    }
}

/// Where the offsets start: the octet after the length block.
pub open spec fn cg_data_start(l: ComplexGroupLayout, p0: int) -> int {
    align8(cg_lengths_start(l, p0) + l.num_groups * l.length_bits)
}

/// The group lengths, as a function of the group.
pub open spec fn cg_lens(l: ComplexGroupLayout, b: Seq<u8>, p0: int) -> spec_fn(int) -> int {
    |g: int| cg_len(l, b, p0, g)
}

/// The bits each group's offsets occupy: width times length.
pub open spec fn cg_bits(l: ComplexGroupLayout, b: Seq<u8>, p0: int) -> spec_fn(int) -> int {
    |g: int| cg_width(l, b, p0, g) * cg_len(l, b, p0, g)
}

/// Code Table 5.5: all ones at `bits` is the primary substitute (management 1
/// or 2), all ones minus one the secondary (management 2).
pub open spec fn is_missing_spec(mvm: u8, raw: int, bits: int) -> bool {
    let sentinel = pow2(bits as nat) - 1;
    (mvm == 1 && raw == sentinel) || (mvm == 2 && (raw == sentinel || raw == sentinel - 1))
}

/// Point `k` of a group of nonzero width whose offsets start at bit `start`.
pub open spec fn complex_point(bytes: Seq<u8>, start: int, width: int, reference: int, mvm: u8, k: int) -> Option<i64> {
    let x = msb_bits(bytes, start + k * width, width) as int;
    if is_missing_spec(mvm, x, width) {
        None
    } else {
        Some((reference + x) as i64)
    }
}

/// Point `k` of a complex-packing group whose offsets start at bit `start`:
/// for a zero-width group, the reference, or `None` when the reference is the
/// missing sentinel at `reference_bits`; otherwise `complex_point`.
pub open spec fn complex_group_point(
    bytes: Seq<u8>,
    start: int,
    width: int,
    reference: int,
    reference_bits: int,
    mvm: u8,
    k: int,
) -> Option<i64> {
    if width == 0 {
        if is_missing_spec(mvm, reference, reference_bits) {
            None
        } else {
            Some(reference as i64)
        }
    } else {
        complex_point(bytes, start, width, reference, mvm, k)
    }
}

/// Point `k` of group `g`: `None` when marked missing, else the group
/// reference plus the point's offset.
pub open spec fn cg_point(l: ComplexGroupLayout, b: Seq<u8>, p0: int, g: int, k: int) -> Option<i64> {
    complex_group_point(
        b,
        cg_data_start(l, p0) + prefix_sum(cg_bits(l, b, p0), g),
        cg_width(l, b, p0, g),
        cg_ref(l, b, p0, g),
        l.reference_bits as int,
        l.missing_value_management,
        k,
    )
}

/// `cg_point` as a function of the group and the point.
pub open spec fn cg_points(l: ComplexGroupLayout, b: Seq<u8>, p0: int) -> spec_fn(int, int) -> Option<i64> {
    |g: int, k: int| cg_point(l, b, p0, g, k)
}

/// What an `Ok` from `expand_complex_groups` guarantees.
pub open spec fn complex_groups_decoded(
    l: ComplexGroupLayout,
    b: Seq<u8>,
    p0: int,
    present_count: int,
    v: Seq<Option<i64>>,
) -> bool {
    &&& 1 <= l.num_groups <= present_count
    // The sum invariant.
    &&& prefix_sum(cg_lens(l, b, p0), l.num_groups as int) == present_count
    &&& v.len() == present_count
    // The width bound: every read of a group's offsets is at most 32 bits.
    &&& forall|g: int| 0 <= g < l.num_groups ==> #[trigger] cg_width(l, b, p0, g) <= 32
    // Every point is where the layout puts it, with the value it defines.
    &&& forall|g: int, k: int|
        0 <= g < l.num_groups && 0 <= k < cg_len(l, b, p0, g) ==> #[trigger] v[prefix_sum(
            cg_lens(l, b, p0),
            g,
        ) + k] == cg_point(l, b, p0, g, k)
}

pub proof fn lemma_prefix_sum_step(f: spec_fn(int) -> int, g: int)
    requires
        0 <= g,
    ensures
        prefix_sum(f, g + 1) == prefix_sum(f, g) + f(g),
{
}

/// A prefix sum of non-negative terms does not decrease.
pub proof fn lemma_prefix_sum_mono(f: spec_fn(int) -> int, h: int, g: int)
    requires
        0 <= h <= g,
        forall|i: int| 0 <= i < g ==> #[trigger] f(i) >= 0,
    ensures
        prefix_sum(f, h) <= prefix_sum(f, g),
    decreases g - h,
{
    if h < g {
        lemma_prefix_sum_mono(f, h, g - 1);
    }
}

/// Prefix sums of two functions that agree below `n` agree up to `n`.
pub proof fn lemma_prefix_sum_ext(f: spec_fn(int) -> int, h: spec_fn(int) -> int, n: int)
    requires
        forall|i: int| 0 <= i < n ==> #[trigger] f(i) == h(i),
    ensures
        forall|g: int| 0 <= g <= n ==> #[trigger] prefix_sum(f, g) == prefix_sum(h, g),
    decreases n,
{
    if n > 0 {
        lemma_prefix_sum_ext(f, h, n - 1);
        assert(prefix_sum(f, n - 1) == prefix_sum(h, n - 1));
        assert(f(n - 1) == h(n - 1));
        assert(prefix_sum(f, n) == prefix_sum(h, n));
    }
}

pub proof fn lemma_usize_seq_nonneg(s: Seq<usize>)
    ensures
        forall|i: int| 0 <= i < s.len() ==> #[trigger] usize_seq(s)(i) >= 0,
{
}

pub proof fn lemma_u32_seq_nonneg(s: Seq<u32>)
    ensures
        forall|i: int| 0 <= i < s.len() ==> #[trigger] u32_seq(s)(i) >= 0,
{
}

pub proof fn lemma_cg_lens_nonneg(l: ComplexGroupLayout, b: Seq<u8>, p0: int)
    ensures
        forall|g: int| #[trigger] cg_lens(l, b, p0)(g) >= 0,
{
    assert forall|g: int| #[trigger] cg_lens(l, b, p0)(g) >= 0 by {
        let s = msb_bits(b, cg_lengths_start(l, p0) + g * l.length_bits, l.length_bits as int);
        assert(s * l.length_increment >= 0) by (nonlinear_arith)
            requires s >= 0, l.length_increment >= 0;
    }
}

/// `(k + 1) · w == k · w + w`, which the solver needs stated.
pub proof fn lemma_mul_succ(k: int, w: int)
    ensures
        (k + 1) * w == k * w + w,
{
    assert((k + 1) * w == k * w + w) by (nonlinear_arith);
}

/// Field `k` of `count` ends where field `k + 1` starts, and inside the first
/// `count` fields.
pub proof fn lemma_next_field(k: int, count: int, width: int)
    requires
        0 <= k < count,
        0 <= width,
    ensures
        (k + 1) * width == k * width + width,
        (k + 1) * width <= count * width,
{
    assert((k + 1) * width == k * width + width) by (nonlinear_arith);
    assert((k + 1) * width <= count * width) by (nonlinear_arith)
        requires
            k + 1 <= count,
            0 <= width,
    ;
}

} // verus!

// Runtime checks of the same claims, on hand-built bit streams. The proof
// covers every input; these pin concrete answers.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_is_its_reference_plus_each_offset() {
        let mut out = vec![-1];
        let mut reader = BitReader::new(&[0b0111_0000]);
        expand_group_into(&mut reader, 2, 2, 10, &mut out).unwrap();
        assert_eq!(out, [-1, 11, 13]);

        // Zero width reads nothing: every point is the reference.
        let mut reader = BitReader::new(&[]);
        expand_group_into(&mut reader, 0, 3, 7, &mut out).unwrap();
        assert_eq!(out, [-1, 11, 13, 7, 7, 7]);

        // The largest reference plus the largest offset fits.
        let mut out = Vec::new();
        let mut reader = BitReader::new(&[0xFF; 4]);
        expand_group_into(&mut reader, 32, 1, u32::MAX, &mut out).unwrap();
        assert_eq!(out, [2 * i64::from(u32::MAX)]);
    }

    #[test]
    fn a_group_wider_than_32_bits_or_longer_than_the_buffer_is_an_error() {
        let mut out = Vec::new();
        let mut reader = BitReader::new(&[0; 8]);
        assert!(expand_group_into(&mut reader, 33, 0, 0, &mut out).is_err());
        let mut reader = BitReader::new(&[0; 1]);
        assert!(expand_group_into(&mut reader, 4, 3, 0, &mut out).is_err());
    }

    #[test]
    fn groups_append_back_to_back_after_what_the_vector_holds() {
        // Widths 4 and 0, lengths 2 and 2: offsets 0xA, 0xB, then none.
        let mut out = vec![100];
        let mut reader = BitReader::new(&[0xAB]);
        expand_groups_into(&mut reader, &[4, 0], &[2, 2], &[1, 5], &mut out).unwrap();
        assert_eq!(out, [100, 11, 12, 5, 5]);
    }

    #[test]
    fn mismatched_group_tables_and_wide_groups_are_errors() {
        let mut out = Vec::new();
        let mut reader = BitReader::new(&[0; 8]);
        assert!(expand_groups_into(&mut reader, &[1, 1], &[1], &[0, 0], &mut out).is_err());
        let err = expand_groups_into(&mut reader, &[1, 40], &[0, 0], &[0, 0], &mut out)
            .expect_err("a width of 40 is malformed");
        assert!(
            err.to_string().contains("group 1 width 40 exceeds 32 bits"),
            "{err}"
        );
    }

    /// Two groups: references 5 and 15 (4 bits), widths 2 and 0 (4 bits), stored
    /// lengths 2 and 9 (4 bits) with the last overridden to 3, then group 0's
    /// offsets 1 and 3 at 2 bits. Each block is padded to an octet.
    const TABLE: [u8; 4] = [0x5F, 0x20, 0x29, 0b0111_0000];

    fn layout(mvm: u8) -> ComplexGroupLayout {
        ComplexGroupLayout {
            num_groups: 2,
            reference_bits: 4,
            width_bits: 4,
            width_reference: 0,
            length_bits: 4,
            length_reference: 0,
            length_increment: 1,
            length_last: 3,
            missing_value_management: mvm,
        }
    }

    #[test]
    fn complex_groups_expand_with_the_last_length_overridden() {
        let mut reader = BitReader::new(&TABLE);
        let v = expand_complex_groups(&mut reader, &layout(0), 5).unwrap();
        assert_eq!(v, [Some(6), Some(8), Some(15), Some(15), Some(15)]);
    }

    #[test]
    fn all_ones_marks_a_point_or_a_zero_width_group_missing() {
        // Offset 3 is all ones at width 2; reference 15 is all ones at 4 bits.
        let mut reader = BitReader::new(&TABLE);
        let v = expand_complex_groups(&mut reader, &layout(1), 5).unwrap();
        assert_eq!(v, [Some(6), None, None, None, None]);
        // Management 2 also treats all ones minus one as missing: offset 2 at
        // width 2. Group 0's offsets are 1 and 3, so nothing more goes missing.
        let mut reader = BitReader::new(&TABLE);
        let v = expand_complex_groups(&mut reader, &layout(2), 5).unwrap();
        assert_eq!(v, [Some(6), None, None, None, None]);
        // Management 0 marks nothing.
        assert!(is_missing(1, 3, 2) && is_missing(2, 2, 2) && !is_missing(0, 3, 2));
        assert!(is_missing(1, i64::from(u32::MAX), 32));
    }

    #[test]
    fn malformed_complex_tables_are_errors() {
        let run = |l: ComplexGroupLayout, present: usize| {
            expand_complex_groups(&mut BitReader::new(&TABLE), &l, present)
        };
        // The lengths sum to 5, not 6.
        assert!(run(layout(0), 6).is_err());
        // More groups than present points, and no groups at all.
        assert!(
            run(
                ComplexGroupLayout {
                    num_groups: 9,
                    ..layout(0)
                },
                5
            )
            .is_err()
        );
        assert!(
            run(
                ComplexGroupLayout {
                    num_groups: 0,
                    ..layout(0)
                },
                5
            )
            .is_err()
        );
        // Field widths over 32.
        assert!(
            run(
                ComplexGroupLayout {
                    reference_bits: 33,
                    ..layout(0)
                },
                5
            )
            .is_err()
        );
        assert!(
            run(
                ComplexGroupLayout {
                    length_bits: 40,
                    ..layout(0)
                },
                5
            )
            .is_err()
        );
        // Each field fits, but the width reference pushes group 0 to 33 bits.
        let err = run(
            ComplexGroupLayout {
                width_reference: 31,
                ..layout(0)
            },
            5,
        )
        .expect_err("a 33-bit group is malformed");
        assert!(err.to_string().contains("group 0 width 33"), "{err}");
        // The buffer ends before the offsets do.
        assert!(expand_complex_groups(&mut BitReader::new(&TABLE[..3]), &layout(0), 5).is_err());
    }
}
