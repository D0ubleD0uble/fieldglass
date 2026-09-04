//! One error type for the whole API surface (ADR-0006 decision 2).

use fieldglass_core::FieldglassError;

/// Everything an API call can fail with.
///
/// One enum, not a per-operation family: a host maps errors once, and
/// [`code`](Error::code) is the stable string it maps on. The `code` is part of
/// the API — a host may branch on it — while [`message`](Error::message) is
/// prose and may be reworded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// a grid with no geometry asked to warp, a spectral field asked to probe.
    Unsupported {
        /// Which operation, and what about this message refuses it.
        detail: String,
    },
    /// A caller-supplied option is out of range or self-contradictory.
    InvalidOption {
        /// Which option, and what it would have had to be.
        detail: String,
    },
}

impl Error {
    /// The stable machine-readable tag. Matches the serde discriminant, so a
    /// host reading the JSON form and one calling this method cannot disagree.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat { .. } => "unsupported_format",
            Self::Decode { .. } => "decode",
            Self::NoSuchMessage { .. } => "no_such_message",
            Self::Unsupported { .. } => "unsupported",
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
            Self::NoSuchMessage { index, count } => {
                format!("message {index} is out of range; the file holds {count}")
            }
            Self::Unsupported { detail } => detail.clone(),
            Self::InvalidOption { detail } => detail.clone(),
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
        Self::Decode {
            detail: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serde tag and `code()` are the same string by contract — a host
    /// that reads the JSON and one that calls the method must branch alike.
    #[test]
    fn the_serde_tag_is_the_code() {
        for e in [
            Error::UnsupportedFormat { detail: "x".into() },
            Error::Decode { detail: "x".into() },
            Error::NoSuchMessage { index: 3, count: 1 },
            Error::Unsupported { detail: "x".into() },
            Error::InvalidOption { detail: "x".into() },
        ] {
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
