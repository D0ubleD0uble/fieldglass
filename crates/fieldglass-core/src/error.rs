/// Everything the parsing and decode surface can fail with.
///
/// `#[non_exhaustive]` (#554) because four crates carrying this type are
/// published, so without it *every* new variant is a breaking change for a
/// downstream `match` — which is what has kept the structured variants this
/// enum still wants out of it. Adding one is now additive, and a consumer
/// matching on it needs a wildcard arm.
// No `Clone` and no `PartialEq` (#556), and the compiler settles it rather
// than taste: the `Io` variant holds a `std::io::Error`, which implements
// neither.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FieldglassError {
    /// Reading the file failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are the right format but a section did not parse.
    ///
    /// The long tail, deliberately (#554): a variant per message would be a
    /// hundred of them. A recurring *shape* earns a variant; a one-off does
    /// not.
    #[error("parse error: {0}")]
    Parse(String),
    /// The leading bytes are not the magic this reader expects.
    ///
    /// Built with [`Self::invalid_magic`], which renders `found` printably.
    #[error("invalid magic bytes: expected {expected}, found {found}")]
    InvalidMagic {
        /// The magic this reader requires, as it would be written.
        expected: &'static str,
        /// What was there instead, escaped for display.
        found: String,
    },
    /// The section parsed, but names a template or packing this build does not
    /// decode.
    #[error("unsupported section: {0}")]
    UnsupportedSection(String),
    /// The call does not apply to the layout the reader opened — asking a
    /// classic NetCDF file for its NetCDF-4 / HDF5 metadata, say. Nothing
    /// failed to decode; the question was put to the wrong file, which is why
    /// this is not [`Self::Parse`]. A caller that matched on the layout first
    /// cannot produce it.
    #[error("not applicable to this file's layout: {0}")]
    WrongLayout(String),
    /// An index outside the collection it addresses.
    #[error("index {index} is outside the {bound} available")]
    OutOfRange {
        /// The index that was asked for.
        index: usize,
        /// How many there are, so the valid range is `0..bound`.
        bound: usize,
    },
    /// A source served fewer bytes than it was asked for.
    ///
    /// Its own variant, not [`Self::Parse`], because it is not a statement
    /// about the file: the bytes that arrived are fine as far as they go, and
    /// there are simply fewer of them than were requested. An in-memory buffer
    /// cannot produce this — [`ByteSource::read`](crate::bytes::ByteSource::read)
    /// bounds-checks against its own size — so it means a transport, and the
    /// right answer to a truncated transfer is to retry it. A corrupt file is
    /// to be reported. A host that saw both as `Parse` could not tell which it
    /// had (#707).
    #[error("the source served {got} of {wanted} bytes at {at}")]
    ShortRead {
        /// Offset in the source the read began at.
        at: u64,
        /// How many bytes came back.
        got: u64,
        /// How many were asked for.
        wanted: u64,
    },
    /// The chunk-grid arithmetic refused something.
    ///
    /// Carried rather than flattened into a string so a reader that returns
    /// this type still hands back the axis and the extent that failed.
    /// [`ArrayError`](crate::array::ArrayError) is its own type because
    /// callers keep it in enums deriving `Clone` and `PartialEq`, which this
    /// one cannot.
    #[error("{0}")]
    Array(#[from] crate::array::ArrayError),
}

impl FieldglassError {
    /// An [`InvalidMagic`](Self::InvalidMagic) naming what was expected and
    /// what the bytes held.
    ///
    /// A constructor rather than a struct literal at each call site so the
    /// rendering of `found` happens once: a reader that built the string itself
    /// would be free to print raw bytes, which is the thing
    /// [`printable_bytes`] exists to prevent. Pass the bytes at the position
    /// the magic was expected; only the leading few are kept, since a magic is
    /// never long and the rest is unbounded attacker data.
    #[must_use]
    pub fn invalid_magic(expected: &'static str, found: &[u8]) -> Self {
        const KEPT: usize = 8;
        Self::InvalidMagic {
            expected,
            found: printable_bytes(&found[..found.len().min(KEPT)]),
        }
    }

    /// An [`OutOfRange`](Self::OutOfRange) for `index` against a collection of
    /// `bound` items.
    #[must_use]
    pub fn out_of_range(index: usize, bound: usize) -> Self {
        Self::OutOfRange { index, bound }
    }
}

/// Render bytes for an error message, escaping anything unprintable.
///
/// The "found" half of a magic mismatch is arbitrary binary — a stale offset
/// lands in packed data — and printing it raw would put control characters into
/// a host's log or a test's assertion output. Public because
/// `fieldglass-fetchplan` reports the same mismatch about a fetched message and
/// had its own copy of this; one implementation means the two cannot render the
/// same bytes differently.
#[must_use]
pub fn printable_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                (b as char).to_string()
            } else {
                format!("\\x{b:02x}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_bytes_escapes_the_unprintable_and_keeps_the_rest() {
        assert_eq!(printable_bytes(b"GRIB"), "GRIB");
        assert_eq!(printable_bytes(&[0x00, 0x1f, 0x7f]), "\\x00\\x1f\\x7f");
        // A space is kept: it is what separates fields in the formats that
        // have text headers, so escaping it would make the output harder to
        // read rather than safer.
        assert_eq!(printable_bytes(b"a b"), "a b");
    }

    #[test]
    fn invalid_magic_keeps_only_the_leading_bytes() {
        // The buffer is attacker-sized; the message must not be.
        let err = FieldglassError::invalid_magic("GRIB", &[b'X'; 4096]);
        let FieldglassError::InvalidMagic { expected, found } = &err else {
            panic!("expected InvalidMagic, got {err:?}");
        };
        assert_eq!(*expected, "GRIB");
        assert_eq!(found, "XXXXXXXX", "eight bytes kept, not four thousand");
    }

    #[test]
    fn invalid_magic_takes_a_buffer_shorter_than_the_window() {
        // The slice is `..min(len, KEPT)`, so a two-byte buffer must not panic.
        let err = FieldglassError::invalid_magic("CDF", b"\x89H");
        assert_eq!(
            err.to_string(),
            "invalid magic bytes: expected CDF, found \\x89H"
        );
    }

    #[test]
    fn out_of_range_states_the_index_and_the_bound() {
        let err = FieldglassError::out_of_range(7, 3);
        assert_eq!(err.to_string(), "index 7 is outside the 3 available");
    }
}
