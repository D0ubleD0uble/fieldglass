//! What a manifest, a source description, or a fetched range can fail with.
//!
//! Every variant carries the values that failed rather than a sentence built
//! around them, so a host can report the mismatch in its own words and a test
//! can assert on the pair instead of on prose.

/// Which manifest dialect a parse failure came from.
///
/// Carried on the error rather than baked into each message so the two dialects
/// share one variant per shape of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// The wgrib2 `.idx` sidecar: colon-separated text, one line per record.
    Wgrib2Idx,
    /// The ECMWF `.index` sidecar: JSON lines, one object per record.
    EcmwfIndex,
    /// A kerchunk reference document: one JSON object mapping a store key to
    /// data or to a byte range of some object.
    Kerchunk,
    /// A Zarr array metadata document — a v2 `.zarray` or a v3 `zarr.json` —
    /// read for the chunk grid and the key spelling, and for nothing else.
    ZarrMetadata,
}

impl std::fmt::Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Wgrib2Idx => "wgrib2 .idx",
            Self::EcmwfIndex => "ECMWF .index",
            Self::Kerchunk => "kerchunk references",
            Self::ZarrMetadata => "Zarr metadata",
        })
    }
}

/// Everything this crate can refuse.
///
/// Line numbers are 1-based and count **every** line of the source text,
/// blank ones included, so a number here is the number an editor shows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FetchPlanError {
    /// A record had fewer fields than the dialect's grammar requires.
    #[error("{dialect} line {line}: expected at least {expected} fields, found {found}")]
    ShortRecord {
        /// Which dialect was being read.
        dialect: Dialect,
        /// 1-based line number.
        line: usize,
        /// How many fields the grammar needs.
        expected: usize,
        /// How many the line had.
        found: usize,
    },

    /// A field that must be an integer was not one.
    #[error("{dialect} line {line}: {field} is not an integer: {value:?}")]
    NotAnInteger {
        /// Which dialect was being read.
        dialect: Dialect,
        /// 1-based line number.
        line: usize,
        /// The field's name in the dialect's own vocabulary (`offset`,
        /// `_length`, …).
        field: &'static str,
        /// What was there instead.
        value: String,
    },

    /// A byte offset or length was negative, which no range can be built from.
    #[error("{dialect} line {line}: {field} must not be negative, got {value}")]
    NegativeOffset {
        /// Which dialect was being read.
        dialect: Dialect,
        /// 1-based line number.
        line: usize,
        /// The field's name in the dialect's own vocabulary.
        field: &'static str,
        /// The negative value.
        value: i64,
    },

    /// A wgrib2 record number was neither `n` nor `n.m`.
    #[error("wgrib2 .idx line {line}: record number {value:?} is neither `n` nor `n.m`")]
    BadRecordNumber {
        /// 1-based line number.
        line: usize,
        /// What was there instead.
        value: String,
    },

    /// A wgrib2 reference-time field did not carry the `d=` prefix.
    #[error("wgrib2 .idx line {line}: expected a `d=` reference time, found {value:?}")]
    MissingDatePrefix {
        /// 1-based line number.
        line: usize,
        /// What was there instead.
        value: String,
    },

    /// Offsets ran backwards.
    ///
    /// A sidecar's records describe one object read front to back, so a smaller
    /// offset after a larger one means the file is not the index it claims to
    /// be — and the range arithmetic (each message ends where the next begins)
    /// would silently produce a negative length.
    #[error(
        "wgrib2 .idx line {line}: offset {offset} is below the previous record's offset \
         {previous_offset}; a sidecar's records must not run backwards"
    )]
    DescendingOffset {
        /// 1-based line number of the offending record.
        line: usize,
        /// Its offset.
        offset: u64,
        /// The offset it should not have fallen below.
        previous_offset: u64,
    },

    /// A JSON-lines record did not parse.
    #[error("ECMWF .index line {line}: {detail}")]
    Json {
        /// 1-based line number.
        line: usize,
        /// `serde_json`'s own message, which already names the column.
        detail: String,
    },

    /// A key the dialect requires was absent.
    #[error("{dialect} line {line}: required key {key:?} is missing")]
    MissingKey {
        /// Which dialect was being read.
        dialect: Dialect,
        /// 1-based line number.
        line: usize,
        /// The absent key.
        key: &'static str,
    },

    /// A key pattern contained a placeholder this crate does not expand.
    ///
    /// Refused rather than passed through: a key with an unexpanded `{ }` in it
    /// fetches nothing, and the 404 that follows is a much worse error message
    /// than this one.
    #[error(
        "key pattern placeholder {{{placeholder}}} is not one of \
         yyyy, yy, mm, dd, doy, HH, H, fff, ff, f"
    )]
    UnknownPlaceholder {
        /// The unrecognised name, without its braces.
        placeholder: String,
    },

    /// A key pattern had a `{` with no matching `}`.
    #[error("key pattern has an unterminated placeholder starting at byte {at}")]
    UnterminatedPlaceholder {
        /// Byte index of the opening brace.
        at: usize,
    },

    /// A source description listed no cycle hours, so it describes no runs.
    #[error("a source must list at least one cycle hour")]
    NoCycleHours,

    /// A cycle hour outside `0..=23`.
    #[error("cycle hour {hour} is not in 0..=23")]
    CycleHourOutOfRange {
        /// The offending hour.
        hour: u8,
    },

    /// Cycle hours were not strictly ascending.
    ///
    /// Strict, so this catches a duplicate as well as an unsorted list; both
    /// would make `candidates` emit the same run twice.
    #[error("cycle hours must be strictly ascending, got {hours:?}")]
    UnsortedCycleHours {
        /// The list as given.
        hours: Vec<u8>,
    },

    /// A timestamp that the proleptic Gregorian arithmetic here cannot express.
    ///
    /// The bound is generous — roughly ±2.9 million years — and exists so the
    /// day-count arithmetic can be plain `i64` without a silent wrap.
    #[error("{unix_secs} is outside the Unix-time range this crate converts")]
    TimeOutOfRange {
        /// The offending value.
        unix_secs: i64,
    },

    /// An open-ended or whole-object range was closed against an object size
    /// that ends before the range starts.
    #[error("cannot close a range starting at {offset} against an object of {object_size} bytes")]
    OffsetPastEnd {
        /// Where the range starts.
        offset: u64,
        /// The object size the host supplied.
        object_size: u64,
    },

    // ── The chunk-addressing dialects ───────────────────────────────────────
    //
    // These carry no line number, and that is the difference rather than an
    // omission: a `.idx` and a `.index` are line grammars, while a kerchunk
    // reference document and a Zarr metadata document are each one JSON value.
    // What locates a failure in them is the key it was under, so that is what
    // these carry.
    /// A whole document did not parse as JSON.
    #[error("{dialect}: {detail}")]
    Document {
        /// Which document was being read.
        dialect: Dialect,
        /// `serde_json`'s own message, which names the line and column.
        detail: String,
    },

    /// A key the document's grammar requires was absent.
    #[error("{dialect}: required field {key:?} is missing")]
    MissingField {
        /// Which document was being read.
        dialect: Dialect,
        /// The absent field, dotted where it is nested.
        key: &'static str,
    },

    /// A field was present and was not the kind of value it has to be.
    #[error("{dialect}: {key:?} must be {expected}")]
    BadFieldType {
        /// Which document was being read.
        dialect: Dialect,
        /// The field's name in the document's own vocabulary.
        key: &'static str,
        /// What the grammar requires, in words.
        expected: &'static str,
    },

    /// The document uses something this crate deliberately does not read.
    ///
    /// Refused rather than skipped: a document read past the part that was not
    /// understood returns a plan that is short by however much that part
    /// addressed, with nothing to say so.
    #[error("{dialect}: unsupported — {feature}")]
    UnsupportedFeature {
        /// Which document was being read.
        dialect: Dialect,
        /// What was there, named so a host can report it.
        feature: &'static str,
    },

    /// A kerchunk document declared a version this crate does not read.
    #[error("kerchunk references: version {found} is not read; only version 1 is")]
    UnsupportedKerchunkVersion {
        /// The value of the document's `version`, as written.
        found: String,
    },

    /// A Zarr metadata document declared an edition this crate does not read.
    #[error("Zarr metadata: zarr_format {found} is not read; only 2 and 3 are")]
    UnsupportedZarrFormat {
        /// The value of the document's `zarr_format`, as written.
        found: String,
    },

    /// The array's chunks are not laid out on a regular grid.
    ///
    /// Only `regular` divides an array into equal chunks, which is the whole of
    /// the arithmetic here; a rectilinear grid states each chunk's extent
    /// separately and needs a different one.
    #[error("Zarr metadata: chunk grid {name:?} is not read; only \"regular\" is")]
    UnsupportedChunkGrid {
        /// The grid's name, as the document spells it.
        name: String,
    },

    /// The array spells its chunk keys with a convention this crate does not
    /// know, so no key it built would find an object.
    #[error(
        "Zarr metadata: chunk key encoding {name:?} is not read; only \"default\" and \"v2\" are"
    )]
    UnsupportedChunkKeyEncoding {
        /// The encoding's name, as the document spells it.
        name: String,
    },

    /// A v3 `zarr.json` describes something other than an array.
    ///
    /// A group's metadata has the same file name and none of the fields, so
    /// this says which node it is rather than reporting a missing `shape`.
    #[error("Zarr metadata: this document describes a {node_type}, not an array")]
    NotAnArray {
        /// The document's own `node_type`.
        node_type: String,
    },

    /// A chunk key separator that is not one of the two the conventions use.
    #[error("Zarr metadata: {found:?} is not a chunk key separator; expected \".\" or \"/\"")]
    BadSeparator {
        /// What the document said.
        found: String,
    },

    /// The array's shape and its chunk shape have different numbers of axes.
    #[error("Zarr metadata: the shape has {shape} axes and the chunk shape has {chunks}")]
    RankMismatch {
        /// How many axes the shape states.
        shape: usize,
        /// How many the chunk shape states.
        chunks: usize,
    },

    /// A chunk extent of zero, which no array has and which the grid
    /// arithmetic would divide by.
    #[error("Zarr metadata: the chunk shape is zero on axis {axis}")]
    ZeroChunkExtent {
        /// Which axis, counted from zero.
        axis: usize,
    },

    /// An index, point or region with the wrong number of axes for the array.
    #[error("this array has {expected} axes, but {found} were given")]
    WrongRank {
        /// How many the array has.
        expected: usize,
        /// How many the caller supplied.
        found: usize,
    },

    /// A chunk index past the end of the chunk grid.
    #[error("chunk index {index} is past the end of axis {axis}, which has {extent} chunks")]
    ChunkIndexOutOfRange {
        /// Which axis, counted from zero.
        axis: usize,
        /// The index asked for.
        index: u64,
        /// How many chunks that axis has.
        extent: u64,
    },

    /// An element index past the end of the array.
    #[error("index {index} is past the end of axis {axis}, which has {extent} elements")]
    PointOutOfRange {
        /// Which axis, counted from zero.
        axis: usize,
        /// The index asked for.
        index: u64,
        /// How many elements that axis has.
        extent: u64,
    },

    /// One entry of a reference document was not a reference.
    ///
    /// Carries the key because a document holds thousands of them and the
    /// detail alone does not say which one is wrong.
    #[error("kerchunk references: {key:?} is not a valid reference: {detail}")]
    BadReference {
        /// The store key the entry was under.
        key: String,
        /// What was wrong with it.
        detail: String,
    },

    /// A `{{…}}` holding something other than a template's name.
    ///
    /// The spec's templates are jinja2, so a value may be an expression or a
    /// call. Approximating one builds a URL that fetches the wrong object and
    /// reports success, so it is refused instead.
    #[error("kerchunk references: {expression:?} is a jinja2 expression, not a template name")]
    UnsupportedTemplate {
        /// What was between the braces.
        expression: String,
    },

    /// A `{{name}}` naming a template the document does not declare.
    #[error("kerchunk references: no template named {name:?}")]
    UnknownTemplate {
        /// The name that was asked for.
        name: String,
    },

    /// A reference document carries no metadata for the array asked about.
    #[error("kerchunk references: no array named {name:?} in this document")]
    NoSuchArray {
        /// The name that was asked for.
        name: String,
    },

    /// A region touches more chunks than this crate will build a list of.
    ///
    /// The bound exists because the array's shape is read out of a document
    /// fetched over the network: `[1000000000000]` in chunks of one is sixty
    /// bytes of JSON and a list of a trillion indices.
    #[error("this region touches more than {limit} chunks, which is more than a plan will hold")]
    RegionTooLarge {
        /// The most chunks a region may touch.
        limit: u64,
    },
}

/// How fetched bytes failed to be what the manifest promised.
///
/// Separate from [`FetchPlanError`] because it is a different claim failing: the
/// manifest parsed fine, the host fetched what it asked for, and the bytes that
/// came back are not the message the sidecar described. Every variant carries
/// both sides — a `.idx` regenerated against a newer object is the common cause,
/// and "expected X, found Y" is what tells a user that is what happened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Mismatch {
    /// The fetched bytes are shorter than a GRIB §0 indicator section.
    #[error("need at least {need} bytes to read the GRIB indicator section, fetched {fetched}")]
    TooShort {
        /// How many bytes §0 needs.
        need: usize,
        /// How many arrived.
        fetched: usize,
    },

    /// The first four bytes are not `GRIB`.
    ///
    /// The overwhelmingly likely cause is a stale sidecar: the object was
    /// regenerated and every offset after the first change now points into the
    /// middle of some message.
    #[error("expected the bytes \"{expected}\" at offset 0, found \"{found}\"")]
    Magic {
        /// Always `"GRIB"`; spelled out so the pair reads as a pair.
        expected: String,
        /// The four bytes that were there, rendered printably.
        found: String,
    },

    /// The edition octet is not one this envelope check understands.
    #[error("expected GRIB edition 1 or 2, found {found}")]
    Edition {
        /// The octet's value.
        found: u8,
    },

    /// The message's own §0 total length disagrees with what was fetched.
    #[error("the message declares a total length of {declared} bytes but {fetched} were fetched")]
    Length {
        /// What §0 says.
        declared: u64,
        /// What the host actually has.
        fetched: u64,
    },

    /// An open-ended fetch came back with fewer bytes than the message needs.
    #[error("the message declares {declared} bytes but only {fetched} were fetched")]
    Truncated {
        /// What §0 says.
        declared: u64,
        /// What the host actually has.
        fetched: u64,
    },

    /// A field the manifest promised is not what the message decodes to.
    ///
    /// Raised by the semantic half of verification, which lives where a decoder
    /// does — this crate names the field and carries the pair so both halves
    /// report a mismatch the same way.
    #[error("the manifest promised {field} {expected:?} but the message has {actual:?}")]
    Field {
        /// Which field disagreed, in the manifest's vocabulary.
        field: &'static str,
        /// What the manifest promised.
        expected: String,
        /// What the message says.
        actual: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both sides of every mismatch have to reach the message, because "the
    /// sidecar is stale" is only legible when the reader can see what was there
    /// instead. A `Display` that dropped one half would still compile and still
    /// look like an error.
    #[test]
    fn a_mismatch_prints_both_sides() {
        let m = Mismatch::Magic {
            expected: "GRIB".into(),
            found: "\\x1a\\x0b..".into(),
        };
        let text = m.to_string();
        assert!(text.contains("GRIB"), "{text}");
        assert!(text.contains("\\x1a\\x0b.."), "{text}");

        let m = Mismatch::Length {
            declared: 100,
            fetched: 97,
        };
        let text = m.to_string();
        assert!(text.contains("100") && text.contains("97"), "{text}");
    }

    /// A parse failure names the dialect, so a host reading both sidecars for
    /// one object can tell which of the two was malformed.
    #[test]
    fn a_parse_failure_names_its_dialect() {
        let e = FetchPlanError::ShortRecord {
            dialect: Dialect::Wgrib2Idx,
            line: 4,
            expected: 6,
            found: 3,
        };
        assert_eq!(
            e.to_string(),
            "wgrib2 .idx line 4: expected at least 6 fields, found 3"
        );

        let e = FetchPlanError::MissingKey {
            dialect: Dialect::EcmwfIndex,
            line: 1,
            key: "_offset",
        };
        assert_eq!(
            e.to_string(),
            "ECMWF .index line 1: required key \"_offset\" is missing"
        );
    }
}
