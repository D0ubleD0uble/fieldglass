//! HDF5 global-heap (`GCOL`) reader (#174, under #33). Variable-length data —
//! a NetCDF-4 `DIMENSION_LIST`, a vlen-string attribute — does not live inline
//! with its dataset/attribute. Instead the inline element is a **global-heap
//! ID** (`length(4) + collection address(offset_size) + object index(4)`) that
//! points into a global-heap collection; this module dereferences that ID to the
//! object's bytes.
//!
//! A collection is signed `GCOL` and holds a run of objects, each with a small
//! header (index, reference count, size) followed by its data padded to an
//! 8-byte boundary. Index 0 is the trailing free-space object that marks the end.
//! Only the single-collection lookup the dimension layer needs is implemented;
//! anything unexpected returns a clear error rather than risk a silent misread,
//! matching `heap.rs`.
//!
//! Each object in a collection has its own index ("Each object has a unique
//! identification number within a collection", Global Heap), so a heap ID
//! names one object. When two objects carry the index a lookup asks for,
//! within the part of the run read, the ID names neither for certain, and the
//! lookup is refused (#1557). A run cut short, at a malformed object or past
//! the object budget, is not read past, so a repeat beyond it goes unseen.
//! A lookup past such a stop is refused for it: a malformed object as a parse
//! error, the budget as `ListingTooLarge` (ADR-0016 decision 4, #1556). An
//! index repeated elsewhere in the collection leaves the asked-for one
//! unambiguous, so it does not refuse other lookups. libhdf5 2.0.0 reads a repeated index
//! as the last object that carries it; this is a known divergence, in the
//! spec's favor (`tests/fixtures/NOTICE.md`, ADR-0016). The callers hold the
//! ID's length to the object it names: see `HeapText` in `attribute.rs` and
//! `decode_dimension_list`.
//!
//! Reference: HDF5 file format specification version 3, "Global Heap".

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use super::source::{Fields, FileCursor, charge_metadata, read_at};
use fieldglass_core::FieldglassError;
use fieldglass_core::array::ArrayError;
use fieldglass_core::bytes::{ByteSource, checked_usize};

const SIG_GLOBAL_HEAP: &[u8; 4] = b"GCOL";

/// Upper bound on the object headers one [`Collections`] scans: across
/// every collection it reads ([`Collections::default`]), or in each one
/// ([`Collections::per_collection`]).
///
/// The string attributes of one resolution share one budget across their
/// collections, which bounds the memo: a file of many small collections
/// named from string attributes otherwise costs memory with no ceiling. An
/// attribute the budget cuts is reported, not dropped, and 0.6.0 read no
/// variable-length string attribute, so this refuses nothing 0.6.0 showed.
///
/// `DIMENSION_LIST` resolution counts it per collection, as 0.6.0 did
/// (#1698). A collection holds at most 65,535 objects with distinct indices,
/// so only one whose indices repeat reaches it, but a file holds as many
/// collections as it likes: libhdf5 2.0.0 grows one to 64 KiB, 2,730 short
/// strings, and a NetCDF-4 file whose `DIMENSION_LIST` references sit in
/// 400 such collections, filled with string data, passed a budget shared
/// across them, so a file 0.6.0 opened failed. There, what bounds the
/// headers together is that collections do not overlap (each claims its own
/// block, see [`Collections`]): every header is at least 16 bytes of one, so
/// at most one header is scanned per 16 bytes of the source. Each
/// collection's memo is also at most 65,536 indices found and as many
/// repeated, since an index is a `u16`.
const MAX_GLOBAL_HEAP_OBJECTS: u32 = 1 << 20;

/// What [`MAX_GLOBAL_HEAP_OBJECTS`] counts across collections, as a
/// `ListingTooLarge` names it. A valid file can hold more objects than
/// this, so passing it is a policy cap (ADR-0016 decision 4), not a
/// malformed file (#1556).
const OBJECTS: &str = "HDF5 global-heap objects";

/// What [`MAX_GLOBAL_HEAP_OBJECTS`] counts in one collection, as a
/// `ListingTooLarge` names it.
const OBJECTS_IN_ONE: &str = "HDF5 global-heap objects in one collection";

/// The only collection version the specification defines.
const GLOBAL_HEAP_VERSION: u8 = 1;

/// What [`charge_metadata`] counts for the one object a lookup reads, against
/// `MAX_METADATA_BYTES` (#997).
///
/// libhdf5 sizes a new collection to the object it is created for
/// (`H5HG_insert`), so the format puts no ceiling on an object, and the
/// collection's own size is checked only against the end of the file: over a
/// source read by range, the size the host declares. An object is read whole,
/// so it is held to the cap before the read. The objects a lookup scans past
/// are skipped, not read, and are not charged. The `DIMENSION_LIST` objects
/// this reads are a few eight-byte references each.
const OBJECT_BYTES: &str = "bytes in one HDF5 global-heap object";

/// Refuse an object of `size` bytes past `MAX_METADATA_BYTES`.
fn check_object(size: u64) -> Result<usize, FieldglassError> {
    charge_metadata(&mut 0, size, OBJECT_BYTES)?;
    checked_usize(size, "global-heap object size")
}

/// Width of the fixed fields that open both the collection header
/// (`signature(4) + version(1) + reserved(3)`) and each object's header
/// (`index(2) + refcount(2) + reserved(4)`); a length-sized size follows each.
const HEADER_FIXED: usize = 8;

/// Each header, fixed fields and size together, padded to a multiple of eight,
/// so the data after it is aligned. libhdf5 writes them this way
/// (`H5HG_SIZEOF_HDR`, `H5HG_SIZEOF_OBJHDR`); the specification gives both as
/// 16 bytes, which is the same when lengths are 8 bytes. With 4-byte lengths
/// each header carries 4 bytes of padding after its size (#922).
fn header_len(length_size: usize) -> usize {
    (HEADER_FIXED + length_size).next_multiple_of(8)
}

/// Read the bytes of the object with `object_index` from the global-heap
/// collection at `collection_addr`. `length_size` is the superblock's size of
/// lengths (the width of the collection-size and object-size fields).
pub fn read_object<S: ByteSource + ?Sized>(
    source: &S,
    collection_addr: u64,
    object_index: u16,
    length_size: u8,
) -> Result<Vec<u8>, FieldglassError> {
    Collections::default().read_object(source, collection_addr, object_index, length_size)
}

/// The global-heap collections one listing has looked objects up in, each
/// scanned at most once (#921).
///
/// A collection is legitimately shared: netCDF-C puts the `DIMENSION_LIST`
/// references of many variables in one, and a lookup has to scan the
/// collection's objects in order to find one. Scanned afresh per lookup, a
/// collection of `n` objects that `n` variables each refer into cost `n²`
/// object headers. This remembers where each object scanned past lies and
/// where the scan stopped, so a later lookup either finds its object or
/// carries on from there.
///
/// A lookup scans the run to its end before it returns an object: only the
/// whole run shows that no other object carries the index (#1557). A run
/// that stops early, at a malformed object or past the object budget, is
/// evidence up to that point: an object before it is found, and one after it
/// refused with the reason the run stopped, whichever is asked for first. A
/// lookup reads object headers only ([`Collections::locate`]); an object's
/// bytes are read after it, and an object past the cap on a single object is
/// refused then, before any of it is read.
///
/// Collections do not overlap: each is a block of the file of its own
/// (`H5HG_create` allocates one per collection), and its declared size says
/// where it ends. A collection header inside a collection already read here
/// is refused (#1549 review). Without that, `n` headers whose first object
/// jumps into one shared run of objects cost that run `n` times, each
/// remembered, from a file the size of the run. With it, the object headers
/// scanned here are bounded by the file's size, and by
/// [`MAX_GLOBAL_HEAP_OBJECTS`]: across every collection, or in each one for
/// [`Collections::per_collection`].
///
/// Two blocks that overlap say neither one's objects are the ones the writer
/// put there, so the collection already read is refused from then on as
/// well as the one that overlaps it (#1695). A caller that took an object
/// from it before the overlap was found asks [`Self::is_refused`].
#[derive(Debug)]
pub(crate) struct Collections {
    by_address: HashMap<u64, Scan>,
    /// Each collection read: start → end, the block its declared size covers.
    blocks: BTreeMap<u64, u64>,
    /// The spans of the file that overlapping collections cover, start →
    /// end, disjoint: a block found to overlap moves here from `blocks`, so a
    /// later header inside it is still found to overlap.
    refused: BTreeMap<u64, u64>,
    /// The collections, by address, that overlap another: each lookup in
    /// one is refused.
    overlapping: HashSet<u64>,
    /// Object headers read so far across every collection, against
    /// `max_objects` unless the budget is counted per collection.
    objects: usize,
    /// The most object headers to read: [`MAX_GLOBAL_HEAP_OBJECTS`], lower
    /// only in a test, which cannot afford a million headers.
    max_objects: u32,
    /// `max_objects` holds for each collection on its own, not for every
    /// collection together ([`Collections::per_collection`]).
    per_collection: bool,
}

/// The object budget shared across every collection, as string attributes
/// use it (see [`MAX_GLOBAL_HEAP_OBJECTS`]).
impl Default for Collections {
    fn default() -> Self {
        Self {
            by_address: HashMap::new(),
            blocks: BTreeMap::new(),
            refused: BTreeMap::new(),
            overlapping: HashSet::new(),
            objects: 0,
            max_objects: MAX_GLOBAL_HEAP_OBJECTS,
            per_collection: false,
        }
    }
}

/// How far one collection has been scanned.
#[derive(Debug)]
struct Scan {
    /// The objects scanned past: index → where its data starts, and its size.
    /// The first object with an index; a second is recorded in `repeated`.
    found: HashMap<u16, (u64, u64)>,
    /// The indices more than one object carries.
    repeated: HashSet<u16>,
    /// The file offset of the next object header.
    next: u64,
    /// The collection's bytes from `next` on. A `u64`, as every size here
    /// is: a collection is bounded by the size the source declares, which on
    /// a 32-bit target can be past `usize`.
    remaining: u64,
    /// Object headers read so far in this collection, against
    /// [`MAX_GLOBAL_HEAP_OBJECTS`] when it is counted per collection.
    objects: u32,
    /// The run ended: the free-space object, no room for another header, or
    /// a stop recorded in `stopped`.
    ended: bool,
    /// Why the run stopped short of its end, when it did: a malformed object
    /// or the object budget. A lookup for an index not found before it is
    /// refused with this.
    stopped: Option<Stop>,
}

/// Why a run stopped short of its end.
#[derive(Debug)]
enum Stop {
    /// An object the collection cannot hold: a `Parse` error.
    Malformed(String),
    /// The object budget, at `limit` headers, across every collection when
    /// `shared` and in this one otherwise: a policy cap, so
    /// `ListingTooLarge`, which says the file is fine (#1556). A string
    /// attribute it leaves unread is then reported, not dropped. A flag, not
    /// the text it picks: a `&str` here would not fit beside `Malformed`'s
    /// `String`, and every [`Scan`] would grow by eight bytes.
    Budget { limit: u32, shared: bool },
}

impl Stop {
    /// The refusal for a lookup the stop leaves unanswered.
    fn refusal(&self) -> FieldglassError {
        match self {
            Self::Malformed(reason) => FieldglassError::Parse(reason.clone()),
            Self::Budget { limit, shared } => FieldglassError::Array(ArrayError::ListingTooLarge {
                what: if *shared { OBJECTS } else { OBJECTS_IN_ONE },
                limit: u64::from(*limit),
            }),
        }
    }
}

/// Where one global-heap object lies, found without reading its bytes
/// ([`Collections::locate`]).
///
/// A caller that knows what the object's size should be, such as the length
/// a heap ID states, checks it and charges it to its own budget before
/// anything is read (#1569). Read first, every reference to one large object
/// read it, however many references the file held.
///
/// The scan that finds an object reads the collection in windows, and an
/// object it passes may lie wholly inside one. Its bytes are then kept from
/// that window, so reading it costs no further request; they were read
/// anyway. Only the object asked for is kept, and a window is at most
/// [`MAX_WINDOW_BYTES`](fieldglass_core::bytes::MAX_WINDOW_BYTES).
#[derive(Debug, Clone)]
pub(crate) struct Located {
    at: u64,
    size: u64,
    /// The object's bytes, when the scan's window already held them all.
    held: Option<Vec<u8>>,
}

impl Located {
    /// The object's size, as its header in the collection states it.
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// The object's size, refused past the cap on one object
    /// ([`OBJECT_BYTES`]).
    pub(crate) fn checked_size(&self) -> Result<usize, FieldglassError> {
        check_object(self.size)
    }

    /// The object's bytes, held to the cap on one object first.
    pub(crate) fn read<S: ByteSource + ?Sized>(
        self,
        source: &S,
    ) -> Result<Vec<u8>, FieldglassError> {
        let size = self.checked_size()?;
        if let Some(held) = self.held {
            return Ok(held);
        }
        Ok(read_at(source, self.at, size)?.into_owned())
    }

    /// The object's first `len` bytes, or all of it when it is shorter.
    pub(crate) fn read_prefix<S: ByteSource + ?Sized>(
        self,
        source: &S,
        len: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        let len = u64::try_from(len).map_or(self.size, |len| len.min(self.size));
        let len = checked_usize(len, "global-heap object size")?;
        if let Some(mut held) = self.held {
            held.truncate(len);
            return Ok(held);
        }
        Ok(read_at(source, self.at, len)?.into_owned())
    }
}

impl Collections {
    /// Collections whose object budget holds for each one on its own, as in
    /// 0.6.0, for `DIMENSION_LIST` resolution, where a lookup past it fails
    /// the file (#1698). See [`MAX_GLOBAL_HEAP_OBJECTS`].
    pub(crate) fn per_collection() -> Self {
        Self {
            per_collection: true,
            ..Self::default()
        }
    }

    /// [`read_object`], scanning each collection at most once across calls.
    pub(crate) fn read_object<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        collection_addr: u64,
        object_index: u16,
        length_size: u8,
    ) -> Result<Vec<u8>, FieldglassError> {
        self.locate(source, collection_addr, object_index, length_size)?
            .read(source)
    }

    /// Where the object with `object_index` lies in the collection at
    /// `collection_addr`, found from the object headers alone: none of the
    /// object is read. Refused as [`read_object`] refuses it, except for the
    /// cap on one object, which [`Located::read`] applies.
    pub(crate) fn locate<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        collection_addr: u64,
        object_index: u16,
        length_size: u8,
    ) -> Result<Located, FieldglassError> {
        if object_index == 0 {
            return Err(FieldglassError::Parse(
                "global-heap object index 0 is the free-space marker, not an object".into(),
            ));
        }
        if self.is_refused(collection_addr) {
            return Err(overlap());
        }
        let l = length_size as usize;
        let Self {
            by_address,
            blocks,
            refused,
            overlapping,
            objects,
            max_objects,
            per_collection,
        } = self;
        let scan = match by_address.entry(collection_addr) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let (scan, end) = open(source, collection_addr, l)?;
                claim((blocks, refused, overlapping), collection_addr, end)?;
                e.insert(scan)
            }
        };
        let budget = Budget {
            shared: (!*per_collection).then_some(objects),
            max: *max_objects,
        };
        scan.locate(source, object_index, l, budget)
    }

    /// Whether the collection at `collection_addr` was found to overlap
    /// another, so that no object in it can be trusted, including one a
    /// lookup returned before the overlap was found (#1695).
    pub(crate) fn is_refused(&self, collection_addr: u64) -> bool {
        self.overlapping.contains(&collection_addr)
    }

    /// Whether any collection was found to overlap another.
    pub(crate) fn any_refused(&self) -> bool {
        !self.overlapping.is_empty()
    }
}

/// The refusal of a lookup in a collection that overlaps another.
fn overlap() -> FieldglassError {
    FieldglassError::Parse("global-heap collection overlaps another collection".into())
}

#[cfg(test)]
impl Collections {
    /// Object headers scanned across every collection.
    fn objects_scanned(&self) -> usize {
        self.by_address.values().map(|s| s.found.len()).sum()
    }
}

/// The object headers a scan may still read: `max` of them across every
/// collection, counted in `shared`, or in the collection scanned when
/// `shared` is `None`.
struct Budget<'a> {
    shared: Option<&'a mut usize>,
    max: u32,
}

impl Budget<'_> {
    /// Whether a collection that has read `objects` headers may read one
    /// more.
    fn spent(&self, objects: u32) -> bool {
        match &self.shared {
            Some(used) => **used >= self.max as usize,
            None => objects >= self.max,
        }
    }

    /// The stop when [`Self::spent`].
    fn stop(&self) -> Stop {
        Stop::Budget {
            limit: self.max,
            shared: self.shared.is_some(),
        }
    }

    /// Count one header read.
    fn charge(&mut self) {
        if let Some(used) = self.shared.as_deref_mut() {
            *used += 1;
        }
    }
}

impl Scan {
    /// Where the object with `index` lies, `l` being the size of lengths.
    ///
    /// The run is scanned to its end first, object headers only: an index
    /// names one object in a collection, and only the whole run shows that
    /// no other object carries it. None of the object is read here; its
    /// bytes are kept only when this scan's window already held them.
    fn locate<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        index: u16,
        l: usize,
        mut budget: Budget<'_>,
    ) -> Result<Located, FieldglassError> {
        let held = self.scan(source, l, &mut budget, index)?;
        self.unique(index)?;
        match (self.found.get(&index), &self.stopped) {
            (Some(&(at, size)), _) => Ok(Located { at, size, held }),
            (None, Some(stop)) => Err(stop.refusal()),
            (None, None) => Err(FieldglassError::Parse(format!(
                "global-heap object {index} not found in collection"
            ))),
        }
    }

    /// Refuse an `index` two objects carry: the heap ID names neither for
    /// certain. libhdf5 2.0.0 reads it as the last of them (#1557).
    fn unique(&self, index: u16) -> Result<(), FieldglassError> {
        if self.repeated.contains(&index) {
            return Err(FieldglassError::Parse(format!(
                "global-heap collection holds object index {index} twice"
            )));
        }
        Ok(())
    }

    /// Scan on to the end of the run, recording where each object lies from
    /// its header. No object's bytes are read. A malformed object, or the
    /// object budget running out, stops the run there for good, the reason
    /// kept in `stopped`. A failed read of the source is returned, and the
    /// scan stays where it was.
    ///
    /// Returns the bytes of the first object with index `want` when this
    /// scan passes it and its window already holds all of them: taking them
    /// costs no request, where reading them later would cost one.
    fn scan<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        l: usize,
        budget: &mut Budget<'_>,
        want: u16,
    ) -> Result<Option<Vec<u8>>, FieldglassError> {
        let mut held = None;
        if self.ended {
            return Ok(held);
        }
        let header_len = header_len(l);
        let header_bytes = header_len as u64;
        let mut cur = FileCursor::at(source, self.next)?;
        while !self.ended {
            if self.remaining < header_bytes {
                self.ended = true;
                break;
            }
            if budget.spent(self.objects) {
                self.stop(budget.stop());
                break;
            }
            let index = cur.u16()?;
            cur.skip(2 + 4)?; // reference count + reserved
            let size = cur.uint(l)?;
            cur.skip(header_len - HEADER_FIXED - l)?; // alignment padding
            // Index 0 is the free-space object that terminates the run.
            if index == 0 {
                self.ended = true;
                break;
            }
            // The object's data must fit in what the collection has left: a
            // corrupt oversized object would be a cross-object misread.
            let body_budget = self.remaining - header_bytes;
            if size > body_budget {
                self.stop(Stop::Malformed(
                    "global-heap object size exceeds the collection bounds".into(),
                ));
                break;
            }
            // Its padding to eight bytes may not fit: a collection that runs
            // to the end of the file can end with its last object unpadded.
            // Held to `body_budget`, the step cannot overflow either.
            let step = size.next_multiple_of(8).min(body_budget);
            let at = cur.position();
            // Only the first object with an index is recorded; a second
            // marks it repeated.
            if index == want && !self.found.contains_key(&index) {
                held = usize::try_from(size)
                    .ok()
                    .and_then(|size| cur.buffered(size))
                    .map(<[u8]>::to_vec);
            }
            self.passed(index, (at, size), step, header_bytes);
            self.objects += 1;
            budget.charge();
            // Skipped, not read: an object passed over costs nothing, however
            // large, so it is not held to the cap. `skip` keeps the cursor's
            // window when the next header is inside it, so a run of small
            // objects is read once; re-seating the cursor for each would drop
            // the window and read the rest of the collection again per object.
            // A skip `usize` cannot hold, past 4 GiB on a 32-bit target,
            // re-seats the cursor instead, so that target passes the object
            // as a 64-bit one does.
            match usize::try_from(step) {
                Ok(step) => cur.skip(step)?,
                Err(_) => cur = FileCursor::at(source, self.next)?,
            }
        }
        Ok(held)
    }

    /// End the run short of its end, for `reason`.
    fn stop(&mut self, reason: Stop) {
        self.ended = true;
        self.stopped = Some(reason);
    }

    /// Record the object `index` whose data is `size` bytes at `at`, and move
    /// past it and its padding, `padded` bytes in all. An index already
    /// recorded is marked repeated, and its first object kept.
    fn passed(&mut self, index: u16, (at, size): (u64, u64), padded: u64, header_len: u64) {
        if let Entry::Vacant(e) = self.found.entry(index) {
            e.insert((at, size));
        } else {
            self.repeated.insert(index);
        }
        self.next = at.saturating_add(padded);
        self.remaining = self
            .remaining
            .saturating_sub(header_len.saturating_add(padded));
    }
}

/// Record the collection block `start..end`, refusing one that overlaps a
/// block already recorded. A refused block, and every block it overlaps,
/// is marked in `overlapping` and moves from `blocks` into `refused`.
fn claim(
    (blocks, refused, overlapping): (
        &mut BTreeMap<u64, u64>,
        &mut BTreeMap<u64, u64>,
        &mut HashSet<u64>,
    ),
    start: u64,
    end: u64,
) -> Result<(), FieldglassError> {
    let in_blocks = overlapped(blocks, start, end);
    let in_refused = overlapped(refused, start, end);
    if in_blocks.is_empty() && in_refused.is_empty() {
        blocks.insert(start, end);
        return Ok(());
    }
    // Each block leaves `blocks` once and each refused span is merged once,
    // so a file of many overlapping headers costs each block a few map
    // operations, not one per header.
    let (mut low, mut high) = (start, end);
    for at in in_blocks {
        let block_end = blocks.remove(&at).unwrap_or(at);
        overlapping.insert(at);
        (low, high) = (low.min(at), high.max(block_end));
    }
    for at in in_refused {
        let span_end = refused.remove(&at).unwrap_or(at);
        (low, high) = (low.min(at), high.max(span_end));
    }
    refused.insert(low, high);
    overlapping.insert(start);
    Err(overlap())
}

/// The starts of the blocks in `held`, which are disjoint, that overlap
/// `start..end`. Their ends rise with their starts, so of those that start
/// before `end`, the ones that reach past `start` are the last few, and the
/// first that does not ends the search.
fn overlapped(held: &BTreeMap<u64, u64>, start: u64, end: u64) -> Vec<u64> {
    held.range(..end)
        .rev()
        .take_while(|(_, held_end)| **held_end > start)
        .map(|(at, _)| *at)
        .collect()
}

/// Read a collection's header and start a scan of its objects. Also returns
/// where the collection's block ends.
fn open<S: ByteSource + ?Sized>(
    source: &S,
    collection_addr: u64,
    l: usize,
) -> Result<(Scan, u64), FieldglassError> {
    let mut cur = FileCursor::at(source, collection_addr)?;
    cur.tag(SIG_GLOBAL_HEAP)?;
    let version = cur.byte()?;
    if version != GLOBAL_HEAP_VERSION {
        return Err(FieldglassError::Parse(format!(
            "global-heap collection version {version} is not supported"
        )));
    }
    cur.skip(3)?; // reserved
    // The declared collection size counts the whole collection (header + objects).
    // Clamp it to the bytes actually on disk so a corrupt size can't make the
    // budget arithmetic below overflow or run past the file.
    // `saturating_sub` rather than a bare one: the cursor above already refused
    // an address past the end of the file, so this cannot go negative — but a
    // subtraction that is only correct because of a check several lines away is
    // the kind that survives a reordering and underflows.
    let available = source.size().saturating_sub(collection_addr);
    let collection_size = cur.uint(l)?.min(available);
    // The collection size counts its own header, so the object run is whatever
    // remains after it. An object's header is the same length.
    let header_len = header_len(l);
    cur.skip(header_len - HEADER_FIXED - l)?; // alignment padding
    let scan = Scan {
        found: HashMap::new(),
        next: cur.position(),
        remaining: collection_size.saturating_sub(header_len as u64),
        objects: 0,
        ended: false,
        repeated: HashSet::new(),
        stopped: None,
    };
    // The block takes in at least the header just read, whatever size the
    // collection declares.
    let end = collection_addr.saturating_add(collection_size.max(header_len as u64));
    Ok((scan, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `GCOL` collection holding `objects` as `(index, data)` pairs,
    /// followed by the index-0 free-space terminator.
    fn collection(objects: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(SIG_GLOBAL_HEAP);
        body.push(1); // version
        body.extend_from_slice(&[0, 0, 0]); // reserved
        let size_pos = body.len();
        body.extend_from_slice(&0u64.to_le_bytes()); // collection size (patched)
        for &(index, data) in objects {
            body.extend_from_slice(&index.to_le_bytes());
            body.extend_from_slice(&1u16.to_le_bytes()); // reference count
            body.extend_from_slice(&[0, 0, 0, 0]); // reserved
            body.extend_from_slice(&(data.len() as u64).to_le_bytes());
            body.extend_from_slice(data);
            body.resize(
                body.len() + (data.len().next_multiple_of(8) - data.len()),
                0,
            );
        }
        // Free-space terminator: index 0, then the remaining size (unused here).
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // refcount + reserved
        body.extend_from_slice(&0u64.to_le_bytes());
        let total = body.len() as u64;
        body[size_pos..size_pos + 8].copy_from_slice(&total.to_le_bytes());
        body
    }

    #[test]
    fn reads_object_by_index() {
        let buf = collection(&[(1, &[0xAA, 0xBB]), (2, &[1, 2, 3, 4, 5])]);
        assert_eq!(read_object(&buf, 0, 1, 8).unwrap(), vec![0xAA, 0xBB]);
        assert_eq!(read_object(&buf, 0, 2, 8).unwrap(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reads_reference_array_object() {
        // A DIMENSION_LIST axis: one 8-byte object-header address.
        let addr = 0x1234u64.to_le_bytes();
        let buf = collection(&[(3, &addr)]);
        let got = read_object(&buf, 0, 3, 8).unwrap();
        assert_eq!(u64::from_le_bytes(got.try_into().unwrap()), 0x1234);
    }

    #[test]
    fn missing_index_errors() {
        let buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 9, 8).is_err());
    }

    #[test]
    fn rejects_free_space_index() {
        let buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 0, 8).is_err());
    }

    #[test]
    fn bad_signature_errors() {
        let buf = vec![0u8; 64];
        assert!(read_object(&buf, 0, 1, 8).is_err());
    }

    #[test]
    fn oversized_object_size_errors_without_panic() {
        // A corrupt object whose declared size is near u64::MAX must yield a clean
        // error, not a `next_multiple_of` overflow panic or a cross-object read.
        let mut buf = Vec::new();
        buf.extend_from_slice(SIG_GLOBAL_HEAP);
        buf.push(1);
        buf.extend_from_slice(&[0, 0, 0]); // reserved
        buf.extend_from_slice(&64u64.to_le_bytes()); // collection size
        buf.extend_from_slice(&5u16.to_le_bytes()); // object index 5
        buf.extend_from_slice(&1u16.to_le_bytes()); // reference count
        buf.extend_from_slice(&[0, 0, 0, 0]); // reserved
        buf.extend_from_slice(&(u64::MAX - 1).to_le_bytes()); // bogus object size
        buf.resize(64, 0);
        // Look up a different index so the scan reaches the size-budget check.
        assert!(read_object(&buf, 0, 9, 8).is_err());
        // And the matched-object path is bounded too.
        assert!(read_object(&buf, 0, 5, 8).is_err());
    }

    /// Many lookups into one collection scan it once (#921): looked up from
    /// the last object back, a fresh scan per lookup read every object header
    /// before its own, `n²/2` of them.
    #[test]
    fn a_collection_is_scanned_once_however_many_lookups() {
        const N: u16 = 512;
        let data: Vec<[u8; 8]> = (1..=N).map(|i| u64::from(i).to_le_bytes()).collect();
        let objects: Vec<(u16, &[u8])> = (1..=N).zip(&data).map(|(i, d)| (i, &d[..])).collect();
        let buf = collection(&objects);
        let source = fieldglass_core::testing::Recording::new(buf.as_slice());
        let mut collections = Collections::default();
        for i in (1..=N).rev() {
            let got = collections.read_object(&source, 0, i, 8).unwrap();
            assert_eq!(got, u64::from(i).to_le_bytes());
        }
        let read: u64 = source.reads().iter().map(|r| r.len).sum();
        // The collection once, then each object's own eight bytes.
        assert!(
            read <= 2 * buf.len() as u64 + 8 * u64::from(N),
            "{read} bytes read from a {}-byte collection",
            buf.len()
        );
    }

    /// `count` collection headers that each jump, by the size of their first
    /// object, into one shared run of `run` empty objects after them, so each
    /// collection's range takes in the run and every header after its own.
    /// Returns the file and the collections' addresses.
    fn overlapping_collections(count: u64, run: u16) -> (Vec<u8>, Vec<u64>) {
        const BLOCK: u64 = 32; // collection header + its first object's header
        let run_start = count * BLOCK;
        let mut buf = Vec::new();
        let total = run_start + (u64::from(run) + 1) * 16;
        for k in 0..count {
            let at = k * BLOCK;
            buf.extend_from_slice(SIG_GLOBAL_HEAP);
            buf.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
            buf.extend_from_slice(&(total - at).to_le_bytes()); // to the end of the file
            buf.extend_from_slice(&u16::MAX.to_le_bytes()); // an index the run lacks
            buf.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
            buf.extend_from_slice(&(run_start - at - BLOCK).to_le_bytes()); // up to the run
        }
        buf.resize(run_start as usize, 0); // the jumped-over bytes
        for index in 1..=run {
            buf.extend_from_slice(&index.to_le_bytes());
            buf.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
            buf.extend_from_slice(&0u64.to_le_bytes()); // an empty object
        }
        buf.extend_from_slice(&[0u8; 16]); // free-space object
        assert_eq!(buf.len() as u64, total);
        (buf, (0..count).map(|k| k * BLOCK).collect())
    }

    /// Collections do not overlap (each is its own block of the file), so a
    /// second header inside one already read is refused rather than scanned:
    /// otherwise `n` headers that each jump into one shared run of objects
    /// cost the run `n` times over, in time and in memory, from a file the
    /// size of the run (#1549 review: 400 of them took 1.29 GB).
    #[test]
    fn overlapping_collections_are_not_each_scanned() {
        const RUN: u16 = 65_000;
        let (buf, addresses) = overlapping_collections(16, RUN);
        let mut collections = Collections::default();
        let mut refused = 0;
        for &at in &addresses {
            // An index none of them holds: a lookup scans the whole run.
            let err = collections.read_object(&buf, at, RUN + 1, 8).unwrap_err();
            refused += usize::from(err.to_string().contains("overlaps"));
        }
        assert_eq!(refused, addresses.len() - 1);
        let scanned = collections.objects_scanned();
        assert!(
            scanned <= usize::from(RUN) + addresses.len(),
            "{scanned} object headers scanned from a run of {RUN}"
        );
    }

    /// Once a second collection is found to overlap one already read, both
    /// are refused from then on, so what the first held stops being found
    /// (#1695): neither can be trusted to be the file's own record. A third
    /// collection over either one's span is refused too, and none of them
    /// is listed as refused before the overlap is found.
    #[test]
    fn an_overlap_refuses_the_collection_already_read() {
        let (buf, addresses) = overlapping_collections(3, 4);
        let (first, second, third) = (addresses[0], addresses[1], addresses[2]);
        let mut collections = Collections::default();
        assert_eq!(collections.read_object(&buf, first, 2, 8).unwrap(), b"");
        assert!(!collections.any_refused());
        assert!(!collections.is_refused(first));

        let err = collections.read_object(&buf, second, 2, 8).unwrap_err();
        assert!(err.to_string().contains("overlaps"), "{err}");
        assert!(collections.is_refused(first) && collections.is_refused(second));
        let err = collections.read_object(&buf, first, 2, 8).unwrap_err();
        assert!(err.to_string().contains("overlaps"), "{err}");

        assert!(!collections.is_refused(third));
        let err = collections.read_object(&buf, third, 2, 8).unwrap_err();
        assert!(err.to_string().contains("overlaps"), "{err}");
        assert!(collections.is_refused(third));
    }

    /// By default the object-header budget spans every collection one
    /// `Collections` reads, as string attributes use it, not each collection
    /// on its own: two collections of eight objects under a budget of ten
    /// find what lies in the first ten headers and refuse the rest, though
    /// each collection alone is within it.
    #[test]
    fn the_object_budget_is_shared_across_collections() {
        let data: Vec<[u8; 1]> = (1..=8u8).map(|i| [i]).collect();
        let objects: Vec<(u16, &[u8])> = (1..=8).zip(&data).map(|(i, d)| (i, &d[..])).collect();
        let one = collection(&objects);
        let mut buf = one.clone();
        buf.extend_from_slice(&one);
        let second = one.len() as u64;
        let mut collections = Collections {
            max_objects: 10,
            ..Collections::default()
        };
        // The whole first collection: eight headers.
        assert_eq!(collections.read_object(&buf, 0, 8, 8).unwrap(), [8]);
        // Two more headers are left, so the second collection's first two
        // objects are found and its third is refused for the budget.
        assert_eq!(collections.read_object(&buf, second, 2, 8).unwrap(), [2]);
        assert_eq!(collections.read_object(&buf, second, 1, 8).unwrap(), [1]);
        let err = collections.read_object(&buf, second, 3, 8).unwrap_err();
        assert!(
            matches!(
                err,
                FieldglassError::Array(ArrayError::ListingTooLarge {
                    what: OBJECTS,
                    limit: 10
                })
            ),
            "{err:?}"
        );
        assert_eq!(collections.read_object(&buf, 0, 1, 8).unwrap(), [1]);
        // Alone, under the same budget, the second collection holds it.
        let mut alone = Collections {
            max_objects: 10,
            ..Collections::default()
        };
        assert_eq!(alone.read_object(&buf, second, 3, 8).unwrap(), [3]);
    }

    /// For `DIMENSION_LIST` resolution the object-header budget is counted
    /// per collection, as 0.6.0 counted it (#1698): a file holds as many
    /// collections as it likes, so a budget shared across them refused valid
    /// files 0.6.0 opened. Two collections of eight objects under a budget of
    /// ten each read whole, and a third of twelve finds its first ten and
    /// refuses the rest for the budget, which leaves the other two as they
    /// were.
    #[test]
    fn the_object_budget_is_counted_per_collection() {
        let data: Vec<[u8; 1]> = (1..=12u8).map(|i| [i]).collect();
        let objects: Vec<(u16, &[u8])> = (1..=12).zip(&data).map(|(i, d)| (i, &d[..])).collect();
        let eight = collection(&objects[..8]);
        let mut buf = eight.clone();
        buf.extend_from_slice(&eight);
        let second = eight.len() as u64;
        let third = buf.len() as u64;
        buf.extend_from_slice(&collection(&objects));
        let mut collections = Collections {
            max_objects: 10,
            ..Collections::per_collection()
        };
        for at in [0, second] {
            for index in (1..=8u8).rev() {
                let got = collections.read_object(&buf, at, u16::from(index), 8);
                assert_eq!(got.unwrap(), [index], "{at} {index}");
            }
        }
        assert_eq!(collections.read_object(&buf, third, 10, 8).unwrap(), [10]);
        let err = collections.read_object(&buf, third, 11, 8).unwrap_err();
        assert!(
            matches!(
                err,
                FieldglassError::Array(ArrayError::ListingTooLarge {
                    what: OBJECTS_IN_ONE,
                    limit: 10
                })
            ),
            "{err:?}"
        );
        assert_eq!(collections.read_object(&buf, third, 1, 8).unwrap(), [1]);
        assert_eq!(collections.read_object(&buf, second, 8, 8).unwrap(), [8]);
    }

    /// The specification defines version 1 of a collection only.
    #[test]
    fn a_collection_of_another_version_is_refused() {
        let mut buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 1, 8).is_ok());
        for version in [0, 2, 0xFF] {
            buf[4] = version;
            let err = read_object(&buf, 0, 1, 8).unwrap_err();
            assert!(err.to_string().contains("version"), "{version}: {err}");
        }
    }

    /// The memo answers as a fresh scan does on either side of a malformed
    /// object, in either order of asking: the run is evidence up to it, so
    /// the object before it is found and the one after it refused (#1557
    /// review: a malformed object late in a run must not take the objects
    /// before it with it).
    #[test]
    fn a_malformed_object_stops_the_scan_the_same_way_whatever_is_asked_first() {
        let mut buf = collection(&[(1, &[0xAA]), (2, &[0xBB]), (3, &[0xCC])]);
        // Object 2's header is at 16 + 24; give it a size past the collection.
        let size_at = 16 + 24 + 8;
        buf[size_at..size_at + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
        for order in [[1u16, 3], [3, 1]] {
            let mut collections = Collections::default();
            for index in order {
                let memo = collections.read_object(&buf, 0, index, 8);
                let fresh = read_object(&buf, 0, index, 8);
                assert_eq!(memo.is_ok(), fresh.is_ok(), "{order:?} {index}");
                assert_eq!(memo.is_ok(), index == 1, "{order:?} {index}");
            }
        }
    }

    /// Each object in a collection has its own index (Global Heap: "Each
    /// object has a unique identification number within a collection"), so
    /// a lookup whose index two objects carry is refused, whether it is
    /// asked before or after the second is reached, and in either order of
    /// asking (#1557). Every other index still reads: a repeat of index 2
    /// leaves no doubt which object index 1 or 3 names. libhdf5 2.0.0 reads
    /// index 2 as `hel`, the last object with it; this diverges, in the
    /// spec's favor.
    #[test]
    fn a_duplicate_object_index_refuses_only_its_own_lookup() {
        let objects: [(u16, &[u8]); 4] = [(1, b"a"), (2, b"hello"), (3, b"c"), (2, b"hel")];
        let distinct = collection(&[(1, b"a"), (2, b"hello"), (3, b"c"), (4, b"hel")]);
        let duplicated = collection(&objects);
        for index in [1u16, 2, 3, 4] {
            let want: &[u8] = [&b"a"[..], b"hello", b"c", b"hel"][usize::from(index) - 1];
            assert_eq!(read_object(&distinct, 0, index, 8).unwrap(), want);
        }
        let twice = |got: Result<Vec<u8>, FieldglassError>| matches!(&got, Err(FieldglassError::Parse(m)) if m == "global-heap collection holds object index 2 twice");
        assert!(twice(read_object(&duplicated, 0, 2, 8)));
        assert_eq!(read_object(&duplicated, 0, 1, 8).unwrap(), b"a");
        assert_eq!(read_object(&duplicated, 0, 3, 8).unwrap(), b"c");
        // One memo, asked in either order.
        for order in [[1u16, 2, 3], [3, 2, 1], [2, 1, 3]] {
            let mut collections = Collections::default();
            for index in order {
                let got = collections.read_object(&duplicated, 0, index, 8);
                if index == 2 {
                    assert!(twice(got), "{order:?}");
                } else {
                    assert_eq!(
                        got.unwrap(),
                        [&b"a"[..], b"c"][usize::from(index) / 2],
                        "{order:?} {index}"
                    );
                }
            }
        }
    }

    /// An index the memo has already found is refused when a second object
    /// carries it further on, though the scan that found it stopped short:
    /// here at an object past the cap, refused when it is asked for. The memo
    /// scans on past that object, unread, as a fresh scan does.
    #[test]
    fn a_found_index_is_checked_to_the_end_of_the_run() {
        let big = vec![0; super::super::source::MAX_METADATA_BYTES as usize + 8];
        let buf = collection(&[(1, b"a"), (2, &big), (1, b"c")]);
        let twice = "global-heap collection holds object index 1 twice";
        let mut collections = Collections::default();
        assert!(matches!(
            collections.read_object(&buf, 0, 2, 8),
            Err(FieldglassError::Array(
                fieldglass_core::array::ArrayError::ListingTooLarge { .. }
            ))
        ));
        for got in [
            collections.read_object(&buf, 0, 1, 8),
            read_object(&buf, 0, 1, 8),
        ] {
            assert!(
                matches!(&got, Err(FieldglassError::Parse(m)) if m == twice),
                "{got:?}"
            );
        }
    }

    /// A malformed object stops the run as a `Parse` error, and the object
    /// budget as `ListingTooLarge`: a valid file can pass the budget, so its
    /// lookups say the file is fine, and a string attribute it leaves unread
    /// is reported rather than dropped (#1556, ADR-0016 decision 4).
    #[test]
    fn the_object_budget_stops_a_run_as_too_large_and_a_malformed_object_as_corrupt() {
        let buf = collection(&[(1, b"a"), (2, b"b"), (3, b"c")]);
        let mut collections = Collections {
            max_objects: 2,
            ..Collections::default()
        };
        assert_eq!(collections.read_object(&buf, 0, 1, 8).unwrap(), b"a");
        for got in [
            collections.read_object(&buf, 0, 3, 8),
            collections.read_object(&buf, 0, 3, 8),
        ] {
            assert!(
                matches!(
                    got,
                    Err(FieldglassError::Array(ArrayError::ListingTooLarge {
                        what: OBJECTS,
                        limit: 2
                    }))
                ),
                "{got:?}"
            );
        }
        let mut malformed = buf.clone();
        let size_at = 16 + 24 + 8; // object 2's size
        malformed[size_at..size_at + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
        let got = Collections::default().read_object(&malformed, 0, 3, 8);
        assert!(
            matches!(&got, Err(FieldglassError::Parse(m)) if m.contains("collection bounds")),
            "{got:?}"
        );
    }

    /// A repeat of the looked-up index is refused as a repeat even when its
    /// second object is past the cap on one object: only the first object
    /// with an index is read, so the second is not held to the cap, and the
    /// lookup says why it is refused (#1557 review).
    #[test]
    fn a_repeat_past_the_object_cap_is_refused_as_a_repeat() {
        let big = vec![0; super::super::source::MAX_METADATA_BYTES as usize + 8];
        let buf = collection(&[(1, b"a"), (1, &big)]);
        let twice = "global-heap collection holds object index 1 twice";
        for got in [
            Collections::default().read_object(&buf, 0, 1, 8),
            read_object(&buf, 0, 1, 8),
        ] {
            assert!(
                matches!(&got, Err(FieldglassError::Parse(m)) if m == twice),
                "{got:?}"
            );
        }
    }

    /// Locating an object reads the object headers, not the objects: a
    /// caller can check and charge a large one before it costs anything
    /// (#1569). Reading it then costs its size.
    #[test]
    fn locating_an_object_reads_none_of_it() {
        let big = vec![0x55u8; 4 << 20];
        let buf = collection(&[(1, &big), (2, b"small")]);
        let source = fieldglass_core::testing::Recording::new(buf.as_slice());
        let mut collections = Collections::default();
        let located = collections.locate(&source, 0, 1, 8).unwrap();
        assert_eq!(located.size(), big.len() as u64);
        let read: u64 = source.reads().iter().map(|r| r.len).sum();
        assert!(read < 1 << 16, "{read} bytes read to locate it");
        source.clear();
        assert_eq!(located.clone().read(&source).unwrap(), big);
        assert_eq!(located.read_prefix(&source, 8).unwrap(), &big[..8]);
        let lens: Vec<u64> = source.reads().iter().map(|r| r.len).collect();
        assert_eq!(lens, [big.len() as u64, 8]);
    }

    /// An object the scan's window already holds is taken from it: reading
    /// it after locating it costs no request (the `open` request count of
    /// the NetCDF-4 perf fixtures, #1569). One the scan only passed, found
    /// later from the memo, is read.
    #[test]
    fn an_object_in_the_scan_window_costs_no_further_read() {
        let buf = collection(&[(1, b"first"), (2, b"second")]);
        let source = fieldglass_core::testing::Recording::new(buf.as_slice());
        let mut collections = Collections::default();
        let located = collections.locate(&source, 0, 1, 8).unwrap();
        let after_locate = source.reads().len();
        assert_eq!(located.clone().read(&source).unwrap(), b"first");
        assert_eq!(located.read_prefix(&source, 3).unwrap(), b"fir");
        assert_eq!(source.reads().len(), after_locate, "{:?}", source.reads());

        let second = collections.locate(&source, 0, 2, 8).unwrap();
        assert_eq!(second.read(&source).unwrap(), b"second");
        assert_eq!(source.reads().len(), after_locate + 1);
    }

    /// Once a malformed object stops a run, every later lookup into it is
    /// answered from the memo: a thousand lookups past the stop read the
    /// source no more than the first one did (#1557 review).
    #[test]
    fn lookups_past_a_stop_read_nothing_more() {
        let mut buf = collection(&[(1, b"a"), (2, b"b"), (3, b"c")]);
        let size_at = 16 + 24 + 8; // object 2's size
        buf[size_at..size_at + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
        let source = fieldglass_core::testing::Recording::new(buf.as_slice());
        let mut collections = Collections::default();
        assert!(collections.read_object(&source, 0, 3, 8).is_err());
        let first = source.reads().len();
        for _ in 0..1_000 {
            assert!(collections.read_object(&source, 0, 3, 8).is_err());
        }
        assert_eq!(source.reads().len(), first);
    }

    /// A collection that runs to the end of the file may end with its last
    /// object unpadded: the eight-byte padding the object would carry is not
    /// there. Both objects read, as on master before #1557, where scanning on
    /// past the last object read past the end of the file.
    #[test]
    fn a_last_object_unpadded_at_the_end_of_the_file_reads() {
        let mut buf = Vec::new();
        buf.extend_from_slice(SIG_GLOBAL_HEAP);
        buf.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
        buf.extend_from_slice(&(16u64 + 16 + 8 + 16 + 3).to_le_bytes());
        for (index, data) in [(1u16, &b"abcde"[..]), (2, b"xyz")] {
            buf.extend_from_slice(&index.to_le_bytes());
            buf.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
            buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
            buf.extend_from_slice(data);
            if index == 1 {
                buf.resize(buf.len().next_multiple_of(8), 0);
            }
        }
        for order in [[1u16, 2], [2, 1]] {
            let mut collections = Collections::default();
            for index in order {
                let want: &[u8] = if index == 1 { b"abcde" } else { b"xyz" };
                assert_eq!(
                    collections.read_object(&buf, 0, index, 8).unwrap(),
                    want,
                    "{order:?}"
                );
            }
        }
    }
}
