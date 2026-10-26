//! The byte shuffle: HDF5's `shuffle` filter (filter id 2) and the byte
//! `shuffle` of blosc and Zarr, which are one transform.
//!
//! The shuffle reorders a buffer of `element_size`-byte elements by position
//! within the element: every element's byte 0, then every element's byte 1,
//! and so on. It does not compress. It makes the compressor that runs after it
//! effective, because the high bytes of a run of similar numbers are nearly
//! constant and gathering them turns a scattered pattern into a run.
//!
//! Read as matrices, the stored buffer is an `element_size × count` matrix,
//! one row per byte position, and the elements are its transpose, a
//! `count × element_size` matrix, one row per element. `unshuffle` computes
//! that transpose and `shuffle` the one back.
//!
//! **A buffer that is not a whole number of elements** keeps its trailing
//! `len % element_size` bytes where they are. The transpose covers the
//! `len / element_size` whole elements and the tail is copied across
//! untouched, in both directions. That is what libhdf5 does
//! (`H5Z__filter_shuffle` in `H5Zshuffle.c` copies the "leftover" bytes after
//! the transposed ones) and what c-blosc does (`unshuffle_generic_inline`), so
//! one rule serves both readers. An element size of 0 or 1 leaves the buffer as
//! it is.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes, the `verus_keep_ghost`
//! `proof!` statements, and the `verus_keep_ghost` items at the bottom, which a
//! normal build never sees. What is proved is listed in
//! `docs/verification.md`: for every input, neither function indexes out of
//! bounds, overflows or panics, each output is exactly the transpose described
//! above, and each function undoes the other.
//!
//! The file names no other item of either crate, and its docs use plain
//! backticks rather than intra-doc links, which would resolve in only one of
//! the two. `tools/check_verified_kernels.py` fails if the verification crate
//! stops including it.

#[cfg(verus_keep_ghost)]
use vstd::arithmetic::div_mod::{
    lemma_div_pos_is_pos, lemma_fundamental_div_mod, lemma_mod_pos_bound,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Undo the byte shuffle: the bytes were grouped by position within the
/// element, so regroup them into consecutive elements.
///
/// Byte `b` of element `e` is read from `data[b * count + e]` and written to
/// `out[e * element_size + b]`, where `count` is `data.len() / element_size`.
/// A trailing partial element is copied across untouched, and an
/// `element_size` of 0 or 1 returns the bytes as they are (see the module
/// docs).
///
/// Proved (see the module docs): no input makes it index out of bounds,
/// overflow or panic; the result has `data`'s length; and every byte is where
/// the rule above puts it.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        is_unshuffle(data@, element_size as int, out@),
))]
#[must_use]
pub fn unshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(data);
    #[cfg(verus_keep_ghost)]
    proof! { assert(out@ =~= data@); }
    if element_size <= 1 {
        return out;
    }
    let count = data.len() / element_size;
    #[cfg(verus_keep_ghost)]
    proof! { lemma_whole_elements(data@.len() as int, element_size as int); }
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            1 < element_size,
            count == data@.len() as int / element_size as int,
            count * element_size <= data@.len() <= usize::MAX,
            out@.len() == data@.len(),
            forall|e: int, b: int|
                0 <= e < count && 0 <= b < it.index@ ==> out@[#[trigger] at(e, b, element_size as int)]
                    == data@[at(b, e, count as int)],
            forall|k: int| count * element_size <= k < data@.len() ==> #[trigger] out@[k] == data@[k],
    ))]
    for byte_pos in 0..element_size {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_row(byte_pos as int, count as int, element_size as int); }
        let base = byte_pos * count;
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                1 < element_size,
                byte_pos < element_size,
                count == data@.len() as int / element_size as int,
                count * element_size <= data@.len() <= usize::MAX,
                base == byte_pos * count,
                base + count <= count * element_size,
                out@.len() == data@.len(),
                forall|e: int, b: int|
                    0 <= e < count && 0 <= b < byte_pos ==> out@[#[trigger] at(e, b, element_size as int)]
                        == data@[at(b, e, count as int)],
                forall|e: int|
                    0 <= e < it.index@ ==> out@[#[trigger] at(e, byte_pos as int, element_size as int)]
                        == data@[at(byte_pos as int, e, count as int)],
                forall|k: int| count * element_size <= k < data@.len() ==> #[trigger] out@[k] == data@[k],
        ))]
        for elem in 0..count {
            #[cfg(verus_keep_ghost)]
            proof! {
                lemma_at(elem as int, byte_pos as int, element_size as int, count as int);
                lemma_at(byte_pos as int, elem as int, count as int, element_size as int);
            }
            out[elem * element_size + byte_pos] = data[base + elem];
        }
    }
    out
}

/// Apply the byte shuffle: the inverse of `unshuffle`, grouping the bytes of
/// consecutive elements by their position within the element.
///
/// Byte `b` of element `e` is read from `data[e * element_size + b]` and
/// written to `out[b * count + e]`. The tail and a degenerate width are
/// treated as in `unshuffle`.
///
/// No reader calls it at runtime: decoding only ever unshuffles. It exists
/// for the round-trip proof, so that `unshuffle` is proved to undo it rather
/// than a test restating the transpose, and for tests, which make shuffled
/// bytes with this proved function instead of a copy of it.
///
/// Proved (see the module docs), as for `unshuffle`, and in addition that
/// each of the two functions undoes the other.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        is_shuffle(data@, element_size as int, out@),
))]
#[must_use]
pub fn shuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(data);
    #[cfg(verus_keep_ghost)]
    proof! { assert(out@ =~= data@); }
    if element_size <= 1 {
        return out;
    }
    let count = data.len() / element_size;
    #[cfg(verus_keep_ghost)]
    proof! { lemma_whole_elements(data@.len() as int, element_size as int); }
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            1 < element_size,
            count == data@.len() as int / element_size as int,
            count * element_size <= data@.len() <= usize::MAX,
            out@.len() == data@.len(),
            forall|e: int, b: int|
                0 <= e < count && 0 <= b < it.index@ ==> out@[#[trigger] at(b, e, count as int)]
                    == data@[at(e, b, element_size as int)],
            forall|k: int| count * element_size <= k < data@.len() ==> #[trigger] out@[k] == data@[k],
    ))]
    for byte_pos in 0..element_size {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_row(byte_pos as int, count as int, element_size as int); }
        let base = byte_pos * count;
        #[cfg_attr(verus_keep_ghost, verus_spec(it =>
            invariant
                1 < element_size,
                byte_pos < element_size,
                count == data@.len() as int / element_size as int,
                count * element_size <= data@.len() <= usize::MAX,
                base == byte_pos * count,
                base + count <= count * element_size,
                out@.len() == data@.len(),
                forall|e: int, b: int|
                    0 <= e < count && 0 <= b < byte_pos ==> out@[#[trigger] at(b, e, count as int)]
                        == data@[at(e, b, element_size as int)],
                forall|e: int|
                    0 <= e < it.index@ ==> out@[#[trigger] at(byte_pos as int, e, count as int)]
                        == data@[at(e, byte_pos as int, element_size as int)],
                forall|k: int| count * element_size <= k < data@.len() ==> #[trigger] out@[k] == data@[k],
        ))]
        for elem in 0..count {
            #[cfg(verus_keep_ghost)]
            proof! {
                lemma_at(elem as int, byte_pos as int, element_size as int, count as int);
                lemma_at(byte_pos as int, elem as int, count as int, element_size as int);
            }
            out[base + elem] = data[elem * element_size + byte_pos];
        }
    }
    out
}

#[cfg(verus_keep_ghost)]
verus! {

/// Position of entry (`row`, `col`) of a matrix with `width` columns, stored
/// row after row.
pub open spec fn at(row: int, col: int, width: int) -> int {
    row * width + col
}

/// How many whole elements of `es` bytes a `len`-byte buffer holds; the
/// transpose covers exactly these.
pub open spec fn whole(len: int, es: int) -> int {
    len / es
}

/// `out` is `data` unshuffled at element size `es`. For `es > 1`, with
/// `count` whole elements: byte `b` of element `e` comes from row `b`,
/// column `e` of the stored `es × count` matrix, and the tail past the whole
/// elements is unchanged. For `es <= 1`, nothing moves.
pub open spec fn is_unshuffle(data: Seq<u8>, es: int, out: Seq<u8>) -> bool {
    let count = whole(data.len() as int, es);
    &&& out.len() == data.len()
    &&& es <= 1 ==> out == data
    &&& es > 1 ==> forall|e: int, b: int|
        0 <= e < count && 0 <= b < es ==> #[trigger] out[at(e, b, es)] == data[at(b, e, count)]
    &&& es > 1 ==> forall|k: int| count * es <= k < data.len() ==> #[trigger] out[k] == data[k]
}

/// `out` is `data` shuffled at element size `es`: the same transpose read the
/// other way, so byte `b` of element `e` goes to row `b`, column `e`.
pub open spec fn is_shuffle(data: Seq<u8>, es: int, out: Seq<u8>) -> bool {
    let count = whole(data.len() as int, es);
    &&& out.len() == data.len()
    &&& es <= 1 ==> out == data
    &&& es > 1 ==> forall|e: int, b: int|
        0 <= e < count && 0 <= b < es ==> #[trigger] out[at(b, e, count)] == data[at(e, b, es)]
    &&& es > 1 ==> forall|k: int| count * es <= k < data.len() ==> #[trigger] out[k] == data[k]
}

/// The whole elements fit in the buffer.
pub proof fn lemma_whole_elements(len: int, es: int)
    requires
        0 <= len,
        0 < es,
    ensures
        0 <= whole(len, es),
        whole(len, es) * es <= len,
{
    lemma_div_pos_is_pos(len, es);
    lemma_fundamental_div_mod(len, es);
    lemma_mod_pos_bound(len, es);
    assert(whole(len, es) * es == es * (len / es)) by (nonlinear_arith);
}

/// Entry (`row`, `col`) of a `rows × width` matrix lies inside it, and no
/// other entry shares its position. The second half is what lets a loop write
/// one entry without disturbing the ones it wrote before.
pub proof fn lemma_at(row: int, col: int, width: int, rows: int)
    requires
        0 <= row < rows,
        0 <= col < width,
    ensures
        0 <= at(row, col, width),
        at(row, col, width) + 1 <= rows * width,
        at(row, col, width) < row * width + width,
        row * width + width <= rows * width,
        forall|r: int, c: int|
            0 <= r && 0 <= c < width && (r != row || c != col) ==> #[trigger] at(r, c, width) != at(
                row,
                col,
                width,
            ),
{
    assert(0 <= row * width) by (nonlinear_arith)
        requires
            0 <= row,
            0 <= width,
    ;
    assert(row * width + width <= rows * width) by (nonlinear_arith)
        requires
            row + 1 <= rows,
            0 <= width,
    ;
    assert forall|r: int, c: int|
        0 <= r && 0 <= c < width && (r != row || c != col) implies #[trigger] at(r, c, width) != at(
        row,
        col,
        width,
    ) by {
        if r < row {
            assert(r * width + c < row * width + col) by (nonlinear_arith)
                requires
                    r + 1 <= row,
                    0 <= c < width,
                    0 <= col,
            ;
        } else if r > row {
            assert(row * width + col < r * width + c) by (nonlinear_arith)
                requires
                    row + 1 <= r,
                    0 <= col < width,
                    0 <= c,
            ;
        }
    }
}

/// Row `row` of a `rows × width` matrix starts at `row * width` and ends
/// inside the matrix, and the matrix holds `rows * width` entries whichever
/// way round the product is written.
pub proof fn lemma_row(row: int, width: int, rows: int)
    requires
        0 <= row < rows,
        0 <= width,
    ensures
        0 <= row * width,
        row * width + width <= rows * width,
        rows * width == width * rows,
{
    assert(0 <= row * width) by (nonlinear_arith)
        requires
            0 <= row,
            0 <= width,
    ;
    assert(row * width + width <= rows * width) by (nonlinear_arith)
        requires
            row + 1 <= rows,
            0 <= width,
    ;
    assert(rows * width == width * rows) by (nonlinear_arith);
}

/// Every position below `rows * width` is some entry (`row`, `col`) of a
/// `rows × width` matrix, namely (`k / width`, `k % width`).
pub proof fn lemma_cover(k: int, width: int, rows: int)
    requires
        0 <= k < rows * width,
        0 < width,
    ensures
        0 <= k / width < rows,
        0 <= k % width < width,
        k == at(k / width, k % width, width),
{
    lemma_div_pos_is_pos(k, width);
    lemma_fundamental_div_mod(k, width);
    lemma_mod_pos_bound(k, width);
    let q = k / width;
    assert(q < rows) by (nonlinear_arith)
        requires
            k == width * q + k % width,
            0 <= k % width,
            k < rows * width,
            0 < width,
    ;
    assert(k == q * width + k % width) by (nonlinear_arith)
        requires
            k == width * q + k % width,
    ;
}

/// Shuffling undoes unshuffling: whatever `unshuffle` returns, `shuffle`
/// turns back into the bytes it started from.
pub proof fn lemma_shuffle_undoes_unshuffle(data: Seq<u8>, es: int, u: Seq<u8>, s: Seq<u8>)
    requires
        is_unshuffle(data, es, u),
        is_shuffle(u, es, s),
    ensures
        s == data,
{
    if es > 1 {
        let count = whole(data.len() as int, es);
        lemma_whole_elements(data.len() as int, es);
        assert forall|k: int| 0 <= k < data.len() implies s[k] == data[k] by {
            if k < count * es {
                assert(count * es == es * count) by (nonlinear_arith);
                assert(0 < count) by (nonlinear_arith)
                    requires
                        0 <= k < es * count,
                        0 <= count,
                ;
                lemma_cover(k, count, es);
                let b = k / count;
                let e = k % count;
                assert(s[at(b, e, count)] == u[at(e, b, es)]);
            }
        }
        assert(s =~= data);
    }
}

/// Unshuffling undoes shuffling.
pub proof fn lemma_unshuffle_undoes_shuffle(data: Seq<u8>, es: int, s: Seq<u8>, u: Seq<u8>)
    requires
        is_shuffle(data, es, s),
        is_unshuffle(s, es, u),
    ensures
        u == data,
{
    if es > 1 {
        let count = whole(data.len() as int, es);
        lemma_whole_elements(data.len() as int, es);
        assert forall|k: int| 0 <= k < data.len() implies u[k] == data[k] by {
            if k < count * es {
                lemma_cover(k, es, count);
                let e = k / es;
                let b = k % es;
                assert(u[at(e, b, es)] == s[at(b, e, count)]);
            }
        }
        assert(u =~= data);
    }
}

/// `is_unshuffle` pins the output down completely: two outputs that both
/// satisfy it are equal. So the `ensures` of `unshuffle` is its whole
/// behaviour, not one property of it.
pub proof fn lemma_unshuffle_is_determined(data: Seq<u8>, es: int, a: Seq<u8>, b: Seq<u8>)
    requires
        is_unshuffle(data, es, a),
        is_unshuffle(data, es, b),
    ensures
        a == b,
{
    if es > 1 {
        let count = whole(data.len() as int, es);
        lemma_whole_elements(data.len() as int, es);
        assert forall|k: int| 0 <= k < data.len() implies a[k] == b[k] by {
            if k < count * es {
                lemma_cover(k, es, count);
                let e = k / es;
                let p = k % es;
                assert(a[at(e, p, es)] == data[at(p, e, count)]);
                assert(b[at(e, p, es)] == data[at(p, e, count)]);
            }
        }
        assert(a =~= b);
    }
}

/// Likewise for `is_shuffle`.
pub proof fn lemma_shuffle_is_determined(data: Seq<u8>, es: int, a: Seq<u8>, b: Seq<u8>)
    requires
        is_shuffle(data, es, a),
        is_shuffle(data, es, b),
    ensures
        a == b,
{
    if es > 1 {
        let count = whole(data.len() as int, es);
        lemma_whole_elements(data.len() as int, es);
        assert forall|k: int| 0 <= k < data.len() implies a[k] == b[k] by {
            if k < count * es {
                assert(count * es == es * count) by (nonlinear_arith);
                assert(0 < count) by (nonlinear_arith)
                    requires
                        0 <= k < es * count,
                        0 <= count,
                ;
                lemma_cover(k, count, es);
                let p = k / count;
                let e = k % count;
                assert(a[at(p, e, count)] == data[at(e, p, es)]);
                assert(b[at(p, e, count)] == data[at(e, p, es)]);
            }
        }
        assert(a =~= b);
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    /// The known layout, spelled out rather than computed, so this test does
    /// not restate the implementation.
    #[test]
    fn unshuffle_regroups_a_known_layout() {
        // Two 4-byte elements, 04 03 02 01 and 08 07 06 05, shuffled to
        // [04 08][03 07][02 06][01 05].
        let shuffled = [0x04, 0x08, 0x03, 0x07, 0x02, 0x06, 0x01, 0x05];
        let elements = [0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05];
        assert_eq!(unshuffle(&shuffled, 4), elements);
        assert_eq!(shuffle(&elements, 4), shuffled);
    }

    /// Three 2-byte elements: not a square matrix, so a transpose that
    /// confused rows and columns would show here.
    #[test]
    fn a_non_square_layout_transposes_the_right_way() {
        let elements = [0xA0, 0xA1, 0xB0, 0xB1, 0xC0, 0xC1];
        let shuffled = [0xA0, 0xB0, 0xC0, 0xA1, 0xB1, 0xC1];
        assert_eq!(shuffle(&elements, 2), shuffled);
        assert_eq!(unshuffle(&shuffled, 2), elements);
    }

    #[test]
    fn both_directions_round_trip_at_every_width() {
        for element_size in 0usize..=9 {
            for len in [0usize, 1, 7, 8, 35, 96] {
                let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
                assert_eq!(unshuffle(&shuffle(&data, element_size), element_size), data);
                assert_eq!(shuffle(&unshuffle(&data, element_size), element_size), data);
            }
        }
    }

    /// The ragged tail stays where it is, as libhdf5 and c-blosc leave it, and
    /// the whole elements before it are still transposed.
    #[test]
    fn a_ragged_tail_is_carried_across_and_the_rest_transposed() {
        // Two 4-byte elements and three trailing bytes.
        let data = [
            0x04, 0x08, 0x03, 0x07, 0x02, 0x06, 0x01, 0x05, 0xE0, 0xE1, 0xE2,
        ];
        assert_eq!(
            unshuffle(&data, 4),
            [
                0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05, 0xE0, 0xE1, 0xE2
            ]
        );
        // Fewer bytes than one element: nothing to transpose.
        assert_eq!(unshuffle(&[1, 2, 3], 4), [1, 2, 3]);
    }

    /// A degenerate width must not divide by zero, and one byte per element
    /// has nothing to reorder.
    #[test]
    fn widths_of_zero_and_one_pass_the_bytes_through() {
        let data = [1u8, 2, 3];
        assert_eq!(unshuffle(&data, 0), data);
        assert_eq!(shuffle(&data, 0), data);
        assert_eq!(unshuffle(&data, 1), data);
        assert_eq!(shuffle(&data, 1), data);
    }
}
