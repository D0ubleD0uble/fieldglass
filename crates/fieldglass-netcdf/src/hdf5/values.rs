//! HDF5 dataset value decode (issue #121, under #33). Reads a dataset's numeric
//! elements into the same `Vec<Option<f64>>` surface the classic NetCDF path
//! produces: `Some(v)` for a present point, `None` where the element equals the
//! variable's `_FillValue` *attribute* (mirroring how `libnetcdf` masks). The
//! decode is decoupled from rendering — it yields the whole variable in
//! row-major (C) order; slice selection happens downstream.
//!
//! Storage is read for the three Data Layout classes a NetCDF-4 file uses:
//! compact, contiguous, and chunked (version-1 B-tree index, the legacy
//! `libver=earliest` form). Chunks pass back through the [`filter`] pipeline
//! (deflate / shuffle) before being scattered into place; any region with no
//! stored chunk reads as the dataset's Fill Value (message `0x0005`) default.
//!
//! Element bytes honour the datatype's byte order — unlike classic NetCDF
//! (always big-endian), HDF5 records it per type and NetCDF-4 writers normally
//! pick the host's little-endian order.

use super::datatype::DatatypeClass;
use super::layout::{ChunkedLayout, DataLayout};
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

    // `_FillValue` *attribute* drives masking, matching classic / libnetcdf.
    let fill_mask = fill_value_attribute(bytes, object_header_address, probe)?;

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
        probe.offset_size,
    )?;

    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let off = i * elem;
        let v = datatype
            .read_element_f64(&raw[off..off + elem])
            .ok_or_else(|| FieldglassError::Parse("dataset element decode failed".into()))?;
        out.push(match fill_mask {
            Some(f) if v == f => None,
            _ => Some(v),
        });
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
    osize: u8,
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
            assemble_chunked(bytes, chunked, shape, elem, pipeline, fill_default, osize)
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

/// Assemble a chunked dataset: walk the version-1 chunk B-tree, reverse each
/// chunk's filters, and scatter it into the row-major output. Unstored regions
/// keep the fill default.
fn assemble_chunked(
    bytes: &[u8],
    chunked: &ChunkedLayout,
    shape: &[u64],
    elem: usize,
    pipeline: &FilterPipeline,
    fill_default: Option<&[u8]>,
    osize: u8,
) -> Result<Vec<u8>, FieldglassError> {
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

    let Some(btree_addr) = chunked.btree_address else {
        return Ok(raw); // no chunk written: all fill
    };

    let chunk_elems: usize = chunked
        .chunk_dims
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d as usize))
        .ok_or_else(|| FieldglassError::Parse("chunk element count overflows usize".into()))?;
    let chunk_bytes = chunk_elems
        .checked_mul(elem)
        .ok_or_else(|| FieldglassError::Parse("chunk byte size overflows usize".into()))?;

    let chunks = collect_chunks(bytes, btree_addr, rank, osize)?;
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

/// The numeric `_FillValue` *attribute*, widened to `f64`, used to mask points.
/// Mirrors the classic path: only an explicit `_FillValue` attribute masks; the
/// HDF5 storage fill default does not.
fn fill_value_attribute(
    bytes: &[u8],
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Option<f64>, FieldglassError> {
    let attrs = attribute::list_attributes(bytes, object_header_address, probe)?;
    // Use the typed first element, not the rendered display string: the display
    // text is rounded to a few decimals, so reparsing it would not bit-match the
    // decoded value and float fills would silently fail to mask.
    Ok(attrs
        .iter()
        .find(|a| a.name == "_FillValue")
        .and_then(|a| a.first_value))
}
