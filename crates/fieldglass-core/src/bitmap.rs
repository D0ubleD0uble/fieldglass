//! Presence bitmaps: one bit per point, packed MSB-first, a set bit meaning
//! the point is present.
//!
//! GRIB stores a missing-value mask this way in several places, and they all
//! read it the same way:
//!
//! - the GRIB1 Bit Map Section (Section 3) and the GRIB2 Bit-Map Section
//!   (Section 6), one bit per grid point;
//! - the secondary bitmaps of matrix-of-values packing (GRIB1 and GRIB2
//!   template 5.1), one bit per matrix cell of each present point;
//! - the secondary bitmap of GRIB1 second-order packing (`constant_width`,
//!   `general_grib1`), one bit per point, a set bit starting a new group.
//!
//! Bit `i` is bit `7 - i % 8` of byte `i / 8`, so point 0 is the most
//! significant bit of the first byte. Bits past the last point, padding to the
//! octet or to the section's length, are never read.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes, the `verus_keep_ghost`
//! `proof!` statements, and the `verus_keep_ghost` items at the bottom, which a
//! normal build never sees. What is proved is listed in
//! `docs/verification.md`: none of the three functions has a precondition, no
//! input makes one index out of bounds, overflow or panic, and each result is
//! exactly the one described on the function.
//!
//! The file names only `crate::bits_model` (the bit reader's model), under
//! Verus only, and its docs use plain backticks rather than intra-doc links,
//! which would resolve in only one of the two crates. `tools/check_verified_kernels.py` fails if the
//! verification crate stops including it.

#[cfg(verus_keep_ghost)]
use crate::bits_model::{bit, msb_bits};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Unpack the first `count` bits of `bytes`, MSB-first, one `bool` per point:
/// `true` where the bit is set.
///
/// Returns `None` when `bytes` holds fewer than `count` bits, so a caller can
/// name the section in its own error. Bits past `count` are ignored.
///
/// Proved (see the module docs): `Some` exactly when `count` is at most
/// `8 · bytes.len()`; then the result has `count` entries and entry `i` is
/// bit `i` of `bytes`, read MSB-first.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Some <==> count <= bytes@.len() * 8,
        r matches Some(v) ==> v@ == bitmap_spec(bytes@, count as int),
))]
#[must_use]
pub fn unpack_bitmap(bytes: &[u8], count: usize) -> Option<Vec<bool>> {
    // `count.div_ceil(8)`, spelled with `is_multiple_of`, which the pinned
    // `vstd` specifies and `div_ceil` it does not.
    let needed = count / 8 + if count.is_multiple_of(8) { 0 } else { 1 };
    if bytes.len() < needed {
        return None;
    }
    let mut bits = Vec::with_capacity(count);
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            count <= bytes@.len() * 8,
            bits@.len() == it.index@,
            forall|k: int| 0 <= k < it.index@ ==> #[trigger] bits@[k] == (bit(bytes@, k) == 1),
    ))]
    for i in 0..count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_bit_of_byte(bytes@, i as int); }
        let byte = bytes[i / 8];
        bits.push((byte >> (7 - i % 8)) & 1 != 0);
    }
    #[cfg(verus_keep_ghost)]
    proof! { assert(bits@ =~= bitmap_spec(bytes@, count as int)); }
    Some(bits)
}

/// The number of bits a bitmap body of `body_len` octets holds when its last
/// `unused_trailing` bits are padding: `8 · body_len − unused_trailing`.
///
/// GRIB1 states that padding count in octet 4 of its Bit Map Section. Returns
/// `None` when the count is larger than the body, or the body's bit length
/// does not fit a `usize`.
///
/// Proved (see the module docs): `Some` exactly in the other case, and then
/// the value is exact.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Some <==> body_len * 8 <= usize::MAX && unused_trailing <= body_len * 8,
        r matches Some(n) ==> n == body_len * 8 - unused_trailing,
))]
#[must_use]
pub fn bitmap_bit_len(body_len: usize, unused_trailing: u8) -> Option<usize> {
    match body_len.checked_mul(8) {
        Some(total) => total.checked_sub(unused_trailing as usize),
        None => None,
    }
}

/// The number of present points in an unpacked bitmap: how many entries are
/// `true`.
///
/// Proved (see the module docs): the result is that count, and it cannot
/// overflow. For a result of `unpack_bitmap(bytes, n)` that is the number of
/// set bits among the first `n` bits of `bytes`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == count_present_spec(bits@),
))]
#[must_use]
#[allow(
    clippy::needless_range_loop,
    reason = "the proof's loop invariant counts by index"
)]
pub fn count_present(bits: &[bool]) -> usize {
    let mut n: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            n == count_present_spec(bits@.take(it.index@ as int)),
            n <= it.index@,
    ))]
    for i in 0..bits.len() {
        #[cfg(verus_keep_ghost)]
        proof! { assert(bits@.take(i as int + 1).drop_last() =~= bits@.take(i as int)); }
        if bits[i] {
            n += 1;
        }
    }
    #[cfg(verus_keep_ghost)]
    proof! { assert(bits@.take(bits@.len() as int) =~= bits@); }
    n
}

#[cfg(verus_keep_ghost)]
verus! {

/// The first `count` bits of `bytes`, MSB-first, `true` where a bit is set.
pub open spec fn bitmap_spec(bytes: Seq<u8>, count: int) -> Seq<bool> {
    Seq::new(count as nat, |i: int| bit(bytes, i) == 1)
}

/// How many entries of `bits` are `true`.
pub open spec fn count_present_spec(bits: Seq<bool>) -> nat
    decreases bits.len(),
{
    if bits.len() == 0 {
        0
    } else {
        count_present_spec(bits.drop_last()) + if bits.last() {
            1nat
        } else {
            0nat
        }
    }
}

/// The sum of the first `n` bits of `bytes`, MSB-first.
pub open spec fn popcount(bytes: Seq<u8>, n: int) -> nat
    decreases n,
{
    if n <= 0 {
        0
    } else {
        popcount(bytes, n - 1) + bit(bytes, n - 1)
    }
}

/// Counting the present entries of an unpacked bitmap is counting its set
/// bits: with the `ensures` of `unpack_bitmap` and `count_present`, the count a
/// decoder takes for its present points is `popcount(bytes, count)`.
pub proof fn lemma_present_is_popcount(bytes: Seq<u8>, n: int)
    requires
        0 <= n,
    ensures
        count_present_spec(bitmap_spec(bytes, n)) == popcount(bytes, n),
    decreases n,
{
    if n > 0 {
        lemma_present_is_popcount(bytes, n - 1);
        assert(bitmap_spec(bytes, n).drop_last() =~= bitmap_spec(bytes, n - 1));
        assert(bit(bytes, n - 1) < 2);
    }
}

/// Entry `i` of an unpacked bitmap is what the proved bit reader's
/// `read_bits(1)` returns at bit `i`: the matrix and second-order secondary
/// bitmaps, which used to be read that way, read the same bits.
pub proof fn lemma_one_bit_read(bytes: Seq<u8>, i: int)
    ensures
        msb_bits(bytes, i, 1) == bit(bytes, i),
{
    assert(msb_bits(bytes, i, 0) == 0);
}

/// The shift and mask the loop computes pick out bit `i` of `bytes`, as the
/// MSB-first model of the bit reader defines it.
pub proof fn lemma_bit_of_byte(bytes: Seq<u8>, i: int)
    requires
        0 <= i,
        i < bytes.len() * 8,
    ensures
        0 <= i / 8 < bytes.len(),
        ((bytes[i / 8] >> ((7 - i % 8) as u8)) & 1u8 != 0u8) == (bit(bytes, i) == 1),
{
    let byte = bytes[i / 8];
    let s = (7 - i % 8) as u8;
    vstd::bits::lemma_u8_shr_is_div(byte, s);
    let y = byte >> s;
    assert(y & 1u8 == y % 2u8) by (bit_vector);
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitReader;

    /// The known layout, spelled out rather than computed, so this test does
    /// not restate the implementation.
    #[test]
    fn point_zero_is_the_most_significant_bit() {
        let bits = unpack_bitmap(&[0b1000_0001, 0b0100_0000], 10).unwrap();
        assert_eq!(
            bits,
            [
                true, false, false, false, false, false, false, true, false, true
            ]
        );
    }

    /// The matrix and second-order secondary bitmaps used to be read with
    /// `read_bits(1)`; this is the same bits, for every length over a buffer
    /// with no pattern to it.
    #[test]
    fn agrees_with_one_bit_reads() {
        let bytes: Vec<u8> = (0u32..37).map(|i| (i * 151 + 7) as u8).collect();
        for count in 0..=bytes.len() * 8 {
            let mut reader = BitReader::new(&bytes);
            let expected: Vec<bool> = (0..count)
                .map(|_| reader.read_bits(1).unwrap() != 0)
                .collect();
            let bits = unpack_bitmap(&bytes, count).unwrap();
            assert_eq!(bits, expected, "count {count}");
            let present = expected.iter().filter(|b| **b).count();
            assert_eq!(count_present(&bits), present, "count {count}");
        }
    }

    #[test]
    fn a_buffer_one_bit_short_is_none() {
        assert_eq!(unpack_bitmap(&[0xFF, 0xFF], 17), None);
        assert_eq!(unpack_bitmap(&[], 1), None);
        assert_eq!(unpack_bitmap(&[], 0), Some(Vec::new()));
        // Padding past the last point is never read.
        assert_eq!(unpack_bitmap(&[0xFF, 0xFF], 16).map(|b| b.len()), Some(16));
    }

    #[test]
    fn bit_len_subtracts_the_padding_and_refuses_too_much() {
        assert_eq!(bitmap_bit_len(2, 0), Some(16));
        assert_eq!(bitmap_bit_len(2, 7), Some(9));
        assert_eq!(bitmap_bit_len(2, 16), Some(0));
        assert_eq!(bitmap_bit_len(2, 17), None);
        assert_eq!(bitmap_bit_len(0, 1), None);
        assert_eq!(bitmap_bit_len(usize::MAX, 0), None);
    }

    #[test]
    fn count_present_counts_the_set_entries() {
        assert_eq!(count_present(&[]), 0);
        assert_eq!(count_present(&[false, true, true, false, true]), 3);
    }
}
