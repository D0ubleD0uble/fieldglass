//! Where the HDF5 reader's bytes come from (#682, [ADR-0005]).
//!
//! Every other module here addresses the file by absolute offset — an object
//! header at `0x2f8`, a B-tree node at `0x1a40`, a chunk at `0x9c00`. Before
//! this module they did it by indexing a `&[u8]` that had to be the whole file.
//! Now they do it through [`ByteSource`], so the same walk runs over a buffer,
//! over a host's prefetched ranges, and over whatever transport #247 and #252
//! add, with no traversal code learning which it got.
//!
//! # Two cursors, and the difference matters
//!
//! [`Cursor`] reads a buffer the caller already holds — a header message body,
//! a decompressed heap block. It borrows, costs nothing, and is what nearly
//! every *decoder* in this tree wants.
//!
//! [`FileCursor`] reads the *file*, at an absolute address, through the seam.
//! It is what the *traversal* wants, and it is deliberately the only thing in
//! this crate that turns a file offset into bytes. It lives in
//! [`fieldglass_core::bytes`] since the GRIB scan came to need the same growing
//! window (#697); what that window is for, and why it grows, is documented
//! there. This module adds the little-endian field reads HDF5 structures are
//! made of.
//!
//! # What this module deliberately does not do
//!
//! **It does not prefetch.** ADR-0005 records that HDF5 fails the strong form
//! of the "state your ranges up front" constraint: a traversal is a chain of
//! dependent reads, and the address of the next structure is inside the current
//! one. The one place a real plan exists is the chunk-fetch loop, once the
//! chunk index has been walked, and that is where
//! [`ByteSource::prefetch`] is called — in
//! [`values`](super::values), not here.
//!
//! [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteSource, checked_usize};

pub(crate) use fieldglass_core::bytes::{FileCursor, read_at, read_up_to, scan_windows};

use super::object_header::read_uint_le;

/// The little-endian field reads both cursors offer.
///
/// A great many HDF5 structures are runs of fixed-width fields, and the same
/// run turns up both inside a buffer already in hand (a B-tree v2 record) and
/// at a file address (a Fixed Array data block). A decoder for one is a decoder
/// for the other, so the chunk-index readers are written against this rather
/// than against either cursor — which is what lets one of them serve the three
/// chunk indexes that reach it from both directions.
pub(crate) trait Fields {
    /// Read a little-endian unsigned integer of `width` bytes (1..=8).
    fn uint(&mut self, width: usize) -> Result<u64, FieldglassError>;

    /// Advance `n` bytes without decoding them.
    fn skip(&mut self, n: usize) -> Result<(), FieldglassError>;

    /// Read a little-endian unsigned integer of `width` bytes and narrow it to
    /// `usize`, so a length or offset that a 32-bit target cannot address fails
    /// the parse instead of wrapping into one it can. See
    /// [`fieldglass_core::bytes::checked_usize`]; only the 8-byte-wide HDF5
    /// length and offset fields can actually exceed a 32-bit `usize`.
    fn usize(&mut self, width: usize) -> Result<usize, FieldglassError> {
        checked_usize(self.uint(width)?, "HDF5 length or offset")
    }

    /// Read a little-endian `u16`.
    fn u16(&mut self) -> Result<u16, FieldglassError> {
        Ok(self.uint(2)? as u16)
    }

    /// Read one byte.
    fn byte(&mut self) -> Result<u8, FieldglassError> {
        Ok(self.uint(1)? as u8)
    }
}

/// A tiny forward cursor over a byte slice, reading little-endian fields with
/// bounds checks.
///
/// For bytes the caller already holds — a header message body, a decompressed
/// heap block, one B-tree record. [`FileCursor`] is the one that reaches the
/// file.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn over(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// The bytes from the current position to the end of the buffer.
    pub(crate) fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], FieldglassError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| FieldglassError::Parse("read past end of buffer".into()))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }
}

impl Fields for Cursor<'_> {
    fn uint(&mut self, width: usize) -> Result<u64, FieldglassError> {
        let value = read_uint_le(self.bytes, self.pos, width)?;
        self.pos += width;
        Ok(value)
    }

    fn skip(&mut self, n: usize) -> Result<(), FieldglassError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| FieldglassError::Parse("skip past end of buffer".into()))?;
        self.pos = end;
        Ok(())
    }
}

impl<S: ByteSource + ?Sized> Fields for FileCursor<'_, S> {
    fn uint(&mut self, width: usize) -> Result<u64, FieldglassError> {
        let bytes = self.take(width)?;
        read_uint_le(bytes, 0, width)
    }

    /// Skipping does not fetch — see [`FileCursor::skip`].
    fn skip(&mut self, n: usize) -> Result<(), FieldglassError> {
        FileCursor::skip(self, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_a_32_bit_target_cannot_address_is_an_error_not_a_wrap() {
        // An 8-byte HDF5 length field holding 0x1_0000_0001. Narrowed with a
        // bare `as usize` this reads as 1 where `usize` is 32 bits wide — in
        // range, past every bounds check that follows, and the reader goes on
        // to take one byte where the file said four gigabytes. The assertion
        // has to hold on both widths: a 64-bit target must pass the value
        // through unchanged, a 32-bit one must refuse it. Neither may wrap.
        let bytes = [0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
        let mut cur = Cursor::over(&bytes);
        match cur.usize(8) {
            Ok(n) => {
                assert_eq!(usize::BITS, 64, "only a 64-bit target can hold this");
                assert_eq!(n as u64, 0x1_0000_0001);
            }
            Err(err) => {
                assert_eq!(usize::BITS, 32);
                assert!(
                    err.to_string().contains("does not fit"),
                    "unexpected message: {err}"
                );
            }
        }
        // Four bytes or fewer always fit, on either width.
        let mut cur = Cursor::over(&bytes);
        assert_eq!(cur.usize(4).unwrap(), 1);
    }

    /// The same narrowing check, on the cursor that reads the file. The two
    /// share no code, so one holding the line says nothing about the other.
    #[test]
    fn the_file_cursor_narrows_with_the_same_check() {
        let bytes: Vec<u8> = vec![0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        match cur.usize(8) {
            Ok(n) => {
                assert_eq!(usize::BITS, 64);
                assert_eq!(n as u64, 0x1_0000_0001);
            }
            Err(err) => {
                assert_eq!(usize::BITS, 32);
                assert!(err.to_string().contains("does not fit"));
            }
        }
    }

    /// Field reads that cross the window boundary have to see the same bytes a
    /// single window would. The window itself is `core`'s and tested there;
    /// this is the little-endian read on top of it.
    #[test]
    fn field_reads_across_windows_land_on_the_right_words() {
        let mut bytes = Vec::new();
        for i in 0..4096u32 {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        for i in 0..4096u32 {
            assert_eq!(cur.uint(4).expect("word in range"), u64::from(i));
        }
        assert!(cur.byte().is_err(), "the file ends here");

        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        Fields::skip(&mut cur, 4 * 3000).expect("in range");
        assert_eq!(cur.uint(4).expect("word in range"), 3000);
    }
}
