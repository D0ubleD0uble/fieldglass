//! HDF5 Data Layout message (`0x0008`) decoder (issue #121, under #33). Tells
//! value decode *where* and *how* a dataset's raw elements are stored:
//!
//! * **Compact** — the elements live inline in the message body itself (tiny
//!   datasets).
//! * **Contiguous** — one unbroken run of elements at a file address.
//! * **Chunked** — the elements are split into fixed-size chunks, each stored
//!   (and optionally filtered) separately and located through a chunk index.
//!
//! Both **version 3** and **version 4** of the message are decoded. Version 3
//! is the layout `libhdf5` has written since 1.6; its chunked form indexes the
//! chunks with a **version-1 B-tree** (node type 1). Version 4 is what libhdf5
//! ≥ 1.10 writes under the "latest format", and its chunked form selects among
//! five chunk indexes — single chunk, implicit, fixed array, extensible array,
//! and v2 B-tree. This decoder handles four of those: **single chunk** (whole
//! dataset is one chunk) and **fixed array** (fixed-shape multi-chunk) for both
//! filtered and unfiltered chunks, **implicit** (fixed-shape, early-allocated,
//! unfiltered chunks stored contiguously with no on-disk index), and
//! **extensible array** (one unlimited dimension) for unfiltered chunks. The
//! v2-B-tree index and filtered extensible arrays are recognised and rejected
//! with a clear per-index error rather than mis-read — they remain a tracked
//! follow-up (#216).
//!
//! Reference: HDF5 file format specification version 3, "Data Layout Message"
//! (version 4 chunked storage property description) and "The Fixed Array Index".

use super::Hdf5Probe;
use super::object_header::{is_undefined_address, read_uint_le};
use fieldglass_core::FieldglassError;

// Layout class codes (the byte after the version).
const CLASS_COMPACT: u8 = 0;
const CLASS_CONTIGUOUS: u8 = 1;
const CLASS_CHUNKED: u8 = 2;

/// Upper bound on a chunked dataset's rank — guards a corrupt dimensionality
/// byte. Real chunked datasets top out at a handful of dimensions.
const MAX_CHUNK_RANK: usize = 32;

/// Where a dataset's raw elements live and how they're stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataLayout {
    /// Elements stored inline in the layout message body.
    Compact { data: Vec<u8> },
    /// One contiguous run at `address`; `size` bytes. `address` is `None` when
    /// the storage is unallocated (undefined-address sentinel) — the dataset
    /// reads entirely as its fill value.
    Contiguous { address: Option<u64>, size: u64 },
    /// Chunked storage, located through a chunk index (see [`ChunkIndex`]).
    Chunked(ChunkedLayout),
}

/// A chunked dataset's geometry and chunk-index location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedLayout {
    /// Chunk edge lengths in elements, in dataset dimension order. Excludes the
    /// trailing element-size pseudo-dimension carried on disk.
    pub chunk_dims: Vec<u32>,
    /// Element size in bytes (the trailing on-disk chunk dimension).
    pub element_size: u32,
    /// How (and where) the chunks are indexed.
    pub index: ChunkIndex,
}

/// The chunk index that locates a chunked dataset's chunks. Version-3 messages
/// always use a version-1 B-tree; version-4 messages select among several — of
/// which single chunk, implicit, fixed array, and extensible array are decoded
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkIndex {
    /// Version-1 B-tree (node type 1) at the given address. `None` when no chunk
    /// has been written yet (undefined-address sentinel) — the dataset reads
    /// entirely as its fill value.
    BTreeV1(Option<u64>),
    /// The whole dataset is a single chunk, stored inline in the layout message
    /// (v4 chunk index type 1). `None` when the chunk is unallocated.
    SingleChunk(Option<SingleChunk>),
    /// Fixed-shape, early-allocated, unfiltered dataset with an Implicit index
    /// (v4 chunk index type 2): the chunks are stored contiguously in row-major
    /// chunk order with no on-disk index structure, so the value is the base
    /// address of that contiguous chunk array. `None` when unallocated.
    Implicit(Option<u64>),
    /// Fixed-shape dataset indexed by a Fixed Array (v4 chunk index type 3); the
    /// address is that of the "FAHD" header. `None` when unallocated.
    FixedArray(Option<u64>),
    /// Unlimited-dimension dataset indexed by an Extensible Array (v4 chunk index
    /// type 4); the address is that of the "EAHD" header. `None` when unallocated.
    ExtensibleArray(Option<u64>),
}

/// The inline location of a single-chunk dataset's one chunk (v4 index type 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleChunk {
    /// File address of the chunk's raw (possibly filtered) bytes.
    pub address: u64,
    /// On-disk filtered byte size when the chunk is filtered; `None` for an
    /// unfiltered chunk, whose size is the full uncompressed chunk byte size.
    pub filtered_size: Option<u64>,
    /// Filter mask (which filters were skipped for this chunk); 0 when unfiltered.
    pub filter_mask: u32,
}

/// Decode a Data Layout message body.
pub fn decode(body: &[u8], probe: &Hdf5Probe) -> Result<DataLayout, FieldglassError> {
    let version = *body
        .first()
        .ok_or_else(|| FieldglassError::Parse("empty data layout message".into()))?;
    if version != 3 && version != 4 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "HDF5 data layout message version {version} is not supported \
             (only versions 3 and 4 are)"
        )));
    }
    let class = *body
        .get(1)
        .ok_or_else(|| FieldglassError::Parse("truncated data layout message".into()))?;
    let osize = probe.offset_size as usize;
    let lsize = probe.length_size as usize;

    // The chunked layout differs between message versions; compact and
    // contiguous share one encoding across both.
    if class == CLASS_CHUNKED {
        return if version == 3 {
            decode_chunked(body, probe, osize)
        } else {
            decode_chunked_v4(body, probe, osize, lsize)
        };
    }

    match class {
        CLASS_COMPACT => {
            // size (2 bytes) then `size` bytes of inline data.
            let size = read_uint_le(body, 2, 2)? as usize;
            let start = 4usize;
            let end = start
                .checked_add(size)
                .filter(|&e| e <= body.len())
                .ok_or_else(|| {
                    FieldglassError::Parse("compact layout data overruns the message".into())
                })?;
            Ok(DataLayout::Compact {
                data: body[start..end].to_vec(),
            })
        }
        CLASS_CONTIGUOUS => {
            // address (offset_size) then size (length_size).
            let raw_addr = read_uint_le(body, 2, osize)?;
            let size = read_uint_le(body, 2 + osize, lsize)?;
            let address = if is_undefined_address(raw_addr, probe.offset_size) {
                None
            } else {
                Some(raw_addr)
            };
            Ok(DataLayout::Contiguous { address, size })
        }
        // CLASS_CHUNKED is dispatched above (its encoding is version-specific).
        other => Err(FieldglassError::Parse(format!(
            "unsupported HDF5 data layout class {other}"
        ))),
    }
}

/// Decode the version-3 chunked layout: dimensionality, B-tree address, then the
/// chunk dimension array (the last entry is the element size, not a real
/// dimension).
fn decode_chunked(
    body: &[u8],
    probe: &Hdf5Probe,
    osize: usize,
) -> Result<DataLayout, FieldglassError> {
    // dimensionality (1) includes the trailing element-size pseudo-dimension.
    let dimensionality = *body
        .get(2)
        .ok_or_else(|| FieldglassError::Parse("truncated chunked layout message".into()))?
        as usize;
    if !(1..=MAX_CHUNK_RANK + 1).contains(&dimensionality) {
        return Err(FieldglassError::Parse(format!(
            "chunked layout dimensionality {dimensionality} out of range"
        )));
    }
    let raw_addr = read_uint_le(body, 3, osize)?;
    let btree_address = if is_undefined_address(raw_addr, probe.offset_size) {
        None
    } else {
        Some(raw_addr)
    };

    // `dimensionality` 4-byte sizes: the first rank are chunk dims, the last is
    // the element size in bytes.
    let mut dims = Vec::with_capacity(dimensionality);
    let mut pos = 3 + osize;
    for _ in 0..dimensionality {
        dims.push(read_uint_le(body, pos, 4)? as u32);
        pos += 4;
    }
    let element_size = dims
        .pop()
        .expect("dimensionality >= 1 guarantees an element-size entry");

    Ok(DataLayout::Chunked(ChunkedLayout {
        chunk_dims: dims,
        element_size,
        index: ChunkIndex::BTreeV1(btree_address),
    }))
}

// Version-4 chunk index type codes (the byte after the encoded chunk dims).
const V4_INDEX_SINGLE_CHUNK: u8 = 1;
const V4_INDEX_IMPLICIT: u8 = 2;
const V4_INDEX_FIXED_ARRAY: u8 = 3;
const V4_INDEX_EXTENSIBLE_ARRAY: u8 = 4;
const V4_INDEX_V2_BTREE: u8 = 5;

/// Flags bit 1 of the v4 chunked property description: a filtered chunk under
/// the Single Chunk index. (Bit 0 is `DONT_FILTER_PARTIAL_BOUND_CHUNKS`; the
/// spec prose mislabels this gate as bit 0, but libhdf5 uses bit 1.)
const V4_FLAG_SINGLE_INDEX_WITH_FILTER: u8 = 0b10;

/// Decode the version-4 chunked layout property description: flags,
/// dimensionality, the encoded chunk dimensions, then a chunk-index-type byte
/// selecting how the chunks are located. Single Chunk (type 1) and Fixed Array
/// (type 3) are decoded; the others return a clear per-index error.
fn decode_chunked_v4(
    body: &[u8],
    probe: &Hdf5Probe,
    osize: usize,
    lsize: usize,
) -> Result<DataLayout, FieldglassError> {
    // version(1) class(1) flags(1) dimensionality(1) dim_encoded_len(1) ...
    let flags = *body
        .get(2)
        .ok_or_else(|| FieldglassError::Parse("truncated v4 chunked layout".into()))?;
    let dimensionality = *body
        .get(3)
        .ok_or_else(|| FieldglassError::Parse("truncated v4 chunked layout".into()))?
        as usize;
    if !(1..=MAX_CHUNK_RANK + 1).contains(&dimensionality) {
        return Err(FieldglassError::Parse(format!(
            "v4 chunked layout dimensionality {dimensionality} out of range"
        )));
    }
    let dim_encoded_len = *body
        .get(4)
        .ok_or_else(|| FieldglassError::Parse("truncated v4 chunked layout".into()))?
        as usize;
    if !(1..=8).contains(&dim_encoded_len) {
        return Err(FieldglassError::Parse(format!(
            "v4 chunked dimension-size encoded length {dim_encoded_len} out of range"
        )));
    }

    // `dimensionality` encoded chunk dims, the last of which is the element size.
    let mut dims = Vec::with_capacity(dimensionality);
    let mut pos = 5usize;
    for _ in 0..dimensionality {
        // The encoded width can be up to 8 bytes; a chunk edge is a u32, so
        // reject an out-of-range value rather than silently truncating it.
        let dim = u32::try_from(read_uint_le(body, pos, dim_encoded_len)?)
            .map_err(|_| FieldglassError::Parse("v4 chunk dimension exceeds u32".into()))?;
        dims.push(dim);
        pos += dim_encoded_len;
    }
    let element_size = dims
        .pop()
        .expect("dimensionality >= 1 guarantees an element-size entry");

    let index_type = *body
        .get(pos)
        .ok_or_else(|| FieldglassError::Parse("truncated v4 chunked layout".into()))?;
    pos += 1;

    let index = match index_type {
        V4_INDEX_SINGLE_CHUNK => {
            let filtered = flags & V4_FLAG_SINGLE_INDEX_WITH_FILTER != 0;
            let (filtered_size, filter_mask) = if filtered {
                // Size of filtered chunk (length_size) then filter mask (4).
                let size = read_uint_le(body, pos, lsize)?;
                pos += lsize;
                let mask = read_uint_le(body, pos, 4)? as u32;
                pos += 4;
                (Some(size), mask)
            } else {
                (None, 0)
            };
            let raw_addr = read_uint_le(body, pos, osize)?;
            if is_undefined_address(raw_addr, probe.offset_size) {
                ChunkIndex::SingleChunk(None)
            } else {
                ChunkIndex::SingleChunk(Some(SingleChunk {
                    address: raw_addr,
                    filtered_size,
                    filter_mask,
                }))
            }
        }
        V4_INDEX_FIXED_ARRAY => {
            // Page Bits (1) then the Fixed Array header address (offset_size).
            pos += 1;
            let raw_addr = read_uint_le(body, pos, osize)?;
            if is_undefined_address(raw_addr, probe.offset_size) {
                ChunkIndex::FixedArray(None)
            } else {
                ChunkIndex::FixedArray(Some(raw_addr))
            }
        }
        V4_INDEX_IMPLICIT => {
            // No index-specific parameters and no on-disk index structure: the
            // base address of the contiguous chunk array follows the index-type
            // byte directly. Every chunk is allocated (early allocation), so an
            // undefined base address means the whole dataset is unwritten.
            let raw_addr = read_uint_le(body, pos, osize)?;
            if is_undefined_address(raw_addr, probe.offset_size) {
                ChunkIndex::Implicit(None)
            } else {
                ChunkIndex::Implicit(Some(raw_addr))
            }
        }
        V4_INDEX_EXTENSIBLE_ARRAY => {
            // Index-specific info: max bits, index-block elements, min data-block
            // pointers, min data-block elements, and data-block page bits — five
            // bytes that duplicate fields the header restates, so skip them and
            // read the header address. The authoritative values come from the
            // "EAHD" header itself at decode time.
            pos += 5;
            let raw_addr = read_uint_le(body, pos, osize)?;
            if is_undefined_address(raw_addr, probe.offset_size) {
                ChunkIndex::ExtensibleArray(None)
            } else {
                ChunkIndex::ExtensibleArray(Some(raw_addr))
            }
        }
        V4_INDEX_V2_BTREE => {
            return Err(FieldglassError::UnsupportedSection(
                "HDF5 v4 chunked layout uses the v2 B-tree index, which is not decoded yet".into(),
            ));
        }
        other => {
            return Err(FieldglassError::Parse(format!(
                "unknown HDF5 v4 chunk index type {other}"
            )));
        }
    };

    Ok(DataLayout::Chunked(ChunkedLayout {
        chunk_dims: dims,
        element_size,
        index,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Hdf5Probe {
        Hdf5Probe {
            superblock_version: 0,
            offset_size: 8,
            length_size: 8,
        }
    }

    #[test]
    fn decodes_contiguous() {
        // version 3, class 1, address = 0x200, size = 40.
        let mut body = vec![3u8, CLASS_CONTIGUOUS];
        body.extend_from_slice(&0x200u64.to_le_bytes());
        body.extend_from_slice(&40u64.to_le_bytes());
        let layout = decode(&body, &probe()).unwrap();
        assert_eq!(
            layout,
            DataLayout::Contiguous {
                address: Some(0x200),
                size: 40
            }
        );
    }

    #[test]
    fn contiguous_undefined_address_is_none() {
        let mut body = vec![3u8, CLASS_CONTIGUOUS];
        body.extend_from_slice(&u64::MAX.to_le_bytes());
        body.extend_from_slice(&40u64.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Contiguous { address, .. } => assert_eq!(address, None),
            other => panic!("expected contiguous, got {other:?}"),
        }
    }

    #[test]
    fn decodes_compact_inline_data() {
        let mut body = vec![3u8, CLASS_COMPACT];
        body.extend_from_slice(&4u16.to_le_bytes());
        body.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(
            decode(&body, &probe()).unwrap(),
            DataLayout::Compact {
                data: vec![1, 2, 3, 4]
            }
        );
    }

    #[test]
    fn decodes_chunked_2d() {
        // version 3, class 2, dimensionality 3 (2 dims + element size),
        // B-tree address 0x400, chunk dims [5, 5], element size 4.
        let mut body = vec![3u8, CLASS_CHUNKED, 3];
        body.extend_from_slice(&0x400u64.to_le_bytes());
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => {
                assert_eq!(c.chunk_dims, vec![5, 5]);
                assert_eq!(c.element_size, 4);
                assert_eq!(c.index, ChunkIndex::BTreeV1(Some(0x400)));
            }
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_single_chunk_unfiltered() {
        // version 4, class 2, flags 0, dimensionality 3 (2 dims + element size),
        // dim_encoded_len 1, dims [4,4,4], index type 1 (single chunk),
        // address 0x800. Mirrors a real libhdf5 whole-dataset-as-one-chunk file.
        let mut body = vec![4u8, CLASS_CHUNKED, 0, 3, 1, 4, 4, 4, V4_INDEX_SINGLE_CHUNK];
        body.extend_from_slice(&0x800u64.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => {
                assert_eq!(c.chunk_dims, vec![4, 4]);
                assert_eq!(c.element_size, 4);
                assert_eq!(
                    c.index,
                    ChunkIndex::SingleChunk(Some(SingleChunk {
                        address: 0x800,
                        filtered_size: None,
                        filter_mask: 0,
                    }))
                );
            }
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_single_chunk_filtered() {
        // flags bit 1 set (SINGLE_INDEX_WITH_FILTER): a size (length_size) and a
        // 4-byte filter mask precede the address.
        let mut body = vec![
            4u8,
            CLASS_CHUNKED,
            V4_FLAG_SINGLE_INDEX_WITH_FILTER,
            3,
            1,
            4,
            4,
            4,
            V4_INDEX_SINGLE_CHUNK,
        ];
        body.extend_from_slice(&37u64.to_le_bytes()); // filtered size
        body.extend_from_slice(&0u32.to_le_bytes()); // filter mask
        body.extend_from_slice(&0x800u64.to_le_bytes()); // address
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => assert_eq!(
                c.index,
                ChunkIndex::SingleChunk(Some(SingleChunk {
                    address: 0x800,
                    filtered_size: Some(37),
                    filter_mask: 0,
                }))
            ),
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_fixed_array() {
        // index type 3 (fixed array): a page-bits byte then the FAHD header
        // address.
        let mut body = vec![
            4u8,
            CLASS_CHUNKED,
            0,
            3,
            1,
            4,
            4,
            4,
            V4_INDEX_FIXED_ARRAY,
            10,
        ];
        body.extend_from_slice(&0x1bfu64.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => assert_eq!(c.index, ChunkIndex::FixedArray(Some(0x1bf))),
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_extensible_array() {
        // Real libhdf5 extensible-array layout message (1-D unlimited dataset,
        // chunk edge 4, element size 4): index type 4, five bytes of index info,
        // then the "EAHD" header address (0x1bf).
        let body = vec![
            4u8,
            CLASS_CHUNKED,
            0,
            2,
            1,
            4,
            4,
            V4_INDEX_EXTENSIBLE_ARRAY,
            0x20,
            0x04,
            0x04,
            0x10,
            0x0a,
        ];
        let mut body = body;
        body.extend_from_slice(&0x1bfu64.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => {
                assert_eq!(c.chunk_dims, vec![4]);
                assert_eq!(c.element_size, 4);
                assert_eq!(c.index, ChunkIndex::ExtensibleArray(Some(0x1bf)));
            }
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_implicit() {
        // Real libhdf5 implicit-index layout message (2-D 8×8 dataset in 4×4
        // chunks, element size 4): dimensionality 3, dims [4,4,4], index type 2,
        // then the base address of the contiguous chunk array (0x800) with no
        // intervening page-bits byte.
        let mut body = vec![4u8, CLASS_CHUNKED, 0, 3, 1, 4, 4, 4, V4_INDEX_IMPLICIT];
        body.extend_from_slice(&0x800u64.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => {
                assert_eq!(c.chunk_dims, vec![4, 4]);
                assert_eq!(c.element_size, 4);
                assert_eq!(c.index, ChunkIndex::Implicit(Some(0x800)));
            }
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn decodes_v4_implicit_unallocated() {
        // An undefined base address (all-0xFF) means every chunk is unwritten.
        let mut body = vec![4u8, CLASS_CHUNKED, 0, 3, 1, 4, 4, 4, V4_INDEX_IMPLICIT];
        body.extend_from_slice(&u64::MAX.to_le_bytes());
        match decode(&body, &probe()).unwrap() {
            DataLayout::Chunked(c) => assert_eq!(c.index, ChunkIndex::Implicit(None)),
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_message_version() {
        let body = vec![5u8, CLASS_CONTIGUOUS];
        assert!(matches!(
            decode(&body, &probe()),
            Err(FieldglassError::UnsupportedSection(_))
        ));
    }

    #[test]
    fn rejects_truncated_chunked() {
        let body = vec![3u8, CLASS_CHUNKED];
        assert!(decode(&body, &probe()).is_err());
    }

    #[test]
    fn rejects_truncated_v4_chunked() {
        // A v4 single-chunk header that stops before the 8-byte address must
        // error rather than read past the buffer.
        let body = vec![
            4u8,
            CLASS_CHUNKED,
            0,
            3,
            1,
            4,
            4,
            4,
            V4_INDEX_SINGLE_CHUNK,
            0,
            0,
        ];
        assert!(decode(&body, &probe()).is_err());
    }

    #[test]
    fn rejects_v4_chunk_dimension_over_u32() {
        // An 8-byte-encoded chunk dimension larger than u32::MAX is rejected,
        // not truncated. dim_encoded_len = 8, first dim = u64::MAX.
        let mut body = vec![4u8, CLASS_CHUNKED, 0, 3, 8];
        body.extend_from_slice(&u64::MAX.to_le_bytes()); // oversized chunk dim
        body.extend_from_slice(&4u64.to_le_bytes()); // second dim
        body.extend_from_slice(&4u64.to_le_bytes()); // element size
        body.push(V4_INDEX_SINGLE_CHUNK);
        body.extend_from_slice(&0x800u64.to_le_bytes());
        assert!(matches!(
            decode(&body, &probe()),
            Err(FieldglassError::Parse(_))
        ));
    }
}
