//! One error type for the whole API surface (ADR-0006 decision 2).

use fieldglass_core::FieldglassError;

/// Everything an API call can fail with.
///
/// One enum, not a per-operation family: a host maps errors once, and
/// [`code`](Error::code) is the stable string it maps on. The `code` is part of
/// the API — a host may branch on it — while [`message`](Error::message) is
/// prose and may be reworded.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Error {
    /// The bytes are not a container this build can open.
    UnsupportedFormat {
        /// What was detected instead, in prose.
        detail: String,
    },
    /// The container opened but a section did not parse.
    Decode {
        /// The underlying `core` error, rendered.
        detail: String,
    },
    /// A message index outside `0..count()`.
    NoSuchMessage {
        /// The index that was asked for.
        index: u32,
        /// How many messages the session actually holds.
        count: u32,
    },
    /// The operation is defined but this message's family does not support it —
    /// a grid with no geometry asked to warp, a spectral field asked to probe —
    /// or the file is valid and the read is too large to make at once: a whole
    /// NetCDF variable past the reader's memory budget, a slice, a region or
    /// a Zarr chunk of more values than one field may hold, or a compressed
    /// NetCDF-4 chunk that holds more values than one field may, or is stored
    /// as or decodes to more than one chunk may; or an HDF5 file that lists
    /// more than one listing holds: more objects, more links in one group, or
    /// more bytes of path-qualified names; or a classic NetCDF header longer
    /// than one header may be; or an HDF5 file whose objects share a dense
    /// attribute or link store or a local heap, which libhdf5 reads and this
    /// reader refuses; or a zstd chunk whose frame states a larger window than
    /// a chunk of its size may keep.
    Unsupported {
        /// Which operation, and what about this message refuses it.
        detail: String,
    },
    /// The call belongs to the other addressing mode: a message index asked of
    /// a variable dataset, or a slice asked of a message stream.
    ///
    /// Distinct from [`Self::NoSuchMessage`] on purpose. A NetCDF file holds no
    /// messages *at all*, so answering "index 0 is outside the 0 available"
    /// would tell a caller its index was wrong when its whole question was.
    /// [`crate::Session::addressing`] is what to ask first.
    WrongAddressing {
        /// The mode this session is in, named as the wire value.
        expected: String,
        /// Which call was made, and what to call instead.
        detail: String,
    },
    /// A source served fewer bytes than it was asked for.
    ///
    /// Its own code, not [`Self::Decode`], because the two call for opposite
    /// responses: a truncated transfer is worth retrying and a corrupt file is
    /// not. A buffer cannot produce this — the reader bounds-checks a range
    /// against the source's own size — so it means the host's transport came back
    /// short, which is the one failure the host can do something about (#707).
    ShortRead {
        /// Offset in the source the read began at.
        at: u64,
        /// How many bytes came back.
        got: u64,
        /// How many were asked for.
        wanted: u64,
    },
    /// A caller-supplied option is out of range or self-contradictory.
    InvalidOption {
        /// Which option, and what it would have had to be.
        detail: String,
        /// For a refused colour range, the same refusal as fields: which end,
        /// the value given, and the field's own range (#1155). `null` for every
        /// other option, so a host checks it before pointing at a control.
        #[serde(default, rename = "rangeRefusal")]
        range_refusal: Option<crate::api::RangeRefusal>,
    },
}

impl Error {
    /// [`Self::InvalidOption`] with `detail`, for an option other than a colour
    /// range.
    pub fn invalid_option(detail: impl Into<String>) -> Self {
        Self::InvalidOption {
            detail: detail.into(),
            range_refusal: None,
        }
    }

    /// The refused colour range this error carries, if it is one (#1155).
    pub fn range_refusal(&self) -> Option<&crate::api::RangeRefusal> {
        match self {
            Self::InvalidOption { range_refusal, .. } => range_refusal.as_ref(),
            _ => None,
        }
    }

    /// The stable machine-readable tag. Matches the serde discriminant, so a
    /// host reading the JSON form and one calling this method cannot disagree.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat { .. } => "unsupported_format",
            Self::Decode { .. } => "decode",
            Self::NoSuchMessage { .. } => "no_such_message",
            Self::Unsupported { .. } => "unsupported",
            Self::WrongAddressing { .. } => "wrong_addressing",
            Self::ShortRead { .. } => "short_read",
            Self::InvalidOption { .. } => "invalid_option",
        }
    }

    /// Human-readable prose. Not stable; branch on [`code`](Self::code).
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedFormat { detail } => {
                format!("not a container this build can open: {detail}")
            }
            Self::Decode { detail } => detail.clone(),
            Self::ShortRead { at, got, wanted } => {
                format!("the source served {got} of {wanted} bytes at {at}; the transfer was short")
            }
            Self::NoSuchMessage { index, count } => {
                format!("message {index} is out of range; the file holds {count}")
            }
            Self::Unsupported { detail } => detail.clone(),
            Self::WrongAddressing { expected, detail } => {
                format!("this session is addressed by {expected}: {detail}")
            }
            Self::InvalidOption { detail, .. } => detail.clone(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for Error {}

impl From<FieldglassError> for Error {
    fn from(e: FieldglassError) -> Self {
        // A short read keeps its shape across the boundary. Folding it into
        // `Decode` — which is what happened before #707 gave it a variant — told
        // a host its file was corrupt when its transfer was truncated, and those
        // call for opposite responses. A variable past the whole-variable
        // budget, a read past the one-field cap, or a chunk decoding to or
        // stored as more than one chunk may decode to, is a valid file asked
        // for too much at once, not a corrupt one, so it is `Unsupported`
        // rather than `Decode` (#847, #942, #970, #997). So is a file listing more objects or longer
        // names than one listing holds (#921), or sharing a dense store or a
        // local heap that libhdf5 reads (#987), and a zstd frame stating a
        // window larger than its chunk's decoder keeps (#989). Everything else
        // is still the long tail.
        match e {
            FieldglassError::ShortRead { at, got, wanted } => Self::ShortRead { at, got, wanted },
            FieldglassError::Array(
                too_large @ (fieldglass_core::array::ArrayError::VariableTooLarge { .. }
                | fieldglass_core::array::ArrayError::FieldTooLarge { .. }
                | fieldglass_core::array::ArrayError::ChunkTooLarge { .. }
                | fieldglass_core::array::ArrayError::StoredChunkTooLarge { .. }
                | fieldglass_core::array::ArrayError::ListingTooLarge { .. }
                | fieldglass_core::array::ArrayError::SharedStorage { .. }
                | fieldglass_core::array::ArrayError::WindowTooLarge { .. }),
            ) => Self::Unsupported {
                detail: too_large.to_string(),
            },
            other => Self::Decode {
                detail: other.to_string(),
            },
        }
    }
}

/// A planner failure, in the codes a host already branches on (#1081).
///
/// Two codes cover it. A source entry or a clock the caller handed over that
/// the planner cannot use is the caller's to fix, so it is `invalid_option`.
/// A manifest that does not parse is a document that did not, which is what
/// `decode` already says about a container; the planner's own variant is kept
/// in the prose, which names the dialect and the line.
#[cfg(feature = "fetchplan")]
impl From<fieldglass_fetchplan::FetchPlanError> for Error {
    fn from(e: fieldglass_fetchplan::FetchPlanError) -> Self {
        use fieldglass_fetchplan::FetchPlanError as E;
        match e {
            E::UnknownPlaceholder { .. }
            | E::UnterminatedPlaceholder { .. }
            | E::NoCycleHours
            | E::CycleHourOutOfRange { .. }
            | E::UnsortedCycleHours { .. }
            | E::TimeOutOfRange { .. }
            | E::OffsetPastEnd { .. } => Self::invalid_option(e.to_string()),
            other => Self::Decode {
                detail: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every variant.
    ///
    /// The `match` below names each variant with no wildcard. Inside this crate
    /// `#[non_exhaustive]` does not apply, so a variant added to `Error` fails
    /// to compile here until it is named, and the list it has to join is the
    /// one directly above. `api_rules` cannot do this from outside the crate,
    /// which is how `WrongAddressing` went unpinned by the conformance suite
    /// from #662 until #679.
    fn one_of_each() -> Vec<Error> {
        let all = vec![
            Error::UnsupportedFormat { detail: "x".into() },
            Error::Decode { detail: "x".into() },
            Error::NoSuchMessage { index: 3, count: 1 },
            Error::Unsupported { detail: "x".into() },
            Error::ShortRead {
                at: 4,
                got: 7,
                wanted: 8,
            },
            Error::WrongAddressing {
                expected: "variables".into(),
                detail: "x".into(),
            },
            Error::invalid_option("x"),
        ];
        for e in &all {
            match e {
                Error::UnsupportedFormat { .. }
                | Error::Decode { .. }
                | Error::NoSuchMessage { .. }
                | Error::Unsupported { .. }
                | Error::WrongAddressing { .. }
                | Error::ShortRead { .. }
                | Error::InvalidOption { .. } => {}
            }
        }
        all
    }

    /// Every code this build can report is one the conformance suite pins, and
    /// the suite pins no code this build cannot report.
    #[cfg(feature = "conformance")]
    #[test]
    fn every_code_is_pinned_by_the_conformance_suite() {
        let mut codes: Vec<String> = one_of_each().iter().map(|e| e.code().to_string()).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes, crate::conformance::error_codes());
    }

    /// The serde tag and `code()` are the same string by contract — a host
    /// that reads the JSON and one that calls the method must branch alike.
    #[test]
    fn the_serde_tag_is_the_code() {
        for e in one_of_each() {
            let json: serde_json::Value = serde_json::to_value(&e).expect("serialise");
            assert_eq!(
                json.get("code").and_then(|c| c.as_str()),
                Some(e.code()),
                "{e:?}"
            );
            assert!(!e.message().is_empty());
        }
    }
}
