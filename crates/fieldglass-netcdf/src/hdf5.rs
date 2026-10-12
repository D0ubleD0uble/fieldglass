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
//! An HDF5 address is relative to the superblock, which a userblock can put
//! past byte 0 (#936). The walkers read through [`Addressed`], which adds that
//! base in one place; [`Hdf5Probe::addressed`] makes one for a probed file.
//!
//! [`ByteSource`]: fieldglass_core::bytes::ByteSource
//!
//! [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md
//!
//! Reference: HDF5 file format specification version 3
//! <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::ByteSource;
use fieldglass_core::bytes::read_head;
use source::read_up_to;

pub use fieldglass_core::HDF5_SIGNATURE;
pub use source::Addressed;

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

/// Header message type of a B-tree 'K' Values message.
const MSG_BTREE_K: u16 = 0x0013;

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
///
/// Every traversal function takes the file as [`addressed`](Self::addressed)
/// returns it, so that the addresses the file states are offsets into what it
/// reads. The superblock readers refuse a file whose superblock is not at 0 in
/// what they are given, so passing the raw bytes of a file with a userblock is
/// an error rather than a walk of the wrong bytes.
#[derive(Default)]
pub struct Hdf5Probe {
    /// Superblock version byte. Versions 0 and 1 share a layout; versions 2
    /// and 3 introduce a different header. We don't go beyond reporting it.
    pub superblock_version: u8,
    /// Size of file offsets in bytes (typically 8).
    pub offset_size: u8,
    /// Size of file lengths in bytes (typically 8).
    pub length_size: u8,
    /// File offset of the superblock, which every address in the file is
    /// relative to: the size of the userblock before it, 0 when there is none
    /// (#936).
    pub base_address: u64,
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
            base_address: 0,
            cache: cache::Hdf5Cache::default(),
        }
    }

    /// `file` as the traversal reads it: from [`base_address`](Self::base_address)
    /// on, so an address the file states is an offset into it (#936).
    ///
    /// Pass the same file this probe was read from, unwrapped. For a file
    /// without a userblock this reads exactly the bytes `file` does.
    pub fn addressed<'a, S: ByteSource + ?Sized>(&self, file: &'a S) -> Addressed<&'a S> {
        Addressed::new(file, self.base_address)
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
            .map(|(header, _)| header)
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
        Self {
            base_address: self.base_address,
            ..Self::new(self.superblock_version, self.offset_size, self.length_size)
        }
    }
}

impl PartialEq for Hdf5Probe {
    fn eq(&self, other: &Self) -> bool {
        self.superblock_version == other.superblock_version
            && self.offset_size == other.offset_size
            && self.length_size == other.length_size
            && self.base_address == other.base_address
    }
}

impl Eq for Hdf5Probe {}

impl std::fmt::Debug for Hdf5Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hdf5Probe")
            .field("superblock_version", &self.superblock_version)
            .field("offset_size", &self.offset_size)
            .field("length_size", &self.length_size)
            .field("base_address", &self.base_address)
            .finish()
    }
}

/// What one whole-file listing has read, so that no block is read for two
/// objects (#921).
///
/// The bounds from #895, #899 and #901 hold within one structure: one object
/// header, one B-tree, one group's names, one heap's objects. They did not
/// span the objects of a listing, so a block that many objects named was read
/// and copied once per object: many headers continuing into one chunk, many
/// datasets pointing at one dense attribute store, many groups sharing one
/// local heap. In a valid file each of those belongs to one object (each group
/// has its own local heap, each object its own dense storage), so a listing
/// claims the file range of every block it reads and refuses a second claim,
/// as [`ClaimedRanges`](source::ClaimedRanges) does inside one structure.
///
/// An object reached twice, by two hard links, is still one object. Its header
/// chunks are claimed on its first visit only, and the walks that read an
/// object's other blocks visit each object once.
///
/// What is legitimately shared is not claimed: one global-heap collection
/// holds the `DIMENSION_LIST` references of many variables.
///
/// libhdf5 reads objects sharing a dense store and groups sharing a local
/// heap, so refusing them is a known divergence; ADR-0016 records why.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    /// Every block this listing has read, as file ranges.
    pub(crate) file: source::ClaimedRanges,
    /// The object headers whose chunks are claimed in `file`.
    headers: std::collections::HashSet<u64>,
}

impl Listing {
    /// The object header at `offset`, its chunks claimed on its first visit.
    pub(crate) fn header<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        probe: &Hdf5Probe,
        offset: u64,
    ) -> Result<std::sync::Arc<object_header::ObjectHeader>, FieldglassError> {
        let (header, chunks) =
            probe
                .cache
                .header(source, offset, probe.offset_size, probe.length_size)?;
        if self.headers.insert(offset) {
            for &(start, len) in chunks.iter() {
                self.file.claim(start, len, "object-header chunk")?;
            }
        }
        Ok(header)
    }
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
/// HDF5 stores the signature at byte 0 or after a userblock, at 512, 1024,
/// 2048, … each doubled. A handful of fixed offsets rather than a scan
/// ([`fieldglass_core::HDF5_SIGNATURE_OFFSETS`]), so this is a bounded number
/// of small reads over any source rather than a walk of the whole file. The
/// search is `core`'s, so format detection finds exactly the files this does.
///
/// A failure of the source is an error, not "no signature" (#998).
pub fn find_signature<S: ByteSource + ?Sized>(source: &S) -> Result<Option<u64>, FieldglassError> {
    fieldglass_core::find_hdf5_signature(source)
}

/// Check that `source` is a file as [`Hdf5Probe::addressed`] returns it, with
/// its superblock at 0, before a superblock field is read from it.
///
/// A file with a userblock passed unwrapped has its superblock further in;
/// reading on would follow the addresses it states into the userblock's bytes.
fn superblock_at_base<S: ByteSource + ?Sized>(source: &S) -> Result<(), FieldglassError> {
    match find_signature(source)? {
        Some(0) => Ok(()),
        Some(at) => Err(FieldglassError::Parse(format!(
            "HDF5 superblock is at byte {at}, after a userblock: read the file \
             through Hdf5Probe::addressed, which makes its addresses offsets"
        ))),
        None => Err(not_hdf5(source)),
    }
}

/// Probe the HDF5 superblock. Reads only the fields whose offsets are
/// version-independent (signature + version byte + sizes), so this is safe
/// against newer superblock layouts.
pub fn probe<S: ByteSource + ?Sized>(source: &S) -> Result<Hdf5Probe, FieldglassError> {
    let off = find_signature(source)?.ok_or_else(|| not_hdf5(source))?;
    probe_at(source, off)
}

/// [`probe`] for a caller that has already found the signature, at `off`
/// ([`find_signature`]'s answer), so it is not searched for twice (#999).
pub fn probe_at<S: ByteSource + ?Sized>(
    source: &S,
    off: u64,
) -> Result<Hdf5Probe, FieldglassError> {
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
    let block = read_head(source, off, 16)?;
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

    // The superblock's own Base Address field is not read: libhdf5 overrides
    // it with the signature's offset when they differ (`H5F__super_read`), and
    // the specification constrains the two to be equal.
    Ok(Hdf5Probe {
        base_address: off,
        ..Hdf5Probe::new(version, offset_size, length_size)
    })
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
    // path of every metadata and decode call (#414). Checked before the memo
    // binds, so a raw userblock file passed by mistake does not bind it to the
    // wrong source for the life of the probe.
    superblock_at_base(source)?;
    probe
        .cache()
        .root(source, || read_root_group_address(source, probe))
}

fn read_root_group_address<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<u64, FieldglassError> {
    let o = probe.offset_size as usize;
    if o == 0 || o > 8 {
        return Err(FieldglassError::Parse(format!(
            "unsupported HDF5 offset size {o}"
        )));
    }
    // Offsets are from the superblock signature, at 0. Layouts:
    //   v0: 24 fixed bytes (through file-consistency flags), then 4 addresses
    //       (base/free-space/eof/driver) and the root symbol-table entry whose
    //       first two fields are link-name offset + object-header address.
    //       The link-name offset is Size of Lengths wide, as libhdf5 reads
    //       and writes it (`H5G_ent_decode`); the specification's table marks
    //       it Size of Offsets, which differs only when the two sizes do (#922).
    //   v1: as v0 but with 4 extra bytes (indexed-storage K + reserved).
    //   v2/3: 12 fixed bytes, then base/superblock-extension/eof addresses and
    //         the root-group object-header address.
    // `o` is at most 8 and `l` at most 255, so the arithmetic cannot overflow.
    let o64 = o as u64;
    let l64 = u64::from(probe.length_size);
    let addr_off = match probe.superblock_version {
        0 => 24 + 4 * o64 + l64,
        1 => 28 + 4 * o64 + l64,
        2 | 3 => 12 + 3 * o64,
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

/// The superblock's version-1 B-tree "K" values, which bound how full a node
/// may be: a node holds at most 2K entries.
///
/// Group Leaf Node K bounds a symbol-table node, Group Internal Node K a group
/// B-tree node and Indexed Storage Internal Node K a chunk B-tree node, at
/// every level of the tree. That is how libhdf5 reads and writes them: it
/// refuses a B-tree node past 2K ("number of children is greater than
/// maximum", `H5B__cache_deserialize`), and a symbol-table node past 2K
/// overruns the buffer it sizes from Group Leaf Node K
/// (`H5G__cache_node_deserialize`). The reader does the same (#920).
///
/// The specification's superblock table words Group Leaf Node K as bounding
/// "each leaf node of a group B-tree", and only internal nodes by the other
/// two. Its own "Symbol Table Nodes" section splits a symbol-table node at 2K,
/// and libhdf5's default files hold level-0 group nodes of up to 32 entries,
/// past twice the default leaf K of 4, so the table's wording is not a bound
/// any writer keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BtreeK {
    /// Group Leaf Node K: bounds a symbol-table node.
    pub group_leaf: u16,
    /// Group Internal Node K: bounds a group B-tree node.
    pub group_internal: u16,
    /// Indexed Storage Internal Node K: bounds a chunk B-tree node.
    pub chunk_internal: u16,
}

impl Default for BtreeK {
    /// The specification's defaults, which apply when a file states no other:
    /// a version-0 superblock has no chunk K, and a version-2 or -3 superblock
    /// states K only in an optional superblock-extension message.
    fn default() -> Self {
        Self {
            group_leaf: 4,
            group_internal: 16,
            chunk_internal: 32,
        }
    }
}

impl BtreeK {
    /// Entries a symbol-table node may hold.
    pub(crate) fn symbol_node_max(self) -> usize {
        2 * usize::from(self.group_leaf)
    }

    /// Entries a group B-tree node may hold.
    pub(crate) fn group_node_max(self) -> usize {
        2 * usize::from(self.group_internal)
    }

    /// Entries a chunk B-tree node may hold.
    pub(crate) fn chunk_node_max(self) -> usize {
        2 * usize::from(self.chunk_internal)
    }
}

/// The B-tree "K" values for this file, read once.
///
/// A version-0 or -1 superblock states the group values, and version 1 the
/// chunk value as well. A version-2 or -3 superblock states them in a B-tree
/// 'K' Values message in its superblock extension, when it has one; libhdf5
/// writes that message for any file created with values other than the
/// defaults (`H5F__super_init`).
pub fn btree_k<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<BtreeK, FieldglassError> {
    superblock_at_base(source)?;
    probe
        .cache()
        .btree_k(source, || read_btree_k(source, probe))
}

fn read_btree_k<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<BtreeK, FieldglassError> {
    let u16_at = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let k = match probe.superblock_version {
        // After the signature (8), four version bytes, the two sizes and a
        // reserved byte: Group Leaf Node K, Group Internal Node K, the
        // consistency flags (4), and in version 1 Indexed Storage Internal
        // Node K.
        0 | 1 => {
            let fields = source::read_at(source, 16, 10)?;
            BtreeK {
                group_leaf: u16_at(&fields, 0),
                group_internal: u16_at(&fields, 2),
                chunk_internal: if probe.superblock_version == 1 {
                    u16_at(&fields, 8)
                } else {
                    BtreeK::default().chunk_internal
                },
            }
        }
        2 | 3 => {
            // Twelve fixed bytes, the base address, then the superblock
            // extension's address.
            let o = probe.offset_size;
            let field = source::read_at(source, 12 + u64::from(o), usize::from(o))?;
            let extension = object_header::read_uint_le(&field, 0, usize::from(o))?;
            if group::is_undefined(extension, o) {
                return Ok(BtreeK::default());
            }
            let header = probe.header(source, extension)?;
            let Some(message) = header.messages.iter().find(|m| m.msg_type == MSG_BTREE_K) else {
                return Ok(BtreeK::default());
            };
            // Version, then Indexed Storage Internal Node K, Group Internal
            // Node K and Group Leaf Node K.
            let body = &message.body;
            if body.len() < 7 {
                return Err(FieldglassError::Parse(
                    "B-tree 'K' values message too small".into(),
                ));
            }
            if body[0] != 0 {
                return Err(FieldglassError::Parse(format!(
                    "unsupported B-tree 'K' values message version {}",
                    body[0]
                )));
            }
            BtreeK {
                chunk_internal: u16_at(body, 1),
                group_internal: u16_at(body, 3),
                group_leaf: u16_at(body, 5),
            }
        }
        v => {
            return Err(FieldglassError::Parse(format!(
                "unsupported HDF5 superblock version {v}"
            )));
        }
    };
    // A zero K is not refused here. It caps its nodes at no entries, so the
    // walker that uses it refuses any node it finds, and a file that never
    // needs it still opens: libhdf5 refuses a zero group K in a version-0 or
    // -1 superblock, but not a zero chunk K or any zero in the K message.
    Ok(k)
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

    /// A version-0 or -1 superblock with the given sizes, through the root
    /// group's symbol-table entry: its link-name offset at Size of Lengths,
    /// then the header address at Size of Offsets (#922).
    fn synth_v01_root(version: u8, o: u8, l: u8, root: u64) -> Vec<u8> {
        let mut v = HDF5_SIGNATURE.to_vec();
        v.push(version);
        v.extend_from_slice(&[0u8; 15]); // through the consistency flags
        v[13] = o;
        v[14] = l;
        if version == 1 {
            v.extend_from_slice(&[0u8; 4]); // indexed-storage K + reserved
        }
        v.resize(v.len() + 4 * usize::from(o), 0xFF); // four addresses
        v.extend_from_slice(&0u64.to_le_bytes()[..usize::from(l)]); // name offset
        v.extend_from_slice(&root.to_le_bytes()[..usize::from(o)]);
        v
    }

    #[test]
    fn root_entry_name_offset_is_length_sized() {
        for version in [0, 1] {
            for (o, l) in [(8, 4), (4, 8), (8, 8), (4, 4)] {
                let bytes = synth_v01_root(version, o, l, 0x1234);
                let p = probe(&bytes).unwrap();
                assert_eq!(
                    read_root_group_address(&bytes, &p).unwrap(),
                    0x1234,
                    "version {version}, sizes ({o}, {l})"
                );
            }
        }
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

/// HDF5 metadata a ranged source can declare far larger than it holds (#997).
///
/// Each test takes a committed fixture, rewrites one length field to claim
/// gigabytes, and opens it through [`NetcdfReader::from_source`] over a source
/// that declares a terabyte and holds the fixture. Before #997 each of these
/// lengths was bounded only by the declared size, so the reader asked for the
/// gigabytes in one read. Now each is refused before the read, as a listing
/// too large (the file itself is fine), and no read anywhere near the claimed
/// length is made.
///
/// The tests live in the crate rather than under `tests/` because a rewritten
/// version-2 header or B-tree leaf has to carry a fresh checksum, and the
/// checksum is the crate's own.
#[cfg(test)]
mod declared_tests {
    use fieldglass_core::FieldglassError;
    use fieldglass_core::array::ArrayError;
    use fieldglass_core::bytes::MAX_WINDOW_BYTES;
    use fieldglass_core::testing::{Declared, Recording};

    use super::object_header::checksum_lookup3;
    use super::source::MAX_METADATA_BYTES;
    use crate::NetcdfReader;

    /// What a test makes a field claim: well past every cap, and far more than any
    /// one read the source allows.
    const CLAIM: u64 = 1 << 32;

    /// What the source declares itself to be: enough to hold [`CLAIM`] anywhere.
    const DECLARED: u64 = 1 << 40;

    /// An address in the declared file well past the bytes a fixture holds,
    /// where an object [`CLAIM`] bytes long overlaps nothing the reader reads.
    const PAST_THE_HELD_BYTES: u64 = 1 << 36;

    const EARLIEST: &[u8] = include_bytes!("../tests/fixtures/hdf5_no_userblock_earliest.h5");
    const LATEST: &[u8] = include_bytes!("../tests/fixtures/hdf5_no_userblock_latest.h5");
    const HUGE_ATTRIBUTES: &[u8] = include_bytes!("../tests/fixtures/netcdf4_huge_attributes.nc");

    /// Open `file` over a source that declares [`DECLARED`] bytes and resolve its
    /// metadata, returning the outcome and the longest single read asked for.
    fn open_declared(file: &[u8]) -> (Result<(), FieldglassError>, u64) {
        let source = Recording::new(Declared::new(file, DECLARED));
        let outcome = NetcdfReader::from_source(&source).and_then(|r| r.hdf5_metadata().map(drop));
        let longest = source.reads().iter().map(|r| r.len).max().unwrap_or(0);
        (outcome, longest)
    }

    /// The rewritten `file` is refused as too large before the claimed read, and
    /// the fixture it came from resolves over the same declared source.
    fn assert_refused_before_the_read(original: &[u8], file: &[u8], what: &str) {
        let (outcome, _) = open_declared(original);
        outcome.expect("the fixture itself resolves over a declared source");

        let (outcome, longest) = open_declared(file);
        let err = outcome.expect_err("a claim past the cap");
        assert!(
            matches!(
                &err,
                FieldglassError::Array(ArrayError::ListingTooLarge { what: w, limit })
                    if *w == what && *limit == MAX_METADATA_BYTES
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("The file itself is fine"), "{err}");
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
    }

    fn u32_at(file: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(file[at..at + 4].try_into().unwrap())
    }

    fn u64_at(file: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(file[at..at + 8].try_into().unwrap())
    }

    fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
        file[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Every offset `needle` starts at in `file`.
    fn find_all(file: &[u8], needle: &[u8]) -> Vec<usize> {
        file.windows(needle.len())
            .enumerate()
            .filter(|(_, w)| *w == needle)
            .map(|(i, _)| i)
            .collect()
    }

    const HEADER: &str = "bytes in one HDF5 object header";

    /// A version-1 header whose size field claims gigabytes (#997): its chunk 0
    /// was read whole at that size.
    #[test]
    fn a_v1_header_size_past_the_cap_is_refused_before_the_read() {
        // The dataset header at 800, whose 4-byte size field follows the
        // version, reserved byte, message count and reference count.
        let mut file = EARLIEST.to_vec();
        assert_eq!(file[800], 1, "a version-1 header");
        assert_eq!(u32_at(&file, 808), 256);
        put(&mut file, 808, &(u32::MAX - 15).to_le_bytes());
        assert_refused_before_the_read(EARLIEST, &file, HEADER);
    }

    /// A version-1 continuation message claiming a gigabyte-long chunk.
    #[test]
    fn a_v1_continuation_past_the_cap_is_refused_before_the_read() {
        // The continuation message body in the header at 800: an 8-byte address
        // then an 8-byte length.
        let mut file = EARLIEST.to_vec();
        let length = u64_at(&file, 880);
        assert!(
            (1..4096).contains(&length),
            "a continuation length: {length}"
        );
        assert!(u64_at(&file, 872) < file.len() as u64, "its address");
        put(&mut file, 880, &CLAIM.to_le_bytes());
        assert_refused_before_the_read(EARLIEST, &file, HEADER);
    }

    /// Where a version-2 header's chunk-0 size field sits, how wide it is, and
    /// where its messages start: `(size_at, width, messages_at)`.
    fn v2_chunk0(file: &[u8], at: usize) -> (usize, usize, usize) {
        assert_eq!(&file[at..at + 4], b"OHDR");
        let flags = file[at + 5];
        let mut pos = at + 6;
        if flags & 0x20 != 0 {
            pos += 16;
        }
        if flags & 0x10 != 0 {
            pos += 4;
        }
        let width = 1usize << (flags & 0x03);
        (pos, width, pos + width)
    }

    /// A version-2 header whose chunk-0 size field claims gigabytes. The read
    /// came before the checksum is checked, so the checksum is left as it is.
    #[test]
    fn a_v2_chunk0_size_past_the_cap_is_refused_before_the_read() {
        let mut file = LATEST.to_vec();
        let headers = find_all(&file, b"OHDR");
        assert!(!headers.is_empty());
        // Widen the first header's size field to four bytes and claim gigabytes.
        let at = headers[0];
        let (size_at, _, _) = v2_chunk0(&file, at);
        file[at + 5] = (file[at + 5] & !0x03) | 0x02;
        put(&mut file, size_at, &(u32::MAX - 15).to_le_bytes());
        assert_refused_before_the_read(LATEST, &file, HEADER);
    }

    /// A version-2 continuation message claiming a gigabyte-long `OCHK` chunk,
    /// in a chunk 0 re-checksummed so it verifies.
    #[test]
    fn a_v2_continuation_past_the_cap_is_refused_before_the_read() {
        let mut file = LATEST.to_vec();
        let mut patched = false;
        for at in find_all(&file, b"OHDR") {
            let (size_at, width, messages_at) = v2_chunk0(&file, at);
            let mut size = 0usize;
            for (i, &b) in file[size_at..size_at + width].iter().enumerate() {
                size |= usize::from(b) << (8 * i);
            }
            let checksum_at = messages_at + size;
            assert_eq!(
                checksum_lookup3(&file[at..checksum_at]),
                u32_at(&file, checksum_at),
                "chunk 0 of the header at {at} is where it was found"
            );
            // Messages are type(1) size(2) flags(1): the fixture tracks no
            // creation order.
            assert_eq!(file[at + 5] & 0x04, 0);
            let mut pos = messages_at;
            while pos + 4 <= checksum_at {
                let msg_type = file[pos];
                let len = usize::from(u16::from_le_bytes([file[pos + 1], file[pos + 2]]));
                if msg_type == 0x10 {
                    // Address (8) then length (8).
                    put(&mut file, pos + 4 + 8, &CLAIM.to_le_bytes());
                    let sum = checksum_lookup3(&file[at..checksum_at]);
                    put(&mut file, checksum_at, &sum.to_le_bytes());
                    patched = true;
                    break;
                }
                pos += 4 + len;
            }
            if patched {
                break;
            }
        }
        assert!(patched, "the fixture has a version-2 continuation");
        assert_refused_before_the_read(LATEST, &file, HEADER);
    }

    /// `netcdf4_huge_attributes.nc` with each huge heap object, the global
    /// `history` and `t`'s `comment`, claiming gigabytes in its huge-object
    /// B-tree record. Each record's leaf (type 1: address, length, ID, eight
    /// bytes each, one record) is re-checksummed so it verifies.
    fn huge_attributes_past_the_cap() -> Vec<u8> {
        let mut file = HUGE_ATTRIBUTES.to_vec();
        let leaves: Vec<usize> = find_all(&file, b"BTLF")
            .into_iter()
            .filter(|&at| {
                file[at + 5] == 1 && checksum_lookup3(&file[at..at + 30]) == u32_at(&file, at + 30)
            })
            .collect();
        assert_eq!(leaves.len(), 2, "the global and the variable heap");
        for (n, leaf) in leaves.into_iter().enumerate() {
            let length = u64_at(&file, leaf + 6 + 8);
            assert!(
                (4096..65536).contains(&length),
                "a huge object's length: {length}"
            );
            // Each object moves past the bytes held, into a part of the
            // declared file nothing else is read from, so it is well formed:
            // in the file and overlapping nothing. Only its length is wrong.
            put(
                &mut file,
                leaf + 6,
                &(PAST_THE_HELD_BYTES << n).to_le_bytes(),
            );
            put(&mut file, leaf + 6 + 8, &CLAIM.to_le_bytes());
            let sum = checksum_lookup3(&file[leaf..leaf + 30]);
            put(&mut file, leaf + 30, &sum.to_le_bytes());
        }
        file
    }

    /// The metadata of `file` over a declared source, its variable `t`'s
    /// values, and the longest single read.
    fn resolve_declared(file: &[u8]) -> (super::dimensions::Hdf5Metadata, Vec<Option<f64>>, u64) {
        let source = Recording::new(Declared::new(file, DECLARED));
        let reader = NetcdfReader::from_source(&source).expect("the file opens");
        let meta = reader.hdf5_metadata().expect("the metadata resolves");
        let t = meta.variables.iter().find(|v| v.name == "t").expect("t");
        let values = reader
            .decode_variable_raw(t.decode_index)
            .expect("t decodes");
        let longest = source.reads().iter().map(|r| r.len).max().unwrap_or(0);
        (meta, values, longest)
    }

    fn names(attributes: &[super::attribute::Hdf5Attribute]) -> Vec<&str> {
        attributes.iter().map(|a| a.name.as_str()).collect()
    }

    /// A dense attribute past the heap-object cap is skipped, and the file
    /// kept (#997): the global `history` and `t`'s `comment` each claim
    /// gigabytes, and the file still lists its variables, their other
    /// attributes and dimensions, and decodes, with no long read.
    #[test]
    fn an_attribute_past_the_cap_is_skipped_and_the_file_kept() {
        let (before, values, _) = resolve_declared(HUGE_ATTRIBUTES);
        assert_eq!(values, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        let t_before = before.variables.iter().find(|v| v.name == "t").unwrap();
        assert!(names(&before.global_attributes).contains(&"history"));
        assert!(names(&t_before.attributes).contains(&"comment"));
        assert_eq!(t_before.dimensions, ["x"]);

        let (after, values, longest) = resolve_declared(&huge_attributes_past_the_cap());
        assert_eq!(values, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
        let t_after = after.variables.iter().find(|v| v.name == "t").unwrap();
        let without = |all: Vec<&str>, gone: &str| -> Vec<String> {
            all.into_iter()
                .filter(|n| *n != gone)
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            names(&after.global_attributes),
            without(names(&before.global_attributes), "history")
        );
        assert_eq!(
            names(&t_after.attributes),
            without(names(&t_before.attributes), "comment")
        );
        assert_eq!(t_after.dimensions, t_before.dimensions);
        assert_eq!(after.dimensions.len(), before.dimensions.len());
    }

    /// A dense link past the heap-object cap is skipped, and the group kept
    /// (#997): `hdf5_huge_link_name.h5`'s one huge link, a subgroup with a
    /// 5,000-character name, claims gigabytes in its huge-object record.
    #[test]
    fn a_link_past_the_cap_is_skipped_and_the_group_kept() {
        const HUGE_LINK: &[u8] = include_bytes!("../tests/fixtures/hdf5_huge_link_name.h5");
        let mut file = HUGE_LINK.to_vec();
        let leaf = find_all(&file, b"BTLF")
            .into_iter()
            .find(|&at| {
                file[at + 5] == 1 && checksum_lookup3(&file[at..at + 30]) == u32_at(&file, at + 30)
            })
            .expect("a one-record huge-object leaf");
        assert!((4096..65536).contains(&u64_at(&file, leaf + 6 + 8)));
        put(&mut file, leaf + 6, &PAST_THE_HELD_BYTES.to_le_bytes());
        put(&mut file, leaf + 6 + 8, &CLAIM.to_le_bytes());
        let sum = checksum_lookup3(&file[leaf..leaf + 30]);
        put(&mut file, leaf + 30, &sum.to_le_bytes());

        let (outcome, _) = open_declared(HUGE_LINK);
        outcome.expect("the fixture resolves");
        let (outcome, longest) = open_declared(&file);
        outcome.expect("the group resolves without its oversized link");
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
    }

    /// `huge_attributes_past_the_cap` with `t`'s `DIMENSION_LIST` name-index
    /// record pointed at the oversized `comment`. With `drop_comment`, the
    /// `comment` record is taken out of the index, so one record names the
    /// object; without it, two do.
    fn dimension_list_past_the_cap(drop_comment: bool) -> Vec<u8> {
        let mut file = huge_attributes_past_the_cap();
        // `t`'s attribute name index is one leaf: signature, version, type,
        // then type-8 records of heap ID(8), flags(1), creation order(4) and
        // the name's lookup3 hash(4), then a checksum.
        let record = |file: &[u8], name: &str| -> usize {
            let hash = checksum_lookup3(name.as_bytes()).to_le_bytes();
            let at = find_all(file, &hash);
            assert_eq!(at.len(), 1, "one record names {name}");
            at[0] - 13
        };
        let dimension_list = record(&file, "DIMENSION_LIST");
        let comment = record(&file, "comment");
        let id: [u8; 8] = file[comment..comment + 8].try_into().unwrap();
        put(&mut file, dimension_list, &id);

        let leaf = find_all(&file, b"BTLF")
            .into_iter()
            .rfind(|&at| at < dimension_list && file[at + 5] == 8)
            .expect("the attribute name leaf");
        // Its header: signature, version, type, node size(4), record size(2),
        // depth(2), split and merge percents, root address(8), root record
        // count(2), total record count(8), then a checksum.
        let header = find_all(&file, b"BTHD")
            .into_iter()
            .find(|&at| file[at + 5] == 8 && u64_at(&file, at + 16) == leaf as u64)
            .expect("the attribute name index");
        let count = usize::from(u16::from_le_bytes([file[header + 24], file[header + 25]]));
        let mut records = leaf + 6 + 17 * count;
        assert_eq!(
            checksum_lookup3(&HUGE_ATTRIBUTES[leaf..records]),
            u32_at(&file, records),
            "the leaf's checksum"
        );
        if drop_comment {
            file.copy_within(comment + 17..records, comment);
            records -= 17;
            let count = (count - 1) as u16;
            put(&mut file, header + 24, &count.to_le_bytes());
            put(&mut file, header + 26, &u64::from(count).to_le_bytes());
            let sum = checksum_lookup3(&file[header..header + 34]);
            put(&mut file, header + 34, &sum.to_le_bytes());
        }
        let sum = checksum_lookup3(&file[leaf..records]);
        put(&mut file, records, &sum.to_le_bytes());
        file
    }

    /// A `DIMENSION_LIST` past the heap-object cap is the attribute skipped
    /// (#997): `t` keeps its values and falls back to an anonymous axis, as a
    /// variable with no `DIMENSION_LIST` does.
    #[test]
    fn a_dimension_list_past_the_cap_falls_back_to_phony_dimensions() {
        let file = dimension_list_past_the_cap(true);
        let (meta, values, longest) = resolve_declared(&file);
        assert_eq!(values, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
        let t = meta.variables.iter().find(|v| v.name == "t").unwrap();
        assert_eq!(t.dimensions, ["phony_dim_0"]);
        assert!(!names(&t.attributes).contains(&"comment"));
    }

    /// Two records naming one oversized object is shared storage, which is
    /// corrupt whatever the object's size, so the listing is refused rather
    /// than both skipped (#997, #899).
    #[test]
    fn two_records_naming_one_oversized_object_are_refused_not_skipped() {
        let (outcome, _) = open_declared(&dimension_list_past_the_cap(false));
        let err = outcome.expect_err("a shared huge object");
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("overlaps one already read")),
            "{err:?}"
        );
    }

    /// A huge object whose record claims more than the cap and runs past the
    /// end of a buffer is corrupt, not oversized, so it is a parse error
    /// rather than skipped (#997).
    #[test]
    fn a_huge_object_past_the_end_of_the_file_is_refused_not_skipped() {
        let mut file = HUGE_ATTRIBUTES.to_vec();
        let leaf = find_all(&file, b"BTLF")
            .into_iter()
            .find(|&at| {
                file[at + 5] == 1 && checksum_lookup3(&file[at..at + 30]) == u32_at(&file, at + 30)
            })
            .expect("a one-record huge-object leaf");
        put(&mut file, leaf + 6 + 8, &CLAIM.to_le_bytes());
        let sum = checksum_lookup3(&file[leaf..leaf + 30]);
        put(&mut file, leaf + 30, &sum.to_le_bytes());

        let err = NetcdfReader::from_bytes(file)
            .and_then(|r| r.hdf5_metadata().map(drop))
            .expect_err("a huge object past the end of the file");
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m == "huge heap object runs past end of file"),
            "{err:?}"
        );
    }

    /// A global-heap collection, holding the `DIMENSION_LIST` references, whose
    /// size and whose first object both claim gigabytes (#997). The collection
    /// size was clamped only to the declared size, so the object was read whole.
    #[test]
    fn a_global_heap_object_past_the_cap_is_refused_before_the_read() {
        let mut file = EARLIEST.to_vec();
        let collections = find_all(&file, b"GCOL");
        assert!(!collections.is_empty());
        for at in collections {
            // signature(4) version(1) reserved(3) size(8), then the first object:
            // index(2) references(2) reserved(4) size(8).
            assert!(u64_at(&file, at + 8) >= 4096, "a collection size");
            assert_ne!(u16::from_le_bytes([file[at + 16], file[at + 17]]), 0);
            put(&mut file, at + 8, &(CLAIM * 2).to_le_bytes());
            put(&mut file, at + 24, &CLAIM.to_le_bytes());
        }
        assert_refused_before_the_read(EARLIEST, &file, "bytes in one HDF5 global-heap object");
    }

    const BTREE_K_SB1: &[u8] = include_bytes!("../tests/fixtures/hdf5_btree_k_sb1.h5");

    /// An unfiltered chunk whose version-1 B-tree key states gigabytes of stored
    /// bytes (#997). A chunk no filter runs on is its stored bytes, and one of the
    /// wrong length was refused only after it was fetched, at the length the key
    /// states.
    #[test]
    fn an_unfiltered_chunk_of_the_wrong_size_is_refused_before_the_read() {
        let decode = |file: &[u8]| {
            let source = Recording::new(Declared::new(file, DECLARED));
            let outcome = NetcdfReader::from_source(&source).and_then(|r| r.decode_variable_raw(0));
            let longest = source.reads().iter().map(|r| r.len).max().unwrap_or(0);
            (outcome, longest)
        };
        let (outcome, _) = decode(BTREE_K_SB1);
        assert_eq!(outcome.expect("the fixture decodes").len(), 100);

        // `v` is 100 int32 in chunks of one: a raw-data leaf (type 1, level 0)
        // whose keys are size(4) mask(4) offsets(2 × 8), after the signature,
        // type, level, entry count and two sibling addresses.
        let mut file = BTREE_K_SB1.to_vec();
        let leaf = find_all(&file, b"TREE")
            .into_iter()
            .find(|&at| file[at + 4] == 1 && file[at + 5] == 0)
            .expect("a raw-data B-tree leaf");
        assert_eq!(u32_at(&file, leaf + 24), 4, "one int32 a chunk");
        put(&mut file, leaf + 24, &(1u32 << 31).to_le_bytes());
        let (outcome, longest) = decode(&file);
        let err = outcome.expect_err("a chunk of the wrong size");
        assert!(
            err.to_string()
                .contains("chunk stored as 2147483648 bytes, expected 4"),
            "{err}"
        );
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
    }

    const V1_SYMBOLTABLE: &[u8] = include_bytes!("../tests/fixtures/hdf5_v1_symboltable.h5");

    /// A filtered chunk whose version-1 B-tree key states gigabytes of stored
    /// bytes (#997). A filtered chunk is read whole at its stored size, and
    /// only its decoded size was held to the 256 MiB chunk ceiling, so the read
    /// was bounded by nothing but the size the source declares.
    #[test]
    fn a_filtered_chunk_stored_past_the_ceiling_is_refused_before_the_read() {
        // Every dataset in the file, decoded, with the longest single read.
        let decode_all = |file: &[u8]| {
            let source = Recording::new(Declared::new(file, DECLARED));
            let reader = NetcdfReader::from_source(&source).expect("the file opens");
            let outcomes: Vec<_> = (0..)
                .take_while(|&i| reader.variable_shape(i).is_ok())
                .map(|i| reader.decode_variable_raw(i).map(drop))
                .collect();
            let longest = source.reads().iter().map(|r| r.len).max().unwrap_or(0);
            (outcomes, longest)
        };
        let (outcomes, _) = decode_all(V1_SYMBOLTABLE);
        assert_eq!(outcomes.len(), 9);
        // `label` is text, which value decode refuses; every other dataset
        // decodes.
        let decoded = |outcomes: &[Result<(), FieldglassError>]| {
            outcomes
                .iter()
                .filter(|o| !matches!(o, Err(FieldglassError::UnsupportedSection(_))))
                .count()
        };
        assert_eq!(decoded(&outcomes), 8, "{outcomes:?}");
        assert!(
            outcomes
                .iter()
                .all(|o| matches!(o, Ok(()) | Err(FieldglassError::UnsupportedSection(_)))),
            "{outcomes:?}"
        );

        // `compressed` is 8×8 float32 in 4×4 chunks through shuffle and
        // deflate: a raw-data leaf (type 1, level 0) whose rank-2 keys are
        // size(4) mask(4) offsets(3 × 8), each followed by an 8-byte child
        // address. Its stored sizes are deflate's, so not the 64 bytes an
        // unfiltered 4×4 float32 chunk is stored as.
        let mut file = V1_SYMBOLTABLE.to_vec();
        let mut rewritten = 0;
        for leaf in find_all(&file, b"TREE") {
            if file[leaf + 4] != 1 || file[leaf + 5] != 0 {
                continue;
            }
            let entries = usize::from(u16::from_le_bytes([file[leaf + 6], file[leaf + 7]]));
            let key = |i: usize| leaf + 24 + i * 40;
            // The only leaf of four chunks: the other is a rank-1 dataset's.
            if entries != 4 || u32_at(&file, key(0)) == 64 {
                continue;
            }
            for i in 0..entries {
                put(&mut file, key(i), &(1u32 << 31).to_le_bytes());
            }
            rewritten += 1;
        }
        assert_eq!(rewritten, 1, "the one filtered raw-data leaf");

        let (outcomes, longest) = decode_all(&file);
        let refused: Vec<_> = outcomes
            .iter()
            .filter_map(|o| o.as_ref().err())
            .filter(|e| !matches!(e, FieldglassError::UnsupportedSection(_)))
            .collect();
        assert_eq!(refused.len(), 1, "{outcomes:?}");
        assert!(
            matches!(
                refused[0],
                FieldglassError::Array(ArrayError::StoredChunkTooLarge { bytes, limit })
                    if *bytes == 1 << 31 && *limit == 256 << 20
            ),
            "{:?}",
            refused[0]
        );
        assert!(
            longest <= MAX_WINDOW_BYTES as u64,
            "read {longest} bytes at once"
        );
    }
}
