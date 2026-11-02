//! HDF5 attribute decoder (issue #40, under #33). Decodes Attribute messages
//! (`0x000C`) — name, datatype, dataspace, and value — for an object header,
//! covering both the inline form (messages in the header) and the dense form
//! (Attribute Info message `0x0015` → fractal heap + version-2 B-tree) that
//! `netCDF4` uses once an object has many attributes.
//!
//! The same entry point serves global attributes (pass the root group's object
//! header) and per-dataset attributes. Values are rendered to a display string
//! the same way as the classic NetCDF path.
//!
//! A variable-length string attribute (netCDF-4's `NC_STRING`, and every `str`
//! attribute h5netcdf and h5py write) holds a global-heap ID per element, not
//! its text; `HeapText` reads the text from the global heap (#1549).
//!
//! Reference: HDF5 file format specification version 3, "Attribute Message",
//! "Attribute Info Message", the Datatype Message's variable-length class, and
//! "Global Heap".

use super::Hdf5Probe;
use super::dataspace::{self, Dataspace};
use super::datatype::{self, Datatype, DatatypeClass};
use super::global_heap;
use super::heap::{self, FractalHeap};
use super::object_header::{self, read_uint_le};
use super::source::{ClaimedRanges, Cursor, Fields, charge_metadata};
use crate::classic;
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::ByteSource;

const MSG_ATTRIBUTE: u16 = 0x000C;
const MSG_ATTRIBUTE_INFO: u16 = 0x0015;

/// Attribute-name B-tree v2 record: the fractal-heap ID comes first, followed
/// by message flags, creation order, and a name hash.
const ATTR_RECORD_HEAP_ID_OFFSET: usize = 0;

/// Upper bound on attributes per object — guards a malformed dense count.
const MAX_ATTRIBUTES: usize = 1 << 20;

/// A decoded HDF5 attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Hdf5Attribute {
    /// The attribute's link name.
    pub name: String,
    /// The attribute's decoded element type.
    pub datatype: Datatype,
    /// The attribute's decoded shape.
    pub dataspace: Dataspace,
    /// Display value: UTF-8 text for strings, comma-separated decimals for
    /// numeric types (matching the classic NetCDF render path). An array of
    /// several strings is its CDL, as `ncdump` prints it: each element quoted,
    /// separated by `", "` (#1549).
    pub value: String,
    /// Every element widened to `f64`, for numeric attributes; empty for
    /// strings. Mirrors the classic [`crate::classic::Attribute::values`], so
    /// value decode and the CF unpack read numbers rather than the display text.
    pub values: Vec<f64>,
}

impl Hdf5Attribute {
    /// The first element, widened to `f64`: how a scalar such as `_FillValue`
    /// is read. `None` for a string and for an empty value.
    pub fn first_value(&self) -> Option<f64> {
        self.values.first().copied()
    }

    /// Whether this is an array of more than one string, whose [`Self::value`]
    /// is CDL rather than the text itself.
    pub(crate) fn is_string_array(&self) -> bool {
        self.datatype.class == DatatypeClass::FixedLengthString
            && element_count(&self.dataspace) > 1
    }
}

/// A raw, undecoded attribute: its name plus the still-encoded datatype message
/// and the value bytes, with only the dataspace decoded. Lets the dimension-scale
/// layer read structured attributes — `DIMENSION_LIST` (vlen of references),
/// `_Netcdf4Dimid` (a scalar int) — that the human [`Hdf5Attribute`] path drops
/// because their datatype class isn't one [`datatype::decode`] maps to `NcType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAttribute {
    /// The attribute's link name.
    pub name: String,
    /// The attribute's datatype message body, undecoded (caller dispatches on
    /// its class — reference, variable-length, fixed-point, …).
    pub datatype_bytes: Vec<u8>,
    /// The attribute's decoded shape — the one message this path does decode,
    /// because the element count is what the caller needs to walk `data`.
    pub dataspace: Dataspace,
    /// The attribute's raw value bytes.
    pub data: Vec<u8>,
}

/// List the attributes attached to the object header at `object_header_address`,
/// sorted by name. Works for the root group (global attributes) and datasets.
pub fn list_attributes<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
    let mut file = ClaimedRanges::default();
    let bodies = message_bodies_in(source, object_header_address, probe, &mut file)?;
    parse_bodies(&bodies, &mut HeapText::new(source, probe))
}

/// The numeric attributes on the object header at `object_header_address`, as
/// [`list_attributes`] lists them less every string held in the global heap.
/// For a value decode, which reads only numbers (`_FillValue`,
/// `missing_value`), so it reads no global heap for text it would not use.
pub(crate) fn list_numeric_attributes<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
    let mut file = ClaimedRanges::default();
    let bodies = message_bodies_in(source, object_header_address, probe, &mut file)?;
    parse_bodies(&bodies, &mut HeapText::numbers_only(source, probe))
}

/// The attributes in an object's message bodies, sorted by name: what
/// [`list_attributes`] returns once [`message_bodies_in`] has read them. A
/// variable-length string's text is read through `heap`, which a listing of
/// many objects shares.
pub(crate) fn parse_bodies<S: ByteSource + ?Sized>(
    bodies: &[Vec<u8>],
    heap: &mut HeapText<'_, S>,
) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
    let length_size = heap.length_size;
    let mut attrs = Vec::new();
    // An attribute we can't parse — most often one whose datatype we don't decode
    // (e.g. a NetCDF-4 `DIMENSION_LIST`, which is a variable-length sequence),
    // but a malformed one too, such as a string whose global-heap ID points
    // nowhere — is skipped rather than failing the whole list, so the rest of
    // an object's attributes, and its value decode, still come through. A
    // failed read of the source is not the file's fault and is not skipped.
    for body in bodies {
        match parse_attribute_message(body, length_size, heap) {
            Ok(attr) => push_attribute(&mut attrs, attr)?,
            Err(err @ FieldglassError::Io(_)) => return Err(err),
            Err(_) => {}
        }
    }
    attrs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(attrs)
}

/// What [`charge_metadata`] counts for the string attribute text one listing
/// keeps, against `MAX_METADATA_BYTES`: the variable-length strings it reads
/// from the global heap, and the CDL of every string array.
///
/// Each global-heap object is held to that cap by itself when it is read; this
/// bounds them together. The text is kept, and a reference is 16 bytes, so
/// without it a small file whose every attribute refers to one large heap
/// object would cost that object's size per attribute. A string array's CDL
/// can be several times the array, its escapes and quotes included.
const TEXT_BYTES: &str = "bytes of HDF5 string attribute text in one listing";

/// Reads the text of variable-length string attributes from the global heap
/// (#1549), for every object one listing reads attributes from.
///
/// On disk each element of such an attribute is a global-heap ID: a 4-byte
/// length (the string's length in bytes), the collection's address and a
/// 4-byte object index. The collections are scanned through one
/// [`global_heap::Collections`], so attributes that share one (libhdf5 puts
/// new strings in a collection with room, so one file's strings share a few)
/// scan it once, however many objects refer into it (#921).
pub(crate) struct HeapText<'a, S: ?Sized> {
    source: &'a S,
    offset_size: u8,
    length_size: u8,
    /// `false` for a listing that wants only numbers: a string's text is then
    /// not read, and the attribute is left out.
    reads_text: bool,
    collections: global_heap::Collections,
    /// Text bytes read so far, against [`TEXT_BYTES`].
    read: u64,
}

impl<'a, S: ByteSource + ?Sized> HeapText<'a, S> {
    /// Read from `source`, the file `probe` was read from.
    pub(crate) fn new(source: &'a S, probe: &Hdf5Probe) -> Self {
        Self {
            source,
            offset_size: probe.offset_size,
            length_size: probe.length_size,
            reads_text: true,
            collections: global_heap::Collections::default(),
            read: 0,
        }
    }

    /// Read no text: every variable-length string attribute is left out.
    pub(crate) fn numbers_only(source: &'a S, probe: &Hdf5Probe) -> Self {
        Self {
            reads_text: false,
            ..Self::new(source, probe)
        }
    }

    /// The `count` strings whose global-heap IDs `data` holds, in order.
    fn strings(&mut self, data: &[u8], count: usize) -> Result<Vec<String>, FieldglassError> {
        if !self.reads_text {
            return Err(FieldglassError::UnsupportedSection(
                "variable-length string text in a numeric listing".into(),
            ));
        }
        let o = usize::from(self.offset_size);
        let id_len = 4 + o + 4;
        // Every ID is inline in the attribute message, so the message bounds
        // the count, and with it everything allocated below.
        let ids = count
            .checked_mul(id_len)
            .and_then(|total| data.get(..total))
            .ok_or_else(|| {
                FieldglassError::Parse(
                    "variable-length string attribute has more elements than IDs".into(),
                )
            })?;
        let strings: Vec<String> = ids
            .chunks_exact(id_len)
            .map(|id| self.string(id, o))
            .collect::<Result<_, _>>()?;
        // Several strings are shown as their CDL, which escapes them: up to
        // four bytes for one (`\ooo`). Each string's own text was charged as
        // it was read; charge what the escaping and quoting add before the
        // CDL is built, so the text kept stays within the budget.
        if strings.len() > 1 {
            self.charge(strings.iter().map(|s| cdl_len(s) - s.len()))?;
        }
        Ok(strings)
    }

    /// Charge `lengths`, in bytes, to the listing's text budget.
    fn charge(&mut self, lengths: impl Iterator<Item = usize>) -> Result<(), FieldglassError> {
        let total = lengths
            .map(|len| len as u64)
            .fold(0u64, u64::saturating_add);
        charge_metadata(&mut self.read, total, TEXT_BYTES)
    }

    /// The string one global-heap ID refers to.
    fn string(&mut self, id: &[u8], o: usize) -> Result<String, FieldglassError> {
        let len = read_uint_le(id, 0, 4)?;
        // An empty string is written as a length of zero, and libhdf5 stores
        // no heap object for it, so there is nothing to read.
        if len == 0 {
            return Ok(String::new());
        }
        let collection = read_uint_le(id, 4, o)?;
        // The on-disk index is 4 bytes and the heap's own is 2: a value past
        // that is a malformed ID, not one to wrap.
        let index = u16::try_from(read_uint_le(id, 4 + o, 4)?)
            .map_err(|_| FieldglassError::Parse("global-heap object index exceeds u16".into()))?;
        let object =
            self.collections
                .read_object(self.source, collection, index, self.length_size)?;
        charge_metadata(&mut self.read, object.len() as u64, TEXT_BYTES)?;
        let text = usize::try_from(len)
            .ok()
            .and_then(|len| object.get(..len))
            .ok_or_else(|| {
                FieldglassError::Parse(
                    "variable-length string runs past its global-heap object".into(),
                )
            })?;
        // A byte that is not UTF-8 becomes U+FFFD, three bytes for one, so
        // the text kept can outgrow the object charged above.
        let text = decode_name(text);
        let grown = text.len().saturating_sub(object.len()) as u64;
        charge_metadata(&mut self.read, grown, TEXT_BYTES)?;
        Ok(text)
    }
}

/// Return the raw bytes (datatype message + dataspace + value) of the attribute
/// named `name` on the object header at `object_header_address`, or `None` if no
/// such attribute exists. Unlike [`list_attributes`], this does not decode the
/// datatype, so it serves the structural NetCDF-4 attributes.
pub fn raw_attribute<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
    name: &str,
) -> Result<Option<RawAttribute>, FieldglassError> {
    let mut file = ClaimedRanges::default();
    let bodies = message_bodies_in(source, object_header_address, probe, &mut file)?;
    raw_in_bodies(&bodies, name, probe.length_size)
}

/// The attribute named `name` among an object's message bodies, as
/// [`raw_attribute`] finds it once [`message_bodies_in`] has read them.
pub(crate) fn raw_in_bodies(
    bodies: &[Vec<u8>],
    name: &str,
    length_size: u8,
) -> Result<Option<RawAttribute>, FieldglassError> {
    for body in bodies {
        let Ok(split) = split_attribute_message(body) else {
            continue;
        };
        if split.name != name {
            continue;
        }
        let dataspace = dataspace::decode(split.dataspace_bytes, length_size)?;
        return Ok(Some(RawAttribute {
            name: split.name,
            datatype_bytes: split.datatype_bytes.to_vec(),
            dataspace,
            data: split.data.to_vec(),
        }));
    }
    Ok(None)
}

/// Collect the raw bodies of every Attribute message on an object header — both
/// inline (messages in the header) and dense (Attribute Info → fractal heap +
/// B-tree v2). The single source of attribute messages for both the human and
/// raw decode paths.
///
/// `file` is the file ranges the listing has read: the dense storage's heap,
/// B-tree and huge objects are claimed in it, so a listing that reads many
/// objects refuses two of them sharing one store (#921).
pub(crate) fn message_bodies_in<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
    file: &mut ClaimedRanges,
) -> Result<Vec<Vec<u8>>, FieldglassError> {
    let header = probe.header(source, object_header_address)?;

    let mut bodies: Vec<Vec<u8>> = header
        .messages
        .iter()
        .filter(|m| m.msg_type == MSG_ATTRIBUTE)
        .map(|m| m.body.clone())
        .collect();
    if let Some(msg) = header
        .messages
        .iter()
        .find(|m| m.msg_type == MSG_ATTRIBUTE_INFO)
    {
        read_dense_attribute_bodies(source, &msg.body, probe, &mut bodies, file)?;
    }
    Ok(bodies)
}

/// Append the dense attribute message bodies from an Attribute Info message.
fn read_dense_attribute_bodies<S: ByteSource + ?Sized>(
    source: &S,
    body: &[u8],
    probe: &Hdf5Probe,
    out: &mut Vec<Vec<u8>>,
    file: &mut ClaimedRanges,
) -> Result<(), FieldglassError> {
    let o = probe.offset_size as usize;
    // version (1) + flags (1), then optional max-creation-index (2).
    if body.len() < 2 {
        return Err(FieldglassError::Parse(
            "attribute info message too small".into(),
        ));
    }
    let flags = body[1];
    let mut pos = 2usize;
    if flags & 0x01 != 0 {
        pos += 2; // maximum creation index
    }
    let heap_addr = read_uint_le(body, pos, o)?;
    // No fractal heap ⇒ no dense attributes (everything was inline).
    if object_header::is_undefined_address(heap_addr, probe.offset_size) {
        return Ok(());
    }
    let btree_addr = read_uint_le(body, pos + o, o)?;

    let heap = FractalHeap::parse(
        source,
        heap_addr,
        probe.offset_size,
        probe.length_size,
        file,
    )?;
    // The format fixes an attribute record's layout: the 8-byte fractal-heap
    // ID, a message flag byte, a 4-byte creation order, and (type 8) a 4-byte
    // name hash (#895).
    let layout = |t: u8, size: usize| match (t, size) {
        (8, 17) | (9, 13) => Ok(()),
        (8 | 9, _) => Err(FieldglassError::Parse(format!(
            "B-tree v2 type {t} attribute records are {size} bytes; the format fixes {}",
            if t == 8 { 17 } else { 13 }
        ))),
        _ => Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {t} for attributes"
        ))),
    };
    let (btree_type, records) = heap::btree_v2_records_in(
        source,
        btree_addr,
        (probe.offset_size, probe.length_size),
        heap::BTreeRole::Attributes,
        &layout,
        file,
    )?;
    if btree_type != 8 && btree_type != 9 {
        return Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {btree_type} for attributes"
        )));
    }

    // One per listing, so no heap object is read twice (#899 review).
    let mut reads = heap::HeapReads::new(file);
    for record in records {
        let id = record
            .get(ATTR_RECORD_HEAP_ID_OFFSET..ATTR_RECORD_HEAP_ID_OFFSET + heap.heap_id_len)
            .ok_or_else(|| {
                FieldglassError::Parse("attribute record too small for a heap ID".into())
            })?;
        // An attribute past the heap-object cap is skipped, not the file
        // refused over it (#997), as `parse_bodies` skips one it cannot parse.
        if let Some(message) = heap.object_within_cap(source, id, &mut reads)? {
            out.push(message);
        }
    }
    Ok(())
}

/// The name + still-encoded sub-blocks of an Attribute message, borrowed from
/// the message body. Shared by the human ([`parse_attribute_message`]) and raw
/// ([`raw_attribute`]) decode paths so the version/padding handling lives once.
struct SplitAttribute<'a> {
    name: String,
    datatype_bytes: &'a [u8],
    dataspace_bytes: &'a [u8],
    data: &'a [u8],
}

/// Split an Attribute message body (versions 1, 2, and 3) into its name and the
/// raw datatype / dataspace / value blocks, handling the version-1 8-byte
/// padding and the version-3 name-charset byte.
fn split_attribute_message(body: &[u8]) -> Result<SplitAttribute<'_>, FieldglassError> {
    let mut cur = Cursor::over(body);
    let version = cur.byte()?;
    cur.skip(1)?; // reserved (v1) / flags (v2, v3)
    let name_size = cur.u16()? as usize;
    let datatype_size = cur.u16()? as usize;
    let dataspace_size = cur.u16()? as usize;

    // Version 1 pads each block to an 8-byte boundary; 2 and 3 don't. Version 3
    // adds a name character-set byte before the name.
    let padded = match version {
        1 => true,
        2 | 3 => false,
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported attribute message version {other}"
            )));
        }
    };
    if version == 3 {
        cur.skip(1)?; // name character set encoding
    }

    let name = decode_name(cur.take(name_size)?);
    skip_padding(&mut cur, name_size, padded)?;
    let datatype_bytes = cur.take(datatype_size)?;
    skip_padding(&mut cur, datatype_size, padded)?;
    let dataspace_bytes = cur.take(dataspace_size)?;
    skip_padding(&mut cur, dataspace_size, padded)?;
    let data = cur.remaining();
    Ok(SplitAttribute {
        name,
        datatype_bytes,
        dataspace_bytes,
        data,
    })
}

/// Parse a single Attribute message body into a human-readable [`Hdf5Attribute`].
/// A variable-length string's text is read through `heap`.
fn parse_attribute_message<S: ByteSource + ?Sized>(
    body: &[u8],
    length_size: u8,
    heap: &mut HeapText<'_, S>,
) -> Result<Hdf5Attribute, FieldglassError> {
    let split = split_attribute_message(body)?;
    let dataspace = dataspace::decode(split.dataspace_bytes, length_size)?;
    if let Some(datatype) = datatype::vlen_string_as_text(split.datatype_bytes) {
        let strings = heap.strings(split.data, element_count(&dataspace))?;
        return Ok(Hdf5Attribute {
            name: split.name,
            datatype,
            dataspace,
            value: render_strings(strings),
            values: Vec::new(),
        });
    }
    let datatype = datatype::decode(split.datatype_bytes)?;
    // The numbers come straight from the raw bytes, beside the display text:
    // value decode masks `_FillValue` against them rather than the rounded text.
    let (value, values) = render_value(split.data, &datatype, &dataspace, heap)?;
    Ok(Hdf5Attribute {
        name: split.name,
        datatype,
        dataspace,
        value,
        values,
    })
}

/// For version-1 messages, advance past the zero padding that rounds a block of
/// `size` bytes up to an 8-byte boundary.
fn skip_padding(cur: &mut Cursor, size: usize, padded: bool) -> Result<(), FieldglassError> {
    if padded {
        cur.skip(size.next_multiple_of(8) - size)?;
    }
    Ok(())
}

/// Render an attribute's raw value bytes to a display string. The CDL of a
/// string array is charged to `heap`'s text budget.
fn render_value<S: ByteSource + ?Sized>(
    data: &[u8],
    datatype: &Datatype,
    dataspace: &Dataspace,
    heap: &mut HeapText<'_, S>,
) -> Result<(String, Vec<f64>), FieldglassError> {
    match datatype.class {
        DatatypeClass::FixedLengthString => {
            let size = datatype.size as usize;
            let count = element_count(dataspace);
            if count <= 1 {
                let len = size.min(data.len());
                return Ok((decode_name(&data[..len]), Vec::new()));
            }
            // An array of fixed-length strings, as h5py writes a list of
            // `bytes`: every element, not only the first.
            let raw = count
                .checked_mul(size)
                .filter(|_| size > 0)
                .and_then(|total| data.get(..total))
                .ok_or_else(|| {
                    FieldglassError::Parse("string attribute value past end of message".into())
                })?;
            let strings: Vec<String> = raw.chunks_exact(size).map(decode_name).collect();
            // The escaping can make the CDL eight times the array: a
            // one-byte control character is `"\001", ` (#1549 review).
            heap.charge(strings.iter().map(|s| cdl_len(s)))?;
            Ok((render_strings(strings), Vec::new()))
        }
        DatatypeClass::FixedPoint | DatatypeClass::FloatingPoint => {
            let elem = datatype.size as usize;
            let count = element_count(dataspace);
            let total = count
                .checked_mul(elem)
                .ok_or_else(|| FieldglassError::Parse("attribute value size overflow".into()))?;
            let raw = data.get(..total).ok_or_else(|| {
                FieldglassError::Parse("attribute value past end of message".into())
            })?;
            // The classic renderer reads big-endian elements that hold just
            // the value. Rewrite through the datatype so byte order and a
            // fixed-point bit offset / precision are applied exactly as for
            // dataset values (#795).
            let normalized = datatype.to_classic_bytes(raw).ok_or_else(|| {
                FieldglassError::Parse("attribute value is not whole elements".into())
            })?;
            // Both from the one bounds-checked slice, so the text and the
            // numbers cannot disagree about how many elements there are.
            Ok((
                classic::render_numeric_values(&normalized, datatype.nc_type),
                classic::decode_elements_f64(&normalized, datatype.nc_type),
            ))
        }
    }
}

/// Number of elements in a dataspace (1 for scalar).
fn element_count(dataspace: &Dataspace) -> usize {
    if dataspace.is_scalar {
        return 1;
    }
    dataspace
        .dims
        .iter()
        .try_fold(1usize, |acc, &d| {
            usize::try_from(d).ok().and_then(|d| acc.checked_mul(d))
        })
        .unwrap_or(usize::MAX)
}

/// A string attribute's display value: the text of a single string, or the
/// CDL of several, as `ncdump` prints an `NC_STRING` array (`pr_att_vals`):
/// each element in double quotes, escaped as [`cdl_char`] says, separated by
/// `", "`. Joined bare, `["a, b"]` and `["a", "b"]` would read the same.
fn render_strings(mut strings: Vec<String>) -> String {
    if strings.len() <= 1 {
        return strings.pop().unwrap_or_default();
    }
    let mut out = String::with_capacity(strings.iter().map(|s| cdl_len(s)).sum());
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push('"');
        for c in s.chars() {
            match cdl_char(c) {
                Cdl::Plain(c) => out.push(c),
                Cdl::Named(escape) => out.push_str(escape),
                Cdl::Octal(byte) => out.push_str(&format!("\\{byte:03o}")),
            }
        }
        out.push('"');
    }
    out
}

/// How `ncdump` writes one character inside a quoted string attribute.
enum Cdl {
    /// As it is.
    Plain(char),
    /// A backslash escape.
    Named(&'static str),
    /// A control character, as a backslash and three octal digits.
    Octal(u8),
}

/// The CDL for `c`, as netCDF-C's `pr_att_string` writes it in a netCDF-4
/// file: the C escapes for backspace, form feed, newline, carriage return
/// and the tabs, a backslash before `\`, `'` and `"`, and any other control
/// character in octal. A NUL never reaches here: the text ends at one.
fn cdl_char(c: char) -> Cdl {
    match c {
        '\u{8}' => Cdl::Named("\\b"),
        '\u{c}' => Cdl::Named("\\f"),
        '\n' => Cdl::Named("\\n"),
        '\r' => Cdl::Named("\\r"),
        '\t' => Cdl::Named("\\t"),
        '\u{b}' => Cdl::Named("\\v"),
        '\\' => Cdl::Named("\\\\"),
        '\'' => Cdl::Named("\\'"),
        '"' => Cdl::Named("\\\""),
        // C's `iscntrl` in the C locale: 0x00 to 0x1F and DEL. A byte past
        // ASCII is part of a UTF-8 character, which ncdump prints as it is.
        c if c.is_ascii_control() => Cdl::Octal(c as u8),
        c => Cdl::Plain(c),
    }
}

/// The length of `s` as one quoted CDL element with the `", "` that may
/// precede it, as [`render_strings`] writes it: what the budget charges.
fn cdl_len(s: &str) -> usize {
    let escaped: usize = s
        .chars()
        .map(|c| match cdl_char(c) {
            Cdl::Plain(c) => c.len_utf8(),
            Cdl::Named(escape) => escape.len(),
            Cdl::Octal(_) => 4,
        })
        .sum();
    escaped + 4
}

fn push_attribute(
    out: &mut Vec<Hdf5Attribute>,
    attr: Hdf5Attribute,
) -> Result<(), FieldglassError> {
    if out.len() >= MAX_ATTRIBUTES {
        return Err(FieldglassError::Parse(
            "object has too many attributes".into(),
        ));
    }
    out.push(attr);
    Ok(())
}

/// Decode a possibly null-terminated, possibly null-padded name as UTF-8.
fn decode_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NcType;

    /// Parse an attribute message that reads no global heap.
    fn parse(msg: &[u8]) -> Result<Hdf5Attribute, FieldglassError> {
        parse_attribute_message(
            msg,
            8,
            &mut HeapText::new(&[][..], &Hdf5Probe::new(0, 8, 8)),
        )
    }

    /// Build an inline (version 1, padded) attribute message.
    fn attr_v1(name: &str, datatype: &[u8], dataspace: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = vec![1u8, 0];
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&(datatype.len() as u16).to_le_bytes());
        v.extend_from_slice(&(dataspace.len() as u16).to_le_bytes());
        let pad = |v: &mut Vec<u8>, n: usize| v.resize(v.len() + (n.next_multiple_of(8) - n), 0);
        v.extend_from_slice(name.as_bytes());
        pad(&mut v, name.len());
        v.extend_from_slice(datatype);
        pad(&mut v, datatype.len());
        v.extend_from_slice(dataspace);
        pad(&mut v, dataspace.len());
        v.extend_from_slice(data);
        v
    }

    /// Build a version-3 (charset byte, unpadded) attribute message.
    fn attr_v3(name: &str, datatype: &[u8], dataspace: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = vec![3u8, 0];
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&(datatype.len() as u16).to_le_bytes());
        v.extend_from_slice(&(dataspace.len() as u16).to_le_bytes());
        v.push(0); // name charset
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(datatype);
        v.extend_from_slice(dataspace);
        v.extend_from_slice(data);
        v
    }

    /// Fixed-point datatype message (signed, given size and bit field),
    /// full precision at offset 0.
    fn dt_int_with(bit_field: u8, size: u32) -> Vec<u8> {
        let mut v = vec![(1 << 4), bit_field, 0, 0]; // class 0
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // bit offset
        v.extend_from_slice(&((size * 8) as u16).to_le_bytes()); // bit precision
        v
    }
    /// Little-endian fixed-point datatype message (signed, given size).
    fn dt_int(size: u32) -> Vec<u8> {
        dt_int_with(0x08, size)
    }
    /// IEEE binary64, little-endian, with its layout properties.
    fn dt_double() -> Vec<u8> {
        // class 1; normalization "implied" (bits 4-5 = 2); sign at bit 63.
        let mut v = vec![(1 << 4) | 1, 0x20, 63, 0];
        v.extend_from_slice(&8u32.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // bit offset
        v.extend_from_slice(&64u16.to_le_bytes()); // bit precision
        v.extend_from_slice(&[52, 11, 0, 52]); // exponent loc/size, mantissa loc/size
        v.extend_from_slice(&1023u32.to_le_bytes()); // exponent bias
        v
    }
    fn dt_string(size: u32) -> Vec<u8> {
        let mut v = vec![(1 << 4) | 3, 0x00, 0, 0]; // class 3
        v.extend_from_slice(&size.to_le_bytes());
        v
    }
    fn ds_scalar() -> Vec<u8> {
        vec![1u8, 0, 0, 0, 0, 0, 0, 0]
    }

    #[test]
    fn decodes_v1_scalar_int() {
        let msg = attr_v1("version", &dt_int(4), &ds_scalar(), &5i32.to_le_bytes());
        let a = parse(&msg).unwrap();
        assert_eq!(a.name, "version");
        assert_eq!(a.datatype.nc_type, NcType::Int);
        assert_eq!(a.value, "5");
    }

    #[test]
    fn decodes_v3_scalar_double() {
        let msg = attr_v3("scale", &dt_double(), &ds_scalar(), &0.25f64.to_le_bytes());
        let a = parse(&msg).unwrap();
        assert_eq!(a.name, "scale");
        assert_eq!(a.datatype.nc_type, NcType::Double);
        assert_eq!(a.value, "0.25");
    }

    #[test]
    fn decodes_string_attribute() {
        let text = b"meters";
        let msg = attr_v3("units", &dt_string(text.len() as u32), &ds_scalar(), text);
        let a = parse(&msg).unwrap();
        assert_eq!(a.name, "units");
        assert_eq!(a.value, "meters");
    }

    #[test]
    fn decodes_big_endian_value() {
        // Big-endian datatype: byte-order bit set; value bytes are big-endian.
        let dt = dt_int_with(0x09, 4); // signed + big-endian
        let msg = attr_v3("be", &dt, &ds_scalar(), &7i32.to_be_bytes());
        let a = parse(&msg).unwrap();
        assert_eq!(a.value, "7");
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut msg = attr_v3("x", &dt_int(4), &ds_scalar(), &1i32.to_le_bytes());
        msg[0] = 9; // version 9
        assert!(parse(&msg).is_err());
    }

    #[test]
    fn rejects_truncated_value() {
        // Declares a 4-byte int but supplies only 2 value bytes.
        let msg = attr_v3("short", &dt_int(4), &ds_scalar(), &[1, 2]);
        assert!(parse(&msg).is_err());
    }

    // Variable-length string attributes (#1549). The file is a global-heap
    // collection at offset 0, as `global_heap`'s own tests lay one out, and
    // each attribute message holds IDs into it.

    /// A `GCOL` collection at offset 0 holding `objects` as `(index, data)`,
    /// then the index-0 free-space object that ends the run.
    fn collection(objects: &[(u16, &[u8])]) -> Vec<u8> {
        let mut v = b"GCOL".to_vec();
        v.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
        v.extend_from_slice(&0u64.to_le_bytes()); // collection size, patched below
        for &(index, data) in objects {
            v.extend_from_slice(&index.to_le_bytes());
            v.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
            v.extend_from_slice(&(data.len() as u64).to_le_bytes());
            v.extend_from_slice(data);
            v.resize(v.len().next_multiple_of(8), 0);
        }
        v.extend_from_slice(&[0u8; 16]); // free-space object
        let total = v.len() as u64;
        v[8..16].copy_from_slice(&total.to_le_bytes());
        v
    }

    /// A variable-length string datatype (class 9, type 1, UTF-8) over a
    /// one-byte base, as libhdf5 writes `NC_STRING`.
    fn dt_vlen_string() -> Vec<u8> {
        let mut v = vec![(1 << 4) | 9, 0x01, 0x01, 0];
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&dt_string(1));
        v
    }

    /// A variable-length sequence (type 0) of the same base: not a string.
    fn dt_vlen_sequence() -> Vec<u8> {
        let mut v = dt_vlen_string();
        v[1] = 0x00;
        v
    }

    /// A version-1 simple dataspace of one dimension of `n`.
    fn ds_1d(n: u64) -> Vec<u8> {
        let mut v = vec![1u8, 1, 0, 0, 0, 0, 0, 0];
        v.extend_from_slice(&n.to_le_bytes());
        v
    }

    /// One global-heap ID: string length, collection address, object index.
    fn id(len: u32, collection: u64, index: u32) -> Vec<u8> {
        let mut v = len.to_le_bytes().to_vec();
        v.extend_from_slice(&collection.to_le_bytes());
        v.extend_from_slice(&index.to_le_bytes());
        v
    }

    /// The attributes `parse_bodies` lists from `bodies` over `file`.
    fn listed(file: &[u8], bodies: &[Vec<u8>]) -> Vec<(String, String)> {
        parse_bodies(bodies, &mut HeapText::new(file, &Hdf5Probe::new(0, 8, 8)))
            .unwrap()
            .into_iter()
            .map(|a| (a.name, a.value))
            .collect()
    }

    fn vlen_parse(file: &[u8], msg: &[u8]) -> Result<Hdf5Attribute, FieldglassError> {
        parse_attribute_message(msg, 8, &mut HeapText::new(file, &Hdf5Probe::new(0, 8, 8)))
    }

    #[test]
    fn decodes_scalar_and_one_element_vlen_strings() {
        let file = collection(&[(1, b"noleap"), (2, b"K")]);
        let scalar = attr_v1("calendar", &dt_vlen_string(), &ds_scalar(), &id(6, 0, 1));
        let one = attr_v3("units", &dt_vlen_string(), &ds_1d(1), &id(1, 0, 2));
        let a = vlen_parse(&file, &scalar).unwrap();
        assert_eq!(a.value, "noleap");
        assert_eq!(a.datatype.nc_type, NcType::Char);
        assert!(a.values.is_empty() && !a.is_string_array());
        let a = vlen_parse(&file, &one).unwrap();
        assert_eq!(a.value, "K");
        assert!(!a.is_string_array());
    }

    /// The string is the ID's length of the object's bytes, and stops at a NUL
    /// as netCDF-C's C string does.
    #[test]
    fn a_vlen_string_takes_the_ids_length_of_its_object() {
        let file = collection(&[(1, b"abc\0def")]);
        let short = attr_v3("s", &dt_vlen_string(), &ds_scalar(), &id(2, 0, 1));
        assert_eq!(vlen_parse(&file, &short).unwrap().value, "ab");
        let nul = attr_v3("s", &dt_vlen_string(), &ds_scalar(), &id(7, 0, 1));
        assert_eq!(vlen_parse(&file, &nul).unwrap().value, "abc");
    }

    #[test]
    fn several_vlen_strings_are_their_cdl() {
        let file = collection(&[(1, b"a"), (2, b"b, c"), (3, br#"q"\x"#)]);
        let mut data = id(1, 0, 1);
        data.extend(id(4, 0, 2));
        data.extend(id(4, 0, 3));
        let msg = attr_v3("tags", &dt_vlen_string(), &ds_1d(3), &data);
        let a = vlen_parse(&file, &msg).unwrap();
        assert_eq!(a.value, r#""a", "b, c", "q\"\\x""#);
        assert!(a.is_string_array());
    }

    /// Every character netCDF-C's `pr_att_string` escapes is escaped the same
    /// way: the named C escapes, `'`, and other control characters in octal.
    /// Text past ASCII is printed as it is.
    #[test]
    fn a_string_list_escapes_as_ncdump_does() {
        let strings = [
            "line1\nline2",
            "tab\there",
            "it's",
            "\u{8}\u{c}\r\u{b}",
            "\u{1}bell\u{7}\u{1b}\u{7f}",
            "q\"\\",
            "température",
        ];
        let got = render_strings(strings.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            got,
            r#""line1\nline2", "tab\there", "it\'s", "\b\f\r\v", "\001bell\007\033\177", "q\"\\", "température""#
        );
        // And the budget charges exactly what is written.
        let charged: usize = strings.iter().map(|s| cdl_len(s)).sum();
        assert_eq!(
            charged,
            got.len() + 2,
            "every element but the first has a \", \""
        );
    }

    /// The CDL of a list is held to the text budget, not only the text it
    /// escapes: two references to a 4 MiB object of `"` read 8 MiB of text
    /// but write 16 MiB of CDL, past the cap, so the list is refused. The same
    /// two references to plain text fit.
    #[test]
    fn a_string_list_is_capped_on_its_escaped_length() {
        const OBJECT: usize = 4 << 20;
        let plain = vec![b'x'; OBJECT];
        let quotes = vec![b'"'; OBJECT];
        let file = collection(&[(1, &plain), (2, &quotes)]);
        let list = |index: u32| {
            let mut data = id(OBJECT as u32, 0, index);
            data.extend(id(OBJECT as u32, 0, index));
            attr_v3("list", &dt_vlen_string(), &ds_1d(2), &data)
        };
        let fits = vlen_parse(&file, &list(1)).unwrap();
        assert_eq!(fits.value.len(), 2 * OBJECT + 6);
        let err = vlen_parse(&file, &list(2)).unwrap_err();
        assert!(err.to_string().contains("string attribute text"), "{err}");
    }

    /// A fixed-length string array's CDL is charged to the listing's text
    /// budget too. A 60,000-element array of one-byte `\x01` strings is
    /// 60,000 bytes in the file and 480,000 of CDL (`"\001", ` each), so of
    /// 35 such attributes in one listing, 34 fit under the cap and the last
    /// is skipped.
    #[test]
    fn a_fixed_length_string_array_is_capped_on_its_cdl() {
        const COUNT: usize = 60_000;
        let msg = attr_v3("s", &dt_string(1), &ds_1d(COUNT as u64), &[1u8; COUNT]);
        let one = parse(&msg).unwrap();
        assert_eq!(one.value.len(), 8 * COUNT - 2);
        assert!(one.value.starts_with(r#""\001", "\001""#));
        let per = 8 * COUNT;
        let fit = (super::super::source::MAX_METADATA_BYTES as usize) / per;
        let bodies: Vec<Vec<u8>> = (0..=fit)
            .map(|i| {
                let name = format!("s{i:02}");
                attr_v3(&name, &dt_string(1), &ds_1d(COUNT as u64), &[1u8; COUNT])
            })
            .collect();
        assert_eq!(listed(&[], &bodies).len(), fit);
    }

    /// Text that is not UTF-8 grows as it is decoded, each byte becoming a
    /// three-byte U+FFFD, and the growth is charged: a 6 MiB object of such
    /// bytes is 18 MiB of text, past the cap.
    #[test]
    fn non_utf8_text_is_capped_on_its_decoded_length() {
        const OBJECT: usize = 6 << 20;
        let bytes = vec![0xffu8; OBJECT];
        let file = collection(&[(1, &bytes)]);
        let msg = attr_v3(
            "s",
            &dt_vlen_string(),
            &ds_scalar(),
            &id(OBJECT as u32, 0, 1),
        );
        let err = vlen_parse(&file, &msg).unwrap_err();
        assert!(err.to_string().contains("string attribute text"), "{err}");
    }

    /// An empty string is a zero length and no heap object; libhdf5 writes the
    /// address as zero. Nothing is read, so a file with no heap at all serves.
    #[test]
    fn a_zero_length_vlen_string_is_empty_without_a_heap_read() {
        let msg = attr_v3("comment", &dt_vlen_string(), &ds_scalar(), &id(0, 0, 0));
        assert_eq!(vlen_parse(&[], &msg).unwrap().value, "");
        let undefined = attr_v3(
            "comment",
            &dt_vlen_string(),
            &ds_scalar(),
            &id(0, u64::MAX, 0),
        );
        assert_eq!(vlen_parse(&[], &undefined).unwrap().value, "");
    }

    /// Text that is not UTF-8 reads with replacement characters, as every
    /// other text attribute in this crate does, rather than being dropped.
    #[test]
    fn non_utf8_vlen_text_is_replaced_not_dropped() {
        let file = collection(&[(1, &[b'a', 0xff, b'b'])]);
        let msg = attr_v3("s", &dt_vlen_string(), &ds_scalar(), &id(3, 0, 1));
        assert_eq!(vlen_parse(&file, &msg).unwrap().value, "a\u{fffd}b");
    }

    /// Every malformed ID refuses the attribute without a panic, and the
    /// listing skips it while keeping its neighbors.
    #[test]
    fn malformed_vlen_ids_are_refused_and_skipped() {
        let file = collection(&[(1, b"noleap")]);
        let attr = |data: &[u8], ds: &[u8]| attr_v3("bad", &dt_vlen_string(), ds, data);
        let mut cut = file.clone();
        cut.truncate(20); // the collection's header claims more than the file has
        let cases: Vec<(&str, &[u8], Vec<u8>)> = vec![
            (
                "address past the file",
                &file,
                attr(&id(6, 1 << 40, 1), &ds_scalar()),
            ),
            (
                "address not a collection",
                &file,
                attr(&id(6, 8, 1), &ds_scalar()),
            ),
            (
                "index not in the collection",
                &file,
                attr(&id(6, 0, 9), &ds_scalar()),
            ),
            (
                "index 0, the free space",
                &file,
                attr(&id(6, 0, 0), &ds_scalar()),
            ),
            (
                "index past u16",
                &file,
                attr(&id(6, 0, 1 << 16 | 1), &ds_scalar()),
            ),
            (
                "length past the object",
                &file,
                attr(&id(7, 0, 1), &ds_scalar()),
            ),
            (
                "length near u32::MAX",
                &file,
                attr(&id(u32::MAX, 0, 1), &ds_scalar()),
            ),
            (
                "collection truncated",
                &cut,
                attr(&id(6, 0, 1), &ds_scalar()),
            ),
            (
                "ID cut short",
                &file,
                attr(&id(6, 0, 1)[..10], &ds_scalar()),
            ),
            (
                "more elements than IDs",
                &file,
                attr(&id(6, 0, 1), &ds_1d(2)),
            ),
            (
                "element count past usize",
                &file,
                attr(&id(6, 0, 1), &ds_1d(u64::MAX)),
            ),
        ];
        let good = attr_v3("units", &dt_string(1), &ds_scalar(), b"K");
        for (what, file, msg) in cases {
            let err = vlen_parse(file, &msg).expect_err(what);
            assert!(!matches!(err, FieldglassError::Io(_)), "{what}: {err}");
            assert_eq!(
                listed(file, &[msg, good.clone()]),
                [("units".to_string(), "K".to_string())],
                "{what}"
            );
        }
    }

    /// One object's attributes are held to `MAX_METADATA_BYTES` of text
    /// together, however often they refer to one heap object: past it, the
    /// rest are skipped rather than read.
    #[test]
    fn vlen_text_on_one_object_is_capped_together() {
        const OBJECT: usize = 1 << 20;
        let big = vec![b'x'; OBJECT];
        let file = collection(&[(1, &big)]);
        let refs = (super::super::source::MAX_METADATA_BYTES as usize) / OBJECT;
        let bodies: Vec<Vec<u8>> = (0..=refs)
            .map(|i| {
                let name = format!("a{i:03}");
                attr_v3(
                    &name,
                    &dt_vlen_string(),
                    &ds_scalar(),
                    &id(OBJECT as u32, 0, 1),
                )
            })
            .collect();
        let got = listed(&file, &bodies);
        assert_eq!(got.len(), refs, "the attribute past the cap is skipped");
        assert!(got.iter().all(|(_, v)| v.len() == OBJECT));
    }

    /// The budget spans the listing, not one object: objects listed through
    /// one reader, each referring once to the same large heap object, stop
    /// reading it at the cap rather than each costing its size.
    #[test]
    fn vlen_text_is_capped_across_the_objects_of_one_listing() {
        const OBJECT: usize = 1 << 20;
        let big = vec![b'x'; OBJECT];
        let file = collection(&[(1, &big)]);
        let probe = Hdf5Probe::new(0, 8, 8);
        let mut text = HeapText::new(file.as_slice(), &probe);
        let one = [attr_v3(
            "a",
            &dt_vlen_string(),
            &ds_scalar(),
            &id(OBJECT as u32, 0, 1),
        )];
        let refs = (super::super::source::MAX_METADATA_BYTES as usize) / OBJECT;
        let listed: usize = (0..=refs)
            .map(|_| parse_bodies(&one, &mut text).unwrap().len())
            .sum();
        assert_eq!(listed, refs);
    }

    /// A numbers-only listing, which a value decode uses for `_FillValue`,
    /// reads no global heap and leaves the string out.
    #[test]
    fn a_numbers_only_listing_reads_no_text() {
        let file = collection(&[(1, b"noleap")]);
        let source = fieldglass_core::testing::Recording::new(file.as_slice());
        let bodies = [
            attr_v3("calendar", &dt_vlen_string(), &ds_scalar(), &id(6, 0, 1)),
            attr_v3(
                "_FillValue",
                &dt_int(4),
                &ds_scalar(),
                &(-1i32).to_le_bytes(),
            ),
        ];
        let probe = Hdf5Probe::new(0, 8, 8);
        let got = parse_bodies(&bodies, &mut HeapText::numbers_only(&source, &probe)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].first_value(), Some(-1.0));
        assert!(source.reads().is_empty());
        let got = parse_bodies(&bodies, &mut HeapText::new(&source, &probe)).unwrap();
        assert_eq!(got.len(), 2);
        assert!(!source.reads().is_empty());
    }

    /// A vlen sequence is not text, and stays outside the human listing
    /// (`DIMENSION_LIST` is one, read through `raw_attribute`).
    #[test]
    fn a_vlen_sequence_is_not_read_as_text() {
        let file = collection(&[(1, b"noleap")]);
        let msg = attr_v3("seq", &dt_vlen_sequence(), &ds_scalar(), &id(6, 0, 1));
        assert!(vlen_parse(&file, &msg).is_err());
    }

    /// An array of fixed-length strings, as h5py writes a list of `bytes`,
    /// reads every element; a truncated one is refused.
    #[test]
    fn a_fixed_length_string_array_reads_every_element() {
        let msg = attr_v3("fixed", &dt_string(3), &ds_1d(2), b"ab\0cde");
        let a = parse(&msg).unwrap();
        assert_eq!(a.value, r#""ab", "cde""#);
        assert!(a.is_string_array());
        let short = attr_v3("fixed", &dt_string(3), &ds_1d(2), b"ab\0cd");
        assert!(parse(&short).is_err());
        let empty = attr_v3("fixed", &dt_string(0), &ds_1d(2), b"");
        assert!(parse(&empty).is_err());
    }
}
