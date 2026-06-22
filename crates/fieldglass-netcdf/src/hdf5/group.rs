//! HDF5 group + link-table traversal — enumerates a group's children (issue
//! #38, under #33). Given the root group's object header, follow its link
//! structures to list each child's name, object-header address, and kind.
//!
//! Two on-disk layouts are handled, matching the two bundled fixtures:
//!
//! * **Legacy symbol table** — a Symbol Table message (`0x0011`) points at a
//!   version-1 B-tree (`TREE`) of `SNOD` nodes plus a local heap (`HEAP`) that
//!   holds the link names.
//! * **Modern link info** — a Link Info message (`0x0002`) points at a fractal
//!   heap (`FRHP`) of link messages indexed by a version-2 B-tree (`BTHD`).
//!   Small groups instead store Link messages (`0x0006`) directly in the object
//!   header ("compact" storage); that case is handled too.
//!
//! Scope is the root group's immediate children (the NetCDF-4 acceptance
//! target). Decoding child *contents* is the next layer (#39/#40). Layouts the
//! fixtures don't exercise — multi-level B-trees, indirect fractal-heap blocks,
//! huge/tiny heap objects, I/O-filtered heaps — return a clear error rather than
//! risk a silent misread.
//!
//! Reference: HDF5 file format specification version 3, "Disk Format: Level 1"
//! <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use super::Hdf5Probe;
use super::heap::{self, Cursor, FractalHeap};
use super::object_header::{self, read_uint_le};
use fieldglass_core::FieldglassError;

// Object-header message types consulted here.
const MSG_DATASPACE: u16 = 0x0001;
const MSG_LINK_INFO: u16 = 0x0002;
const MSG_DATATYPE: u16 = 0x0003;
const MSG_LINK: u16 = 0x0006;
const MSG_SYMBOL_TABLE: u16 = 0x0011;

// On-disk structure signatures owned by this module.
const SIG_LOCAL_HEAP: &[u8; 4] = b"HEAP";
const SIG_BTREE_V1: &[u8; 4] = b"TREE";
const SIG_SNOD: &[u8; 4] = b"SNOD";

/// Link-name B-tree v2 record: `hash(4)` then the fractal-heap ID.
const LINK_RECORD_HEAP_ID_OFFSET: usize = 4;

/// Upper bound on children enumerated from one group — guards malformed counts.
const MAX_CHILDREN: usize = 1 << 20;
/// Upper bound on B-tree v1 nodes visited — guards cyclic sibling/child links.
const MAX_BTREE_NODES: usize = 4096;

/// What an object header turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    Group,
    Dataset,
    CommittedDatatype,
}

/// A child object of a group: its link name, the address of its object header,
/// and its classified kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChild {
    pub name: String,
    pub object_header_address: u64,
    pub kind: ChildKind,
}

/// Enumerate the root group's immediate children, sorted by name.
pub fn list_root_children(
    bytes: &[u8],
    probe: &Hdf5Probe,
) -> Result<Vec<GroupChild>, FieldglassError> {
    let root = super::root_group_address(bytes, probe)?;
    let osize = probe.offset_size;
    let lsize = probe.length_size;
    let header = object_header::walk(bytes, root, osize, lsize)?;

    // A group uses exactly one of the two link layouts. Symbol Table wins if
    // present (legacy files); otherwise Link Info drives the modern path.
    let mut links: Vec<(String, u64)> = if let Some(msg) = header
        .messages
        .iter()
        .find(|m| m.msg_type == MSG_SYMBOL_TABLE)
    {
        symbol_table_links(bytes, &msg.body, osize, lsize)?
    } else if let Some(msg) = header.messages.iter().find(|m| m.msg_type == MSG_LINK_INFO) {
        link_info_links(bytes, &header, &msg.body, osize, lsize)?
    } else {
        Vec::new()
    };

    links.sort_by(|a, b| a.0.cmp(&b.0));
    links
        .into_iter()
        .map(|(name, addr)| {
            let kind = classify(bytes, addr, osize, lsize)?;
            Ok(GroupChild {
                name,
                object_header_address: addr,
                kind,
            })
        })
        .collect()
}

/// Classify a child by walking its object header and inspecting message types.
fn classify(bytes: &[u8], addr: u64, osize: u8, lsize: u8) -> Result<ChildKind, FieldglassError> {
    let header = object_header::walk(bytes, addr, osize, lsize)?;
    let has = |t: u16| header.messages.iter().any(|m| m.msg_type == t);
    // Groups carry link structures; datasets carry a dataspace; a committed
    // datatype is a bare datatype with no dataspace.
    if has(MSG_SYMBOL_TABLE) || has(MSG_LINK_INFO) {
        Ok(ChildKind::Group)
    } else if has(MSG_DATASPACE) {
        Ok(ChildKind::Dataset)
    } else if has(MSG_DATATYPE) {
        Ok(ChildKind::CommittedDatatype)
    } else {
        // No distinguishing message — treat as a group (the container default).
        Ok(ChildKind::Group)
    }
}

// ---------------------------------------------------------------------------
// Legacy symbol-table path
// ---------------------------------------------------------------------------

/// Resolve links from a Symbol Table message body: B-tree v1 address + local
/// heap address.
fn symbol_table_links(
    bytes: &[u8],
    body: &[u8],
    osize: u8,
    lsize: u8,
) -> Result<Vec<(String, u64)>, FieldglassError> {
    let o = osize as usize;
    if body.len() < 2 * o {
        return Err(FieldglassError::Parse(
            "symbol table message too small".into(),
        ));
    }
    let btree_addr = read_uint_le(body, 0, o)?;
    let heap_addr = read_uint_le(body, o, o)?;
    let heap_data = local_heap_data_segment(bytes, heap_addr, osize, lsize)?;

    let mut snods = Vec::new();
    collect_snods(bytes, btree_addr, osize, &mut snods)?;

    let mut links = Vec::new();
    for snod in snods {
        read_snod(bytes, snod, heap_data, osize, &mut links)?;
    }
    Ok(links)
}

/// Read a local heap and return the file offset of its data segment.
fn local_heap_data_segment(
    bytes: &[u8],
    addr: u64,
    osize: u8,
    lsize: u8,
) -> Result<u64, FieldglassError> {
    let mut cur = Cursor::at(bytes, addr)?;
    cur.tag(SIG_LOCAL_HEAP)?;
    cur.skip(4)?; // version (1) + reserved (3)
    cur.uint(lsize as usize)?; // data segment size
    cur.uint(lsize as usize)?; // free-list head offset
    cur.uint(osize as usize) // address of data segment
}

/// Walk a version-1 B-tree group node, collecting the addresses of its leaf
/// `SNOD` nodes. Traversal is iterative with an explicit work-list (not native
/// recursion) and bounded by [`MAX_BTREE_NODES`], so a malformed or cyclic tree
/// terminates with an error rather than overflowing the stack.
fn collect_snods(
    bytes: &[u8],
    addr: u64,
    osize: u8,
    out: &mut Vec<u64>,
) -> Result<(), FieldglassError> {
    let o = osize as usize;
    let mut pending = vec![addr];
    let mut visited = 0usize;
    while let Some(node_addr) = pending.pop() {
        visited += 1;
        if visited > MAX_BTREE_NODES {
            return Err(FieldglassError::Parse(
                "B-tree v1 too large or cyclic".into(),
            ));
        }
        let mut cur = Cursor::at(bytes, node_addr)?;
        cur.tag(SIG_BTREE_V1)?;
        let node_type = cur.byte()?;
        if node_type != 0 {
            return Err(FieldglassError::Parse(format!(
                "expected B-tree v1 group node, got node type {node_type}"
            )));
        }
        let level = cur.byte()?;
        let entries = cur.u16()? as usize;
        cur.skip(2 * o)?; // left + right sibling addresses
        // Keys and child pointers interleave: key, child, key, child, …, key. We
        // only need the child pointers; leaves hold SNOD addresses, internal
        // nodes hold child B-tree nodes.
        for _ in 0..entries {
            cur.uint(o)?; // key (byte offset into the local heap)
            let child = cur.uint(o)?;
            if level == 0 {
                if out.len() >= MAX_CHILDREN {
                    return Err(FieldglassError::Parse(
                        "B-tree v1 has too many nodes".into(),
                    ));
                }
                out.push(child);
            } else {
                pending.push(child);
            }
        }
    }
    Ok(())
}

/// Read a symbol-table node's entries, resolving names from the heap data
/// segment.
fn read_snod(
    bytes: &[u8],
    addr: u64,
    heap_data: u64,
    osize: u8,
    out: &mut Vec<(String, u64)>,
) -> Result<(), FieldglassError> {
    let o = osize as usize;
    let mut cur = Cursor::at(bytes, addr)?;
    cur.tag(SIG_SNOD)?;
    cur.skip(2)?; // version (1) + reserved (1)
    let count = cur.u16()? as usize;
    for _ in 0..count {
        let name_offset = cur.uint(o)?;
        let oh_addr = cur.uint(o)?;
        cur.skip(4 + 4 + 16)?; // cache type + reserved + scratch-pad
        let name = read_heap_name(bytes, heap_data, name_offset)?;
        push_link(out, name, oh_addr)?;
    }
    Ok(())
}

/// Read a null-terminated link name from the local heap data segment.
fn read_heap_name(bytes: &[u8], heap_data: u64, offset: u64) -> Result<String, FieldglassError> {
    let start = heap::checked_add(heap_data, offset)?;
    let start = usize::try_from(start)
        .map_err(|_| FieldglassError::Parse("heap name offset too large".into()))?;
    let tail = bytes
        .get(start..)
        .ok_or_else(|| FieldglassError::Parse("heap name offset past end of file".into()))?;
    let end = tail
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| FieldglassError::Parse("unterminated heap name".into()))?;
    decode_name(&tail[..end])
}

// ---------------------------------------------------------------------------
// Modern link-info path
// ---------------------------------------------------------------------------

/// Resolve links from a Link Info message: either compact Link messages in the
/// header, or a fractal heap indexed by a version-2 B-tree.
fn link_info_links(
    bytes: &[u8],
    header: &object_header::ObjectHeader,
    body: &[u8],
    osize: u8,
    lsize: u8,
) -> Result<Vec<(String, u64)>, FieldglassError> {
    let o = osize as usize;
    // version (1) + flags (1), then an optional max-creation-index (length-size).
    if body.len() < 2 {
        return Err(FieldglassError::Parse("link info message too small".into()));
    }
    let flags = body[1];
    let mut pos = 2usize;
    if flags & 0x01 != 0 {
        pos += lsize as usize; // maximum creation index
    }
    let heap_addr = read_uint_le(body, pos, o)?;

    // Undefined fractal-heap address ⇒ links are stored compactly in the header.
    if is_undefined(heap_addr, osize) {
        let mut links = Vec::new();
        for msg in header.messages.iter().filter(|m| m.msg_type == MSG_LINK) {
            if let Some(link) = parse_link_message(&msg.body, osize)? {
                push_link(&mut links, link.0, link.1)?;
            }
        }
        return Ok(links);
    }

    let btree_addr = read_uint_le(body, pos + o, o)?;
    let heap = FractalHeap::parse(bytes, heap_addr, osize, lsize)?;
    let (btree_type, records) = heap::btree_v2_records(bytes, btree_addr, osize, lsize)?;
    if btree_type != 5 && btree_type != 6 {
        return Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {btree_type} for links"
        )));
    }

    let mut links = Vec::new();
    for record in records {
        let id = record
            .get(LINK_RECORD_HEAP_ID_OFFSET..LINK_RECORD_HEAP_ID_OFFSET + heap.heap_id_len)
            .ok_or_else(|| FieldglassError::Parse("link record too small for a heap ID".into()))?;
        let object = heap.managed_object(bytes, id)?;
        if let Some(link) = parse_link_message(&object, osize)? {
            push_link(&mut links, link.0, link.1)?;
        }
    }
    Ok(links)
}

/// Parse a Link message body, returning `(name, object_header_address)` for hard
/// links. Soft/external links (which target a path, not an object header) are
/// skipped by returning `None`.
fn parse_link_message(body: &[u8], osize: u8) -> Result<Option<(String, u64)>, FieldglassError> {
    let mut cur = Cursor::over(body);
    cur.skip(1)?; // version
    let flags = cur.byte()?;
    let link_type = if flags & 0x08 != 0 { cur.byte()? } else { 0 };
    if flags & 0x04 != 0 {
        cur.skip(8)?; // creation order
    }
    if flags & 0x10 != 0 {
        cur.skip(1)?; // link name character set
    }
    let name_len_width = 1usize << (flags & 0x03);
    let name_len = cur.uint(name_len_width)? as usize;
    let name = decode_name(cur.take(name_len)?)?;
    if link_type != 0 {
        return Ok(None); // not a hard link → no object-header target
    }
    let addr = cur.uint(osize as usize)?;
    Ok(Some((name, addr)))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn push_link(out: &mut Vec<(String, u64)>, name: String, addr: u64) -> Result<(), FieldglassError> {
    if out.len() >= MAX_CHILDREN {
        return Err(FieldglassError::Parse("group has too many links".into()));
    }
    out.push((name, addr));
    Ok(())
}

fn decode_name(raw: &[u8]) -> Result<String, FieldglassError> {
    String::from_utf8(raw.to_vec())
        .map_err(|_| FieldglassError::Parse("link name is not valid UTF-8".into()))
}

/// Whether an address field is the HDF5 "undefined address" sentinel (all ones).
fn is_undefined(address: u64, osize: u8) -> bool {
    let o = osize as usize;
    if o >= 8 {
        address == u64::MAX
    } else {
        address == (1u64 << (8 * o)) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overwrite `buf` at `at` with `data`, growing the buffer as needed.
    fn put(buf: &mut Vec<u8>, at: usize, data: &[u8]) {
        if buf.len() < at + data.len() {
            buf.resize(at + data.len(), 0);
        }
        buf[at..at + data.len()].copy_from_slice(data);
    }

    #[test]
    fn parses_hard_link_message() {
        // version(1) flags(0 → 1-byte name length) name_len(1)=4 name addr(8).
        let mut body = vec![1u8, 0x00, 4];
        body.extend_from_slice(b"node");
        body.extend_from_slice(&0x1234u64.to_le_bytes());
        let link = parse_link_message(&body, 8).unwrap().unwrap();
        assert_eq!(link, ("node".to_string(), 0x1234));
    }

    #[test]
    fn skips_soft_link_message() {
        // flags bit3 set ⇒ explicit link type; type 1 (soft) is not a hard link.
        let mut body = vec![1u8, 0x08, 1, 4];
        body.extend_from_slice(b"soft");
        assert!(parse_link_message(&body, 8).unwrap().is_none());
    }

    #[test]
    fn walks_symbol_table_group() {
        let mut buf = vec![0u8; 0x500];
        // Heap data segment with two null-terminated names.
        put(&mut buf, 0x200, b"alpha\0");
        put(&mut buf, 0x206, b"beta\0");
        // Local heap header pointing at the data segment.
        put(&mut buf, 0x100, SIG_LOCAL_HEAP);
        put(&mut buf, 0x118, &0x200u64.to_le_bytes()); // data segment address
        // B-tree v1 group node: one leaf entry → the SNOD.
        put(&mut buf, 0x300, SIG_BTREE_V1);
        put(&mut buf, 0x306, &1u16.to_le_bytes()); // entries used
        put(&mut buf, 0x308, &u64::MAX.to_le_bytes()); // left sibling
        put(&mut buf, 0x310, &u64::MAX.to_le_bytes()); // right sibling
        put(&mut buf, 0x320, &0x400u64.to_le_bytes()); // child0 → SNOD (after key0)
        // SNOD with two entries (name offset + object-header address).
        put(&mut buf, 0x400, SIG_SNOD);
        buf[0x404] = 1; // version
        put(&mut buf, 0x406, &2u16.to_le_bytes()); // symbol count
        put(&mut buf, 0x408, &0u64.to_le_bytes()); // entry0 name offset → "alpha"
        put(&mut buf, 0x410, &0xAAAAu64.to_le_bytes()); // entry0 OH address
        put(&mut buf, 0x430, &6u64.to_le_bytes()); // entry1 name offset → "beta"
        put(&mut buf, 0x438, &0xBBBBu64.to_le_bytes()); // entry1 OH address

        let mut body = Vec::new();
        body.extend_from_slice(&0x300u64.to_le_bytes()); // B-tree v1 address
        body.extend_from_slice(&0x100u64.to_le_bytes()); // local heap address
        let links = symbol_table_links(&buf, &body, 8, 8).unwrap();
        assert_eq!(
            links,
            vec![("alpha".to_string(), 0xAAAA), ("beta".to_string(), 0xBBBB)]
        );
    }

    #[test]
    fn rejects_bad_local_heap_signature() {
        let buf = vec![0u8; 64];
        let err = local_heap_data_segment(&buf, 0, 8, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn rejects_cyclic_btree_v1() {
        // An internal node (level 1) whose only child points back at itself must
        // terminate via the visit budget rather than recursing forever.
        let mut buf = vec![0u8; 64];
        put(&mut buf, 0, SIG_BTREE_V1);
        buf[5] = 1; // level 1 (internal)
        put(&mut buf, 6, &1u16.to_le_bytes()); // one entry
        // key0 @24, child0 @32 left at 0 → self-reference.
        let mut out = Vec::new();
        let err = collect_snods(&buf, 0, 8, &mut out).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn address_past_eof_errors_without_panic() {
        let buf = vec![0u8; 16];
        assert!(Cursor::at(&buf, 4096).is_err());
        let mut body = Vec::new();
        body.extend_from_slice(&4096u64.to_le_bytes());
        body.extend_from_slice(&4096u64.to_le_bytes());
        assert!(symbol_table_links(&buf, &body, 8, 8).is_err());
    }

    #[test]
    fn undefined_address_detection() {
        assert!(is_undefined(u64::MAX, 8));
        assert!(is_undefined(0xFFFF, 2));
        assert!(!is_undefined(0x1234, 8));
    }
}
