//! HDF5 Data Layout message (`0x0008`) decoder (issue #121, under #33). Tells
//! value decode *where* and *how* a dataset's raw elements are stored:
//!
//! * **Compact** — the elements live inline in the message body itself (tiny
//!   datasets).
//! * **Contiguous** — one unbroken run of elements at a file address.
//! * **Chunked** — the elements are split into fixed-size chunks, each stored
//!   (and optionally filtered) separately and located through a chunk index.
//!
//! Only **version 3** of the message is decoded — the layout `libhdf5` has
//! written since 1.6 and what every NetCDF-4 file in the wild uses. For chunked
//! version-3 datasets the chunk index is a **version-1 B-tree** (node type 1);
//! that is the index this decoder records. The newer version-4 message (whose
//! chunked form selects among single-chunk / implicit / fixed-array /
//! extensible-array / v2-B-tree indexes) is recognised and rejected with a clear
//! error rather than mis-read — it is a tracked follow-up.
//!
//! Reference: HDF5 file format specification version 3, "Data Layout Message".

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
    /// Chunked storage indexed by a version-1 B-tree.
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
    /// File address of the version-1 B-tree that indexes the chunks; `None`
    /// when no chunk has been written yet (undefined-address sentinel).
    pub btree_address: Option<u64>,
}

/// Decode a Data Layout message body.
pub fn decode(body: &[u8], probe: &Hdf5Probe) -> Result<DataLayout, FieldglassError> {
    let version = *body
        .first()
        .ok_or_else(|| FieldglassError::Parse("empty data layout message".into()))?;
    if version != 3 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "HDF5 data layout message version {version} is not supported \
             (only version 3 is; version 4 chunk indexes are a follow-up)"
        )));
    }
    let class = *body
        .get(1)
        .ok_or_else(|| FieldglassError::Parse("truncated data layout message".into()))?;
    let osize = probe.offset_size as usize;
    let lsize = probe.length_size as usize;

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
        CLASS_CHUNKED => decode_chunked(body, probe, osize),
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
        btree_address,
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
                assert_eq!(c.btree_address, Some(0x400));
            }
            other => panic!("expected chunked, got {other:?}"),
        }
    }

    #[test]
    fn rejects_version_4() {
        let body = vec![4u8, CLASS_CHUNKED, 3];
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
}
