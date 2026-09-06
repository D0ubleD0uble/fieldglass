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
}

impl std::fmt::Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Wgrib2Idx => "wgrib2 .idx",
            Self::EcmwfIndex => "ECMWF .index",
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
