//! Shared HDF5 fractal-heap + version-2 B-tree readers, plus the little-endian
//! byte cursor they're built on. These back both the dense-link path (group
//! traversal, #38) and the dense-attribute path (#40): in each case a B-tree v2
//! indexes records that carry a fractal-heap ID, and the heap dereferences that
//! ID to the actual message bytes.
//!
//! Both single-level (depth 0) and multi-level (depth > 0) B-trees are walked:
//! once a dense attribute / link index outgrows one leaf the root becomes an
//! internal node, and the records are gathered from every leaf in key order.
//! For the fractal heap, both a single root direct block and a root *indirect*
//! block (the doubling table that real attribute-rich files spill into once
//! their dense storage outgrows one direct block) are handled, but only while
//! every row holds direct blocks — child indirect blocks, huge/tiny objects,
//! and I/O filters return a clear error rather than risk a silent misread.
//!
//! Reference: HDF5 file format specification version 3, "Fractal Heap" and
//! "Version 2 B-trees".

use super::object_header::{is_undefined_address, read_uint_le};
use fieldglass_core::FieldglassError;

const SIG_BTREE_V2_HDR: &[u8; 4] = b"BTHD";
const SIG_BTREE_V2_INTERNAL: &[u8; 4] = b"BTIN";
const SIG_BTREE_V2_LEAF: &[u8; 4] = b"BTLF";
const SIG_FRACTAL_HEAP: &[u8; 4] = b"FRHP";
const SIG_FRACTAL_DIRECT: &[u8; 4] = b"FHDB";
const SIG_FRACTAL_INDIRECT: &[u8; 4] = b"FHIB";

/// Guards a malformed row count in a root indirect block.
const MAX_HEAP_ROWS: usize = 64;

/// Guards a malformed doubling-table width (columns per row).
const MAX_HEAP_TABLE_WIDTH: usize = 1024;

/// Upper bound on B-tree v2 records gathered across all leaves — guards a
/// malformed record or node count.
const MAX_BTREE_V2_RECORDS: usize = 1 << 20;

/// Upper bound on B-tree v2 depth — guards a malformed header and bounds the
/// node-walk recursion. Real indexes are only a few levels deep.
const MAX_BTREE_V2_DEPTH: u16 = 32;

/// Upper bound on a B-tree v2 node size — guards a malformed header. Real node
/// sizes are a few KB; this cap keeps the per-level record counts (and the
/// sizing recurrence) in a sane range rather than near `u32::MAX`.
const MAX_BTREE_V2_NODE_SIZE: usize = 1 << 20;

/// Fixed overhead of any B-tree v2 node image: signature(4) + version(1) +
/// type(1) at the front and a Jenkins checksum(4) at the back.
const BTREE_V2_NODE_OVERHEAD: usize = 6 + 4;

/// Read every record of a version-2 B-tree, descending through internal nodes
/// when the tree has more than one level, and return the B-tree `type` with the
/// records in key order. Callers slice the fractal-heap ID out of each record at
/// the offset their record type dictates (link-name records put it after a
/// 4-byte hash; attribute-name records put it first).
pub(crate) fn btree_v2_records(
    bytes: &[u8],
    addr: u64,
    osize: u8,
    lsize: u8,
) -> Result<(u8, Vec<Vec<u8>>), FieldglassError> {
    let mut hdr = Cursor::at(bytes, addr)?;
    hdr.tag(SIG_BTREE_V2_HDR)?;
    hdr.skip(1)?; // version
    let btree_type = hdr.byte()?;
    let node_size = hdr.uint(4)? as usize;
    let record_size = hdr.u16()? as usize;
    let depth = hdr.u16()?;
    hdr.skip(2)?; // split + merge percent
    let root_addr = hdr.uint(osize as usize)?;
    let root_nrec = hdr.u16()? as usize;
    let _total = hdr.uint(lsize as usize)?;

    if record_size == 0 {
        return Err(FieldglassError::Parse("zero B-tree v2 record size".into()));
    }
    if node_size > MAX_BTREE_V2_NODE_SIZE {
        return Err(FieldglassError::Parse(
            "implausible B-tree v2 node size".into(),
        ));
    }
    if depth > MAX_BTREE_V2_DEPTH {
        return Err(FieldglassError::Parse("implausible B-tree v2 depth".into()));
    }
    if root_nrec > MAX_BTREE_V2_RECORDS {
        return Err(FieldglassError::Parse(
            "implausible B-tree v2 record count".into(),
        ));
    }

    // The width of a node pointer's "number of records" fields is derived from
    // how many records each level can hold, so precompute the per-level geometry
    // before walking. `levels[u]` describes a node sitting `u` levels above the
    // leaves (level 0).
    let levels = BTreeV2Levels::compute(node_size, record_size, osize as usize, depth)?;

    let mut records = Vec::new();
    walk_btree_v2_node(
        bytes,
        root_addr,
        depth,
        root_nrec,
        record_size,
        osize as usize,
        &levels,
        &mut records,
    )?;
    Ok((btree_type, records))
}

/// Per-level sizing for a version-2 B-tree, mirroring libhdf5's `node_info`
/// table. A node pointer to a level-`u` child stores that child's record count
/// in `count_bytes[u]` bytes, and — when the child is itself internal — its
/// whole-subtree record count in `cum_bytes[u]` bytes.
struct BTreeV2Levels {
    /// Bytes used to encode the record count of a level-`u` child node.
    count_bytes: Vec<usize>,
    /// Bytes used to encode the total record count beneath a level-`u` child
    /// (only read when that child is internal, i.e. `u >= 1`).
    cum_bytes: Vec<usize>,
    /// Maximum records a level-`u` node can hold, used to reject malformed
    /// node-pointer record counts.
    max_nrec: Vec<usize>,
}

impl BTreeV2Levels {
    fn compute(
        node_size: usize,
        record_size: usize,
        osize: usize,
        depth: u16,
    ) -> Result<Self, FieldglassError> {
        let usable = node_size
            .checked_sub(BTREE_V2_NODE_OVERHEAD)
            .filter(|&u| u >= record_size)
            .ok_or_else(|| FieldglassError::Parse("B-tree v2 node too small".into()))?;

        // Level 0 — leaves hold as many fixed-size records as the node fits.
        let mut max_nrec = vec![usable / record_size];
        let mut cum_max = vec![max_nrec[0] as u64];
        let mut count_bytes = vec![count_field_bytes(max_nrec[0] as u64)];
        let mut cum_bytes = vec![0usize]; // unused for leaves

        for u in 1..=depth as usize {
            // A pointer to the level below: address + its record-count field, plus
            // its subtree-total field once that child is itself internal.
            let child = u - 1;
            let ptr = osize + count_bytes[child] + if child >= 1 { cum_bytes[child] } else { 0 };
            let denom = record_size + ptr;
            // n records need n*(record+ptr) + ptr bytes (n+1 pointers); solve for n.
            let n = usable
                .checked_sub(ptr)
                .map(|room| room / denom)
                .filter(|&n| n > 0)
                .ok_or_else(|| {
                    FieldglassError::Parse("B-tree v2 internal node holds no records".into())
                })?;
            // Saturate: a subtree total that approaches `u64::MAX` only widens
            // the (skipped) total field to its 8-byte ceiling, so over-estimating
            // is harmless, where an unchecked multiply would overflow-panic on a
            // crafted deep header.
            let cum = (n as u64).saturating_add((n as u64 + 1).saturating_mul(cum_max[child]));
            max_nrec.push(n);
            count_bytes.push(count_field_bytes(n as u64));
            cum_bytes.push(count_field_bytes(cum));
            cum_max.push(cum);
        }

        Ok(Self {
            count_bytes,
            cum_bytes,
            max_nrec,
        })
    }
}

/// Least number of bytes needed to encode values up to `max` (libhdf5's
/// `H5B2_SIZEOF_RECORDS_PER_NODE`): `floor(log2(max)) / 8 + 1`, at least one.
fn count_field_bytes(max: u64) -> usize {
    if max == 0 {
        1
    } else {
        (max.ilog2() as usize) / 8 + 1
    }
}

/// Collect the records under one B-tree v2 node at `level` (0 = leaf). Internal
/// nodes store all `nrec` records first, then `nrec + 1` node pointers; the
/// records interleave between the subtrees in key order.
#[allow(clippy::too_many_arguments)]
fn walk_btree_v2_node(
    bytes: &[u8],
    addr: u64,
    level: u16,
    nrec: usize,
    record_size: usize,
    osize: usize,
    levels: &BTreeV2Levels,
    out: &mut Vec<Vec<u8>>,
) -> Result<(), FieldglassError> {
    // This guard is also the total-work bound, not just per-node validation:
    // a node fans out to `nrec + 1` children, so producing more than one child
    // requires `nrec >= 1`, and those records are pushed into `out` below. The
    // `out.len() + nrec` cap therefore limits how many fan-out nodes can be
    // visited, while `level` strictly decreasing (depth ≤ MAX_BTREE_V2_DEPTH)
    // bounds the depth of any `nrec == 0` chain — together ruling out the
    // exponential blow-up a back-edge could otherwise cause. Keep the record
    // push *after* the recursion so this stays true.
    if nrec > levels.max_nrec[level as usize] || out.len() + nrec > MAX_BTREE_V2_RECORDS {
        return Err(FieldglassError::Parse(
            "B-tree v2 node record count out of range".into(),
        ));
    }
    let mut cur = Cursor::at(bytes, addr)?;

    if level == 0 {
        cur.tag(SIG_BTREE_V2_LEAF)?;
        cur.skip(2)?; // version + type
        for _ in 0..nrec {
            out.push(cur.take(record_size)?.to_vec());
        }
        return Ok(());
    }

    cur.tag(SIG_BTREE_V2_INTERNAL)?;
    cur.skip(2)?; // version + type
    let mut records = Vec::with_capacity(nrec);
    for _ in 0..nrec {
        records.push(cur.take(record_size)?.to_vec());
    }

    // Node pointers to the level below: address, record count, and — when that
    // child is internal — its subtree total (read past, not needed here).
    let child = level - 1;
    let count_bytes = levels.count_bytes[child as usize];
    let cum_bytes = if child >= 1 {
        levels.cum_bytes[child as usize]
    } else {
        0
    };
    let mut children = Vec::with_capacity(nrec + 1);
    for _ in 0..=nrec {
        let child_addr = cur.uint(osize)?;
        let child_nrec = cur.uint(count_bytes)? as usize;
        if cum_bytes > 0 {
            cur.skip(cum_bytes)?;
        }
        children.push((child_addr, child_nrec));
    }

    for (i, &(child_addr, child_nrec)) in children.iter().enumerate() {
        walk_btree_v2_node(
            bytes,
            child_addr,
            child,
            child_nrec,
            record_size,
            osize,
            levels,
            out,
        )?;
        if let Some(rec) = records.get(i) {
            out.push(rec.clone());
        }
    }
    Ok(())
}

/// One managed direct block, placed in the heap's linear address space. A heap
/// offset is resolved by finding the block whose `[logical, logical + size)`
/// range contains it; the object then lives at `file_addr + (offset - logical)`
/// (the heap offset counts the block's prefix bytes, matching the file layout).
struct DirectBlock {
    /// Start of this block in the heap's linear address space.
    logical: u64,
    /// Block size in bytes (prefix included).
    size: u64,
    /// File address of the block's `FHDB` signature.
    file_addr: u64,
}

/// The subset of a fractal heap needed to dereference managed objects.
pub(crate) struct FractalHeap {
    /// Total length of a heap ID for this heap.
    pub(crate) heap_id_len: usize,
    /// Byte width of the offset field inside a managed heap ID.
    offset_bytes: usize,
    /// Byte width of the length field inside a managed heap ID.
    length_bytes: usize,
    /// The heap's direct blocks, ordered by their place in the address space.
    blocks: Vec<DirectBlock>,
}

impl FractalHeap {
    pub(crate) fn parse(
        bytes: &[u8],
        addr: u64,
        osize: u8,
        lsize: u8,
    ) -> Result<Self, FieldglassError> {
        let o = osize as usize;
        let l = lsize as usize;
        let mut cur = Cursor::at(bytes, addr)?;
        cur.tag(SIG_FRACTAL_HEAP)?;
        cur.skip(1)?; // version
        let heap_id_len = cur.u16()? as usize;
        let io_filter_len = cur.u16()? as usize;
        if io_filter_len != 0 {
            return Err(FieldglassError::Parse(
                "I/O-filtered fractal heaps not supported".into(),
            ));
        }
        cur.skip(1)?; // flags
        cur.skip(4)?; // maximum size of managed objects
        // Skip the statistics block between here and "table width". It is ten
        // length-sized fields (next-huge id, free space, managed/allocated
        // space, iterator offset, #managed, size/#huge, size/#tiny) plus two
        // offset-sized addresses (huge-object B-tree, free-space manager).
        cur.skip(l * 10 + o * 2)?;
        let table_width = cur.u16()? as usize;
        let starting_block_size = cur.uint(l)?;
        let max_direct_block_size = cur.uint(l)?;
        let max_heap_bits = cur.u16()? as usize;
        cur.skip(2)?; // starting # rows in root indirect block
        let root_block_addr = cur.uint(o)?;
        let cur_rows = cur.u16()? as usize;

        let offset_bytes = max_heap_bits.div_ceil(8);
        // A managed heap ID is flags(1) + offset(offset_bytes) + length(>=1).
        if heap_id_len < offset_bytes + 2 {
            return Err(FieldglassError::Parse(
                "fractal heap ID too small for its offset + length fields".into(),
            ));
        }
        let length_bytes = heap_id_len - 1 - offset_bytes;

        let blocks = if cur_rows == 0 {
            // A single root direct block of the starting size.
            validate_direct_block(bytes, root_block_addr, addr, o)?;
            vec![DirectBlock {
                logical: 0,
                size: starting_block_size,
                file_addr: root_block_addr,
            }]
        } else {
            parse_root_indirect(
                bytes,
                root_block_addr,
                addr,
                o,
                offset_bytes,
                table_width,
                starting_block_size,
                max_direct_block_size,
                cur_rows,
            )?
        };

        Ok(Self {
            heap_id_len,
            offset_bytes,
            length_bytes,
            blocks,
        })
    }

    /// Dereference a managed heap ID to its object bytes. The heap ID is
    /// `flags(1) + offset(offset_bytes) + length(length_bytes)`.
    pub(crate) fn managed_object(
        &self,
        bytes: &[u8],
        id: &[u8],
    ) -> Result<Vec<u8>, FieldglassError> {
        if id.len() < self.heap_id_len {
            return Err(FieldglassError::Parse("truncated heap ID".into()));
        }
        // Bits 4-5 of the first byte are the ID type; 0 == managed.
        if (id[0] >> 4) & 0x03 != 0 {
            return Err(FieldglassError::Parse(
                "only managed fractal-heap objects are supported".into(),
            ));
        }
        let offset = read_uint_le(id, 1, self.offset_bytes)?;
        let length = read_uint_le(id, 1 + self.offset_bytes, self.length_bytes)? as usize;
        let block = self
            .blocks
            .iter()
            .find(|b| offset >= b.logical && offset - b.logical < b.size)
            .ok_or_else(|| FieldglassError::Parse("heap offset outside any direct block".into()))?;
        let obj_addr = checked_add(block.file_addr, offset - block.logical)?;
        let start = usize::try_from(obj_addr)
            .map_err(|_| FieldglassError::Parse("heap object address too large".into()))?;
        let end = start
            .checked_add(length)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| FieldglassError::Parse("heap object runs past end of file".into()))?;
        Ok(bytes[start..end].to_vec())
    }
}

/// Confirm a direct block: the address lands on an `FHDB` signature whose
/// back-pointer is the owning heap header. A cheap guard against mis-parsing the
/// variable-width header.
fn validate_direct_block(
    bytes: &[u8],
    block_addr: u64,
    heap_addr: u64,
    osize: usize,
) -> Result<(), FieldglassError> {
    let mut block = Cursor::at(bytes, block_addr)?;
    block.tag(SIG_FRACTAL_DIRECT)?;
    block.skip(1)?; // version
    if block.uint(osize)? != heap_addr {
        return Err(FieldglassError::Parse(
            "fractal-heap direct block back-pointer mismatch".into(),
        ));
    }
    Ok(())
}

/// Block size for row `r` of the doubling table: rows 0 and 1 share the starting
/// size, and it doubles every row after that.
fn row_block_size(starting: u64, row: usize) -> u64 {
    if row <= 1 {
        starting
    } else {
        starting.saturating_mul(1u64 << (row - 1).min(63))
    }
}

/// Walk a root indirect block's doubling table into its direct blocks. Every row
/// must be direct (block size ≤ the max direct size); a row of *child* indirect
/// blocks is rejected, as are huge/tiny objects elsewhere.
#[allow(clippy::too_many_arguments)]
fn parse_root_indirect(
    bytes: &[u8],
    indirect_addr: u64,
    heap_addr: u64,
    osize: usize,
    offset_bytes: usize,
    table_width: usize,
    starting_block_size: u64,
    max_direct_block_size: u64,
    cur_rows: usize,
) -> Result<Vec<DirectBlock>, FieldglassError> {
    if table_width == 0 || table_width > MAX_HEAP_TABLE_WIDTH {
        return Err(FieldglassError::Parse(
            "implausible heap table width".into(),
        ));
    }
    if cur_rows > MAX_HEAP_ROWS {
        return Err(FieldglassError::Parse(
            "implausible fractal-heap row count".into(),
        ));
    }

    let mut cur = Cursor::at(bytes, indirect_addr)?;
    cur.tag(SIG_FRACTAL_INDIRECT)?;
    cur.skip(1)?; // version
    if cur.uint(osize)? != heap_addr {
        return Err(FieldglassError::Parse(
            "fractal-heap indirect block back-pointer mismatch".into(),
        ));
    }
    cur.skip(offset_bytes)?; // this block's offset in the heap address space (0 at the root)

    let mut blocks = Vec::new();
    let mut logical = 0u64;
    for row in 0..cur_rows {
        let size = row_block_size(starting_block_size, row);
        if size > max_direct_block_size {
            return Err(FieldglassError::Parse(
                "child indirect fractal-heap blocks not supported".into(),
            ));
        }
        for _ in 0..table_width {
            let block_addr = cur.uint(osize)?;
            // An undefined address marks an unallocated table slot — skip it, but
            // still advance `logical` so later blocks keep their place in the
            // heap's linear address space.
            if !is_undefined_address(block_addr, osize as u8) {
                validate_direct_block(bytes, block_addr, heap_addr, osize)?;
                blocks.push(DirectBlock {
                    logical,
                    size,
                    file_addr: block_addr,
                });
            }
            logical = checked_add(logical, size)?;
        }
    }
    if blocks.is_empty() {
        return Err(FieldglassError::Parse(
            "fractal-heap root indirect block has no direct blocks".into(),
        ));
    }
    Ok(blocks)
}

pub(crate) fn checked_add(a: u64, b: u64) -> Result<u64, FieldglassError> {
    a.checked_add(b)
        .ok_or_else(|| FieldglassError::Parse("address arithmetic overflow".into()))
}

/// A tiny forward cursor over a byte slice, reading little-endian fields with
/// bounds checks. `at`/`over` choose between an absolute file offset and a
/// borrowed message body.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn at(bytes: &'a [u8], addr: u64) -> Result<Self, FieldglassError> {
        let pos = usize::try_from(addr)
            .map_err(|_| FieldglassError::Parse("address too large for this platform".into()))?;
        if pos > bytes.len() {
            return Err(FieldglassError::Parse("address past end of file".into()));
        }
        Ok(Self { bytes, pos })
    }

    pub(crate) fn over(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn uint(&mut self, width: usize) -> Result<u64, FieldglassError> {
        let value = read_uint_le(self.bytes, self.pos, width)?;
        self.pos += width;
        Ok(value)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FieldglassError> {
        Ok(self.uint(2)? as u16)
    }

    pub(crate) fn byte(&mut self) -> Result<u8, FieldglassError> {
        Ok(self.uint(1)? as u8)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<(), FieldglassError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| FieldglassError::Parse("skip past end of buffer".into()))?;
        self.pos = end;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut Vec<u8>, at: usize, data: &[u8]) {
        if buf.len() < at + data.len() {
            buf.resize(at + data.len(), 0);
        }
        buf[at..at + data.len()].copy_from_slice(data);
    }

    // Node size every test B-tree advertises; with the small record sizes used
    // here every count/total field is one byte wide (see `level_geometry`).
    const NODE_SIZE: usize = 512;

    /// Write a B-tree v2 header at offset 0 pointing at `root_addr`.
    fn btree_v2_header(
        buf: &mut Vec<u8>,
        btree_type: u8,
        record_size: usize,
        depth: u16,
        root_addr: u64,
        root_nrec: usize,
    ) {
        // Header: sig(4) version(1) type(1) node_size(4) record_size(2) depth(2)
        //         split/merge(2) root_addr(8) root_nrec(2) total(8) checksum(4).
        put(buf, 0, SIG_BTREE_V2_HDR);
        buf[5] = btree_type;
        put(buf, 6, &(NODE_SIZE as u32).to_le_bytes());
        put(buf, 10, &(record_size as u16).to_le_bytes());
        put(buf, 12, &depth.to_le_bytes());
        put(buf, 16, &root_addr.to_le_bytes());
        put(buf, 24, &(root_nrec as u16).to_le_bytes());
    }

    /// A depth-0 B-tree v2 (header + a single leaf) carrying `records`.
    fn btree_v2(btree_type: u8, record_size: usize, records: &[Vec<u8>]) -> Vec<u8> {
        let leaf_addr = 0x100usize;
        let mut buf = vec![0u8; 0x200];
        btree_v2_header(
            &mut buf,
            btree_type,
            record_size,
            0,
            leaf_addr as u64,
            records.len(),
        );
        write_leaf(&mut buf, leaf_addr, record_size, records);
        buf
    }

    /// Leaf node image: sig(4) version(1) type(1) then the fixed-size records.
    fn write_leaf(buf: &mut Vec<u8>, addr: usize, record_size: usize, records: &[Vec<u8>]) {
        put(buf, addr, SIG_BTREE_V2_LEAF);
        let mut p = addr + 6;
        for r in records {
            put(buf, p, r);
            p += record_size;
        }
    }

    /// Internal node image: sig(4) version(1) type(1), then all records, then
    /// `children.len()` node pointers (address + 1-byte record count + an
    /// optional 1-byte subtree total when `cum` is set).
    fn write_internal(
        buf: &mut Vec<u8>,
        addr: usize,
        record_size: usize,
        records: &[Vec<u8>],
        children: &[(u64, u8, u8)], // (address, record count, subtree total)
        cum: bool,
    ) {
        put(buf, addr, SIG_BTREE_V2_INTERNAL);
        let mut p = addr + 6;
        for r in records {
            put(buf, p, r);
            p += record_size;
        }
        for &(child_addr, child_nrec, child_total) in children {
            put(buf, p, &child_addr.to_le_bytes());
            p += 8;
            buf[p] = child_nrec;
            p += 1;
            if cum {
                buf[p] = child_total;
                p += 1;
            }
        }
    }

    /// The per-level geometry the reader derives, exposed so tests can assert it
    /// matches a hand computation and reuse it when laying out nodes.
    fn level_geometry(record_size: usize, depth: u16) -> BTreeV2Levels {
        BTreeV2Levels::compute(NODE_SIZE, record_size, 8, depth).unwrap()
    }

    #[test]
    fn reads_leaf_records() {
        let records = vec![vec![1u8; 11], vec![2u8; 11]];
        let buf = btree_v2(5, 11, &records);
        let (btype, got) = btree_v2_records(&buf, 0, 8, 8).unwrap();
        assert_eq!(btype, 5);
        assert_eq!(got, records);
    }

    #[test]
    fn count_field_byte_widths() {
        // floor(log2(max)) / 8 + 1, matching libhdf5's record-count sizing.
        assert_eq!(count_field_bytes(0), 1);
        assert_eq!(count_field_bytes(29), 1);
        assert_eq!(count_field_bytes(255), 1);
        assert_eq!(count_field_bytes(256), 2);
        assert_eq!(count_field_bytes(65_535), 2);
        assert_eq!(count_field_bytes(65_536), 3);
    }

    #[test]
    fn level_geometry_matches_hand_computation() {
        // node_size 512, record_size 200: every node holds two records and every
        // count/total field fits in one byte. Mirrors the depth-2 walk below.
        let g = level_geometry(200, 2);
        assert_eq!(g.max_nrec, vec![2, 2, 2]);
        assert_eq!(g.count_bytes, vec![1, 1, 1]);
        // Leaf level carries no subtree-total field; internal levels need one.
        assert_eq!(g.cum_bytes[1], 1);
        assert_eq!(g.cum_bytes[2], 1);
    }

    #[test]
    fn walks_depth_one_btree() {
        // Root internal node with one record between two leaves. In-order result
        // is leaf0's record, the root record, then leaf1's record.
        let rs = 11;
        let (leaf0, leaf1) = (0x100usize, 0x200usize);
        let mut buf = vec![0u8; 0x400];
        btree_v2_header(&mut buf, 8, rs, 1, 0x40, 1);
        write_internal(
            &mut buf,
            0x40,
            rs,
            &[vec![0xBB; rs]],
            &[(leaf0 as u64, 1, 0), (leaf1 as u64, 1, 0)],
            false, // children are leaves: no subtree-total field
        );
        write_leaf(&mut buf, leaf0, rs, &[vec![0xAA; rs]]);
        write_leaf(&mut buf, leaf1, rs, &[vec![0xCC; rs]]);

        let (_, got) = btree_v2_records(&buf, 0, 8, 8).unwrap();
        assert_eq!(got, vec![vec![0xAA; rs], vec![0xBB; rs], vec![0xCC; rs]]);
    }

    #[test]
    fn walks_depth_two_btree() {
        // Three levels, two records per node, exercising the subtree-total field
        // on the internal-to-internal pointers. Records are tagged with their
        // in-order rank so the flattened output must come back 0,1,2,…,8.
        let rs = 200;
        let rec = |n: u8| vec![n; rs];
        let (lo, mid_l, mid_r, root) = (0x1000usize, 0x400usize, 0x800usize, 0x40usize);
        let leaves = [lo, lo + 0x200, lo + 0x400]; // under mid_l
        let leaves_r = [lo + 0x600, lo + 0x800, lo + 0xA00]; // under mid_r
        let mut buf = vec![0u8; 0x2000];
        btree_v2_header(&mut buf, 8, rs, 2, root as u64, 1);
        // Root: record #5 between the two internal children.
        write_internal(
            &mut buf,
            root,
            rs,
            &[rec(5)],
            // Child record counts must match each subtree node; totals (5 and 3
            // records) are read past but not used.
            &[(mid_l as u64, 2, 5), (mid_r as u64, 1, 3)],
            true,
        );
        // Left internal: records #1, #3 between three leaves (#0, #2, #4).
        write_internal(
            &mut buf,
            mid_l,
            rs,
            &[rec(1), rec(3)],
            &[
                (leaves[0] as u64, 1, 0),
                (leaves[1] as u64, 1, 0),
                (leaves[2] as u64, 1, 0),
            ],
            false,
        );
        // Right internal: records #7 between two leaves (#6, #8).
        write_internal(
            &mut buf,
            mid_r,
            rs,
            &[rec(7)],
            &[(leaves_r[0] as u64, 1, 0), (leaves_r[1] as u64, 1, 0)],
            false,
        );
        write_leaf(&mut buf, leaves[0], rs, &[rec(0)]);
        write_leaf(&mut buf, leaves[1], rs, &[rec(2)]);
        write_leaf(&mut buf, leaves[2], rs, &[rec(4)]);
        write_leaf(&mut buf, leaves_r[0], rs, &[rec(6)]);
        write_leaf(&mut buf, leaves_r[1], rs, &[rec(8)]);

        let (_, got) = btree_v2_records(&buf, 0, 8, 8).unwrap();
        assert_eq!(got, (0u8..9).map(rec).collect::<Vec<_>>());
    }

    #[test]
    fn rejects_implausible_depth() {
        let mut buf = btree_v2(5, 11, &[]);
        put(&mut buf, 12, &1000u16.to_le_bytes()); // depth far past any real tree
        assert!(matches!(
            btree_v2_records(&buf, 0, 8, 8),
            Err(FieldglassError::Parse(_))
        ));
    }

    #[test]
    fn leaf_records_past_eof_error_without_panic() {
        let mut buf = btree_v2(5, 11, &[]);
        // Claim 1000 records but supply none.
        put(&mut buf, 24, &1000u16.to_le_bytes());
        assert!(btree_v2_records(&buf, 0, 8, 8).is_err());
    }

    #[test]
    fn rejects_implausible_node_size() {
        let mut buf = btree_v2(5, 11, &[]);
        put(&mut buf, 6, &u32::MAX.to_le_bytes()); // 4 GiB node size
        assert!(matches!(
            btree_v2_records(&buf, 0, 8, 8),
            Err(FieldglassError::Parse(_))
        ));
    }

    #[test]
    fn deep_geometry_saturates_without_overflow_panic() {
        // A large (but in-range) node with single-byte records and the deepest
        // allowed tree makes the subtree-total counts explode past u64; the
        // recurrence must saturate rather than overflow-panic in debug builds.
        let g = BTreeV2Levels::compute(MAX_BTREE_V2_NODE_SIZE, 1, 8, MAX_BTREE_V2_DEPTH).unwrap();
        // The deepest level's total field saturates to the 8-byte ceiling.
        assert_eq!(*g.cum_bytes.last().unwrap(), 8);
    }

    #[test]
    fn doubling_table_row_sizes() {
        // Rows 0 and 1 share the starting size; it doubles thereafter.
        assert_eq!(row_block_size(64, 0), 64);
        assert_eq!(row_block_size(64, 1), 64);
        assert_eq!(row_block_size(64, 2), 128);
        assert_eq!(row_block_size(64, 3), 256);
        assert_eq!(row_block_size(512, 5), 512 * 16);
    }

    // Heap geometry shared by the indirect-root fixture below (offset/length
    // sizes of 8, a 16-bit heap address space, table width 2, 64-byte blocks).
    const HEAP_ADDR: u64 = 0;
    const INDIRECT_ADDR: u64 = 0x100;
    const BLOCK0_ADDR: u64 = 0x200;
    const BLOCK1_ADDR: u64 = 0x300;
    const BLOCK_PREFIX: usize = 4 + 1 + 8 + 2; // FHDB sig + ver + heap addr + block offset

    /// A minimal fractal heap whose root is an *indirect* block: one row of two
    /// 64-byte direct blocks (`cur_rows = 1`, `table_width = 2`). Heap IDs are
    /// `flags(1) + offset(2) + length(2)`.
    fn frhp_indirect() -> Vec<u8> {
        let mut buf = vec![0u8; 0x400];
        // --- FRHP header ---
        put(&mut buf, 0, SIG_FRACTAL_HEAP);
        put(&mut buf, 5, &5u16.to_le_bytes()); // heap_id_len
        // io_filter_len(7)=0, flags(9)=0, max_managed(10..14)=0,
        // stats block 14..110 (l*10 + o*2 = 96), all zero.
        put(&mut buf, 110, &2u16.to_le_bytes()); // table_width
        put(&mut buf, 112, &64u64.to_le_bytes()); // starting_block_size
        put(&mut buf, 120, &65536u64.to_le_bytes()); // max_direct_block_size
        put(&mut buf, 128, &16u16.to_le_bytes()); // max_heap_size bits -> offset_bytes = 2
        // starting_rows(130)=0
        put(&mut buf, 132, &INDIRECT_ADDR.to_le_bytes()); // root_block_addr
        put(&mut buf, 140, &1u16.to_le_bytes()); // cur_rows

        // --- FHIB root indirect block: 2 direct-block addresses ---
        put(&mut buf, 0x100, SIG_FRACTAL_INDIRECT);
        put(&mut buf, 0x100 + 5, &HEAP_ADDR.to_le_bytes()); // heap header back-pointer
        // block offset (offset_bytes = 2) at 0x100+13 stays 0.
        put(&mut buf, 0x100 + 15, &BLOCK0_ADDR.to_le_bytes());
        put(&mut buf, 0x100 + 15 + 8, &BLOCK1_ADDR.to_le_bytes());

        // --- two FHDB direct blocks, each pointing back at the heap header ---
        for addr in [BLOCK0_ADDR, BLOCK1_ADDR] {
            let a = addr as usize;
            put(&mut buf, a, SIG_FRACTAL_DIRECT);
            put(&mut buf, a + 5, &HEAP_ADDR.to_le_bytes());
        }
        buf
    }

    #[test]
    fn indirect_root_dereferences_object_in_second_block() {
        let mut buf = frhp_indirect();
        // Place a 4-byte object in the second direct block (logical offset 64).
        let payload = [0xDEu8, 0xAD, 0xBE, 0xEF];
        let in_block = BLOCK_PREFIX; // first object slot after the block prefix
        put(&mut buf, BLOCK1_ADDR as usize + in_block, &payload);

        let heap = FractalHeap::parse(&buf, HEAP_ADDR, 8, 8).unwrap();
        // Heap offset = second block's logical start (64) + position within it.
        let offset = 64u64 + in_block as u64;
        let mut id = vec![0u8]; // flags: managed
        id.extend_from_slice(&(offset as u16).to_le_bytes()); // offset (2)
        id.extend_from_slice(&(payload.len() as u16).to_le_bytes()); // length (2)

        assert_eq!(heap.managed_object(&buf, &id).unwrap(), payload);
    }

    #[test]
    fn rejects_child_indirect_rows() {
        let mut buf = frhp_indirect();
        // Force the single row to exceed the max direct block size so its
        // entries would be child indirect blocks (unsupported).
        put(&mut buf, 120, &32u64.to_le_bytes()); // max_direct_block_size < starting (64)
        assert!(matches!(
            FractalHeap::parse(&buf, HEAP_ADDR, 8, 8),
            Err(FieldglassError::Parse(_))
        ));
    }
}
