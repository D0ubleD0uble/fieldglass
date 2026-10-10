//! HDF5 dataset value decode (issue #121, under #33). Reads a dataset's numeric
//! elements into the same `Vec<Option<f64>>` surface the classic NetCDF path
//! produces: `Some(v)` for a present point, `None` where the element equals the
//! variable's `_FillValue` *attribute* (mirroring how `libnetcdf` masks). The
//! decode is decoupled from rendering — it yields the whole variable, or a
//! region of it (#939), in row-major (C) order, and reads only what that
//! region needs.
//!
//! Storage is read for the three Data Layout classes a NetCDF-4 file uses:
//! compact, contiguous, and chunked. Chunked datasets are located through their
//! chunk index — the version-1 B-tree of the legacy `libver=earliest` form, or
//! the version-4/5 single-chunk, fixed-array, extensible-array, and implicit
//! indexes of the "latest format".
//! Chunks pass back through the [`filter`](super::filter) pipeline
//! (deflate, shuffle, fletcher32, zstd, szip), must come back exactly one
//! chunk long, and are then scattered into place; any region with no
//! stored chunk reads as the dataset's Fill Value (message `0x0005`) default.
//!
//! Element bytes honour the datatype's byte order — unlike classic NetCDF
//! (always big-endian), HDF5 records it per type and NetCDF-4 writers normally
//! pick the host's little-endian order — and a fixed-point type's bit offset
//! and precision, through [`Datatype::element_bits`](super::datatype::Datatype::element_bits)
//! (#795).

use super::cache::ExpandedKey;
use super::datatype::DatatypeClass;
use super::layout::{ChunkIndex, ChunkedLayout, DataLayout};
use super::object_header::{self, read_usize_le};
use super::source::{Cursor, Fields, FileCursor, read_at};
use super::{Hdf5Probe, attribute, dataspace, filter::FilterPipeline, layout};
use crate::classic::NcType;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::copy_block_elements;
use fieldglass_core::bytes::{ByteRange, ByteSource};

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
///
/// The whole dataset, held to the whole-variable budget. A caller that wants
/// part of it wants [`read_dataset_region`], which reads only that part.
pub fn read_dataset_values<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    read_dataset(source, object_header_address, probe, None)
}

/// Decode `region` of the dataset at `object_header_address`: one half-open
/// element range per dimension, the values in the region's C order (#939).
///
/// Reads only what the region needs. A chunked dataset reads and reverses the
/// filters of the chunks the region overlaps and no others; one the index does
/// not store reads as the fill value, as it does whole. A contiguous dataset
/// reads the runs of the region; a compact one is in its header already. So a
/// region costs the region plus the chunks it covers, whatever the dataset's
/// size, and is refused past
/// [`MAX_FIELD_POINTS`](fieldglass_core::MAX_FIELD_POINTS) elements, as one
/// field is.
///
/// Every value equals the one [`read_dataset_values`] returns at the same
/// position. The chunk index is checked over the records the region reads: a
/// record conflict elsewhere in the index fails a whole read and not this one,
/// the way a record outside the shape fails neither.
pub fn read_dataset_region<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
    region: &[Range<u64>],
) -> Result<Vec<Option<f64>>, FieldglassError> {
    read_dataset(source, object_header_address, probe, Some(region))
}

/// [`read_dataset_values`] (`region` is `None`) or [`read_dataset_region`].
fn read_dataset<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
    region: Option<&[Range<u64>]>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let header = probe.header(source, object_header_address)?;
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
    let fills = missing_sentinels(source, object_header_address, probe)?;

    let shape: Vec<u64> = dataspace.dims.clone();
    let elem = datatype.size as usize;
    if elem == 0 {
        return Err(FieldglassError::Parse(
            "dataset element size is zero".into(),
        ));
    }
    // A whole read is held to the whole-variable budget and is the region that
    // covers every axis; a region read is held to the one-field bound, and the
    // dataset it is cut from need only have an element count, so that every
    // offset in it is a number.
    let whole: Vec<Range<u64>>;
    let (region, total) = match region {
        None => {
            let total = checked_total(&shape, elem)?;
            whole = shape.iter().map(|&n| 0..n).collect();
            (whole.as_slice(), total)
        }
        Some(region) => {
            let total = crate::region::element_count(&shape, region)?;
            if total > 0 {
                element_count_u64(&shape)?;
            }
            (region, total)
        }
    };
    if total == 0 {
        return Ok(Vec::new());
    }

    // Assemble the region's raw element bytes, then decode them uniformly.
    let raw = assemble_raw(
        source,
        object_header_address,
        &data_layout,
        &shape,
        region,
        elem,
        &pipeline,
        fill_default.as_deref(),
        probe,
    )?;

    // Walk `raw` by the element width rather than indexing `off..off + elem`:
    // the stride and the bound are then the same value, so an assembly that
    // returned fewer bytes than `total * elem` is caught by the length check
    // below instead of panicking on the slice.
    //
    // Whether the type is packed is decided once here, not per element: each
    // branch gets its own copy of the loop with its reader inlined (#795).
    if datatype.is_packed() {
        decode_elements(&raw, elem, total, &fills, |c| datatype.read_packed_f64(c))
    } else {
        decode_elements(&raw, elem, total, &fills, |c| {
            datatype.read_full_width_f64(c)
        })
    }
}

/// Decode `total` elements of `elem` bytes from `raw` with `read`, masking any
/// that equal one of `fills`. Generic over the reader so the per-element loop
/// is compiled once per reader, with no dispatch inside it.
fn decode_elements(
    raw: &[u8],
    elem: usize,
    total: usize,
    fills: &[f64],
    read: impl Fn(&[u8]) -> Option<f64>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let mut out = Vec::with_capacity(total);
    for chunk in raw.chunks_exact(elem).take(total) {
        let v = read(chunk)
            .ok_or_else(|| FieldglassError::Parse("dataset element decode failed".into()))?;
        out.push(if fills.contains(&v) { None } else { Some(v) });
    }
    if out.len() != total {
        return Err(FieldglassError::Parse(format!(
            "dataset assembled {} of {total} elements",
            out.len()
        )));
    }
    Ok(out)
}

/// Total element count for `shape` of `elem`-byte elements, with the same
/// overflow and budget guards the classic path applies. A rank-0 (scalar)
/// dataset has one element.
///
/// The budget counts the stored bytes as well as the output, because
/// [`assemble_raw`] holds all of them while the output is built. The file's
/// size bounds neither: a chunked dataset that stores no chunks reads whole as
/// its fill value (#847).
fn checked_total(shape: &[u64], elem: usize) -> Result<usize, FieldglassError> {
    let total_u64 = element_count_u64(shape)?;
    fieldglass_core::whole_variable_read_bytes(total_u64, elem as u64)?;
    usize::try_from(total_u64)
        .map_err(|_| FieldglassError::Parse("dataset element count exceeds usize".into()))
}

/// The element count of `shape`, refused when it does not fit a `u64`. A
/// rank-0 (scalar) dataset has one element.
fn element_count_u64(shape: &[u64]) -> Result<u64, FieldglassError> {
    shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| FieldglassError::Parse(format!("dataset shape {shape:?} overflows")))
}

/// Produce the raw element bytes of `region` of the dataset (`elem` bytes for
/// each of the region's elements, in its C order) for any layout class. Parts
/// with no stored data read as the fill default (or zero).
///
/// A whole read is the region covering every axis, and reads what it always
/// did: the compact bytes, the one contiguous run, every chunk.
#[allow(clippy::too_many_arguments)]
fn assemble_raw<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    data_layout: &DataLayout,
    shape: &[u64],
    region: &[Range<u64>],
    elem: usize,
    pipeline: &FilterPipeline,
    fill_default: Option<&[u8]>,
    probe: &Hdf5Probe,
) -> Result<Vec<u8>, FieldglassError> {
    let lens: Vec<u64> = region.iter().map(|r| r.end - r.start).collect();
    let span = byte_span(&lens, elem)?;

    match data_layout {
        DataLayout::Compact { data } => {
            // The dataset is in its object header, so it is held whole however
            // little of it is asked for, and must be all there.
            let whole = byte_span(shape, elem)?;
            if data.len() < whole {
                return Err(FieldglassError::Parse(format!(
                    "compact dataset holds {} bytes, needs {whole}",
                    data.len()
                )));
            }
            let mut raw = vec![0u8; span];
            copy_block_elements(
                &data[..whole],
                shape,
                &vec![0; shape.len()],
                region,
                &mut raw,
                elem,
            );
            Ok(raw)
        }
        DataLayout::Contiguous { address, .. } => {
            let mut raw = fill_buffer(span, elem, fill_default);
            if let Some(addr) = address {
                // The region's runs, each one range, resolved in one batch
                // before any is read as ADR-0005 asks. A whole read is one run.
                let overflow =
                    || FieldglassError::Parse("contiguous data offset overflows u64".into());
                let elem_u64 = elem as u64;
                let plan = crate::region::runs(shape, region)?
                    .into_iter()
                    .map(|(offset, len)| {
                        let start = offset
                            .checked_mul(elem_u64)
                            .and_then(|o| addr.checked_add(o))
                            .ok_or_else(overflow)?;
                        let len = len.checked_mul(elem_u64).ok_or_else(overflow)?;
                        Ok(ByteRange::new(start, len))
                    })
                    .collect::<Result<Vec<ByteRange>, FieldglassError>>()?;
                source.prefetch(&plan)?;
                let mut at = 0usize;
                for range in &plan {
                    // Each run is part of the region, whose bytes fit `raw`.
                    let len = range.len as usize;
                    let data = read_at(source, range.start, len).map_err(|_| {
                        FieldglassError::Parse(format!(
                            "contiguous data [{}, +{len}) exceeds file size {}",
                            range.start,
                            source.size()
                        ))
                    })?;
                    let Some(to) = raw.get_mut(at..at + len) else {
                        return Err(FieldglassError::Parse(
                            "contiguous runs overran the region".into(),
                        ));
                    };
                    to.copy_from_slice(&data);
                    at += len;
                }
            }
            Ok(raw)
        }
        DataLayout::Chunked(chunked) => assemble_chunked(
            source,
            object_header_address,
            chunked,
            shape,
            region,
            elem,
            pipeline,
            fill_default,
            probe,
        ),
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

/// Assemble `region` of a chunked dataset: gather its chunk records from
/// whichever chunk index the layout uses, reverse the filters of each chunk the
/// region overlaps, and copy its part of the region into the row-major output.
/// Unstored parts keep the fill default.
#[allow(clippy::too_many_arguments)]
fn assemble_chunked<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    chunked: &ChunkedLayout,
    shape: &[u64],
    region: &[Range<u64>],
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
    let lens: Vec<u64> = region.iter().map(|r| r.end - r.start).collect();
    let span = byte_span(&lens, elem)?;
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
    let chunk_shape: Vec<u64> = chunked.chunk_dims.iter().map(|&d| u64::from(d)).collect();

    // Every chunk index resolves to the same per-chunk record; only the way the
    // records are located differs. An unallocated index leaves the buffer as
    // all fill.
    // The four traversed indexes (B-tree v1/v2, fixed and extensible array) are
    // memoised on the probe by `(index address, rank)`: re-decoding a variable
    // must not re-walk its chunk index (#414). Single-chunk and implicit are
    // computed from the layout message itself, so there is nothing to remember.
    let chunks: std::sync::Arc<Vec<ChunkRecord>> = match &chunked.index {
        ChunkIndex::BTreeV1(None)
        | ChunkIndex::SingleChunk(None)
        | ChunkIndex::Implicit(None)
        | ChunkIndex::FixedArray(None)
        | ChunkIndex::ExtensibleArray(None)
        | ChunkIndex::V2Btree(None) => return Ok(raw),
        ChunkIndex::BTreeV1(Some(addr)) => {
            probe.cache().chunk_records(source, *addr, rank, || {
                let max_entries = super::btree_k(source, probe)?.chunk_node_max();
                collect_chunks(source, *addr, rank, osize, max_entries)
            })?
        }
        ChunkIndex::SingleChunk(Some(single)) => {
            let size = single
                .filtered_size
                .unwrap_or(chunk_bytes as u64)
                .try_into()
                .map_err(|_| FieldglassError::Parse("single-chunk size exceeds u32".into()))?;
            std::sync::Arc::new(vec![ChunkRecord {
                address: single.address,
                size,
                filter_mask: single.filter_mask,
                offset: vec![0u64; rank],
            }])
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
            std::sync::Arc::new(collect_implicit_chunks(
                *base,
                shape,
                &chunked.chunk_dims,
                chunk_bytes,
            )?)
        }
        ChunkIndex::FixedArray(Some(addr)) => {
            probe.cache().chunk_records(source, *addr, rank, || {
                collect_fixed_array_chunks(
                    source,
                    *addr,
                    shape,
                    &chunked.chunk_dims,
                    chunk_bytes,
                    probe.offset_size,
                    probe.length_size,
                )
            })?
        }
        ChunkIndex::ExtensibleArray(Some(addr)) => {
            probe.cache().chunk_records(source, *addr, rank, || {
                collect_extensible_array_chunks(
                    source,
                    *addr,
                    shape,
                    &chunked.chunk_dims,
                    chunk_bytes,
                    probe.offset_size,
                    probe.length_size,
                )
            })?
        }
        ChunkIndex::V2Btree(Some(addr)) => {
            probe.cache().chunk_records(source, *addr, rank, || {
                collect_v2_btree_chunks(
                    source,
                    *addr,
                    &chunked.chunk_dims,
                    chunk_bytes,
                    probe.offset_size,
                    probe.length_size,
                )
            })?
        }
    };
    // The one place in the HDF5 reader where ADR-0005's *strong* form holds:
    // once the chunk index has been walked, every chunk's address and size is
    // known, so the region is one batch resolve followed by reads. The
    // walk that produced the records could not be planned — that is the weak
    // form, and why the traversal above issues none.
    let pipeline_bits = match pipeline.filters.len() {
        n if n >= 32 => u32::MAX,
        n => (1u32 << n) - 1,
    };
    let mut chunks = chunks_in_region(&chunks, shape, region, &chunked.chunk_dims, pipeline_bits)?;
    // Records at different origins may name the same stored chunk: nothing in
    // the format forbids it, and libhdf5 reads such a file. Each stored chunk is
    // read and reversed once and placed at every origin that names it, so the
    // cost is the distinct stored chunks, not the records (#837).
    //
    // A stored chunk is its address (#888). The filter mask is compared only
    // over the filters this pipeline has: `reverse` reads no other bit, so
    // masks that differ above them name the same decode. Records at one address
    // that still disagree, on stored size or on a filter actually applied,
    // would each decode the same bytes differently, and reading each is the
    // records-times-chunk-size cost again; the index is refused (ADR-0013).
    // The sort is stable, so a group keeps its records in origin order.
    let storage = |c: &&ChunkRecord| (c.address, c.size, c.filter_mask & pipeline_bits);
    chunks.sort_by_key(storage);
    let groups: Vec<&[&ChunkRecord]> = chunks.chunk_by(|a, b| storage(a) == storage(b)).collect();
    if let Some(pair) = groups
        .windows(2)
        .find(|pair| pair[0][0].address == pair[1][0].address)
    {
        let (a, b) = (pair[0][0], pair[1][0]);
        return Err(FieldglassError::Parse(format!(
            "chunk index names the stored chunk at {} twice with different storage \
             (size {} and {}, filter mask {:#x} and {:#x})",
            a.address,
            a.size,
            b.size,
            a.filter_mask & pipeline_bits,
            b.filter_mask & pipeline_bits
        )));
    }
    // A filtered chunk the file's memo still holds decompressed is not read
    // again (#939): a time scrub over chunks that span several planes would
    // otherwise inflate each chunk once per frame. Those are left out of the
    // plan, so the batch names exactly the chunks that will be read.
    let filtered = !pipeline.filters.is_empty();
    let key = |c: &ChunkRecord| -> ExpandedKey {
        (
            object_header_address,
            c.address,
            c.size,
            c.filter_mask & pipeline_bits,
        )
    };
    let held: Vec<Option<std::sync::Arc<Vec<u8>>>> = groups
        .iter()
        .map(|g| {
            filtered
                .then(|| probe.cache().expanded_chunk(source, key(g[0])))
                .flatten()
        })
        .collect();
    let plan: Vec<ByteRange> = groups
        .iter()
        .zip(&held)
        .filter(|(_, hit)| hit.is_none())
        .map(|(g, _)| ByteRange::new(g[0].address, u64::from(g[0].size)))
        .collect();
    source.prefetch(&plan)?;

    for (group, hit) in groups.into_iter().zip(held) {
        let chunk = group[0];
        let expanded = match hit {
            Some(hit) => hit,
            None => {
                let stored = read_at(source, chunk.address, chunk.size as usize)?;
                let expanded = if filtered {
                    let mask = chunk.filter_mask & pipeline_bits;
                    pipeline.reverse(stored.into_owned(), mask, elem, chunk_bytes)?
                } else {
                    stored.into_owned()
                };
                // Exactly one chunk, not at least one: the copy reads only the
                // first `chunk_bytes`, so a longer result would be cut
                // silently, and a chunk that decodes to the wrong length is
                // corrupt whichever way it is wrong. This is also the check
                // that stands behind szip's size prefix when a
                // length-changing filter precedes it. A held chunk passed it
                // when it was kept.
                if expanded.len() != chunk_bytes {
                    return Err(FieldglassError::Parse(format!(
                        "chunk decoded to {} bytes, expected {chunk_bytes}",
                        expanded.len()
                    )));
                }
                let expanded = std::sync::Arc::new(expanded);
                if filtered {
                    probe
                        .cache()
                        .keep_expanded_chunk(source, key(chunk), &expanded);
                }
                expanded
            }
        };
        // Only the part of the chunk inside the region is walked, one
        // contiguous run along the last dimension at a time (#837): a chunk
        // may legally be far larger than the dataset, and the region lies
        // inside the shape, so the part past an edge is never copied.
        for record in group {
            copy_block_elements(
                &expanded,
                &chunk_shape,
                &record.offset,
                region,
                &mut raw,
                elem,
            );
        }
    }
    Ok(raw)
}

/// One leaf entry of a version-1 chunk B-tree: where a chunk lives and the
/// element-space offset of its origin.
#[derive(Debug)]
pub(crate) struct ChunkRecord {
    address: u64,
    size: u32,
    filter_mask: u32,
    /// Element-space origin per dataset dimension (length `rank`).
    offset: Vec<u64>,
}

/// Walk the version-1 B-tree at `addr` (node type 1) and collect every leaf
/// chunk record. Iterative with an explicit work-list and bounded by
/// [`MAX_BTREE_NODES`], so a malformed or cyclic tree errors out. A node
/// holding more than `max_entries` (2K, Indexed Storage Internal Node K) is
/// refused (#920).
fn collect_chunks<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    rank: usize,
    osize: u8,
    max_entries: usize,
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
        let mut cur = FileCursor::at(source, node_addr)?;
        cur.tag(SIG_BTREE_V1)?;
        let node_type = cur.byte()?;
        if node_type != 1 {
            return Err(FieldglassError::Parse(format!(
                "expected B-tree v1 chunk node, got node type {node_type}"
            )));
        }
        let level = cur.byte()?;
        let entries = cur.u16()? as usize;
        if entries > max_entries {
            return Err(FieldglassError::Parse(format!(
                "chunk B-tree node holds {entries} entries, more than 2K = {max_entries}"
            )));
        }
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
    let (grid, grid_count) = chunk_grid(shape, chunk_dims, "implicit")?;
    let grid_count = grid_count as u64;

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

/// The row-major chunk grid of a dataset, `ceil(shape / chunk)` per dimension,
/// and its cell count, refused past [`MAX_BTREE_NODES`] chunks.
///
/// The three indexes computed from the grid rather than walked (implicit,
/// fixed and extensible array) take their record count from it, so it is the
/// number that sizes their allocations and loops. A whole read used to meet
/// the whole-variable budget first, which bounded the shape and with it the
/// grid; a region read does not, so the grid is bounded here, at the cap the
/// B-tree walk holds its record count to (#939 review). The cap is a `usize`,
/// so the count is exact on a 32-bit target.
fn chunk_grid(
    shape: &[u64],
    chunk_dims: &[u32],
    index: &str,
) -> Result<(Vec<u64>, usize), FieldglassError> {
    let grid: Vec<u64> = shape
        .iter()
        .zip(chunk_dims)
        .map(|(&s, &c)| s.div_ceil(u64::from(c)))
        .collect();
    let count = grid.iter().fold(1u64, |acc, &n| acc.saturating_mul(n));
    if count > MAX_BTREE_NODES as u64 {
        return Err(FieldglassError::Parse(format!(
            "{index} chunk grid has {count} chunks, more than the {MAX_BTREE_NODES} \
             a chunk index may hold"
        )));
    }
    Ok((grid, count as usize))
}

/// Fixed Array header / data-block signatures (v4 chunk index type 3).
const SIG_FIXED_ARRAY_HEADER: &[u8; 4] = b"FAHD";
const SIG_FIXED_ARRAY_DBLOCK: &[u8; 4] = b"FADB";

/// Collect chunk records from a version-4 Fixed Array index, used for
/// fixed-shape chunked datasets under the HDF5 "latest format". The array holds
/// one element per chunk in row-major chunk order; an element is a chunk address
/// (unfiltered) or address + on-disk size + filter mask (filtered). Each chunk's
/// element-space offset is computed from its linear position in the chunk grid.
fn collect_fixed_array_chunks<S: ByteSource + ?Sized>(
    source: &S,
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
    let mut h = FileCursor::at(source, header_addr)?;
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
    let num_entries = h.usize(l)?; // max num entries == chunk count
    let dblock_addr = h.uint(o)?;

    // Row-major chunk grid: ceil(shape / chunk) per dimension. Its cell count
    // must match the array's entry count, so the count is bounded before it
    // sizes anything below (#939 review).
    let (grid, grid_count) = chunk_grid(shape, chunk_dims, "Fixed Array")?;
    if grid_count != num_entries {
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
    let mut d = FileCursor::at(source, dblock_addr)?;
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
fn collect_extensible_array_chunks<S: ByteSource + ?Sized>(
    source: &S,
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
    let mut h = FileCursor::at(source, header_addr)?;
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
    let (grid, grid_count) = chunk_grid(shape, chunk_dims, "extensible array")?;

    // Index Block: prefix, the first `idx_blk_elmts` elements, then the direct
    // data-block addresses, then the secondary-block addresses.
    let mut ib = FileCursor::at(source, index_block_addr)?;
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
        // `s` counts super blocks, and the `checked_shl` below fails once the
        // shift reaches 32 — so `s < 64` on every iteration that gets here and
        // the narrowing to `u32` is exact on any target.
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
            read_ea_secondary_dblk_addrs(source, sblk_addr, ndblks_s, o, arr_off_size)?
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
            let mut db = FileCursor::at(source, dblk_addr)?;
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
fn collect_v2_btree_chunks<S: ByteSource + ?Sized>(
    source: &S,
    header_addr: u64,
    chunk_dims: &[u32],
    chunk_bytes: usize,
    osize: u8,
    lsize: u8,
) -> Result<Vec<ChunkRecord>, FieldglassError> {
    let o = osize as usize;
    let rank = chunk_dims.len();
    // A chunk record is the chunk's address, then (filtered, type 11) a
    // 1-8 byte stored size and a 4-byte filter mask, then one 8-byte scaled
    // offset per dimension (#895).
    let layout = |t: u8, size: usize| {
        let fixed = o.saturating_add(rank.saturating_mul(8));
        match t {
            BTREE_V2_TYPE_CHUNK_UNFILTERED if size == fixed => Ok(()),
            BTREE_V2_TYPE_CHUNK_FILTERED
                if size
                    .checked_sub(fixed.saturating_add(4))
                    .is_some_and(|w| (1..=8).contains(&w)) =>
            {
                Ok(())
            }
            BTREE_V2_TYPE_CHUNK_UNFILTERED | BTREE_V2_TYPE_CHUNK_FILTERED => {
                Err(FieldglassError::Parse(format!(
                    "B-tree v2 type {t} chunk records are {size} bytes, which no rank-{rank} \
                     record measures"
                )))
            }
            other => Err(FieldglassError::Parse(format!(
                "unsupported B-tree v2 type {other} for a chunk index (expected 10 or 11)"
            ))),
        }
    };
    let (btree_type, records) =
        super::heap::btree_v2_records(source, header_addr, osize, lsize, &layout)?;
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

        let mut cur = Cursor::over(record);
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
    cur: &mut impl Fields,
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
fn read_ea_secondary_dblk_addrs<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    ndblks: usize,
    o: usize,
    arr_off_size: usize,
) -> Result<Vec<u64>, FieldglassError> {
    let mut c = FileCursor::at(source, addr)?;
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

/// Check every chunk record against the chunk grid and keep the ones that
/// overlap `region` of the dataset's current shape, each chunk once (#837,
/// ADR-0013, #939).
///
/// An index names each chunk once, by its element-space origin, which is a
/// multiple of the chunk edge in every dimension: a v1 B-tree's keys are
/// strictly ordered, and a v2 B-tree's records carry the chunk-grid coordinate.
/// Four cases depart from that, and each is settled against libhdf5:
///
/// - **Records identical in origin, address, size and filter mask** are read
///   once. libhdf5 reads the value they all name. Reading each copy is how one
///   16 KB deflate stream that inflates to 16 MB was inflated per record.
/// - **Records at one origin naming different storage** are refused. The value
///   is ambiguous: libhdf5's answer depends on the records' order and on its
///   tree search, and this is the one case where the reader diverges from it.
/// - **A record whose origin lies outside the shape** is skipped without being
///   read, as libhdf5 never looks such a chunk up.
/// - **An origin inside the shape but off the chunk grid** is refused, as
///   libhdf5 refuses it.
///
/// Records at *different* origins naming the same storage are legal, and the
/// caller groups them so that chunk is read once.
///
/// A record whose chunk does not overlap the region is skipped unread before
/// any of this, as one past the shape is: a whole read's region is the shape,
/// so for it the two are the same rule. A region read therefore judges only the
/// records it reads.
fn chunks_in_region<'a>(
    chunks: &'a [ChunkRecord],
    shape: &[u64],
    region: &[Range<u64>],
    chunk_dims: &[u32],
    pipeline_bits: u32,
) -> Result<Vec<&'a ChunkRecord>, FieldglassError> {
    let mut kept = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.offset.len() != chunk_dims.len() {
            return Err(FieldglassError::Parse(format!(
                "chunk record has {} offsets for a rank-{} dataset",
                chunk.offset.len(),
                chunk_dims.len()
            )));
        }
        if !chunk.offset.iter().zip(shape).all(|(&o, &s)| o < s) {
            continue;
        }
        // The chunk's box against the region's, axis by axis.
        let overlaps = chunk
            .offset
            .iter()
            .zip(chunk_dims)
            .zip(region)
            .all(|((&o, &c), r)| {
                r.start < r.end && o < r.end && o.saturating_add(u64::from(c)) > r.start
            });
        if !overlaps {
            continue;
        }
        if let Some(d) =
            (0..chunk_dims.len()).find(|&d| chunk.offset[d] % u64::from(chunk_dims[d]) != 0)
        {
            return Err(FieldglassError::Parse(format!(
                "chunk record origin {:?} is not on the chunk grid (dimension {d}, chunk edge {})",
                chunk.offset, chunk_dims[d]
            )));
        }
        kept.push(chunk);
    }
    kept.sort_unstable_by(|a, b| a.offset.cmp(&b.offset));
    // Storage compared as the caller's grouping compares it: the filter mask
    // only over the pipeline's own filters (#888).
    let same_storage = |a: &ChunkRecord, b: &ChunkRecord| {
        (a.address, a.size, a.filter_mask & pipeline_bits)
            == (b.address, b.size, b.filter_mask & pipeline_bits)
    };
    if let Some(pair) = kept
        .windows(2)
        .find(|pair| pair[0].offset == pair[1].offset && !same_storage(pair[0], pair[1]))
    {
        return Err(FieldglassError::Parse(format!(
            "chunk index names two different chunks at origin {:?}",
            pair[0].offset
        )));
    }
    kept.dedup_by(|a, b| a.offset == b.offset);
    Ok(kept)
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
    let size = read_usize_le(body, size_pos, 4)?;
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
/// first element here; the CF unpack that follows ([`crate::unpack_cf_data`])
/// masks the rest.
fn missing_sentinels<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<f64>, FieldglassError> {
    let attrs = attribute::list_attributes(source, object_header_address, probe)?;
    // Use the typed first element, not the rendered display string: the display
    // text is rounded to a few decimals, so reparsing it would not bit-match the
    // decoded value and float sentinels would silently fail to mask.
    Ok(["_FillValue", "missing_value"]
        .into_iter()
        .filter_map(|name| attrs.iter().find(|a| a.name == name))
        .filter_map(|a| a.first_value())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element-by-element walk `scatter_chunk` replaced (#837): every
    /// element of the chunk, placed when it lands inside the shape.
    fn scatter_reference(
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
            let mut rem = c;
            for d in (0..rank).rev() {
                coord[d] = rem % chunk_dims[d] as usize;
                rem /= chunk_dims[d] as usize;
            }
            let mut ds_index = 0u64;
            let mut in_bounds = true;
            for d in 0..rank {
                let ds_coord = origin[d] + coord[d] as u64;
                if ds_coord >= shape[d] {
                    in_bounds = false;
                    break;
                }
                ds_index = ds_index * shape[d] + ds_coord;
            }
            if in_bounds {
                let (src, dst) = (c * elem, ds_index as usize * elem);
                raw[dst..dst + elem].copy_from_slice(&chunk[src..src + elem]);
            }
        }
    }

    #[test]
    fn clipped_scatter_places_what_the_element_walk_placed() {
        // Ranks 1-3, interior, edge and oversized chunks, origins on and past
        // the shape, one- and four-byte elements.
        let cases: &[(&[u64], &[u32])] = &[
            (&[7], &[3]),
            (&[1], &[16]),
            (&[5, 4], &[2, 3]),
            (&[3, 2], &[8, 8]),
            (&[4, 3, 5], &[2, 2, 3]),
            (&[2, 1, 3], &[1, 4, 2]),
        ];
        for &(shape, chunk_dims) in cases {
            for elem in [1usize, 4] {
                let n: usize = shape.iter().product::<u64>() as usize;
                let chunk_elems: usize = chunk_dims.iter().map(|&d| d as usize).product();
                let chunk: Vec<u8> = (0..chunk_elems * elem)
                    .map(|i| (i % 251) as u8 + 1)
                    .collect();
                let grid: Vec<u64> = shape
                    .iter()
                    .zip(chunk_dims)
                    .map(|(&s, &c)| s.div_ceil(u64::from(c)) + 1)
                    .collect();
                for i in 0..grid.iter().product::<u64>() {
                    let origin = chunk_offset_from_linear(i, &grid, chunk_dims);
                    let mut got = vec![0u8; n * elem];
                    let mut want = vec![0u8; n * elem];
                    let chunk_shape: Vec<u64> = chunk_dims.iter().map(|&d| u64::from(d)).collect();
                    let whole: Vec<Range<u64>> = shape.iter().map(|&n| 0..n).collect();
                    copy_block_elements(&chunk, &chunk_shape, &origin, &whole, &mut got, elem);
                    scatter_reference(&mut want, &chunk, shape, chunk_dims, &origin, elem);
                    assert_eq!(
                        got, want,
                        "shape {shape:?} chunk {chunk_dims:?} origin {origin:?}"
                    );
                }
            }
        }
    }

    fn record(offset: &[u64]) -> ChunkRecord {
        ChunkRecord {
            address: 4096,
            size: 16,
            filter_mask: 0,
            offset: offset.to_vec(),
        }
    }

    /// [`chunks_in_region`] for a whole read, whose region is the shape.
    fn chunks_in_shape<'a>(
        chunks: &'a [ChunkRecord],
        shape: &[u64],
        chunk_dims: &[u32],
        pipeline_bits: u32,
    ) -> Result<Vec<&'a ChunkRecord>, FieldglassError> {
        let whole: Vec<Range<u64>> = shape.iter().map(|&n| 0..n).collect();
        chunks_in_region(chunks, shape, &whole, chunk_dims, pipeline_bits)
    }

    /// A region keeps exactly the chunks whose boxes it overlaps, and judges
    /// only those: a conflict in a chunk it does not read is not its failure.
    #[test]
    fn a_region_keeps_the_chunks_it_overlaps() {
        let (shape, dims) = ([10u64, 6], [4u32, 3]);
        let grid: Vec<ChunkRecord> = [[0, 0], [0, 3], [4, 0], [4, 3], [8, 0], [8, 3]]
            .iter()
            .map(|o| record(o))
            .collect();
        let origins = |region: &[Range<u64>]| -> Vec<Vec<u64>> {
            chunks_in_region(&grid, &shape, region, &dims, 1)
                .unwrap()
                .iter()
                .map(|c| c.offset.clone())
                .collect()
        };
        // One row inside the second chunk row: two chunks.
        assert_eq!(origins(&[5..6, 0..6]), [vec![4, 0], vec![4, 3]]);
        // A region ending exactly on a chunk boundary does not pull in the next.
        assert_eq!(origins(&[0..4, 0..3]), [vec![0, 0]]);
        // One cell at the far corner: the ragged edge chunk.
        assert_eq!(origins(&[9..10, 5..6]), [vec![8, 3]]);
        // An empty range overlaps nothing.
        assert!(origins(&[3..3, 0..6]).is_empty());

        let mut conflicting = grid.iter().map(|c| record(&c.offset)).collect::<Vec<_>>();
        let mut other = record(&[8, 3]);
        other.address += 64;
        conflicting.push(other);
        assert!(chunks_in_region(&conflicting, &shape, &[0..4, 0..6], &dims, 1).is_ok());
        assert!(chunks_in_region(&conflicting, &shape, &[8..10, 3..6], &dims, 1).is_err());
    }

    #[test]
    fn chunk_records_are_held_to_the_grid() {
        let (shape, dims) = ([10u64, 6], [4u32, 3]);
        // Every cell once, plus one past the shape: the last is skipped unread.
        let chunks = vec![
            record(&[0, 0]),
            record(&[0, 3]),
            record(&[4, 0]),
            record(&[4, 3]),
            record(&[8, 0]),
            record(&[8, 3]),
            record(&[12, 0]),
        ];
        let kept = chunks_in_shape(&chunks, &shape, &dims, 1).expect("a valid index");
        assert_eq!(kept.len(), 6);

        // The same record twice, wherever the repeat sits, is read once.
        let dup = vec![record(&[4, 3]), record(&[0, 0]), record(&[4, 3])];
        let kept = chunks_in_shape(&dup, &shape, &dims, 1).expect("identical copies");
        let origins: Vec<&[u64]> = kept.iter().map(|c| c.offset.as_slice()).collect();
        assert_eq!(origins, [&[0u64, 0][..], &[4, 3]]);

        // Two records at one origin naming different storage are refused.
        for differ in [
            |r: &mut ChunkRecord| r.address += 64,
            |r: &mut ChunkRecord| r.size += 1,
            |r: &mut ChunkRecord| r.filter_mask = 1,
        ] {
            let mut other = record(&[4, 3]);
            differ(&mut other);
            let conflict = vec![record(&[4, 3]), record(&[0, 0]), other];
            let err = chunks_in_shape(&conflict, &shape, &dims, 1).expect_err("ambiguous");
            assert!(err.to_string().contains("two different chunks"), "{err}");
        }

        // Masks that differ only above the pipeline's filters name the same
        // stored chunk (#888); over a filter the pipeline has, they conflict.
        let mut masked = record(&[4, 3]);
        masked.filter_mask = 2;
        let pair = vec![record(&[4, 3]), masked];
        assert_eq!(chunks_in_shape(&pair, &shape, &dims, 1).unwrap().len(), 1);
        let err = chunks_in_shape(&pair, &shape, &dims, 3).expect_err("filter 1 differs");
        assert!(err.to_string().contains("two different chunks"), "{err}");

        // An origin off the chunk grid is refused inside the shape, and
        // skipped unread outside it like any other record there.
        let off = vec![record(&[2, 0])];
        let err = chunks_in_shape(&off, &shape, &dims, 1).expect_err("off the grid");
        assert!(err.to_string().contains("not on the chunk grid"), "{err}");
        let off_outside = vec![record(&[0, 0]), record(&[13, 1])];
        assert_eq!(
            chunks_in_shape(&off_outside, &shape, &dims, 1)
                .unwrap()
                .len(),
            1
        );

        // A conflict that lies wholly outside the shape is never read, so it is
        // skipped like any other record there.
        let mut far = record(&[12, 0]);
        far.address += 64;
        let outside = vec![record(&[0, 0]), record(&[12, 0]), far];
        assert_eq!(
            chunks_in_shape(&outside, &shape, &dims, 1).unwrap().len(),
            1
        );
    }
}
