//! The GRIB1 surface a host needs beyond what [`Session`](crate::Session) answers.
//!
//! **[`Session`](crate::Session) is the way in, and this is not an alternative to it.** A host
//! that wants a message list, a decoded field or a placed raster asks that
//! and gets the same answer whatever container it opened, which is the point of
//! the umbrella ([ADR-0006]). What lives here is the remainder — and for GRIB1
//! that remainder is one thing: **editing**.
//!
//! A session is read-only by design. Every method on it answers a question
//! about bytes the host already holds, and ADR-0005 has hosts hand bytes *in*
//! rather than ask the library to hand a file back. Rewriting an octet does not
//! fit that shape, and giving it a mutating method to accommodate one
//! GRIB1 field would be the wrong trade. So the edit is a free function: bytes
//! in, bytes out, with the format knowledge on this side of the boundary where
//! it belongs.
//!
//! It exists so **no host names a format crate in its manifest** (#726), the
//! same reason the `netcdf` module does — named rather than linked, because
//! that module is behind the `netcdf` feature and this one is behind `grib1`,
//! so a link would only resolve in a build that happens to carry both.
//!
//! [ADR-0006]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md

use crate::error::Error;

/// One GRIB1 file's bytes with message `index`'s `P1` octet set to `value`.
///
/// `P1` is the forecast time in the unit §1 declares — the field a metadata
/// editor changes to re-stamp a message's lead time (#411).
///
/// **The whole file comes back, not a patch.** A GRIB1 message's `P1` is a
/// single octet at a fixed position in its §1, so the edit is one byte, but a
/// host holding a buffer wants the buffer it can write out. Editing is rare and
/// allocating the file again costs the same as the copy a host would make
/// itself.
///
/// The file is re-indexed to find the octet, which is why this takes bytes
/// rather than hanging off an open [`Session`](crate::Session): a session is a
/// read-only view, and an edit that invalidated it would have to say so. Bytes
/// in, bytes out, and the caller decides what to do with the result.
///
/// # Errors
///
/// [`Error::Decode`] when the bytes are not a GRIB1 file this build can index,
/// [`Error::NoSuchMessage`] for an index outside the file, and
/// [`Error::InvalidOption`] for a `value` that does not fit the octet.
pub fn with_p1_octet(bytes: &[u8], index: u32, value: u32) -> Result<Vec<u8>, Error> {
    let octet = u8::try_from(value).map_err(|_| Error::InvalidOption {
        detail: format!("`P1` is one octet, so 0..=255; {value} was given"),
    })?;
    let reader = fieldglass_grib1::Grib1Reader::from_bytes(bytes.to_vec())?;
    let count = u32::try_from(reader.messages.len()).unwrap_or(u32::MAX);
    let message = reader
        .messages
        .get(index as usize)
        .ok_or(Error::NoSuchMessage { index, count })?;
    // A file offset, so `u64` on the reader; the buffer it indexed is in memory,
    // so it fits a `usize` on any target that could have held the file.
    let at = fieldglass_core::bytes::checked_usize(message.pds_p1_offset(), "PDS P1 offset")?;
    let mut out = reader.bytes().to_vec();
    // `checked_usize` bounds the cast, not the buffer: an offset inside a
    // message this reader indexed is inside the buffer it indexed, but the
    // index is still checked rather than trusted, because a panic here would
    // abort a host's worker.
    *out.get_mut(at).ok_or_else(|| Error::Decode {
        detail: format!("the `P1` octet for message {index} lies at {at}, past the file's end"),
    })? = octet;
    Ok(out)
}
