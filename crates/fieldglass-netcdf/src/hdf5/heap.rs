//! Shared HDF5 fractal-heap + version-2 B-tree readers, plus the little-endian
//! byte cursor they're built on. These back both the dense-link path (group
//! traversal, #38) and the dense-attribute path (#40): in each case a B-tree v2
//! indexes records that carry a fractal-heap ID, and the heap dereferences that
//! ID to the actual message bytes.
//!
//! A single-level B-tree (depth 0) is the only B-tree layout implemented. For
//! the fractal heap, both a single root direct block and a root *indirect* block
//! (the doubling table that real attribute-rich files spill into once their
//! dense storage outgrows one direct block) are handled, but only while every
//! row holds direct blocks — child indirect blocks, huge/tiny objects, and I/O
//! filters return a clear error rather than risk a silent misread.
//!
//! Reference: HDF5 file format specification version 3, "Fractal Heap" and
//! "Version 2 B-trees".

use super::object_header::{is_undefined_address, read_uint_le};
use fieldglass_core::FieldglassError;

const SIG_BTREE_V2_HDR: &[u8; 4] = b"BTHD";
const SIG_BTREE_V2_LEAF: &[u8; 4] = b"BTLF";
const SIG_FRACTAL_HEAP: &[u8; 4] = b"FRHP";
const SIG_FRACTAL_DIRECT: &[u8; 4] = b"FHDB";
const SIG_FRACTAL_INDIRECT: &[u8; 4] = b"FHIB";

/// Guards a malformed row count in a root indirect block.
const MAX_HEAP_ROWS: usize = 64;

/// Guards a malformed doubling-table width (columns per row).
const MAX_HEAP_TABLE_WIDTH: usize = 1024;

/// Upper bound on B-tree v2 leaf records — guards a malformed record count.
const MAX_BTREE_V2_RECORDS: usize = 1 << 20;

/// Read the records of a single-level (depth 0) version-2 B-tree, returning the
/// B-tree `type` and the raw record bytes. Callers slice the fractal-heap ID out
/// of each record at the offset their record type dictates (link-name records
/// put it after a 4-byte hash; attribute-name records put it first).
pub(crate) fn btree_v2_leaf_records(
    bytes: &[u8],
    addr: u64,
    osize: u8,
    lsize: u8,
) -> Result<(u8, Vec<Vec<u8>>), FieldglassError> {
    let mut hdr = Cursor::at(bytes, addr)?;
    hdr.tag(SIG_BTREE_V2_HDR)?;
    hdr.skip(1)?; // version
    let btree_type = hdr.byte()?;
    hdr.uint(4)?; // node size
    let record_size = hdr.u16()? as usize;
    let depth = hdr.u16()?;
    if depth != 0 {
        return Err(FieldglassError::Parse(
            "multi-level B-tree v2 not supported".into(),
        ));
    }
    hdr.skip(2)?; // split + merge percent
    let root_addr = hdr.uint(osize as usize)?;
    let root_nrec = hdr.u16()? as usize;
    let _total = hdr.uint(lsize as usize)?;

    if root_nrec > MAX_BTREE_V2_RECORDS {
        return Err(FieldglassError::Parse(
            "implausible B-tree v2 record count".into(),
        ));
    }

    let mut leaf = Cursor::at(bytes, root_addr)?;
    leaf.tag(SIG_BTREE_V2_LEAF)?;
    leaf.skip(2)?; // version + type
    let mut records = Vec::with_capacity(root_nrec);
    for _ in 0..root_nrec {
        records.push(leaf.take(record_size)?.to_vec());
    }
    Ok((btree_type, records))
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

    /// Build a depth-0 B-tree v2 header + leaf carrying `records`.
    fn btree_v2(btree_type: u8, record_size: usize, records: &[Vec<u8>]) -> Vec<u8> {
        let header_addr = 0usize;
        let leaf_addr = 0x100usize;
        let mut buf = vec![0u8; 0x200];
        // Header: sig(4) version(1) type(1) node_size(4) record_size(2) depth(2)
        //         split/merge(2) root_addr(8) root_nrec(2) total(8) checksum(4).
        put(&mut buf, header_addr, SIG_BTREE_V2_HDR);
        buf[header_addr + 5] = btree_type;
        put(
            &mut buf,
            header_addr + 10,
            &(record_size as u16).to_le_bytes(),
        );
        // depth (offset 12) stays 0; split/merge percent at 14-15.
        put(
            &mut buf,
            header_addr + 16,
            &(leaf_addr as u64).to_le_bytes(),
        );
        put(
            &mut buf,
            header_addr + 24,
            &(records.len() as u16).to_le_bytes(),
        );
        // Leaf: sig(4) version(1) type(1) then records.
        put(&mut buf, leaf_addr, SIG_BTREE_V2_LEAF);
        let mut p = leaf_addr + 6;
        for r in records {
            put(&mut buf, p, r);
            p += record_size;
        }
        buf
    }

    #[test]
    fn reads_leaf_records() {
        let records = vec![vec![1u8; 11], vec![2u8; 11]];
        let buf = btree_v2(5, 11, &records);
        let (btype, got) = btree_v2_leaf_records(&buf, 0, 8, 8).unwrap();
        assert_eq!(btype, 5);
        assert_eq!(got, records);
    }

    #[test]
    fn rejects_multi_level_btree() {
        let mut buf = btree_v2(5, 11, &[]);
        put(&mut buf, 12, &1u16.to_le_bytes()); // depth = 1
        let err = btree_v2_leaf_records(&buf, 0, 8, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn leaf_records_past_eof_error_without_panic() {
        let mut buf = btree_v2(5, 11, &[]);
        // Claim 1000 records but supply none.
        put(&mut buf, 24, &1000u16.to_le_bytes());
        assert!(btree_v2_leaf_records(&buf, 0, 8, 8).is_err());
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
