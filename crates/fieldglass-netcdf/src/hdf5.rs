//! HDF5 (NetCDF-4) backing. This module root probes the superblock — confirming
//! the file is HDF5 and reading its version into [`Hdf5Probe`] — and declares the
//! `hdf5/` submodule tree that does the deep, pure-Rust traversal on demand:
//! object headers, group and link tables, dataspace and datatype messages,
//! attribute messages, dimension scales, the fractal heap + B-tree v2 dense
//! metadata indexes, the filter pipeline, and dataset value decode.
//!
//! The probe is eager but cheap; the deep walk runs only as far as a metadata or
//! value request reaches (decision 0003), so scanning a file stays inexpensive.
//! Started as issue #29 ("parse enough to validate the file and tell the user
//! what's going on") and completed under the #33 umbrella (#37–#40, #121, #174).
//!
//! Every read here goes through [`ByteSource`] rather than by indexing a
//! whole-file slice (#682, [ADR-0005]). The `source` submodule holds the two
//! cursors that do it — one over bytes the caller already has, one over the
//! file — and records why the traversal prefetches nothing while the chunk
//! fetch does.
//!
//! [`ByteSource`]: fieldglass_core::bytes::ByteSource
//!
//! [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md
//!
//! Reference: HDF5 file format specification version 3
//! <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::ByteSource;
use source::read_up_to;

pub mod attribute;
pub(crate) mod cache;
pub mod dataset;
pub mod dataspace;
pub mod datatype;
pub mod dimensions;
pub mod filter;
pub mod global_heap;
pub mod group;
pub mod heap;
pub mod layout;
pub mod object_header;
pub(crate) mod source;
pub mod values;

/// HDF5 signature: `\x89HDF\r\n\x1a\n`.
pub const HDF5_SIGNATURE: [u8; 8] = [0x89, b'H', b'D', b'F', b'\r', b'\n', 0x1a, b'\n'];

/// The per-file HDF5 handle: what we surface from the superblock, plus the
/// traversal memo the deep walk fills as it goes (#414).
///
/// The superblock fields are deliberately tiny. The memo is private and carries
/// no meaning of its own — it only remembers work already done for *these*
/// bytes, so two probes with the same three fields are equal and a clone starts
/// cold.
///
/// A probe is only valid against the source [`probe`] was called on — it
/// carries that file's offset sizes, and the memo is keyed by that file's
/// offsets. Pairing one with a different file is guarded: the memo binds to its
/// source's [identity](fieldglass_core::bytes::ByteSource::identity) on first
/// use and steps aside for any other, so a mismatch is slow rather than wrong.
/// That guard used to be the slice's *length*, which two files of equal size
/// pass — they aliased, and the second was answered with the first's structure
/// (#681).
#[derive(Default)]
pub struct Hdf5Probe {
    /// Superblock version byte. Versions 0 and 1 share a layout; versions 2
    /// and 3 introduce a different header. We don't go beyond reporting it.
    pub superblock_version: u8,
    /// Size of file offsets in bytes (typically 8).
    pub offset_size: u8,
    /// Size of file lengths in bytes (typically 8).
    pub length_size: u8,
    /// Per-file traversal memo. Not part of the probe's identity.
    cache: cache::Hdf5Cache,
}

impl Hdf5Probe {
    /// A probe with the given superblock fields and an empty memo. Prefer
    /// [`probe`], which reads them from a real superblock; this exists for
    /// tests and for callers driving the traversal against a hand-built file.
    pub fn new(superblock_version: u8, offset_size: u8, length_size: u8) -> Self {
        Self {
            superblock_version,
            offset_size,
            length_size,
            cache: cache::Hdf5Cache::default(),
        }
    }

    /// The object header at `offset`, parsed once per file.
    ///
    /// The traversal calls this instead of [`object_header::walk`] directly, so
    /// every path that reaches a given header — group classification, dataset
    /// shape, attributes, value decode — shares one parse.
    pub(crate) fn header<S: ByteSource + ?Sized>(
        &self,
        source: &S,
        offset: u64,
    ) -> Result<std::sync::Arc<object_header::ObjectHeader>, FieldglassError> {
        self.cache
            .header(source, offset, self.offset_size, self.length_size)
    }

    /// How many structure walks this probe has actually performed — object
    /// headers parsed plus chunk indexes collected — as opposed to served from
    /// its memo.
    ///
    /// Instrumentation for tests and measurement: it says what a call really
    /// cost in file traversal, which a wall-clock timing cannot separate from
    /// I/O noise, and which is the whole cost once the bytes arrive over a
    /// byte-range transport. It is not part of the probe's identity, so it
    /// survives neither [`Clone`] nor equality.
    pub fn traversals(&self) -> u64 {
        self.cache.traversals()
    }

    /// Access to the memo for the traversal modules.
    pub(crate) fn cache(&self) -> &cache::Hdf5Cache {
        &self.cache
    }
}

// The memo is not part of the probe's identity: equality compares the
// superblock fields, `Debug` prints them, and a clone starts with a cold cache
// rather than sharing (or copying) another probe's work.
impl Clone for Hdf5Probe {
    fn clone(&self) -> Self {
        Self::new(self.superblock_version, self.offset_size, self.length_size)
    }
}

impl PartialEq for Hdf5Probe {
    fn eq(&self, other: &Self) -> bool {
        self.superblock_version == other.superblock_version
            && self.offset_size == other.offset_size
            && self.length_size == other.length_size
    }
}

impl Eq for Hdf5Probe {}

impl std::fmt::Debug for Hdf5Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hdf5Probe")
            .field("superblock_version", &self.superblock_version)
            .field("offset_size", &self.offset_size)
            .field("length_size", &self.length_size)
            .finish()
    }
}

/// HDF5 stores the signature at one of a sequence of offsets — 0, 512, 1024,
/// 2048, … each doubled. This list covers the practical range; files with
/// signatures further out are rare enough we don't search forever.
fn signature_offsets() -> [u64; 7] {
    [0, 512, 1024, 2048, 4096, 8192, 16384]
}

/// The "this is not an HDF5 file" error, quoting the bytes that are there.
///
/// A source that cannot even be read for the quote still has to report the
/// magic failure rather than the read one — the caller asked whether this is
/// HDF5, and the answer is no either way.
fn not_hdf5<S: ByteSource + ?Sized>(source: &S) -> FieldglassError {
    let found = read_up_to(source, 0, HDF5_SIGNATURE.len()).unwrap_or_default();
    FieldglassError::invalid_magic("\\x89HDF\\r\\n\\x1a\\n", &found)
}

/// Find the file offset at which the HDF5 signature appears, if any.
///
/// A handful of fixed offsets rather than a scan, so this is a bounded number
/// of small reads over any source rather than a walk of the whole file.
pub fn find_signature<S: ByteSource + ?Sized>(source: &S) -> Option<u64> {
    for off in signature_offsets() {
        let at = read_up_to(source, off, HDF5_SIGNATURE.len()).ok()?;
        if at.len() < HDF5_SIGNATURE.len() {
            return None;
        }
        if at[..HDF5_SIGNATURE.len()] == HDF5_SIGNATURE {
            return Some(off);
        }
    }
    None
}

/// Probe the HDF5 superblock. Reads only the fields whose offsets are
/// version-independent (signature + version byte + sizes), so this is safe
/// against newer superblock layouts.
pub fn probe<S: ByteSource + ?Sized>(source: &S) -> Result<Hdf5Probe, FieldglassError> {
    let off = find_signature(source).ok_or_else(|| not_hdf5(source))?;

    // Superblock fields after the 8-byte signature, common to all versions:
    //   off + 8  : superblock version (1 byte)
    //
    // For versions 0 and 1 the next two bytes are free-space + root group
    // symbol table version. For version 2 / 3 the layout is:
    //   off + 9  : size of offsets
    //   off + 10 : size of lengths
    //
    // For versions 0 / 1:
    //   off + 13 : size of offsets
    //   off + 14 : size of lengths
    //
    // We report sizes by branching on version.
    // The one bounded, plan-ahead read in the whole HDF5 path: the fields below
    // are all within 16 bytes of the signature, whatever the version.
    let need = off + 16;
    let block = read_up_to(source, off, 16)?;
    if block.len() < 16 {
        return Err(FieldglassError::Parse(format!(
            "HDF5 superblock truncated: need at least {need} bytes, have {}",
            source.size()
        )));
    }
    let version = block[8];
    let (offset_size, length_size) = match version {
        0 | 1 => (block[13], block[14]),
        2 | 3 => (block[9], block[10]),
        v => {
            return Err(FieldglassError::Parse(format!(
                "unrecognized HDF5 superblock version {v}"
            )));
        }
    };

    Ok(Hdf5Probe::new(version, offset_size, length_size))
}

/// File offset of the root group's object header, read from the superblock.
///
/// This is the bootstrap address the [`object_header`] walker and the
/// higher-layer group traversal (#38) start from. For superblock versions 0/1
/// it lives in the root-group symbol-table entry; for versions 2/3 it's a
/// dedicated superblock field. Reading it is superblock-level work, not deep
/// parsing, so it lives here alongside [`probe`].
pub fn root_group_address<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<u64, FieldglassError> {
    // Memoised: the whole-file walk bootstraps from here, and it is on the hot
    // path of every metadata and decode call (#414).
    probe
        .cache()
        .root(source, || read_root_group_address(source, probe))
}

fn read_root_group_address<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<u64, FieldglassError> {
    let base = find_signature(source).ok_or_else(|| not_hdf5(source))?;
    let o = probe.offset_size as usize;
    if o == 0 || o > 8 {
        return Err(FieldglassError::Parse(format!(
            "unsupported HDF5 offset size {o}"
        )));
    }
    // Offsets are relative to the superblock signature. Layouts:
    //   v0: 24 fixed bytes (through file-consistency flags), then 4 addresses
    //       (base/free-space/eof/driver) and the root symbol-table entry whose
    //       first two fields are link-name offset + object-header address.
    //   v1: as v0 but with 4 extra bytes (indexed-storage K + reserved).
    //   v2/3: 12 fixed bytes, then base/superblock-extension/eof addresses and
    //         the root-group object-header address.
    // `base` is a file address and `o` at most 8, so the arithmetic is in `u64`
    // and cannot overflow: the signature offsets top out at 16384.
    let o64 = o as u64;
    let addr_off = match probe.superblock_version {
        0 => base + 24 + 5 * o64,
        1 => base + 28 + 5 * o64,
        2 | 3 => base + 12 + 3 * o64,
        v => {
            return Err(FieldglassError::Parse(format!(
                "unsupported HDF5 superblock version {v}"
            )));
        }
    };
    let field = source::read_at(source, addr_off, o)?;
    let address = object_header::read_uint_le(&field, 0, o)?;
    // All-ones is HDF5's "undefined address" sentinel; a valid file always has
    // a root group.
    let undefined = if o == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * o)) - 1
    };
    if address == undefined {
        return Err(FieldglassError::Parse(
            "HDF5 superblock has no root-group address".into(),
        ));
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(version: u8, size_offsets_v01: bool) -> Vec<u8> {
        // Build a fake superblock: signature, version, then a block of zeros
        // long enough to cover offset/length size fields at either layout.
        let mut v = Vec::new();
        v.extend_from_slice(&HDF5_SIGNATURE);
        v.push(version);
        v.extend_from_slice(&[0u8; 8]); // padding to land at off+9..=off+16
        // Place offset_size = 8 / length_size = 8 at the version-appropriate
        // slot. signature is 8 bytes, so off=0; field offsets are absolute.
        if size_offsets_v01 {
            v[13] = 8;
            v[14] = 8;
        } else {
            v[9] = 8;
            v[10] = 8;
        }
        v
    }

    #[test]
    fn probe_v0() {
        let bytes = synth(0, true);
        let p = probe(&bytes).unwrap();
        assert_eq!(p.superblock_version, 0);
        assert_eq!(p.offset_size, 8);
        assert_eq!(p.length_size, 8);
    }

    #[test]
    fn probe_v2() {
        let bytes = synth(2, false);
        let p = probe(&bytes).unwrap();
        assert_eq!(p.superblock_version, 2);
        assert_eq!(p.offset_size, 8);
        assert_eq!(p.length_size, 8);
    }

    #[test]
    fn missing_signature_errors() {
        let bytes = vec![0u8; 32];
        let err = probe(&bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::InvalidMagic { .. }));
    }

    #[test]
    fn truncated_superblock_errors() {
        let mut bytes = HDF5_SIGNATURE.to_vec();
        bytes.push(0); // version
        // Only 9 bytes total — well short of off+16.
        let err = probe(&bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }
}
