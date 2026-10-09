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
//! [`list_root_children`] enumerates one group's immediate children;
//! [`list_all_children`] descends the whole tree, presenting objects in nested
//! groups with path-qualified names (#219). Decoding child *contents* is the
//! next layer (#39/#40). Layouts the fixtures don't exercise — multi-level
//! B-trees, indirect fractal-heap blocks, huge/tiny heap objects, I/O-filtered
//! heaps — return a clear error rather than risk a silent misread.
//!
//! Reference: HDF5 file format specification version 3, "Disk Format: Level 1"
//! <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use super::Hdf5Probe;
use super::heap::{self, FractalHeap};
use super::object_header::{self, read_uint_le};
use super::source::{ClaimedRanges, Cursor, Fields, FileCursor, read_up_to, scan_windows};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::ByteSource;
use std::collections::HashSet;

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

/// Ceiling on a link name read out of a local heap.
///
/// A heap name is null-terminated and its length is stated nowhere, so the
/// reader has to look for the terminator. Before the byte-access seam that
/// search ran to the end of the file image, which was free; through a
/// transport it would fetch the whole object to fail. netCDF caps a name at
/// `NC_MAX_NAME` (256 bytes) and HDF5 link names are of the same order, so this
/// is several hundred times any real name and still a bounded read.
const MAX_LINK_NAME_BYTES: usize = 64 << 10;

/// What an object header turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    /// A group — descend into it for more children.
    Group,
    /// A dataset — a NetCDF variable, or a pure dimension scale.
    Dataset,
    /// A named datatype. Not a variable, and not descended into.
    CommittedDatatype,
}

/// A child object of a group: its link name, the address of its object header,
/// and its classified kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChild {
    /// The child's link name within its parent group.
    pub name: String,
    /// Byte offset of the child's object header, which is also the identity a
    /// `DIMENSION_LIST` reference resolves against.
    pub object_header_address: u64,
    /// What the object header turned out to be.
    pub kind: ChildKind,
}

/// Enumerate the root group's immediate children, sorted by name.
pub fn list_root_children<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<Vec<GroupChild>, FieldglassError> {
    let root = super::root_group_address(source, probe)?;
    list_group_children(source, root, probe)
}

/// Enumerate one group's immediate children (by object-header address), sorted
/// by name. The building block for both the root listing and the recursive
/// descendant walk ([`list_all_children`]).
pub fn list_group_children<S: ByteSource + ?Sized>(
    source: &S,
    group_addr: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<GroupChild>, FieldglassError> {
    let osize = probe.offset_size;
    let lsize = probe.length_size;
    let header = probe.header(source, group_addr)?;

    // A group uses exactly one of the two link layouts. Symbol Table wins if
    // present (legacy files); otherwise Link Info drives the modern path.
    let mut links: Vec<(String, u64)> = if let Some(msg) = header
        .messages
        .iter()
        .find(|m| m.msg_type == MSG_SYMBOL_TABLE)
    {
        symbol_table_links(source, &msg.body, osize, lsize)?
    } else if let Some(msg) = header.messages.iter().find(|m| m.msg_type == MSG_LINK_INFO) {
        link_info_links(source, &header, &msg.body, osize, lsize)?
    } else {
        Vec::new()
    };

    links.sort_by(|a, b| a.0.cmp(&b.0));
    links
        .into_iter()
        .map(|(name, addr)| {
            let kind = classify(source, addr, probe)?;
            Ok(GroupChild {
                name,
                object_header_address: addr,
                kind,
            })
        })
        .collect()
}

/// Enumerate every dataset and committed datatype reachable from the root group,
/// descending into child groups, with **path-qualified** names. A root-group
/// child keeps its bare name (`temp`); a child of group `G` is named `/G/temp`,
/// and a grandchild `/G/H/temp` — the netCDF path convention (#219). Groups
/// themselves are not emitted (they are containers, not variables), but they are
/// recursed into.
///
/// Depth-first, pre-order, children name-sorted at every level, so the order is
/// deterministic and — for a file with no nested groups — identical to
/// [`list_root_children`]. That keeps the emitted order (and hence each
/// dataset's decode index) stable and shared between metadata resolution and the
/// decode path, which both walk this list.
///
/// A `visited` set of object-header addresses guards against a cyclic hard-link
/// graph, and a running count against a maliciously wide tree.
///
/// The traversal is an explicit frame stack, not native recursion: a
/// pathologically deep group chain (thousands of nested groups, each tiny on
/// disk) would otherwise overflow the call stack. Each frame holds one group's
/// name-sorted children, a cursor into them, and that group's path prefix;
/// `visited` bounds the total number of groups the stack can hold.
pub fn list_all_children<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<Vec<GroupChild>, FieldglassError> {
    all_children(source, probe).map(|c| (*c).clone())
}

/// [`list_all_children`] without the copy, memoised on the probe (#414).
///
/// The walk is `O(datasets)` object headers and every metadata or decode call
/// starts from it, so it is done once per file. Callers inside the crate take
/// the shared `Arc`; the public function above hands out an owned clone to keep
/// its signature.
pub(crate) fn all_children<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<std::sync::Arc<Vec<GroupChild>>, FieldglassError> {
    probe
        .cache()
        .children(source, || walk_all_children(source, probe))
}

fn walk_all_children<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<Vec<GroupChild>, FieldglassError> {
    /// One group's in-progress traversal: its children, the next to visit, and
    /// the path prefix (`""` for the root, `/G` for a nested group `G`).
    struct Frame {
        children: Vec<GroupChild>,
        next: usize,
        prefix: String,
    }

    let root = super::root_group_address(source, probe)?;
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(root);
    let mut stack = vec![Frame {
        children: list_group_children(source, root, probe)?,
        next: 0,
        prefix: String::new(),
    }];

    while let Some(frame) = stack.last_mut() {
        let Some(child) = frame.children.get(frame.next).cloned() else {
            stack.pop();
            continue;
        };
        frame.next += 1;
        let prefix = frame.prefix.clone();
        // The `frame` borrow ends here (below only touches `stack` as a whole).

        if out.len() >= MAX_CHILDREN {
            return Err(FieldglassError::Parse(
                "HDF5 group tree has too many objects".into(),
            ));
        }
        // Root-group children keep their bare name (`temp`); a child of a nested
        // group is qualified by that group's leading-slash path (`/G/temp`).
        let path = format!("{prefix}/{}", child.name);
        match child.kind {
            ChildKind::Group => {
                // A hard-link cycle (or a shared subgroup reached twice) would
                // otherwise loop forever / double-count; skip an already-seen
                // group object. `path` (e.g. `/G`) becomes the child prefix.
                if visited.insert(child.object_header_address) {
                    let children = list_group_children(source, child.object_header_address, probe)?;
                    stack.push(Frame {
                        children,
                        next: 0,
                        prefix: path,
                    });
                }
            }
            ChildKind::Dataset | ChildKind::CommittedDatatype => {
                let name = if prefix.is_empty() {
                    child.name.clone()
                } else {
                    path
                };
                out.push(GroupChild { name, ..child });
            }
        }
    }
    Ok(out)
}

/// Classify a child by walking its object header and inspecting message types.
fn classify<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    probe: &Hdf5Probe,
) -> Result<ChildKind, FieldglassError> {
    let header = probe.header(source, addr)?;
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
fn symbol_table_links<S: ByteSource + ?Sized>(
    source: &S,
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
    let heap_data = local_heap_data_segment(source, heap_addr, osize, lsize)?;

    // Every B-tree node and SNOD is claimed by its range of the file, and
    // every name by its range of the heap, so none is read twice (#901): one
    // SNOD named 8,192 times, its entries all naming one 60 KB name, made a
    // 197 KB file use 3.85 GB. A valid group gives each its own storage.
    let mut nodes = ClaimedRanges::default();
    let mut names = ClaimedRanges::default();
    let mut snods = Vec::new();
    collect_snods(source, btree_addr, osize, lsize, &mut snods, &mut nodes)?;

    let mut links = Vec::new();
    for snod in snods {
        read_snod(
            source,
            snod,
            heap_data,
            osize,
            lsize,
            &mut links,
            (&mut nodes, &mut names),
        )?;
    }
    Ok(links)
}

/// A local heap's data segment: where it is in the file and how long it is.
/// A name is an offset into it, and lies inside it (#908).
#[derive(Debug, Clone, Copy)]
struct HeapSegment {
    address: u64,
    size: u64,
}

/// A symbol-table entry's cache type for a symbolic (soft) link.
const SYMBOL_TABLE_SOFT_LINK: u64 = 2;

/// Read a local heap's header and return its data segment.
fn local_heap_data_segment<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    osize: u8,
    lsize: u8,
) -> Result<HeapSegment, FieldglassError> {
    let mut cur = FileCursor::at(source, addr)?;
    cur.tag(SIG_LOCAL_HEAP)?;
    cur.skip(4)?; // version (1) + reserved (3)
    let size = cur.uint(lsize as usize)?; // data segment size
    cur.uint(lsize as usize)?; // free-list head offset
    let address = cur.uint(osize as usize)?; // address of data segment
    Ok(HeapSegment { address, size })
}

/// Walk a version-1 B-tree group node, collecting the addresses of its leaf
/// `SNOD` nodes. Traversal is iterative with an explicit work-list (not native
/// recursion) and bounded by [`MAX_BTREE_NODES`], so a malformed or cyclic tree
/// terminates with an error rather than overflowing the stack.
fn collect_snods<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    osize: u8,
    lsize: u8,
    out: &mut Vec<u64>,
    nodes: &mut ClaimedRanges,
) -> Result<(), FieldglassError> {
    let o = osize as usize;
    let l = lsize as usize;
    let mut pending = vec![addr];
    let mut visited = 0usize;
    while let Some(node_addr) = pending.pop() {
        visited += 1;
        if visited > MAX_BTREE_NODES {
            return Err(FieldglassError::Parse(
                "B-tree v1 too large or cyclic".into(),
            ));
        }
        let mut cur = FileCursor::at(source, node_addr)?;
        cur.tag(SIG_BTREE_V1)?;
        let node_type = cur.byte()?;
        if node_type != 0 {
            return Err(FieldglassError::Parse(format!(
                "expected B-tree v1 group node, got node type {node_type}"
            )));
        }
        let level = cur.byte()?;
        let entries = cur.u16()? as usize;
        // The node's bytes: signature, type, level and count (8), two sibling
        // addresses, then `entries` key/child pairs and a closing key. A key
        // is a local-heap offset, Size of Lengths wide (specification,
        // "Version 1 B-trees"; libhdf5 `H5G_node_decode_key`), and a child an
        // address (#922).
        let node_len = (8 + 2 * o + entries * (l + o) + l) as u64;
        nodes.claim(node_addr, node_len, "group B-tree node")?;
        cur.skip(2 * o)?; // left + right sibling addresses
        // Keys and child pointers interleave: key, child, key, child, …, key. We
        // only need the child pointers; leaves hold SNOD addresses, internal
        // nodes hold child B-tree nodes.
        for _ in 0..entries {
            cur.uint(l)?; // key (byte offset into the local heap)
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
///
/// `claims` are the group's node ranges in the file and name ranges in the
/// heap; this SNOD and each name it reads are claimed in them (#901).
fn read_snod<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    heap_data: HeapSegment,
    osize: u8,
    lsize: u8,
    out: &mut Vec<(String, u64)>,
    (nodes, names): (&mut ClaimedRanges, &mut ClaimedRanges),
) -> Result<(), FieldglassError> {
    let o = osize as usize;
    let l = lsize as usize;
    let mut cur = FileCursor::at(source, addr)?;
    cur.tag(SIG_SNOD)?;
    cur.skip(2)?; // version (1) + reserved (1)
    let count = cur.u16()? as usize;
    // Signature, version, reserved and count (8), then `count` entries of a
    // name offset, a header address, cache type, reserved and scratch-pad.
    // The name offset is Size of Lengths wide, as libhdf5 reads and writes it
    // (`H5G_ent_decode`), like every other local-heap offset; the
    // specification's "Symbol Table Entry" table marks it Size of Offsets.
    // The two agree unless the sizes differ, and libhdf5 writes these files
    // (#922).
    nodes.claim(addr, (8 + count * (l + o + 24)) as u64, "symbol-table node")?;
    for _ in 0..count {
        let name_offset = cur.uint(l)?;
        let oh_addr = cur.uint(o)?;
        let cache_type = cur.uint(4)?;
        cur.skip(4 + 16)?; // reserved + scratch-pad
        // Cache type 2 is a symbolic link (HDF5 File Format Specification,
        // "Symbol Table Entry"): its header address is undefined and the
        // scratch-pad holds its target's offset in the heap. Hard links are
        // what a listing names, as `parse_link_message` skips a soft one in
        // the newer link messages, so it is skipped unread (#914). Reading an
        // object header at its undefined address failed the whole group.
        if cache_type == SYMBOL_TABLE_SOFT_LINK {
            continue;
        }
        // Its first byte before it is read, so a repeated offset is refused
        // without reading the name again; then the rest and its terminator.
        names.claim(name_offset, 1, "group member name")?;
        let name = read_heap_name(source, heap_data, name_offset)?;
        if !name.is_empty() {
            names.claim(name_offset + 1, name.len() as u64, "group member name")?;
        }
        push_link(out, name, oh_addr)?;
    }
    Ok(())
}

/// Read a null-terminated link name from the local heap data segment.
///
/// The name's length is not stated anywhere, so this reads a window and looks
/// for the terminator in it, widening the window only when it is not there.
/// Two bounds, for two different hazards. [`MAX_LINK_NAME_BYTES`] stops a name
/// with no terminator from being scanned to the end of the file, which over a
/// transport would fetch the whole object to fail. Starting small stops the
/// *ordinary* case — a group with many links, each named a dozen bytes — from
/// fetching the ceiling once per link, which is the fixed-large-window mistake
/// [`source`](super::source) exists to avoid.
fn read_heap_name<S: ByteSource + ?Sized>(
    source: &S,
    heap: HeapSegment,
    offset: u64,
) -> Result<String, FieldglassError> {
    // The name lies inside the heap's data segment (HDF5 File Format
    // Specification, "Local Heap"; #908). An offset at or past its end names
    // nothing, and the scan reads no further than the end. libhdf5 refuses the
    // first ("unable to offset into local heap data block") but accepts a name
    // whose terminator falls past the end; the spec puts the whole string in
    // the segment, so that is refused here too, a known divergence.
    if offset >= heap.size {
        return Err(FieldglassError::Parse(format!(
            "heap name offset {offset} is past the local heap's {}-byte data segment",
            heap.size
        )));
    }
    let start = heap::checked_add(heap.address, offset)?;
    let room = usize::try_from(heap.size - offset).unwrap_or(usize::MAX);
    let ceiling = MAX_LINK_NAME_BYTES.min(room);
    for want in scan_windows(ceiling) {
        let window = read_up_to(source, start, want)
            .map_err(|_| FieldglassError::Parse("heap name offset past end of file".into()))?;
        if let Some(end) = window.iter().position(|&b| b == 0) {
            return decode_name(&window[..end]);
        }
        // No terminator, and the file had no more to give: the name runs off
        // the end rather than being longer than the window.
        if window.len() < want {
            break;
        }
    }
    if room <= MAX_LINK_NAME_BYTES {
        return Err(FieldglassError::Parse(
            "heap name runs past the end of its local heap's data segment".into(),
        ));
    }
    Err(FieldglassError::Parse("unterminated heap name".into()))
}

// ---------------------------------------------------------------------------
// Modern link-info path
// ---------------------------------------------------------------------------

/// Resolve links from a Link Info message: either compact Link messages in the
/// header, or a fractal heap indexed by a version-2 B-tree.
fn link_info_links<S: ByteSource + ?Sized>(
    source: &S,
    header: &object_header::ObjectHeader,
    body: &[u8],
    osize: u8,
    lsize: u8,
) -> Result<Vec<(String, u64)>, FieldglassError> {
    let o = osize as usize;
    // version (1) + flags (1), then an optional maximum creation index, a
    // fixed 8 bytes (specification, "Link Info Message"; libhdf5 reads it with
    // `INT64DECODE`), not Size of Lengths (#922).
    if body.len() < 2 {
        return Err(FieldglassError::Parse("link info message too small".into()));
    }
    let flags = body[1];
    let mut pos = 2usize;
    if flags & 0x01 != 0 {
        pos += 8; // maximum creation index
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
    let heap = FractalHeap::parse(source, heap_addr, osize, lsize)?;
    // The format fixes a link record's layout: a 4-byte name hash or 8-byte
    // creation order, then the 7-byte fractal-heap ID (#895).
    let layout = |t: u8, size: usize| match (t, size) {
        (5, 11) | (6, 15) => Ok(()),
        (5 | 6, _) => Err(FieldglassError::Parse(format!(
            "B-tree v2 type {t} link records are {size} bytes; the format fixes {}",
            if t == 5 { 11 } else { 15 }
        ))),
        _ => Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {t} for links"
        ))),
    };
    let (btree_type, records) = heap::btree_v2_records(source, btree_addr, osize, lsize, &layout)?;
    if btree_type != 5 && btree_type != 6 {
        return Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {btree_type} for links"
        )));
    }

    let mut links = Vec::new();
    // One per listing, so no heap object is read twice (#899 review).
    let mut reads = heap::HeapReads::default();
    for record in records {
        let id = record
            .get(LINK_RECORD_HEAP_ID_OFFSET..LINK_RECORD_HEAP_ID_OFFSET + heap.heap_id_len)
            .ok_or_else(|| FieldglassError::Parse("link record too small for a heap ID".into()))?;
        let object = heap.object(source, id, &mut reads)?;
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
    let name_len = cur.usize(name_len_width)?;
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
        put(&mut buf, 0x108, &11u64.to_le_bytes()); // data segment size: both names
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
        let err =
            collect_snods(&buf, 0, 8, 8, &mut out, &mut ClaimedRanges::default()).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn address_past_eof_errors_without_panic() {
        let buf = vec![0u8; 16];
        assert!(FileCursor::at(&buf, 4096).is_err());
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
