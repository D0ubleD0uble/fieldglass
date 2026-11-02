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
use super::source::{ClaimedRanges, Cursor, Fields, MAX_METADATA_BYTES, charge_metadata};
use crate::classic;
use fieldglass_core::FieldglassError;
use fieldglass_core::array::ArrayError;
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
///
/// # Errors
///
/// [`ArrayError::ListingTooLarge`] when the object's string attributes hold
/// more text than one listing reads (16 MiB), or one attribute is larger
/// than one heap object may be, rather than a list that leaves the rest out
/// without saying so (#1556). [`FieldglassError::Parse`] when a string's
/// global-heap reference is damaged: it names no object, one that two
/// objects carry, or one of another length (#1569).
///
/// This stays strict: any attribute not read is an error, even a
/// descriptive one such as `long_name` for which the dataset view keeps the
/// variable. A caller of this function has no other way to learn of the
/// loss.
pub fn list_attributes<S: ByteSource + ?Sized>(
    source: &S,
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
    let mut file = ClaimedRanges::default();
    let bodies = message_bodies_in(source, object_header_address, probe, &mut file)?;
    bodies.parse(&mut HeapText::new(source, probe))?.complete()
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
    // Only a string can pass the text budget or be damaged in the global
    // heap, and a value decode reads none, so what the listing reports left
    // out here is nothing it would have used. A dense message past the cap
    // on one heap object is the exception, and the full listing reports it:
    // its variable is left out of the view (#1569).
    Ok(bodies
        .parse(&mut HeapText::numbers_only(source, probe))?
        .attributes)
}

/// One object's attributes as [`parse_bodies`] lists them: the ones it read,
/// and the ones a listing cap left out.
#[derive(Debug)]
pub(crate) struct Listed {
    /// The attributes read, sorted by name.
    pub(crate) attributes: Vec<Hdf5Attribute>,
    /// The attributes a listing cap left out, if any.
    pub(crate) unread: Option<Unread>,
}

/// The attributes of one object that a listing cap left out (#1556), named
/// under each refusal they met.
///
/// A cap, unlike a datatype this reader does not decode, leaves out
/// attributes that are fine, such as a `calendar` the text budget has no
/// room left for. Left out without a word, a non-standard calendar read as
/// absent and its times as Gregorian (#1033).
///
/// One object can meet more than one cap: a string past the text budget and
/// another past the global-heap object budget, or a heap object larger than
/// any one listing reads. Each attribute is named with its own refusal
/// (#1568), so the report does not blame one cap for what another cut.
///
/// A string attribute whose global-heap reference is damaged is named here
/// too (#1569), under a refusal of its own that says the file is at fault: a
/// `calendar` lost that way dated `noleap` times as Gregorian just as one
/// past a cap did. So is a dense attribute message past the cap on one heap
/// object (#997), which is counted rather than named: its name is in the
/// part not read.
#[derive(Debug)]
pub(crate) struct Unread {
    /// Each distinct refusal, in the order first met, with the attributes
    /// it left out.
    cuts: Vec<Cut>,
}

/// The attributes one refusal left out of an object: names sorted once the
/// listing is done.
#[derive(Debug)]
struct Cut {
    refusal: Refusal,
    names: Vec<String>,
    /// Attributes left out whose names were not read.
    unnamed: usize,
}

/// Why attributes were left out of a listing.
#[derive(Debug)]
enum Refusal {
    /// A listing cap: the attributes are fine, and the file with them
    /// (ADR-0016 decision 4).
    Cap(FieldglassError),
    /// Each string's text cannot be read for certain: its global-heap ID
    /// names no object, one that two objects carry (#1557), or one of
    /// another length, or the IDs inline in the message are malformed. The
    /// error is the first one met.
    Damaged(FieldglassError),
}

impl Refusal {
    /// Whether `other` says the same thing, so the attributes it left out
    /// join this one's.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Cap(a), Self::Cap(b)) => same_refusal(a, b),
            // Each damaged reference has its own detail, the object index
            // among it; one line for all of them keeps an object with a
            // million damaged attributes to one comparison each.
            (Self::Damaged(_), Self::Damaged(_)) => true,
            _ => false,
        }
    }

    fn into_error(self) -> FieldglassError {
        match self {
            Self::Cap(err) | Self::Damaged(err) => err,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cap(err) => write!(f, "{err}"),
            // Neutral on where: the IDs inline in the attribute message, or
            // the global-heap objects they name. The error says which.
            Self::Damaged(err) => write!(f, "the file's record of the text is damaged ({err})"),
        }
    }
}

/// The attributes a variable can lose and still be drawn (#1569): text for
/// the reader, which no reader of the file computes with. None of them
/// changes a value, a mask, a position or a time, so a variable that loses
/// only these is drawn exactly as it would be with them.
///
/// - `long_name`, `title`, `description`: a name or summary for display (CF
///   §3.2, NUG; `description` is the common non-CF spelling).
/// - `comment`, `history`, `source`, `references`, `institution`: provenance
///   and free text (CF §2.6.2, NUG).
///
/// Spelled as CF and the NUG spell them, and matched exactly: `Units` is not
/// `units`, and is not on this list either. Attributes whose name starts
/// with [`NOTE_PREFIX`] count as well.
const DESCRIPTIVE_ATTRIBUTES: [&str; 8] = [
    "long_name",
    "title",
    "description",
    "comment",
    "history",
    "source",
    "references",
    "institution",
];

/// The start of a free-text note's name (`note`, `notes`, `note_00`): no
/// convention gives it a meaning, so it is descriptive like
/// [`DESCRIPTIVE_ATTRIBUTES`].
const NOTE_PREFIX: &str = "note";

/// Whether losing the attribute `name` leaves what is drawn unchanged.
fn is_descriptive(name: &str) -> bool {
    DESCRIPTIVE_ATTRIBUTES.contains(&name) || name.starts_with(NOTE_PREFIX)
}

/// How many of the unread attributes [`Cut::reason`] names before it counts
/// the rest.
const NAMED_UNREAD: usize = 8;

impl Unread {
    /// Whether the object that lost these attributes is left out of the
    /// view, named with them (#1569, the owner's decision on the issue).
    ///
    /// It is kept only when every attribute lost is one the user reads and
    /// nothing draws with: a name each, all on [`DESCRIPTIVE_ATTRIBUTES`] or
    /// a note. Any other loss leaves it out, since a lost `calendar`,
    /// `units`, `_FillValue` or placement attribute changes what is drawn,
    /// and so does an attribute counted without a name. A kept object is
    /// listed without what it lost: there is no channel yet to name the loss
    /// on a variable that stays.
    pub(crate) fn leaves_out_the_object(&self) -> bool {
        !self
            .cuts
            .iter()
            .all(|cut| cut.unnamed == 0 && cut.names.iter().all(|n| is_descriptive(n)))
    }

    /// Name `name` as left out by `refusal`, beside the others that refusal
    /// left out. `None` counts an attribute whose name was not read.
    fn push(&mut self, name: Option<String>, refusal: Refusal) {
        let at = match self.cuts.iter().position(|cut| cut.refusal.same(&refusal)) {
            Some(at) => at,
            None => {
                self.cuts.push(Cut {
                    refusal,
                    names: Vec::new(),
                    unnamed: 0,
                });
                self.cuts.len() - 1
            }
        };
        let cut = &mut self.cuts[at];
        match name {
            Some(name) => cut.names.push(name),
            None => cut.unnamed += 1,
        }
    }

    /// Why the object's description is incomplete, naming what it lacks
    /// under each refusal, in the order the refusals were met.
    pub(crate) fn reason(&self) -> String {
        let reasons: Vec<String> = self.cuts.iter().map(Cut::reason).collect();
        reasons.join("; ")
    }
}

/// Whether `a` and `b` say the same thing: two refusals by one cap name what
/// it counts and its limit, and nothing about the attribute. Compared by
/// those fields, not by the text, since an object can have a million
/// attributes left out.
fn same_refusal(a: &FieldglassError, b: &FieldglassError) -> bool {
    match (a, b) {
        (
            FieldglassError::Array(ArrayError::ListingTooLarge { what, limit }),
            FieldglassError::Array(ArrayError::ListingTooLarge {
                what: other,
                limit: other_limit,
            }),
        ) => what == other && limit == other_limit,
        _ => a.to_string() == b.to_string(),
    }
}

impl Cut {
    /// The attributes this refusal left out, and the refusal.
    fn reason(&self) -> String {
        let shown: Vec<String> = self
            .names
            .iter()
            .take(NAMED_UNREAD)
            .map(|n| format!("`{n}`"))
            .collect();
        let total = self.names.len() + self.unnamed;
        let more = total - shown.len();
        let noun = if total == 1 {
            "attribute"
        } else {
            "attributes"
        };
        let names = match (shown.is_empty(), more) {
            // Only unnamed ones: say how many.
            (true, _) if total == 1 => "an attribute".to_string(),
            (true, _) => format!("{total} {noun}"),
            (false, 0) => format!("{noun} {}", shown.join(", ")),
            (false, more) => format!("{noun} {} and {more} more", shown.join(", ")),
        };
        format!("{names} not read: {}", self.refusal)
    }
}

impl Listed {
    /// The attributes, or the refusal when a cap or damage left any out: for
    /// a listing with no other way to say an attribute is missing.
    pub(crate) fn complete(self) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
        // One error says one thing: the first refusal met.
        match self
            .unread
            .and_then(|unread| unread.cuts.into_iter().next())
        {
            None => Ok(self.attributes),
            Some(cut) => Err(cut.refusal.into_error()),
        }
    }
}

/// The attributes in an object's message bodies, sorted by name: what
/// [`list_attributes`] returns once [`message_bodies_in`] has read them. A
/// variable-length string's text is read through `heap`, which a listing of
/// many objects shares.
pub(crate) fn parse_bodies<S: ByteSource + ?Sized>(
    bodies: &[Vec<u8>],
    heap: &mut HeapText<'_, S>,
) -> Result<Listed, FieldglassError> {
    let length_size = heap.length_size;
    let mut attrs = Vec::new();
    let mut unread: Option<Unread> = None;
    // An attribute we can't parse — most often one whose datatype we don't decode
    // (e.g. a NetCDF-4 `DIMENSION_LIST`, which is a variable-length sequence),
    // but a malformed one too, such as a string whose global-heap ID points
    // nowhere — is skipped rather than failing the whole list, so the rest of
    // an object's attributes, and its value decode, still come through. A
    // failed read of the source is not the file's fault and is not skipped.
    //
    // One past a listing cap is skipped too, but named in `unread` (#1556):
    // it is an attribute that is fine, which the caller has to say is
    // missing. So is a string whose global-heap reference is damaged
    // (#1569): it is an attribute the file has, such as a `calendar`, and
    // dropped without a word it read as absent.
    for body in bodies {
        let refusal = match heap.kept_only(|heap| parse_attribute_message(body, length_size, heap))
        {
            Ok(attr) => {
                push_attribute(&mut attrs, attr)?;
                continue;
            }
            Err(NotRead::Io(err)) => return Err(err),
            Err(NotRead::Skipped) => continue,
            Err(NotRead::Refused(err)) => Refusal::Cap(err),
            Err(NotRead::Damaged(err)) => Refusal::Damaged(err),
        };
        // The message split before its text was read, so it splits again.
        let name = split_attribute_message(body)?.name;
        unread
            .get_or_insert_with(|| Unread { cuts: Vec::new() })
            .push(Some(name), refusal);
    }
    attrs.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(unread) = &mut unread {
        for cut in &mut unread.cuts {
            cut.names.sort();
        }
    }
    Ok(Listed {
        attributes: attrs,
        unread,
    })
}

/// Why [`parse_attribute_message`] did not return an attribute, which decides
/// what [`parse_bodies`] says about it.
#[derive(Debug)]
enum NotRead {
    /// Left out without a word: a datatype the listing does not show, such
    /// as a `DIMENSION_LIST`, or a message it cannot parse.
    Skipped,
    /// A string whose global-heap reference is damaged: the attribute is
    /// named as left out, and the file blamed (#1569).
    Damaged(FieldglassError),
    /// A listing cap: the attribute is named as left out, and the file is
    /// fine (#1556).
    Refused(FieldglassError),
    /// A failed or short read of the source, which is not the file's fault:
    /// the listing fails.
    Io(FieldglassError),
}

impl NotRead {
    /// An error met reading a string's text from the global heap: one the
    /// file is at fault for is damage there.
    fn in_heap(err: FieldglassError) -> Self {
        match err {
            FieldglassError::Parse(_) => Self::Damaged(err),
            other => other.into(),
        }
    }

    /// The error, for a test; a skipped attribute keeps none, so it gets a
    /// stand-in.
    #[cfg(test)]
    fn into_error(self) -> FieldglassError {
        match self {
            Self::Skipped => FieldglassError::UnsupportedSection("attribute skipped".into()),
            Self::Damaged(err) | Self::Refused(err) | Self::Io(err) => err,
        }
    }
}

impl From<FieldglassError> for NotRead {
    fn from(err: FieldglassError) -> Self {
        match err {
            // A source that failed or served short is not the file's fault,
            // and a host retries a short read (#707): the listing fails
            // rather than leave the attribute out as if the file lacked it.
            FieldglassError::Io(_) | FieldglassError::ShortRead { .. } => Self::Io(err),
            FieldglassError::Array(ArrayError::ListingTooLarge { .. }) => Self::Refused(err),
            _ => Self::Skipped,
        }
    }
}

/// An object's attribute messages, as [`message_bodies_in`] reads them.
#[derive(Debug, Default)]
pub(crate) struct Bodies {
    /// Each attribute message read.
    pub(crate) messages: Vec<Vec<u8>>,
    /// The cap each dense attribute message past the cap on one heap object
    /// met (#997), as the `what` and `limit` of its `ListingTooLarge`. Its
    /// name is in the part not read.
    past_cap: Vec<(&'static str, u64)>,
}

impl Bodies {
    /// The attributes these messages hold, as [`parse_bodies`] lists them,
    /// with each message past the heap-object cap counted among the unread
    /// (#1569).
    pub(crate) fn parse<S: ByteSource + ?Sized>(
        &self,
        heap: &mut HeapText<'_, S>,
    ) -> Result<Listed, FieldglassError> {
        let mut listed = parse_bodies(&self.messages, heap)?;
        for &(what, limit) in &self.past_cap {
            let refusal = ArrayError::ListingTooLarge { what, limit };
            listed
                .unread
                .get_or_insert_with(|| Unread { cuts: Vec::new() })
                .push(None, Refusal::Cap(FieldglassError::Array(refusal)));
        }
        Ok(listed)
    }
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

/// What [`HeapText::read`] counts: the string bytes one listing reads from
/// the global heap, against [`MAX_TEXT_READ`].
const TEXT_READ_BYTES: &str = "bytes of HDF5 string attribute text read from the global heap";

/// The most string bytes one listing reads from the global heap: four times
/// the text it keeps (#1569).
///
/// Each string is charged to the text budget before it is read, so a
/// listing reads past the text it keeps only for an attribute left out
/// after some of its text was read: an array whose later string does not
/// fit, or text that grows past the budget as U+FFFD or CDL. Such an
/// attribute is refunded what it charged (#1568), so the next one like it
/// can read as much again, and without this a file of many of them reads
/// the same large object once for each.
///
/// Why four: each attribute left out this way reads less than the text
/// budget, so four budgets leave room for at least three whole budgets of
/// text read and then left out on top of the text kept. A file meets that
/// only by referring again and again to strings that do not fit, which a
/// file written to be read does not do. Eight would admit no file four
/// refuses, and would double what a hostile file costs over a range source.
///
/// The bound is per [`HeapText`], so per resolution, not per file: a host
/// that resolves a file more than once pays it each time (the napi handle
/// resolves about twice per open, and `decode_variable_physical` resolves
/// again). And the counter is shared and never refunded, with the global
/// attributes read first: a file whose globals spend it reading text that is
/// then left out leaves every variable's strings refused, and each such
/// variable left out and named. That is the cost a hostile file can impose
/// with the counter, and a bounded one; without it, it imposed unbounded
/// reads.
const MAX_TEXT_READ: u64 = 4 * MAX_METADATA_BYTES;

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
    /// Text bytes charged so far, against [`TEXT_BYTES`]; never more than
    /// the budget. An attribute left out is refunded what it charged (see
    /// [`Self::kept_only`]). The text of an attribute that was read stays
    /// charged even when its object is left out afterwards, such as a
    /// variable a cap cut, so the count can be higher than the text shown,
    /// never lower.
    kept: u64,
    /// String bytes read from the global heap so far, against
    /// [`MAX_TEXT_READ`]; never more than that. Refunds do not touch it:
    /// what was read stays read (#1569).
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
            kept: 0,
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

    /// Run `parse` over one attribute, and keep what it charged to the text
    /// budget only if it returns the attribute (#1568).
    ///
    /// An attribute left out keeps none of its text, so it is charged
    /// nothing: not the strings of an array read before the one that did not
    /// fit, and not the bytes that would have passed the budget. Kept, a
    /// failed charge left the count past the budget, and every later string
    /// was refused however short, so one oversized global cut every
    /// variable's `units` and `calendar`. Refunded, a later attribute that
    /// fits in what is left still reads, and the count stays at most the
    /// budget.
    ///
    /// This bounds the text kept, not the bytes read: the strings of an
    /// attribute left out part way were read. [`Self::read`] bounds those.
    fn kept_only<T, E>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T, E>) -> Result<T, E> {
        let charged = self.kept;
        let parsed = parse(self);
        if parsed.is_err() {
            self.kept = charged;
        }
        parsed
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
        charge_metadata(&mut self.kept, total, TEXT_BYTES)
    }

    /// Count `len` more bytes read from the global heap, before they are
    /// read. Refused past [`MAX_TEXT_READ`], and then not counted: nothing
    /// was read, so a later, shorter string can still be.
    fn charge_read(&mut self, len: u64) -> Result<(), FieldglassError> {
        let read = self.read.saturating_add(len);
        if read > MAX_TEXT_READ {
            return Err(FieldglassError::Array(ArrayError::ListingTooLarge {
                what: TEXT_READ_BYTES,
                limit: MAX_TEXT_READ,
            }));
        }
        self.read = read;
        Ok(())
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
        // Found from the collection's object headers, so everything below
        // is checked and charged before any of the string is read (#1569).
        let located = self
            .collections
            .locate(self.source, collection, index, self.length_size)?;
        // The object is the string: its size is the string's length, the
        // characters being one byte each. A length either side of it is a
        // malformed ID, not a prefix to take or a string to pad (#1557);
        // libhdf5 refuses both ("Expected global heap object size does not
        // match"). The specification states no length field to check against.
        // Checked before any cap, so a damaged ID is reported as damaged,
        // not as text too large to read.
        if len != located.size() {
            return Err(FieldglassError::Parse(
                "variable-length string length does not match its global-heap object".into(),
            ));
        }
        let size = located.checked_size()?;
        charge_metadata(&mut self.kept, size as u64, TEXT_BYTES)?;
        self.charge_read(size as u64)?;
        let object = located.read(self.source)?;
        let text = object.as_slice();
        // A byte that is not UTF-8 becomes U+FFFD, three bytes for one, so
        // the text kept can outgrow the object charged above.
        let text = decode_name(text);
        let grown = text.len().saturating_sub(object.len()) as u64;
        charge_metadata(&mut self.kept, grown, TEXT_BYTES)?;
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
    raw_in_bodies(&bodies.messages, name, probe.length_size)
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
) -> Result<Bodies, FieldglassError> {
    let header = probe.header(source, object_header_address)?;

    let mut bodies = Bodies {
        messages: header
            .messages
            .iter()
            .filter(|m| m.msg_type == MSG_ATTRIBUTE)
            .map(|m| m.body.clone())
            .collect(),
        past_cap: Vec::new(),
    };
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
    out: &mut Bodies,
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
        // refused over it (#997), and counted so the listing can say an
        // attribute is missing (#1569). Its name is in the message not read.
        match heap.object_or_cap(source, id, &mut reads)? {
            Ok(message) => out.messages.push(message),
            Err(cap) => out.past_cap.push(cap),
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
) -> Result<Hdf5Attribute, NotRead> {
    let split = split_attribute_message(body)?;
    // The datatype first: an attribute this cannot read (a `DIMENSION_LIST`
    // or `REFERENCE_LIST`, on every netCDF-4 variable) is refused before its
    // dataspace is decoded, which allocates its dimensions.
    let (datatype, vlen_text) = match datatype::vlen_string_as_text(split.datatype_bytes) {
        Some(datatype) => (datatype, true),
        None => (datatype::decode(split.datatype_bytes)?, false),
    };
    let dataspace = dataspace::decode(split.dataspace_bytes, length_size)?;
    if vlen_text {
        let strings = heap
            .strings(split.data, element_count(&dataspace))
            .map_err(NotRead::in_heap)?;
        return Ok(Hdf5Attribute {
            name: split.name,
            datatype,
            dataspace,
            value: render_strings(strings),
            values: Vec::new(),
        });
    }
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
        .map_err(NotRead::into_error)
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
        listing(file, bodies)
            .attributes
            .into_iter()
            .map(|a| (a.name, a.value))
            .collect()
    }

    /// Everything `parse_bodies` gives for `bodies` over `file`.
    fn listing(file: &[u8], bodies: &[Vec<u8>]) -> Listed {
        parse_bodies(bodies, &mut HeapText::new(file, &Hdf5Probe::new(0, 8, 8))).unwrap()
    }

    /// The names a listing left out over a cap, and the refusal they met,
    /// for a listing that met one cap.
    fn unread(listed: &Listed) -> (&[String], &FieldglassError) {
        let unread = listed.unread.as_ref().expect("a cap left attributes out");
        let [cut] = unread.cuts.as_slice() else {
            panic!("one refusal: {unread:?}");
        };
        let Refusal::Cap(refusal) = &cut.refusal else {
            panic!("a cap, not damage: {cut:?}");
        };
        (&cut.names, refusal)
    }

    fn vlen_parse(file: &[u8], msg: &[u8]) -> Result<Hdf5Attribute, FieldglassError> {
        parse_attribute_message(msg, 8, &mut HeapText::new(file, &Hdf5Probe::new(0, 8, 8)))
            .map_err(NotRead::into_error)
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

    /// The string is its whole object, and stops at a NUL as netCDF-C's C
    /// string does. An ID whose length is shorter or longer than the object
    /// is refused rather than read as a prefix (#1557), as libhdf5 2.0.0
    /// refuses it ("Expected global heap object size does not match").
    #[test]
    fn a_vlen_string_is_its_whole_object() {
        let file = collection(&[(1, b"abc\0def")]);
        let nul = attr_v3("s", &dt_vlen_string(), &ds_scalar(), &id(7, 0, 1));
        assert_eq!(vlen_parse(&file, &nul).unwrap().value, "abc");
        for len in [2, 6, 8] {
            let wrong = attr_v3("s", &dt_vlen_string(), &ds_scalar(), &id(len, 0, 1));
            let err = vlen_parse(&file, &wrong).unwrap_err();
            assert!(err.to_string().contains("does not match"), "{len}: {err}");
        }
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
    /// rest are skipped rather than read, and named as left out (#1556).
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
        let got = listing(&file, &bodies);
        assert_eq!(
            got.attributes.len(),
            refs,
            "the attribute past the cap is skipped"
        );
        assert!(got.attributes.iter().all(|a| a.value.len() == OBJECT));
        let (names, refusal) = unread(&got);
        assert_eq!(names, [format!("a{refs:03}")], "and named");
        assert!(
            matches!(
                refusal,
                FieldglassError::Array(ArrayError::ListingTooLarge { what, .. }) if *what == TEXT_BYTES
            ),
            "{refusal:?}"
        );
        let refusal = refusal.to_string();
        assert_eq!(
            got.unread.as_ref().unwrap().reason(),
            format!("attribute `a{refs:03}` not read: {refusal}")
        );
        assert!(matches!(
            got.complete(),
            Err(FieldglassError::Array(ArrayError::ListingTooLarge { .. }))
        ));
        // Under the cap, nothing is left out, and the listing is complete.
        let fits = listing(&file, &bodies[..refs]);
        assert!(fits.unread.is_none());
        assert_eq!(fits.complete().unwrap().len(), refs);
    }

    /// The attributes left out are named in name order, whatever order their
    /// messages come in.
    #[test]
    fn the_unread_are_named_in_name_order() {
        const OBJECT: usize = 1 << 20;
        let big = vec![b'x'; OBJECT];
        let file = collection(&[(1, &big)]);
        let refs = (super::super::source::MAX_METADATA_BYTES as usize) / OBJECT;
        let mut bodies: Vec<Vec<u8>> = (0..refs + 2)
            .map(|i| {
                attr_v3(
                    &format!("a{i:03}"),
                    &dt_vlen_string(),
                    &ds_scalar(),
                    &id(OBJECT as u32, 0, 1),
                )
            })
            .collect();
        bodies.reverse();
        let got = listing(&file, &bodies);
        assert_eq!(unread(&got).0, ["a000", "a001"]);
    }

    /// The report names at most eight unread attributes, in name order, and
    /// counts the rest.
    #[test]
    fn the_report_names_eight_and_counts_the_rest() {
        let unread = Unread {
            cuts: vec![Cut {
                names: (0..10).map(|i| format!("n{i}")).collect(),
                unnamed: 0,
                refusal: Refusal::Cap(FieldglassError::Array(ArrayError::ListingTooLarge {
                    what: TEXT_BYTES,
                    limit: 1,
                })),
            }],
        };
        let reason = unread.reason();
        assert!(
            reason.starts_with(
                "attributes `n0`, `n1`, `n2`, `n3`, `n4`, `n5`, `n6`, `n7` and 2 more not read: "
            ),
            "{reason}"
        );
    }

    /// A string that does not fit in what is left of the budget is left
    /// out, and charged nothing: a later, smaller one still reads (#1568).
    /// Kept, the failed charge put the count past the budget, and every
    /// later string was refused however short.
    #[test]
    fn a_string_past_the_budget_leaves_the_rest_its_room() {
        const MIB: usize = 1 << 20;
        let (ten, twelve) = (vec![b'x'; 10 * MIB], vec![b'y'; 12 * MIB]);
        let file = collection(&[(1, &ten), (2, &twelve), (3, b"noleap")]);
        let probe = Hdf5Probe::new(0, 8, 8);
        let mut text = HeapText::new(file.as_slice(), &probe);
        let bodies = [
            attr_v3(
                "a",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(ten.len() as u32, 0, 1),
            ),
            attr_v3(
                "b",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(twelve.len() as u32, 0, 2),
            ),
            attr_v3("calendar", &dt_vlen_string(), &ds_scalar(), &id(6, 0, 3)),
        ];
        let got = parse_bodies(&bodies, &mut text).unwrap();
        let names: Vec<&str> = got.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["a", "calendar"]);
        assert_eq!(got.attributes[1].value, "noleap");
        let (cut, refusal) = unread(&got);
        assert_eq!(cut, ["b"]);
        assert!(
            matches!(
                refusal,
                FieldglassError::Array(ArrayError::ListingTooLarge { what, .. }) if *what == TEXT_BYTES
            ),
            "{refusal:?}"
        );
        // What is counted is the text kept, and it is within the budget.
        assert_eq!(text.kept, (10 * MIB + 6) as u64);
    }

    /// An array of strings left out part way is charged nothing, not the
    /// strings read before the one that did not fit: they are not kept
    /// either. A later string that fits only once they are refunded reads.
    #[test]
    fn an_array_cut_part_way_is_charged_nothing() {
        const MIB: usize = 1 << 20;
        let (ten, four) = (vec![b'x'; 10 * MIB], vec![b'y'; 4 * MIB]);
        let file = collection(&[(1, &ten), (2, &four)]);
        let probe = Hdf5Probe::new(0, 8, 8);
        let mut text = HeapText::new(file.as_slice(), &probe);
        let four_id = id(four.len() as u32, 0, 2);
        let pair = [four_id.clone(), four_id.clone()].concat();
        let bodies = [
            attr_v3(
                "a",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(ten.len() as u32, 0, 1),
            ),
            // 10 + 4 fits, and 10 + 4 + 4 does not.
            attr_v3("b", &dt_vlen_string(), &ds_1d(2), &pair),
            // 10 + 4 fits only if `b`'s first string was refunded.
            attr_v3("c", &dt_vlen_string(), &ds_scalar(), &four_id),
        ];
        let got = parse_bodies(&bodies, &mut text).unwrap();
        let names: Vec<&str> = got.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(unread(&got).0, ["b"]);
        assert_eq!(text.kept, (14 * MIB) as u64);
        let kept: usize = got.attributes.iter().map(|a| a.value.len()).sum();
        assert_eq!(kept as u64, text.kept, "the count is the text kept");
    }

    /// An array left out as damaged part way is refunded too: the strings
    /// read before its bad ID are not kept, whatever the reason it was left
    /// out. It is named as damaged, with every other string whose reference
    /// is (#1569), under one refusal that blames the file.
    #[test]
    fn an_array_left_out_as_damaged_is_charged_nothing() {
        const MIB: usize = 1 << 20;
        let (ten, four) = (vec![b'x'; 10 * MIB], vec![b'y'; 4 * MIB]);
        let file = collection(&[(1, &ten), (2, &four)]);
        let probe = Hdf5Probe::new(0, 8, 8);
        let mut text = HeapText::new(file.as_slice(), &probe);
        let four_id = id(four.len() as u32, 0, 2);
        // The second ID names object 9, which the collection does not hold.
        let bad = [four_id.clone(), id(4, 0, 9)].concat();
        let bodies = [
            attr_v3(
                "a",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(ten.len() as u32, 0, 1),
            ),
            attr_v3("b", &dt_vlen_string(), &ds_1d(2), &bad),
            attr_v3("c", &dt_vlen_string(), &ds_scalar(), &four_id),
            attr_v3("d", &dt_vlen_string(), &ds_scalar(), &id(1, 0, 9)),
        ];
        let got = parse_bodies(&bodies, &mut text).unwrap();
        let names: Vec<&str> = got.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        let unread = got.unread.as_ref().expect("the damaged are named");
        assert_eq!(
            unread.reason(),
            "attributes `b`, `d` not read: the file's record of the text is damaged \
             (parse error: global-heap object 9 not found in collection)"
        );
        assert_eq!(text.kept, (14 * MIB) as u64);
        // And then one more 4 MiB string does not fit, so the count is not
        // simply refunded to zero.
        let more = parse_bodies(&bodies[2..3], &mut text).unwrap();
        assert!(more.attributes.is_empty() && more.unread.is_some());
        assert_eq!(text.kept, (14 * MIB) as u64);
    }

    /// Refunds bound the text kept, not the bytes read (#1569). Each of these
    /// arrays reads its first 10 MiB string and is then left out on its
    /// second, which does not fit, so it is refunded and the next reads as
    /// much again: without a read bound, every one costs 10 MiB. The read
    /// counter stops them at [`MAX_TEXT_READ`], and the one it stops is
    /// named with it.
    #[test]
    fn the_strings_one_listing_reads_are_bounded_though_refunded() {
        const MIB: usize = 1 << 20;
        let ten = vec![b'x'; 10 * MIB];
        let file = collection(&[(1, &ten), (2, &ten)]);
        let source = fieldglass_core::testing::Recording::new(file.as_slice());
        let pair = [id(ten.len() as u32, 0, 1), id(ten.len() as u32, 0, 2)].concat();
        let bodies: Vec<Vec<u8>> = (0..12)
            .map(|i| attr_v3(&format!("a{i:02}"), &dt_vlen_string(), &ds_1d(2), &pair))
            .collect();
        let probe = Hdf5Probe::new(0, 8, 8);
        let mut text = HeapText::new(&source, &probe);
        let got = parse_bodies(&bodies, &mut text).unwrap();
        assert!(got.attributes.is_empty());
        let read: u64 = source.reads().iter().map(|r| r.len).sum();
        let heads = 16 * 3 * bodies.len() as u64; // collection and object headers
        assert!(read <= MAX_TEXT_READ + heads, "read {read} bytes");
        assert_eq!(
            text.read,
            6 * (10 * MIB) as u64,
            "six strings fit in 64 MiB"
        );
        let unread = got.unread.as_ref().unwrap();
        let [text_cut, read_cut] = unread.cuts.as_slice() else {
            panic!("two refusals: {unread:?}");
        };
        assert_eq!(text_cut.names.len(), 6);
        assert_eq!(read_cut.names.len(), 6);
        assert!(
            matches!(
                &read_cut.refusal,
                Refusal::Cap(FieldglassError::Array(ArrayError::ListingTooLarge { what, .. }))
                    if *what == TEXT_READ_BYTES
            ),
            "{read_cut:?}"
        );
    }

    /// A source that serves a string's text short fails the listing as a
    /// short read, which a host can retry (#707), rather than leaving the
    /// attribute out as if the file lacked it (#1569 review).
    #[test]
    fn a_string_served_short_fails_the_listing() {
        let file = collection(&[(1, b"noleap")]);
        // Object 1's text starts after the collection and object headers.
        let source = fieldglass_core::testing::Short::new(file.as_slice(), 32, 1);
        let bodies = [attr_v3(
            "calendar",
            &dt_vlen_string(),
            &ds_scalar(),
            &id(6, 0, 1),
        )];
        let probe = Hdf5Probe::new(0, 8, 8);
        let got = parse_bodies(&bodies, &mut HeapText::new(&source, &probe));
        assert!(
            matches!(got, Err(FieldglassError::ShortRead { .. })),
            "{got:?}"
        );
        // Served whole, it reads.
        assert_eq!(
            listed(&file, &bodies),
            [("calendar".into(), "noleap".into())]
        );
    }

    /// A string refused by the text budget is refused before its object is
    /// read: the bytes it would have cost are never fetched (#1569).
    #[test]
    fn a_string_past_the_budget_is_not_read() {
        const MIB: usize = 1 << 20;
        let (ten, twelve) = (vec![b'x'; 10 * MIB], vec![b'y'; 12 * MIB]);
        let file = collection(&[(1, &ten), (2, &twelve)]);
        let source = fieldglass_core::testing::Recording::new(file.as_slice());
        let bodies = [
            attr_v3(
                "a",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(ten.len() as u32, 0, 1),
            ),
            attr_v3(
                "b",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(twelve.len() as u32, 0, 2),
            ),
        ];
        let probe = Hdf5Probe::new(0, 8, 8);
        let got = parse_bodies(&bodies, &mut HeapText::new(&source, &probe)).unwrap();
        assert_eq!(unread(&got).0, ["b"]);
        let largest = source.reads().iter().map(|r| r.len).max().unwrap();
        assert_eq!(largest, (10 * MIB) as u64, "`b`'s 12 MiB is never read");
    }

    /// A length that does not match its object is damage, and is reported
    /// as damage even when the budget has no room left for it: a damaged ID
    /// is not text too large to read (#1569).
    #[test]
    fn a_damaged_length_is_damage_with_the_budget_spent() {
        const MIB: usize = 1 << 20;
        let sixteen = vec![b'x'; 16 * MIB];
        let file = collection(&[(1, &sixteen), (2, b"noleap")]);
        let bodies = [
            attr_v3(
                "a",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(sixteen.len() as u32, 0, 1),
            ),
            attr_v3("calendar", &dt_vlen_string(), &ds_scalar(), &id(5, 0, 2)),
        ];
        let got = listing(&file, &bodies);
        assert_eq!(
            got.unread.as_ref().unwrap().reason(),
            "attribute `calendar` not read: the file's record of the text is damaged \
             (parse error: variable-length string length does not match its \
             global-heap object)"
        );
    }

    /// Attributes left out without a name, such as a dense attribute message
    /// past the heap-object cap, are counted in the reason (#1569).
    #[test]
    fn unnamed_attributes_are_counted() {
        let cap = || {
            Refusal::Cap(FieldglassError::Array(ArrayError::ListingTooLarge {
                what: TEXT_BYTES,
                limit: 1,
            }))
        };
        let refusal = cap().to_string();
        let mut unread = Unread { cuts: Vec::new() };
        unread.push(None, cap());
        assert_eq!(unread.reason(), format!("an attribute not read: {refusal}"));
        unread.push(None, cap());
        assert_eq!(unread.reason(), format!("2 attributes not read: {refusal}"));
        unread.push(Some("units".into()), cap());
        assert_eq!(
            unread.reason(),
            format!("attributes `units` and 2 more not read: {refusal}")
        );
    }

    /// An object whose attributes meet two caps names each attribute with the
    /// cap it met, rather than all of them with the first (#1568): here one
    /// heap object larger than any listing reads, and one string past what is
    /// left of the text budget. A listing refused whole says the first.
    #[test]
    fn each_cap_names_the_attributes_it_cut() {
        const MIB: usize = 1 << 20;
        let huge = vec![b'x'; 16 * MIB + 8];
        let nine = vec![b'y'; 9 * MIB];
        let file = collection(&[(1, &huge), (2, &nine)]);
        let nine_id = id(nine.len() as u32, 0, 2);
        let bodies = [
            attr_v3(
                "huge",
                &dt_vlen_string(),
                &ds_scalar(),
                &id(huge.len() as u32, 0, 1),
            ),
            attr_v3("first", &dt_vlen_string(), &ds_scalar(), &nine_id),
            attr_v3("second", &dt_vlen_string(), &ds_scalar(), &nine_id),
            attr_v3("third", &dt_vlen_string(), &ds_scalar(), &nine_id),
        ];
        let got = listing(&file, &bodies);
        let names: Vec<&str> = got.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["first"]);
        let too_large = |what| {
            FieldglassError::Array(ArrayError::ListingTooLarge {
                what,
                limit: super::super::source::MAX_METADATA_BYTES,
            })
            .to_string()
        };
        let object = too_large("bytes in one HDF5 global-heap object");
        let text = too_large(TEXT_BYTES);
        assert_eq!(
            got.unread.as_ref().unwrap().reason(),
            format!(
                "attribute `huge` not read: {object}; attributes `second`, `third` not read: {text}"
            )
        );
        let err = got.complete().expect_err("a cap left attributes out");
        assert_eq!(err.to_string(), object);
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
        let listings: Vec<Listed> = (0..=refs)
            .map(|_| parse_bodies(&one, &mut text).unwrap())
            .collect();
        let listed: usize = listings.iter().map(|l| l.attributes.len()).sum();
        assert_eq!(listed, refs);
        // The one object the budget ran out on says so; the ones before it
        // are complete.
        assert!(listings[..refs].iter().all(|l| l.unread.is_none()));
        assert_eq!(unread(&listings[refs]).0, ["a"]);
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
        let got = parse_bodies(&bodies, &mut HeapText::numbers_only(&source, &probe))
            .unwrap()
            .complete()
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].first_value(), Some(-1.0));
        assert!(source.reads().is_empty());
        let got = parse_bodies(&bodies, &mut HeapText::new(&source, &probe))
            .unwrap()
            .complete()
            .unwrap();
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
