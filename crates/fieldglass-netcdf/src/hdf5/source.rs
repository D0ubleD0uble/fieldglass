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
//! this crate that turns a file offset into bytes.
//!
//! # Why `FileCursor` reads a window and not a field
//!
//! A structure walk asks for two bytes here and eight there. One
//! [`ByteSource::read`] per field would be free over a buffer and a round trip
//! per integer over a network — which is the cost ADR-0005 exists to keep
//! visible rather than to hide. So a `FileCursor` refills in windows of at
//! least [`WINDOW_BYTES`], and a B-tree node or a heap block is then one read
//! rather than forty.
//!
//! The window is a ceiling on waste, not a promise: a read larger than it is
//! served in one call, and a window is clamped at the end of the file, so the
//! last structure never asks for bytes that are not there.
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
use fieldglass_core::bytes::{ByteRange, ByteSource, checked_usize};
use std::borrow::Cow;

use super::object_header::read_uint_le;

/// A [`FileCursor`]'s first window.
///
/// Small on purpose. Most structures the traversal walks are a few dozen bytes
/// — a B-tree node prefix, a symbol-table entry, an array header — and a
/// generous first window would read a hundred times what they need. 64 bytes
/// covers the prefix of every structure in the format.
const FIRST_WINDOW_BYTES: usize = 64;

/// The largest window a [`FileCursor`] will grow to.
///
/// Each refill doubles, so a cursor that keeps going — a long B-tree node, a
/// heap direct block — reaches a structure of `N` bytes in `log2(N/64)` reads
/// having fetched under `2N`, rather than one read per field (a round trip per
/// integer over a transport) or one fixed large window per structure (a
/// hundredfold over-read on the small ones). Both axes matter and this is the
/// shape that bounds both.
const MAX_WINDOW_BYTES: usize = 64 << 10;

/// Read exactly `len` bytes at file address `addr`.
///
/// "Exactly" is the whole point. [`ByteSource::read`] bounds-checks the range
/// against the source's own size, which an in-memory buffer can always honour;
/// a transport with a truncated response cannot, and every caller here goes on
/// to index the result at offsets the structure told it about. So the length is
/// checked once, here, rather than at each of the forty call sites that would
/// otherwise each have to — the same guard `decode_variable_raw_from` grew for
/// NetCDF classic, at the level where it covers the whole reader.
pub(crate) fn read_at<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    len: usize,
) -> Result<Cow<'_, [u8]>, FieldglassError> {
    let got = source.read(ByteRange::new(addr, len as u64))?;
    if got.len() != len {
        return Err(FieldglassError::Parse(format!(
            "the source served {} of {len} bytes at {addr}",
            got.len()
        )));
    }
    Ok(got)
}

/// Read up to `len` bytes at `addr`, stopping at the end of the file.
///
/// For the reads whose length is not yet known — a superblock prefix, an object
/// header whose size field is inside the bytes being read — where running short
/// is ordinary and the parse that follows does its own bounds checking. An
/// `addr` past the end of the file is still an error: that is a bad address,
/// not a short read.
pub(crate) fn read_up_to<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    len: usize,
) -> Result<Cow<'_, [u8]>, FieldglassError> {
    let size = source.size();
    if addr > size {
        return Err(FieldglassError::Parse(format!(
            "address {addr} past end of file ({size} bytes)"
        )));
    }
    let available = size - addr;
    source.read(ByteRange::new(addr, (len as u64).min(available)))
}

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

/// A forward cursor over the *file*, at an absolute address, reading through
/// [`ByteSource`].
///
/// The same field reads [`Cursor`] offers, over bytes it fetches itself. It
/// holds one window at a time and refills when a read runs off the end of it —
/// see the module doc for why that is a window and not a field.
pub(crate) struct FileCursor<'a, S: ?Sized> {
    source: &'a S,
    /// Total file size, cached so every bounds check is local.
    size: u64,
    /// File address of byte zero of `window`.
    base: u64,
    window: Cow<'a, [u8]>,
    /// Position within `window`.
    pos: usize,
    /// How much the next refill asks for — see [`MAX_WINDOW_BYTES`].
    next_window: usize,
}

impl<'a, S: ByteSource + ?Sized> FileCursor<'a, S> {
    /// A cursor positioned at file address `addr`.
    ///
    /// Nothing is read yet — the first field read fills the window. The address
    /// itself is checked here, so a structure pointer past the end of the file
    /// is refused at the seek rather than at whatever it would have read.
    pub(crate) fn at(source: &'a S, addr: u64) -> Result<Self, FieldglassError> {
        let size = source.size();
        if addr > size {
            return Err(FieldglassError::Parse("address past end of file".into()));
        }
        Ok(Self {
            source,
            size,
            base: addr,
            window: Cow::Borrowed(&[]),
            pos: 0,
            next_window: FIRST_WINDOW_BYTES,
        })
    }

    /// The address the cursor is about to read from.
    fn address(&self) -> u64 {
        self.base + self.pos as u64
    }

    /// Make at least `n` bytes available from the current position.
    ///
    /// Refills from the file when they are not already in the window, taking a
    /// whole [`WINDOW_BYTES`] window where the file has one — a bigger read
    /// costs a buffer nothing and saves a transport a round trip.
    fn need(&mut self, n: usize) -> Result<(), FieldglassError> {
        if self
            .pos
            .checked_add(n)
            .is_some_and(|e| e <= self.window.len())
        {
            return Ok(());
        }
        let addr = self.address();
        let available = self.size.saturating_sub(addr);
        if (n as u64) > available {
            return Err(FieldglassError::Parse("read past end of file".into()));
        }
        let want = (n.max(self.next_window) as u64).min(available);
        self.next_window = self.next_window.saturating_mul(2).min(MAX_WINDOW_BYTES);
        self.window = self.source.read(ByteRange::new(addr, want))?;
        // A source that served short would leave the window smaller than the
        // parse is about to index. Catching it here is what keeps every reader
        // below from having to.
        if self.window.len() < n {
            return Err(FieldglassError::Parse(format!(
                "the source served {} of {n} bytes at {addr}",
                self.window.len()
            )));
        }
        self.base = addr;
        self.pos = 0;
        Ok(())
    }

    /// Take `n` bytes, borrowed from the window the cursor is holding.
    ///
    /// Tied to the cursor rather than to the file, because a source that is not
    /// one contiguous buffer has nothing file-lived to lend. Callers that keep
    /// the bytes copy them out, which is what they did before.
    pub(crate) fn take(&mut self, n: usize) -> Result<&[u8], FieldglassError> {
        self.need(n)?;
        let out = &self.window[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(crate) fn tag(&mut self, signature: &[u8; 4]) -> Result<(), FieldglassError> {
        let got = self.take(4)?;
        if got != signature {
            return Err(FieldglassError::Parse(format!(
                "expected signature {:?}, got {:?}",
                std::str::from_utf8(signature).unwrap_or("?"),
                String::from_utf8_lossy(got)
            )));
        }
        Ok(())
    }
}

impl<S: ByteSource + ?Sized> Fields for FileCursor<'_, S> {
    fn uint(&mut self, width: usize) -> Result<u64, FieldglassError> {
        self.need(width)?;
        let value = read_uint_le(&self.window, self.pos, width)?;
        self.pos += width;
        Ok(value)
    }

    /// Advance `n` bytes without reading them.
    ///
    /// Skipping does not fetch: a field the reader does not want costs nothing
    /// over a transport. Skipping past the end of the file is still an error,
    /// because the structure said it had bytes there.
    fn skip(&mut self, n: usize) -> Result<(), FieldglassError> {
        if self
            .pos
            .checked_add(n)
            .is_some_and(|e| e <= self.window.len())
        {
            self.pos += n;
            return Ok(());
        }
        let addr = self
            .address()
            .checked_add(n as u64)
            .filter(|&a| a <= self.size)
            .ok_or_else(|| FieldglassError::Parse("skip past end of file".into()))?;
        self.base = addr;
        self.pos = 0;
        self.window = Cow::Borrowed(&[]);
        Ok(())
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

    #[test]
    fn a_cursor_cannot_be_placed_past_the_end_of_the_file() {
        let bytes: Vec<u8> = vec![0u8; 64];
        assert!(FileCursor::at(&bytes, 4096).is_err());
        // The end of the file is a legal position — it is where a zero-length
        // structure sits — and only reading from it fails.
        let mut at_end = FileCursor::at(&bytes, 64).expect("end is a position");
        assert!(at_end.byte().is_err());
    }

    /// Reads that cross the window boundary have to see the same bytes a single
    /// window would, or every structure larger than [`WINDOW_BYTES`] is decoded
    /// from the wrong offsets.
    #[test]
    fn refilling_does_not_move_the_file_position() {
        // Each 4-byte little-endian word holds its own index, so a value read
        // at any offset names where it came from.
        let mut bytes = Vec::new();
        for i in 0..4096u32 {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        for i in 0..4096u32 {
            assert_eq!(cur.uint(4).expect("word in range"), u64::from(i));
        }
        assert!(cur.byte().is_err(), "the file ends here");

        // Skipping across windows lands in the same place as reading across
        // them.
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        cur.skip(4 * 3000).expect("in range");
        assert_eq!(cur.uint(4).expect("word in range"), 3000);
    }

    /// A read wider than the window is served whole rather than clipped to it.
    #[test]
    fn a_read_larger_than_the_window_is_one_read() {
        let bytes: Vec<u8> = (0..u8::MAX).cycle().take(MAX_WINDOW_BYTES * 3).collect();
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        let got = cur.take(MAX_WINDOW_BYTES * 2).expect("in range");
        assert_eq!(got.len(), MAX_WINDOW_BYTES * 2);
        assert_eq!(got, &bytes[..MAX_WINDOW_BYTES * 2]);
    }

    /// The window grows, so a long run is a handful of reads rather than one
    /// per field — and a short one does not pay for a run it never makes.
    #[test]
    fn the_window_doubles_rather_than_starting_large() {
        #[derive(Default)]
        struct Counting {
            bytes: Vec<u8>,
            reads: std::cell::RefCell<Vec<u64>>,
        }
        impl ByteSource for Counting {
            fn size(&self) -> u64 {
                self.bytes.len() as u64
            }
            fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
                self.reads.borrow_mut().push(range.len);
                self.bytes.read(range)
            }
        }

        let source = Counting {
            bytes: vec![0u8; 1 << 20],
            ..Default::default()
        };

        // A short structure: one small read, not one large one.
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        for _ in 0..8 {
            cur.uint(4).expect("in range");
        }
        assert_eq!(
            source.reads.borrow().as_slice(),
            &[FIRST_WINDOW_BYTES as u64],
            "32 bytes of fields should cost one 64-byte read"
        );

        // A long one: a few doubling reads, not 4096 one-field ones.
        source.reads.borrow_mut().clear();
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        for _ in 0..4096 {
            cur.uint(4).expect("in range");
        }
        let reads = source.reads.borrow().clone();
        assert!(
            reads.len() <= 10,
            "16 KiB of fields took {} reads: {reads:?}",
            reads.len()
        );
        let fetched: u64 = reads.iter().sum();
        assert!(
            fetched < 4 * 4096 * 2,
            "fetched {fetched} bytes for 16384 bytes of fields"
        );
    }

    /// `read_up_to` is for the reads whose length is not known yet, so a short
    /// file shortens the read rather than failing it — but a bad address still
    /// fails.
    #[test]
    fn read_up_to_shortens_at_the_end_of_the_file() {
        let bytes: Vec<u8> = vec![7u8; 10];
        assert_eq!(read_up_to(&bytes, 6, 16).expect("short read").len(), 4);
        assert_eq!(read_up_to(&bytes, 10, 16).expect("empty read").len(), 0);
        assert!(read_up_to(&bytes, 11, 1).is_err());
        assert!(read_at(&bytes, 6, 16).is_err());
    }
}
