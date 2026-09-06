//! The two transposition filters: byte shuffle and bit shuffle.
//!
//! Neither compresses. Both reorder a buffer so that like-significance bits sit
//! together, which is what makes the compressor that runs after them effective:
//! the high bytes of a run of similar floats are nearly constant, and gathering
//! them turns a scattered pattern into a run.
//!
//! * **Byte shuffle** groups by position within the element — every element's
//!   byte 0, then every element's byte 1, and so on. This is the same transpose
//!   the HDF5 `shuffle` filter applies, and Zarr reaches it two ways: as the
//!   standalone `shuffle` filter in a v2 array, and as blosc's own `shuffle`
//!   setting inside a compressed block.
//! * **Bit shuffle** goes one level finer, transposing the whole bit matrix of
//!   the block: every element's bit 0, then every element's bit 1. It only
//!   exists inside blosc, and only over whole groups of eight elements — the
//!   `bitshuffle` library leaves a trailing group of fewer than eight
//!   untouched, and so must anything that reverses it.
//!
//! Both are their own inverse only for a square matrix, so both directions are
//! written out: the forward ones are `#[cfg(test)]`, because nothing in this
//! crate encodes, but a round-trip test is the only check that does not
//! restate the inverse it is testing.

/// Undo the byte shuffle: bytes were grouped by position-within-element, so
/// regroup them into consecutive elements.
///
/// A buffer that is not a whole number of elements has a tail with no defined
/// home. It is copied verbatim rather than dropped or transposed, which is what
/// `unshuffle_generic_inline` in c-blosc does with the same input — the
/// transpose covers `len / element_size` whole elements and the remaining
/// `len % element_size` bytes are moved across untouched.
#[must_use]
pub fn unshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    if element_size <= 1 {
        return data.to_vec();
    }
    let count = data.len() / element_size;
    let mut out = data.to_vec();
    for byte_pos in 0..element_size {
        let base = byte_pos * count;
        for elem in 0..count {
            out[elem * element_size + byte_pos] = data[base + elem];
        }
    }
    out
}

/// Undo the bit shuffle over a block of `element_size`-byte elements.
///
/// The stored layout is the transpose of the block's bit matrix: bit *b* of
/// every element, in element order, packed eight elements to a byte with the
/// lowest element index in the least-significant bit. Bits are numbered within
/// the element from its first stored byte's least-significant bit, which is
/// the order `bitshuffle` writes and blosc inherits.
///
/// **A block whose element count is not a multiple of eight was never
/// shuffled**, so it is returned untouched. That is not a tolerance this
/// decoder chose: `blosc_internal_bitunshuffle` refuses the transform outright
/// on such a block and copies the bytes, so a reverse that transposed the
/// aligned head anyway would corrupt every one of them. It is the classic
/// bitshuffle-decoder bug, and it is invisible on the common case where the
/// count divides.
#[must_use]
pub fn unbitshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    let Some(count) = transposable_elements(data.len(), element_size) else {
        return data.to_vec();
    };
    let mut out = data.to_vec();
    let row_bytes = count / 8;
    for bit in 0..element_size * 8 {
        let row = bit * row_bytes;
        for elem in 0..count {
            let stored = (data[row + elem / 8] >> (elem % 8)) & 1;
            let target = elem * element_size + bit / 8;
            let shift = bit % 8;
            // Every destination bit is written exactly once, so clearing first
            // is unnecessary — but `out` starts as a copy of the input, not as
            // zeros, so it is necessary here.
            out[target] = (out[target] & !(1 << shift)) | (stored << shift);
        }
    }
    out
}

/// The element count a bit transpose covers, or `None` when the block is one
/// blosc leaves alone.
///
/// The trailing `len % element_size` bytes are outside the transpose in either
/// case and are copied through by the callers.
const fn transposable_elements(len: usize, element_size: usize) -> Option<usize> {
    if element_size == 0 {
        return None;
    }
    let count = len / element_size;
    if count == 0 || !count.is_multiple_of(8) {
        return None;
    }
    Some(count)
}

#[cfg(test)]
pub(crate) fn shuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    if element_size <= 1 {
        return data.to_vec();
    }
    let count = data.len() / element_size;
    let mut out = data.to_vec();
    for elem in 0..count {
        for byte_pos in 0..element_size {
            out[byte_pos * count + elem] = data[elem * element_size + byte_pos];
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn bitshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    let Some(count) = transposable_elements(data.len(), element_size) else {
        return data.to_vec();
    };
    let mut out = data.to_vec();
    let row_bytes = count / 8;
    for bit in 0..element_size * 8 {
        let row = bit * row_bytes;
        for elem in 0..count {
            let source = (data[elem * element_size + bit / 8] >> (bit % 8)) & 1;
            let target = row + elem / 8;
            let shift = elem % 8;
            out[target] = (out[target] & !(1 << shift)) | (source << shift);
        }
    }
    out
}

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
        assert_eq!(
            unshuffle(&shuffled, 4),
            vec![0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05]
        );
    }

    #[test]
    fn shuffle_round_trips_at_every_width() {
        for element_size in [1usize, 2, 4, 8] {
            let data: Vec<u8> = (0..96u16).map(|i| (i * 7 % 251) as u8).collect();
            assert_eq!(
                unshuffle(&shuffle(&data, element_size), element_size),
                data,
                "byte shuffle at width {element_size}"
            );
        }
    }

    #[test]
    fn bitshuffle_round_trips_at_every_width() {
        for element_size in [1usize, 2, 4, 8] {
            let data: Vec<u8> = (0..128u16).map(|i| (i * 13 % 253) as u8).collect();
            assert_eq!(
                unbitshuffle(&bitshuffle(&data, element_size), element_size),
                data,
                "bit shuffle at width {element_size}"
            );
        }
    }

    /// An element count that is not a multiple of eight is left **entirely**
    /// alone, head included. Transposing the aligned head and copying the tail
    /// is the plausible-looking alternative, and it corrupts every such block:
    /// blosc never shuffled it in the first place.
    #[test]
    fn a_count_that_is_not_a_multiple_of_eight_is_untouched() {
        // 10 four-byte elements.
        let data: Vec<u8> = (0..40u8).collect();
        assert_eq!(bitshuffle(&data, 4), data);
        assert_eq!(unbitshuffle(&data, 4), data);
        // Fewer than eight elements, likewise.
        let short: Vec<u8> = (0..12u8).collect();
        assert_eq!(unbitshuffle(&short, 4), short);
    }

    /// The byte transpose covers whole elements and carries a ragged tail
    /// across verbatim, which is what c-blosc's own generic path does.
    #[test]
    fn the_byte_transpose_carries_a_ragged_tail_across() {
        let data: Vec<u8> = (0..35u8).collect(); // 8 four-byte elements + 3
        let out = unshuffle(&shuffle(&data, 4), 4);
        assert_eq!(out, data);
        assert_eq!(&shuffle(&data, 4)[32..], &data[32..]);
    }

    /// A degenerate width must not panic on the division.
    #[test]
    fn a_zero_width_is_passed_through() {
        let data = [1u8, 2, 3];
        assert_eq!(unshuffle(&data, 0), data);
        assert_eq!(unbitshuffle(&data, 0), data);
    }
}
