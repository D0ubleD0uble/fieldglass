//! HDF5 dataset value decode (issue #121, under #33). Reads a dataset's numeric
//! elements into the same `Vec<Option<f64>>` surface the classic NetCDF path
//! produces: `Some(v)` for a present point, `None` where the element equals the
//! variable's `_FillValue` *attribute* (mirroring how `libnetcdf` masks). The
//! decode is decoupled from rendering — it yields the whole variable in
//! row-major (C) order; slice selection happens downstream.
//!
//! Storage is read for the three Data Layout classes a NetCDF-4 file uses:
//! compact, contiguous, and chunked. Chunked datasets are located through their
//! chunk index — the version-1 B-tree of the legacy `libver=earliest` form, or
//! the version-4/5 single-chunk, fixed-array, extensible-array, and implicit
//! indexes of the "latest format".
//! Chunks pass back through the [`filter`](super::filter) pipeline
//! (deflate / shuffle) before being scattered into place; any region with no
//! stored chunk reads as the dataset's Fill Value (message `0x0005`) default.
//!
//! Element bytes honour the datatype's byte order — unlike classic NetCDF
//! (always big-endian), HDF5 records it per type and NetCDF-4 writers normally
//! pick the host's little-endian order.

use super::datatype::DatatypeClass;
use super::layout::{ChunkIndex, ChunkedLayout, DataLayout};
use super::object_header::{self, read_uint_le};
use super::{Hdf5Probe, attribute, dataspace, filter::FilterPipeline, layout};
use crate::classic::{MAX_VAR_ELEMENTS, NcType};
use fieldglass_core::FieldglassError;

const MSG_DATASPACE: u16 = 0x0001;
const MSG_DATATYPE: u16 = 0x0003;
const MSG_FILL_VALUE: u16 = 0x0005;
const MSG_DATA_LAYOUT: u16 = 0x0008;
const MSG_FILTER_PIPELINE: u16 = 0x000B;

/// B-tree v1 chunk-node signature.
const SIG_BTREE_V1: &[u8; 4] = b"TREE";
/// Upper bound on B-tree nodes visited while collecting chunks — guards a
/// malformed or cyclic chunk index.
const MAX_BTREE_NODES: usize = 1 << 20;

/// Decode the dataset whose object header is at `object_header_address` into
/// row-major `Vec<Option<f64>>`. Numeric types widen to `f64`; string / `char`
/// datasets hold text, not numbers, and are rejected.
pub fn read_dataset_values(
    bytes: &[u8],
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let header = object_header::walk(
        bytes,
        object_header_address,
        probe.offset_size,
        probe.length_size,
    )?;
    let body = |msg_type: u16| {
        header
            .messages
            .iter()
            .find(|m| m.msg_type == msg_type)
            .map(|m| m.body.as_slice())
    };

    let dataspace = dataspace::decode(
        body(MSG_DATASPACE)
            .ok_or_else(|| FieldglassError::Parse("dataset has no dataspace".into()))?,
        probe.length_size,
    )?;
    let datatype = super::datatype::decode(
        body(MSG_DATATYPE)
            .ok_or_else(|| FieldglassError::Parse("dataset has no datatype".into()))?,
    )?;
    if matches!(datatype.class, DatatypeClass::FixedLengthString)
        || matches!(datatype.nc_type, NcType::Char)
    {
        return Err(FieldglassError::UnsupportedSection(
            "HDF5 string dataset holds text, not numbers; value decode does not apply".into(),
        ));
    }
    let data_layout = layout::decode(
        body(MSG_DATA_LAYOUT)
            .ok_or_else(|| FieldglassError::Parse("dataset has no data layout".into()))?,
        probe,
    )?;
    let pipeline = match body(MSG_FILTER_PIPELINE) {
        Some(b) => FilterPipeline::decode(b)?,
        None => FilterPipeline::default(),
    };
    let fill_default = body(MSG_FILL_VALUE)
        .and_then(|b| fill_value_default(b).ok())
        .flatten();

    // `_FillValue` and CF `missing_value` *attributes* drive masking, matching
    // classic / libnetcdf.
    let fills = missing_sentinels(bytes, object_header_address, probe)?;

    let shape: Vec<u64> = dataspace.dims.clone();
    let total = checked_total(&shape)?;
    let elem = datatype.size as usize;
    if elem == 0 {
        return Err(FieldglassError::Parse(
            "dataset element size is zero".into(),
        ));
    }
    if total == 0 {
        return Ok(Vec::new());
    }

    // Assemble the dataset's raw element bytes, then decode them uniformly.
    let raw = assemble_raw(
        bytes,
        &data_layout,
        &shape,
        elem,
        &pipeline,
        fill_default.as_deref(),
        probe,
    )?;

    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let off = i * elem;
        let v = datatype
            .read_element_f64(&raw[off..off + elem])
            .ok_or_else(|| FieldglassError::Parse("dataset element decode failed".into()))?;
        out.push(if fills.contains(&v) { None } else { Some(v) });
    }
    Ok(out)
}

/// Total element count for `shape`, with the same overflow / cap guards the
/// classic path applies. A rank-0 (scalar) dataset has one element.
fn checked_total(shape: &[u64]) -> Result<usize, FieldglassError> {
    let total_u64 = shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| FieldglassError::Parse(format!("dataset shape {shape:?} overflows")))?;
    let total = usize::try_from(total_u64)
        .map_err(|_| FieldglassError::Parse("dataset element count exceeds usize".into()))?;
    if total > MAX_VAR_ELEMENTS {
        return Err(FieldglassError::Parse(format!(
            "dataset has {total} elements, exceeds cap of {MAX_VAR_ELEMENTS}"
        )));
    }
    Ok(total)
}

/// Produce the dataset's raw element bytes (`total * elem` long) for any layout
/// class. Regions with no stored data read as the fill default (or zero).
fn assemble_raw(
    bytes: &[u8],
    data_layout: &DataLayout,
    shape: &[u64],
    elem: usize,
    pipeline: &FilterPipeline,
    fill_default: Option<&[u8]>,
    probe: &Hdf5Probe,
) -> Result<Vec<u8>, FieldglassError> {
    let span = byte_span(shape, elem)?;

    match data_layout {
        DataLayout::Compact { data } => {
            if data.len() < span {
                return Err(FieldglassError::Parse(format!(
                    "compact dataset holds {} bytes, needs {span}",
                    data.len()
                )));
            }
            Ok(data[..span].to_vec())
        }
        DataLayout::Contiguous { address, .. } => {
            let mut raw = fill_buffer(span, elem, fill_default);
            if let Some(addr) = address {
                let start = usize::try_from(*addr)
                    .map_err(|_| FieldglassError::Parse("data address exceeds usize".into()))?;
                let end = start
                    .checked_add(span)
                    .filter(|&e| e <= bytes.len())
                    .ok_or_else(|| {
                        FieldglassError::Parse(format!(
                            "contiguous data [{start}, +{span}) exceeds file size {}",
                            bytes.len()
                        ))
                    })?;
                raw.copy_from_slice(&bytes[start..end]);
            }
            Ok(raw)
        }
        DataLayout::Chunked(chunked) => {
            assemble_chunked(bytes, chunked, shape, elem, pipeline, fill_default, probe)
        }
    }
}

/// Total byte size of a dataset (`product(shape) * elem`), overflow-checked.
fn byte_span(shape: &[u64], elem: usize) -> Result<usize, FieldglassError> {
    shape
        .iter()
        .try_fold(elem, |acc, &d| acc.checked_mul(usize::try_from(d).ok()?))
        .ok_or_else(|| FieldglassError::Parse("dataset byte size overflows usize".into()))
}

/// A `span`-byte buffer pre-filled with the dataset's fill default, repeated per
/// element. Falls back to zeros when no usable fill default is present.
fn fill_buffer(span: usize, elem: usize, fill_default: Option<&[u8]>) -> Vec<u8> {
    match fill_default {
        Some(fill) if fill.len() == elem && fill.iter().any(|&b| b != 0) => {
            let mut raw = Vec::with_capacity(span);
            while raw.len() < span {
                raw.extend_from_slice(fill);
            }
            raw.truncate(span);
            raw
        }
        _ => vec![0u8; span],
    }
}

/// Assemble a chunked dataset: gather its chunk records from whichever chunk
/// index the layout uses, reverse each chunk's filters, and scatter it into the
/// row-major output. Unstored regions keep the fill default.
fn assemble_chunked(
    bytes: &[u8],
    chunked: &ChunkedLayout,
    shape: &[u64],
    elem: usize,
    pipeline: &FilterPipeline,
    fill_default: Option<&[u8]>,
    probe: &Hdf5Probe,
) -> Result<Vec<u8>, FieldglassError> {
    let osize = probe.offset_size;
    let rank = shape.len();
    if chunked.chunk_dims.len() != rank {
        return Err(FieldglassError::Parse(format!(
            "chunk rank {} disagrees with dataset rank {rank}",
            chunked.chunk_dims.len()
        )));
    }
    if chunked.element_size as usize != elem {
        return Err(FieldglassError::Parse(format!(
            "chunk element size {} disagrees with datatype size {elem}",
            chunked.element_size
        )));
    }
    let span = byte_span(shape, elem)?;
    let mut raw = fill_buffer(span, elem, fill_default);

    // A zero chunk edge is malformed; reject it up front so the chunk-grid math
    // (which divides by each chunk edge) can't divide by zero.
    if chunked.chunk_dims.contains(&0) {
        return Err(FieldglassError::Parse(
            "chunked layout has a zero-length chunk dimension".into(),
        ));
    }
    let chunk_elems: usize = chunked
        .chunk_dims
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d as usize))
        .ok_or_else(|| FieldglassError::Parse("chunk element count overflows usize".into()))?;
    let chunk_bytes = chunk_elems
        .checked_mul(elem)
        .ok_or_else(|| FieldglassError::Parse("chunk byte size overflows usize".into()))?;

    // Every chunk index resolves to the same per-chunk record; only the way the
    // records are located differs. An unallocated index leaves the buffer as
    // all fill.
    let chunks = match &chunked.index {
        ChunkIndex::BTreeV1(None)
        | ChunkIndex::SingleChunk(None)
        | ChunkIndex::Implicit(None)
        | ChunkIndex::FixedArray(None)
        | ChunkIndex::ExtensibleArray(None)
        | ChunkIndex::V2Btree(None) => return Ok(raw),
        ChunkIndex::BTreeV1(Some(addr)) => collect_chunks(bytes, *addr, rank, osize)?,
        ChunkIndex::SingleChunk(Some(single)) => {
            let size = single
                .filtered_size
                .unwrap_or(chunk_bytes as u64)
                .try_into()
                .map_err(|_| FieldglassError::Parse("single-chunk size exceeds u32".into()))?;
            vec![ChunkRecord {
                address: single.address,
                size,
                filter_mask: single.filter_mask,
                offset: vec![0u64; rank],
            }]
        }
        ChunkIndex::Implicit(Some(base)) => {
            // An implicit index is only ever written for unfiltered chunks; a
            // filter pipeline on such a dataset is malformed, and treating its
            // full-size chunks as filtered would mis-decode them.
            if !pipeline.filters.is_empty() {
                return Err(FieldglassError::Parse(
                    "HDF5 implicit chunk index cannot carry a filter pipeline".into(),
                ));
            }
            collect_implicit_chunks(*base, shape, &chunked.chunk_dims, chunk_bytes)?
        }
        ChunkIndex::FixedArray(Some(addr)) => collect_fixed_array_chunks(
            bytes,
            *addr,
            shape,
            &chunked.chunk_dims,
            chunk_bytes,
            probe.offset_size,
            probe.length_size,
        )?,
        ChunkIndex::ExtensibleArray(Some(addr)) => collect_extensible_array_chunks(
            bytes,
            *addr,
            shape,
            &chunked.chunk_dims,
            chunk_bytes,
            probe.offset_size,
            probe.length_size,
        )?,
        ChunkIndex::V2Btree(Some(addr)) => collect_v2_btree_chunks(
            bytes,
            *addr,
            &chunked.chunk_dims,
            chunk_bytes,
            probe.offset_size,
            probe.length_size,
        )?,
    };
    for chunk in chunks {
        let expanded = if pipeline.filters.is_empty() {
            read_at(bytes, chunk.address, chunk.size as usize)?.to_vec()
        } else {
            let raw_chunk = read_at(bytes, chunk.address, chunk.size as usize)?.to_vec();
            pipeline.reverse(raw_chunk, chunk.filter_mask, elem)?
        };
        if expanded.len() < chunk_bytes {
            return Err(FieldglassError::Parse(format!(
                "chunk decoded to {} bytes, expected {chunk_bytes}",
                expanded.len()
            )));
        }
        scatter_chunk(
            &mut raw,
            &expanded,
            shape,
            &chunked.chunk_dims,
            &chunk.offset,
            elem,
        );
    }
    Ok(raw)
}

/// One leaf entry of a version-1 chunk B-tree: where a chunk lives and the
/// element-space offset of its origin.
struct ChunkRecord {
    address: u64,
    size: u32,
    filter_mask: u32,
    /// Element-space origin per dataset dimension (length `rank`).
    offset: Vec<u64>,
}

/// Walk the version-1 B-tree at `addr` (node type 1) and collect every leaf
/// chunk record. Iterative with an explicit work-list and bounded by
/// [`MAX_BTREE_NODES`], so a malformed or cyclic tree errors out.
fn collect_chunks(
    bytes: &[u8],
    addr: u64,
    rank: usize,
    osize: u8,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    let o = osize as usize;
    // Key = chunk size (4) + filter mask (4) + (rank+1) 8-byte offsets.
    let key_offsets = rank + 1;
    let mut out = Vec::new();
    let mut pending = vec![addr];
    let mut visited = 0usize;
    while let Some(node_addr) = pending.pop() {
        visited += 1;
        if visited > MAX_BTREE_NODES {
            return Err(FieldglassError::Parse(
                "chunk B-tree too large or cyclic".into(),
            ));
        }
        let mut cur = super::heap::Cursor::at(bytes, node_addr)?;
        cur.tag(SIG_BTREE_V1)?;
        let node_type = cur.byte()?;
        if node_type != 1 {
            return Err(FieldglassError::Parse(format!(
                "expected B-tree v1 chunk node, got node type {node_type}"
            )));
        }
        let level = cur.byte()?;
        let entries = cur.u16()? as usize;
        cur.skip(2 * o)?; // left + right sibling addresses
        for _ in 0..entries {
            let size = cur.uint(4)? as u32;
            let filter_mask = cur.uint(4)? as u32;
            let mut offset = Vec::with_capacity(rank);
            for d in 0..key_offsets {
                let v = cur.uint(8)?;
                if d < rank {
                    offset.push(v);
                }
            }
            let child = cur.uint(o)?;
            if level == 0 {
                if out.len() >= MAX_BTREE_NODES {
                    return Err(FieldglassError::Parse(
                        "chunk B-tree has too many chunks".into(),
                    ));
                }
                out.push(ChunkRecord {
                    address: child,
                    size,
                    filter_mask,
                    offset,
                });
            } else {
                pending.push(child);
            }
        }
    }
    Ok(out)
}

/// Collect chunk records from a version-4 Implicit index (chunk index type 2).
/// This index has no on-disk structure at all: for a fixed-shape, early-
/// allocated, unfiltered dataset libhdf5 allocates every chunk of the chunk grid
/// contiguously from `base`, in row-major chunk order. Chunk `i` therefore lives
/// at `base + i * chunk_bytes`, is exactly `chunk_bytes` long, and is always
/// present (no undefined-address holes and no per-chunk filter mask).
fn collect_implicit_chunks(
    base: u64,
    shape: &[u64],
    chunk_dims: &[u32],
    chunk_bytes: usize,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    // Row-major chunk grid: ceil(shape / chunk) per dimension. Every cell is an
    // allocated chunk.
    let grid: Vec<u64> = shape
        .iter()
        .zip(chunk_dims)
        .map(|(&s, &c)| s.div_ceil(c as u64))
        .collect();
    let grid_count: u64 = grid.iter().product();
    // Bound the chunk count like the B-tree walk so a malformed shape can't
    // drive an unbounded allocation.
    if grid_count > MAX_BTREE_NODES as u64 {
        return Err(FieldglassError::Parse(
            "implicit chunk grid has too many chunks".into(),
        ));
    }

    let size = u32::try_from(chunk_bytes)
        .map_err(|_| FieldglassError::Parse("implicit chunk size exceeds u32".into()))?;
    let chunk_bytes = chunk_bytes as u64;

    let mut out = Vec::with_capacity(grid_count as usize);
    for i in 0..grid_count {
        let address = i
            .checked_mul(chunk_bytes)
            .and_then(|off| base.checked_add(off))
            .ok_or_else(|| FieldglassError::Parse("implicit chunk address overflows u64".into()))?;
        out.push(ChunkRecord {
            address,
            size,
            filter_mask: 0,
            offset: chunk_offset_from_linear(i, &grid, chunk_dims),
        });
    }
    Ok(out)
}

/// Fixed Array header / data-block signatures (v4 chunk index type 3).
const SIG_FIXED_ARRAY_HEADER: &[u8; 4] = b"FAHD";
const SIG_FIXED_ARRAY_DBLOCK: &[u8; 4] = b"FADB";

/// Collect chunk records from a version-4 Fixed Array index, used for
/// fixed-shape chunked datasets under the HDF5 "latest format". The array holds
/// one element per chunk in row-major chunk order; an element is a chunk address
/// (unfiltered) or address + on-disk size + filter mask (filtered). Each chunk's
/// element-space offset is computed from its linear position in the chunk grid.
fn collect_fixed_array_chunks(
    bytes: &[u8],
    header_addr: u64,
    shape: &[u64],
    chunk_dims: &[u32],
    chunk_bytes: usize,
    osize: u8,
    lsize: u8,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    let o = osize as usize;
    let l = lsize as usize;

    // Fixed Array Header: signature, version, client id, entry size, page bits,
    // max num entries (length_size), data block address (offset_size), checksum.
    let mut h = super::heap::Cursor::at(bytes, header_addr)?;
    h.tag(SIG_FIXED_ARRAY_HEADER)?;
    let version = h.byte()?;
    if version != 0 {
        return Err(FieldglassError::Parse(format!(
            "unsupported Fixed Array header version {version}"
        )));
    }
    let client_id = h.byte()?;
    if client_id > 1 {
        // Only 0 (unfiltered chunks) and 1 (filtered chunks) are defined for a
        // dataset-chunk Fixed Array; anything else is malformed or a client type
        // this reader doesn't handle.
        return Err(FieldglassError::Parse(format!(
            "unsupported Fixed Array client id {client_id} (expected 0 or 1)"
        )));
    }
    let entry_size = h.byte()? as usize;
    let page_bits = h.byte()?;
    let num_entries = h.uint(l)? as usize; // max num entries == chunk count
    let dblock_addr = h.uint(o)?;

    // Row-major chunk grid: ceil(shape / chunk) per dimension. Its cell count
    // must match the array's entry count.
    let grid: Vec<u64> = shape
        .iter()
        .zip(chunk_dims)
        .map(|(&s, &c)| s.div_ceil(c as u64))
        .collect();
    let grid_count: u64 = grid.iter().product();
    if grid_count != num_entries as u64 {
        return Err(FieldglassError::Parse(format!(
            "Fixed Array holds {num_entries} entries but the chunk grid has {grid_count}"
        )));
    }

    // The data block is paged when the entry count exceeds one page; that layout
    // (a page bitmap plus per-page checksums) is a follow-up.
    let per_page = 1u64.checked_shl(page_bits as u32).unwrap_or(u64::MAX);
    if num_entries as u64 > per_page {
        return Err(FieldglassError::UnsupportedSection(
            "HDF5 Fixed Array data block is paged, which is not decoded yet".into(),
        ));
    }

    // Data Block: signature, version, client id, header back-pointer, then the
    // elements (non-paged), then a checksum.
    let mut d = super::heap::Cursor::at(bytes, dblock_addr)?;
    d.tag(SIG_FIXED_ARRAY_DBLOCK)?;
    let dversion = d.byte()?;
    if dversion != 0 {
        return Err(FieldglassError::Parse(format!(
            "unsupported Fixed Array data block version {dversion}"
        )));
    }
    let dclient = d.byte()?;
    if dclient != client_id {
        return Err(FieldglassError::Parse(
            "Fixed Array data block client id disagrees with its header".into(),
        ));
    }
    d.skip(o)?; // header back-pointer address

    // Element layout (shared with the Extensible Array): unfiltered (client 0) =
    // address only; filtered (client 1) = address + on-disk chunk size + 4-byte
    // filter mask.
    let filtered = client_id == 1;
    let size_width = filtered_element_width(entry_size, o, filtered, "Fixed Array")?;

    let mut out = Vec::with_capacity(num_entries);
    for i in 0..num_entries {
        let elem = read_chunk_element(&mut d, o, filtered, size_width, chunk_bytes)?;
        push_chunk_record(&mut out, &elem, i, &grid, chunk_dims, osize)?;
    }
    Ok(out)
}

/// Extensible Array header / index / data / secondary-block signatures (v4
/// chunk index type 4).
const SIG_EXT_ARRAY_HEADER: &[u8; 4] = b"EAHD";
const SIG_EXT_ARRAY_INDEX: &[u8; 4] = b"EAIB";
const SIG_EXT_ARRAY_DATA: &[u8; 4] = b"EADB";
const SIG_EXT_ARRAY_SECONDARY: &[u8; 4] = b"EASB";

/// Collect chunk records from a version-4 Extensible Array index, used for a
/// chunked dataset with one unlimited dimension. The array stores one chunk
/// address per chunk in chunk order, spread across the index block (the first
/// `idx_blk_elmts`), then a doubling hierarchy of data blocks that are located
/// either directly from the index block (the first `nsblks_direct` super blocks)
/// or through a secondary block. Data blocks are read in order and their
/// elements assigned to consecutive chunks.
///
/// Both unfiltered (client id 0, address-only elements) and filtered (client id
/// 1, address + on-disk size + filter mask elements) arrays decode. Paged data
/// blocks (only reached by very large datasets) return a clear error.
fn collect_extensible_array_chunks(
    bytes: &[u8],
    header_addr: u64,
    shape: &[u64],
    chunk_dims: &[u32],
    chunk_bytes: usize,
    osize: u8,
    lsize: u8,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    let o = osize as usize;
    let l = lsize as usize;

    // Extensible Array Header: a 6-byte fixed run of parameters, then six
    // length_size statistics, then the index block address, then a checksum.
    let mut h = super::heap::Cursor::at(bytes, header_addr)?;
    h.tag(SIG_EXT_ARRAY_HEADER)?;
    let version = h.byte()?;
    if version != 0 {
        return Err(FieldglassError::Parse(format!(
            "unsupported Extensible Array header version {version}"
        )));
    }
    let client_id = h.byte()?;
    if client_id > 1 {
        // Only 0 (unfiltered chunks) and 1 (filtered chunks) are defined for a
        // dataset-chunk Extensible Array; anything else is malformed or a client
        // type this reader doesn't handle.
        return Err(FieldglassError::Parse(format!(
            "unsupported Extensible Array client id {client_id} (expected 0 or 1)"
        )));
    }
    let filtered = client_id == 1;
    let element_size = h.byte()? as usize;
    let max_nelmts_bits = h.byte()? as usize;
    let idx_blk_elmts = h.byte()? as usize;
    let data_blk_min_elmts = h.byte()? as usize;
    let sup_blk_min_data_ptrs = h.byte()? as usize;
    let max_dblk_page_nelmts_bits = h.byte()? as u32;
    h.skip(6 * l)?; // six length_size statistics (block/element counts and sizes)
    let index_block_addr = h.uint(o)?;

    // Element layout mirrors the Fixed Array: unfiltered (client 0) is an address
    // only (element_size == offset_size); filtered (client 1) is address + on-disk
    // chunk size + 4-byte filter mask.
    let size_width = filtered_element_width(element_size, o, filtered, "Extensible Array")?;
    if !data_blk_min_elmts.is_power_of_two() || !sup_blk_min_data_ptrs.is_power_of_two() {
        return Err(FieldglassError::Parse(
            "extensible array block parameters must be powers of two".into(),
        ));
    }
    if !(1..=64).contains(&(max_nelmts_bits)) {
        return Err(FieldglassError::Parse(format!(
            "extensible array max-nelmts bits {max_nelmts_bits} out of range"
        )));
    }
    // The block offset field width and the per-super-block counts, per the
    // libhdf5 layout (H5EA__hdr_init / H5EA__iblock_alloc).
    let arr_off_size = max_nelmts_bits.div_ceil(8);
    let dblk_page_nelmts = 1usize
        .checked_shl(max_dblk_page_nelmts_bits)
        .unwrap_or(usize::MAX);
    let nsblks_direct = 2 * sup_blk_min_data_ptrs.trailing_zeros() as usize;
    let ndblk_addrs = 2 * (sup_blk_min_data_ptrs - 1);
    let hdr_nsblks = 1 + max_nelmts_bits
        .checked_sub(data_blk_min_elmts.trailing_zeros() as usize)
        .ok_or_else(|| FieldglassError::Parse("extensible array header is inconsistent".into()))?;
    let nsblk_addrs = hdr_nsblks.saturating_sub(nsblks_direct);

    // The chunk grid, as for the fixed array; the unlimited dimension is already
    // resolved to its current extent in `shape`.
    let grid: Vec<u64> = shape
        .iter()
        .zip(chunk_dims)
        .map(|(&s, &c)| s.div_ceil(c as u64))
        .collect();
    let grid_count = usize::try_from(grid.iter().product::<u64>())
        .map_err(|_| FieldglassError::Parse("chunk grid exceeds usize".into()))?;

    // Index Block: prefix, the first `idx_blk_elmts` elements, then the direct
    // data-block addresses, then the secondary-block addresses.
    let mut ib = super::heap::Cursor::at(bytes, index_block_addr)?;
    ib.tag(SIG_EXT_ARRAY_INDEX)?;
    ib.skip(2)?; // version + client id
    ib.skip(o)?; // header back-pointer
    let mut direct_elems = Vec::with_capacity(idx_blk_elmts);
    for _ in 0..idx_blk_elmts {
        direct_elems.push(read_chunk_element(
            &mut ib,
            o,
            filtered,
            size_width,
            chunk_bytes,
        )?);
    }
    let mut direct_dblk_addrs = Vec::with_capacity(ndblk_addrs);
    for _ in 0..ndblk_addrs {
        direct_dblk_addrs.push(ib.uint(o)?);
    }
    let mut sblk_addrs = Vec::with_capacity(nsblk_addrs);
    for _ in 0..nsblk_addrs {
        sblk_addrs.push(ib.uint(o)?);
    }

    let mut out = Vec::new();
    let mut chunk = 0usize;

    // The first `idx_blk_elmts` chunks are addressed directly in the index block.
    for elem in direct_elems.iter().take(grid_count) {
        push_chunk_record(&mut out, elem, chunk, &grid, chunk_dims, osize)?;
        chunk += 1;
    }

    // Then the doubling super-block hierarchy. Super block `s` holds
    // `2^(s/2)` data blocks of `data_blk_min_elmts * 2^((s+1)/2)` elements each.
    let mut s = 0usize;
    let mut direct_ord = 0usize; // running index into `direct_dblk_addrs`
    while chunk < grid_count {
        let ndblks_s = 1usize
            .checked_shl((s / 2) as u32)
            .ok_or_else(|| FieldglassError::Parse("extensible array is too large".into()))?;
        // libhdf5's H5EA_SBLK_DBLK_NELMTS: data_blk_min_elmts * 2^((s+1)/2).
        // `(s + 1) / 2` is exactly `s.div_ceil(2)` for a non-negative `s`.
        let dblk_nelmts_s = data_blk_min_elmts
            .checked_shl(s.div_ceil(2) as u32)
            .ok_or_else(|| FieldglassError::Parse("extensible array is too large".into()))?;
        // `checked_shl` only guards the shift width, not value overflow; a
        // zero result would stall the walk, so reject it explicitly. (Bounded
        // `grid_count` keeps this unreachable in practice, but the guard makes
        // termination hold by construction rather than incidentally.)
        if dblk_nelmts_s == 0 {
            return Err(FieldglassError::Parse(
                "extensible array data-block element count overflowed".into(),
            ));
        }
        if dblk_nelmts_s > dblk_page_nelmts {
            return Err(FieldglassError::UnsupportedSection(
                "HDF5 extensible array uses paged data blocks, which are not decoded yet".into(),
            ));
        }

        // This super block's data-block addresses: directly from the index block
        // for the first `nsblks_direct` super blocks, else via a secondary block.
        let dblk_addrs: Vec<u64> = if s < nsblks_direct {
            let end = direct_ord + ndblks_s;
            let slice = direct_dblk_addrs
                .get(direct_ord..end)
                .ok_or_else(|| {
                    FieldglassError::Parse(
                        "extensible array direct data-block slot out of range".into(),
                    )
                })?
                .to_vec();
            direct_ord = end;
            slice
        } else {
            let slot = s - nsblks_direct;
            let sblk_addr = *sblk_addrs.get(slot).ok_or_else(|| {
                FieldglassError::Parse("extensible array secondary-block slot out of range".into())
            })?;
            if object_header::is_undefined_address(sblk_addr, osize) {
                // Whole super block unallocated: skip its chunks (they stay
                // fill) without fabricating sentinel addresses, whose width
                // would otherwise have to match the file's offset size.
                chunk = chunk.saturating_add(ndblks_s.saturating_mul(dblk_nelmts_s));
                s += 1;
                continue;
            }
            read_ea_secondary_dblk_addrs(bytes, sblk_addr, ndblks_s, o, arr_off_size)?
        };

        for &dblk_addr in &dblk_addrs {
            if chunk >= grid_count {
                break;
            }
            if object_header::is_undefined_address(dblk_addr, osize) {
                chunk += dblk_nelmts_s; // unwritten data block: its chunks stay fill
                continue;
            }
            // Data Block: prefix, header back-pointer, block offset, then the
            // elements (chunk addresses).
            let mut db = super::heap::Cursor::at(bytes, dblk_addr)?;
            db.tag(SIG_EXT_ARRAY_DATA)?;
            db.skip(2 + o + arr_off_size)?; // version + client + header addr + block offset
            for _ in 0..dblk_nelmts_s {
                if chunk >= grid_count {
                    break;
                }
                let elem = read_chunk_element(&mut db, o, filtered, size_width, chunk_bytes)?;
                push_chunk_record(&mut out, &elem, chunk, &grid, chunk_dims, osize)?;
                chunk += 1;
            }
        }
        s += 1;
    }
    Ok(out)
}

/// v2 B-tree chunk-index B-tree type IDs: type 10 indexes non-filtered dataset
/// chunks and type 11 filtered ones (libhdf5 `H5B2_CDSET_ID` /
/// `H5B2_CDSET_FILT_ID`).
const BTREE_V2_TYPE_CHUNK_UNFILTERED: u8 = 10;
const BTREE_V2_TYPE_CHUNK_FILTERED: u8 = 11;

/// Collect chunk records from a version-4 v2 B-tree index (chunk index type 5),
/// which libhdf5 selects for a chunked dataset with more than one unlimited
/// dimension. Unlike the Fixed and Extensible Arrays — where a chunk's grid
/// position is implied by its element's ordinal — each v2 B-tree record carries
/// the chunk's *scaled* (chunk-grid) coordinate explicitly, so the chunk's
/// element-space origin is `scaled[d] * chunk_dims[d]`. The records reuse the
/// shared v2 B-tree reader ([`super::heap::btree_v2_records`]); their two shapes
/// match the Fixed / Extensible Array element prefix — type 10 = address only,
/// type 11 = address + on-disk size + filter mask — followed by one 8-byte scaled
/// offset per dataset dimension.
fn collect_v2_btree_chunks(
    bytes: &[u8],
    header_addr: u64,
    chunk_dims: &[u32],
    chunk_bytes: usize,
    osize: u8,
    lsize: u8,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    let o = osize as usize;
    let rank = chunk_dims.len();
    let (btree_type, records) = super::heap::btree_v2_records(bytes, header_addr, osize, lsize)?;
    let filtered = match btree_type {
        BTREE_V2_TYPE_CHUNK_UNFILTERED => false,
        BTREE_V2_TYPE_CHUNK_FILTERED => true,
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported B-tree v2 type {other} for a chunk index (expected 10 or 11)"
            )));
        }
    };
    // One 8-byte scaled offset per dataset dimension trails every record.
    let scaled_bytes = rank
        .checked_mul(8)
        .ok_or_else(|| FieldglassError::Parse("chunk rank overflows a record size".into()))?;

    let mut out = Vec::with_capacity(records.len());
    for record in &records {
        // The on-disk chunk-size field of a *filtered* record takes up whatever
        // the record has left after the address, 4-byte filter mask, and scaled
        // offsets. libhdf5 sizes that field from the chunk byte size, and its
        // width has varied across versions, so derive it from the record width
        // the B-tree header advertised rather than recomputing the formula. An
        // unfiltered record is an address followed straight by the scaled offsets.
        let size_width = if filtered {
            record
                .len()
                .checked_sub(o + 4 + scaled_bytes)
                .filter(|&w| (1..=8).contains(&w))
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "filtered v2 B-tree chunk record is {} bytes, too small for rank {rank}",
                        record.len()
                    ))
                })?
        } else {
            if record.len() != o + scaled_bytes {
                return Err(FieldglassError::Parse(format!(
                    "unfiltered v2 B-tree chunk record is {} bytes, expected {}",
                    record.len(),
                    o + scaled_bytes
                )));
            }
            0
        };

        let mut cur = super::heap::Cursor::over(record);
        let elem = read_chunk_element(&mut cur, o, filtered, size_width, chunk_bytes)?;
        // A v2 B-tree only ever records written chunks, but skip a stray
        // undefined address rather than fabricate a chunk at the sentinel.
        if object_header::is_undefined_address(elem.addr, osize) {
            continue;
        }
        let mut offset = Vec::with_capacity(rank);
        for &cd in chunk_dims {
            let scaled = cur.uint(8)?;
            offset.push(scaled.checked_mul(cd as u64).ok_or_else(|| {
                FieldglassError::Parse("v2 B-tree chunk offset overflows".into())
            })?);
        }
        let size = u32::try_from(elem.size)
            .map_err(|_| FieldglassError::Parse("chunk size exceeds u32".into()))?;
        out.push(ChunkRecord {
            address: elem.addr,
            size,
            filter_mask: elem.filter_mask,
            offset,
        });
    }
    Ok(out)
}

/// The on-disk width of the chunk-size field inside a *filtered* chunk-index
/// element — the same layout for a Fixed Array entry and an Extensible Array
/// element: the element/entry byte size (`field_size`) minus the chunk address
/// (`o`) and the 4-byte filter mask. Returns 0 for an unfiltered index, whose
/// element is an address only and must therefore be exactly `o` bytes wide.
/// `label` names the index in the error message.
fn filtered_element_width(
    field_size: usize,
    o: usize,
    filtered: bool,
    label: &str,
) -> Result<usize, FieldglassError> {
    if filtered {
        field_size
            .checked_sub(o + 4)
            .filter(|&w| (1..=8).contains(&w))
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "{label} filtered element size {field_size} too small for a chunk element"
                ))
            })
    } else if field_size == o {
        Ok(0)
    } else {
        Err(FieldglassError::Parse(format!(
            "{label} unfiltered element size {field_size} != offset size {o}"
        )))
    }
}

/// One chunk-index element: the chunk's file address plus, for a filtered index,
/// its on-disk byte size and filter mask. An unfiltered element is address-only
/// and carries the full uncompressed chunk byte size with a zero mask.
struct ChunkElement {
    addr: u64,
    size: u64,
    filter_mask: u32,
}

/// Read one chunk-index element from `cur` (a Fixed Array entry or an Extensible
/// Array element, whose element layout is identical). An unfiltered element is
/// just a chunk address (its size is the full chunk byte size); a filtered
/// element is address + on-disk size (`size_width` bytes) + 4-byte filter mask.
fn read_chunk_element(
    cur: &mut super::heap::Cursor,
    o: usize,
    filtered: bool,
    size_width: usize,
    chunk_bytes: usize,
) -> Result<ChunkElement, FieldglassError> {
    let addr = cur.uint(o)?;
    let (size, filter_mask) = if filtered {
        (cur.uint(size_width)?, cur.uint(4)? as u32)
    } else {
        (chunk_bytes as u64, 0)
    };
    Ok(ChunkElement {
        addr,
        size,
        filter_mask,
    })
}

/// Push one chunk record at linear chunk index `chunk`, skipping an unwritten
/// (undefined) address so the chunk stays fill.
fn push_chunk_record(
    out: &mut Vec<ChunkRecord>,
    elem: &ChunkElement,
    chunk: usize,
    grid: &[u64],
    chunk_dims: &[u32],
    osize: u8,
) -> Result<(), FieldglassError> {
    if object_header::is_undefined_address(elem.addr, osize) {
        return Ok(());
    }
    let size = u32::try_from(elem.size)
        .map_err(|_| FieldglassError::Parse("chunk size exceeds u32".into()))?;
    out.push(ChunkRecord {
        address: elem.addr,
        size,
        filter_mask: elem.filter_mask,
        offset: chunk_offset_from_linear(chunk as u64, grid, chunk_dims),
    });
    Ok(())
}

/// Read the `ndblks` data-block addresses from an Extensible Array secondary
/// block (unpaged: no per-data-block page bitmap precedes the addresses).
fn read_ea_secondary_dblk_addrs(
    bytes: &[u8],
    addr: u64,
    ndblks: usize,
    o: usize,
    arr_off_size: usize,
) -> Result<Vec<u64>, FieldglassError> {
    let mut c = super::heap::Cursor::at(bytes, addr)?;
    c.tag(SIG_EXT_ARRAY_SECONDARY)?;
    c.skip(2 + o + arr_off_size)?; // version + client + header addr + block offset
    let mut out = Vec::with_capacity(ndblks);
    for _ in 0..ndblks {
        out.push(c.uint(o)?);
    }
    Ok(out)
}

/// Element-space origin of the chunk at row-major linear index `i` within a
/// chunk grid of dimensions `grid`, given the chunk edge lengths `chunk_dims`.
fn chunk_offset_from_linear(mut i: u64, grid: &[u64], chunk_dims: &[u32]) -> Vec<u64> {
    let mut coord = vec![0u64; grid.len()];
    for d in (0..grid.len()).rev() {
        coord[d] = i % grid[d];
        i /= grid[d];
    }
    coord
        .iter()
        .zip(chunk_dims)
        .map(|(&c, &cd)| c * cd as u64)
        .collect()
}

/// Copy a chunk's decoded elements into the dataset's row-major buffer,
/// clipping any portion of an edge chunk that hangs past the dataset bounds.
fn scatter_chunk(
    raw: &mut [u8],
    chunk: &[u8],
    shape: &[u64],
    chunk_dims: &[u32],
    origin: &[u64],
    elem: usize,
) {
    let rank = shape.len();
    let chunk_elems: usize = chunk_dims.iter().map(|&d| d as usize).product();
    let mut coord = vec![0usize; rank];
    for c in 0..chunk_elems {
        // Decompose `c` into per-dim chunk coordinates (row-major).
        let mut rem = c;
        for d in (0..rank).rev() {
            let cd = chunk_dims[d] as usize;
            coord[d] = rem % cd;
            rem /= cd;
        }
        // Map to a dataset coordinate; skip elements past the dataset edge.
        let mut ds_index = 0usize;
        let mut in_bounds = true;
        for d in 0..rank {
            let ds_coord = origin[d] as usize + coord[d];
            if ds_coord >= shape[d] as usize {
                in_bounds = false;
                break;
            }
            ds_index = ds_index * shape[d] as usize + ds_coord;
        }
        if !in_bounds {
            continue;
        }
        let src = c * elem;
        let dst = ds_index * elem;
        if src + elem <= chunk.len() && dst + elem <= raw.len() {
            raw[dst..dst + elem].copy_from_slice(&chunk[src..src + elem]);
        }
    }
}

/// Read exactly `len` bytes at file offset `addr`, bounds-checked.
fn read_at(bytes: &[u8], addr: u64, len: usize) -> Result<&[u8], FieldglassError> {
    let start = usize::try_from(addr)
        .map_err(|_| FieldglassError::Parse("chunk address exceeds usize".into()))?;
    let end = start
        .checked_add(len)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "chunk data [{start}, +{len}) exceeds file size {}",
                bytes.len()
            ))
        })?;
    Ok(&bytes[start..end])
}

/// Decode the Fill Value message (`0x0005`) into the raw fill-element bytes, if
/// the message both defines and stores a fill value. Versions 1–3 are handled.
fn fill_value_default(body: &[u8]) -> Result<Option<Vec<u8>>, FieldglassError> {
    let version = *body
        .first()
        .ok_or_else(|| FieldglassError::Parse("empty fill value message".into()))?;
    let (defined, size_pos) = match version {
        1 | 2 => {
            // version, space-allocation time, fill-write time, fill-defined flag,
            // then (if defined) size + value.
            let defined = *body.get(3).unwrap_or(&0);
            if version == 2 && defined == 0 {
                return Ok(None);
            }
            (true, 4)
        }
        3 => {
            // version, flags. Bit 5 (0x20) set ⇒ a fill value is defined.
            let flags = *body.get(1).unwrap_or(&0);
            if flags & 0x20 == 0 {
                return Ok(None);
            }
            (true, 2)
        }
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported fill value message version {other}"
            )));
        }
    };
    if !defined {
        return Ok(None);
    }
    let size = read_uint_le(body, size_pos, 4)? as usize;
    if size == 0 {
        return Ok(None);
    }
    let start = size_pos + 4;
    let end = start
        .checked_add(size)
        .filter(|&e| e <= body.len())
        .ok_or_else(|| FieldglassError::Parse("fill value runs past the message".into()))?;
    Ok(Some(body[start..end].to_vec()))
}

/// The numeric sentinel values that mask a point: the `_FillValue` and the CF
/// `missing_value` *attributes*, widened to `f64`. Mirrors the classic path
/// ([`crate::classic::Variable::missing_sentinels`]): only explicit attributes
/// mask (the HDF5 storage fill default does not), `libnetcdf` masks a point
/// equal to either, and a multi-valued `missing_value` contributes only its
/// first element.
fn missing_sentinels(
    bytes: &[u8],
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<f64>, FieldglassError> {
    let attrs = attribute::list_attributes(bytes, object_header_address, probe)?;
    // Use the typed first element, not the rendered display string: the display
    // text is rounded to a few decimals, so reparsing it would not bit-match the
    // decoded value and float sentinels would silently fail to mask.
    Ok(["_FillValue", "missing_value"]
        .into_iter()
        .filter_map(|name| attrs.iter().find(|a| a.name == name))
        .filter_map(|a| a.first_value)
        .collect())
}
