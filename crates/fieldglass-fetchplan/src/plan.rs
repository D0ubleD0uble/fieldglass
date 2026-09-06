//! What a planner hands back: a key, a byte range, and what the bytes in it are
//! supposed to be.
//!
//! **A plan is a claim, not a fact.** A sidecar can be stale — NODD regenerates
//! objects and every offset after the first change then points into the middle
//! of a message — an open-ended last range can over- or under-shoot, and an
//! `n.m` sub-message shares its offset with its sibling. So every [`PlanItem`]
//! carries the [`Expect`] its line promised, and the consumer checks the bytes
//! it fetched against it before decoding them.

use crate::error::{FetchPlanError, Mismatch};
use crate::level::LevelSpec;

/// The GRIB §0 indicator section, which is the first thing a fetched range must
/// look like. Four magic bytes, two reserved, discipline, edition.
const GRIB_MAGIC: &[u8; 4] = b"GRIB";

/// GRIB edition 2 states its total length as a `u64` in octets 9..16, so §0 is
/// sixteen octets.
const GRIB2_SECTION0_LEN: usize = 16;

/// GRIB edition 1 states its total length as a 24-bit big-endian integer in
/// octets 5..7, with the edition in octet 8, so §0 is eight octets.
const GRIB1_SECTION0_LEN: usize = 8;

/// A byte range to fetch, in the three shapes a manifest can state one.
///
/// The distinction is not cosmetic. A wgrib2 `.idx` gives the offset of every
/// message and the length of none: a message ends where the next one begins, so
/// the **last** record has no stated end at all. Some buckets will not tell a
/// browser what the object's size is either — NOAA's NODD buckets expose
/// neither `Content-Length` nor `Content-Range` cross-origin, while ECMWF does
/// — so the open end has to survive as far as the host, which either issues an
/// open-ended `Range` header or closes it with a size it knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[non_exhaustive]
pub enum PlanRange {
    /// Both ends known: `offset..offset + length`.
    Exact {
        /// First byte, from the start of the object.
        offset: u64,
        /// How many bytes.
        length: u64,
    },
    /// The start is known and the end is the end of the object.
    OpenEnded {
        /// First byte, from the start of the object.
        offset: u64,
    },
    /// The whole object, because the manifest addresses it as one unit.
    Whole,
}

impl PlanRange {
    /// The first byte of the range.
    pub fn offset(&self) -> u64 {
        match self {
            Self::Exact { offset, .. } | Self::OpenEnded { offset } => *offset,
            Self::Whole => 0,
        }
    }

    /// One past the last byte, when the range states an end.
    pub fn end_exclusive(&self) -> Option<u64> {
        match self {
            Self::Exact { offset, length } => Some(offset.saturating_add(*length)),
            Self::OpenEnded { .. } | Self::Whole => None,
        }
    }

    /// The length, when the range states one.
    pub fn length(&self) -> Option<u64> {
        match self {
            Self::Exact { length, .. } => Some(*length),
            Self::OpenEnded { .. } | Self::Whole => None,
        }
    }

    /// Close both ends against an object size the host has learned.
    ///
    /// Returns [`fieldglass_core::ByteRange`] — the type
    /// [`ByteSource::prefetch`](fieldglass_core::ByteSource::prefetch) takes —
    /// rather than another `PlanRange`. That is the distinction the two types
    /// are for: a `PlanRange` is what a *manifest* could state, and a
    /// `ByteRange` is a settled `[start, start + len)`. Closing is the step
    /// between them, and handing the result straight to the byte source is what
    /// a host does next.
    ///
    /// A stated length is used as-is rather than re-derived from `object_size`:
    /// a manifest that stated one knows better than a `Content-Length` does, and
    /// a disagreement between the two is for verification to report, not for
    /// this to paper over.
    pub fn close(self, object_size: u64) -> Result<fieldglass_core::ByteRange, FetchPlanError> {
        let (offset, length) = match self {
            Self::Exact { offset, length } => (offset, length),
            Self::Whole => (0, object_size),
            Self::OpenEnded { offset } => (
                offset,
                object_size
                    .checked_sub(offset)
                    .ok_or(FetchPlanError::OffsetPastEnd {
                        offset,
                        object_size,
                    })?,
            ),
        };
        Ok(fieldglass_core::ByteRange::new(offset, length))
    }

    /// The value of an HTTP `Range` header, or `None` when the whole object is
    /// wanted and no header should be sent.
    ///
    /// `None` rather than `Some("bytes=0-")` on purpose: the two are equivalent
    /// to a compliant server, but an unconditional header turns every whole
    /// fetch into a `206 Partial Content`, which some caches decline to store.
    pub fn http_range_header(&self) -> Option<String> {
        match self {
            // `length` of zero has no valid header form — `bytes=n-(n-1)` is
            // backwards — and no message is zero bytes long, so it is reported
            // as "nothing to fetch" rather than as a malformed header.
            Self::Exact { length: 0, .. } => None,
            Self::Exact { offset, length } => {
                Some(format!("bytes={offset}-{}", offset + length - 1))
            }
            Self::OpenEnded { offset } => Some(format!("bytes={offset}-")),
            Self::Whole => None,
        }
    }
}

/// What the manifest line promised about the bytes in a [`PlanItem`]'s range.
///
/// Every field is optional because the dialects promise different things: a
/// wgrib2 `.idx` states an abbreviation, a level and a forecast in NCEP's own
/// vocabulary and no length at all, while an ECMWF `.index` states a length and
/// a `param` in ECMWF's. What they have in common is that each is a *claim* a
/// consumer can check.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Expect {
    /// The parameter's short name in the **sidecar's own** vocabulary — `TMP`
    /// on an NCEP `.idx`, `2t` on an ECMWF `.index`. Not translated: the whole
    /// point of an expectation is that it is what the manifest said.
    pub abbreviation: Option<String>,
    /// The parameter's WMO codes, when the manifest states them outright or a
    /// [`ParameterResolver`](crate::ParameterResolver) resolved them.
    ///
    /// wgrib2 writes them in the line itself for a parameter its own tables do
    /// not name — `var discipline=0 center=7 local_table=1 parmcat=16 parm=201`
    /// — which is the one case where a `.idx` is *more* precise than its
    /// abbreviation, not less.
    pub parameter: Option<ParameterId>,
    /// The level exactly as the manifest renders it: `2 m above ground`,
    /// `500 mb`, `sfc`.
    pub level: Option<String>,
    /// The level parsed into a surface and a value, where the grammar in
    /// [`crate::level`] recognises it.
    pub level_spec: Option<LevelSpec>,
    /// The forecast field as the manifest renders it: `anl`, `6 hour fcst`,
    /// `0-1 hour acc fcst`, or an ECMWF `step` in hours.
    pub forecast: Option<String>,
    /// The reference time as the manifest renders it — a wgrib2 `d=` value with
    /// the prefix stripped (`2026090400`), or an ECMWF `date` and `time`
    /// joined (`2026090400`).
    pub reference_time: Option<String>,
    /// The message's total length in bytes, when the manifest states it.
    /// ECMWF's `_length` does; a wgrib2 `.idx` never does.
    pub total_length: Option<u64>,
    /// Everything else the line carried, in order — `ENS=+1`, `prob <304.8`,
    /// `prob fcst 3/7`. Matched verbatim by a query and otherwise passed
    /// through, because the vocabulary is per-product and open-ended.
    pub qualifiers: Vec<String>,
}

impl Expect {
    /// An expectation that promises nothing, to add to.
    ///
    /// These builders exist because [`Expect`] is `#[non_exhaustive]`, which
    /// stops a consumer outside this crate writing a struct literal — and a
    /// consumer that has bytes from somewhere other than a manifest still
    /// wants to state what they should be and check them. Without a way to
    /// construct one, verification would be reachable only through a parsed
    /// sidecar.
    pub fn new() -> Self {
        Self::default()
    }

    /// State the short name the bytes should carry.
    #[must_use]
    pub fn with_abbreviation(mut self, abbreviation: impl Into<String>) -> Self {
        self.abbreviation = Some(abbreviation.into());
        self
    }

    /// State the level, in an NCEP `.idx`'s wording.
    ///
    /// Fills [`level_spec`](Self::level_spec) from the same string through
    /// [`parse_ncep_level`](crate::parse_ncep_level), so the two cannot
    /// disagree — which they could if a caller set them separately.
    #[must_use]
    pub fn with_level(mut self, level: impl Into<String>) -> Self {
        let level = level.into();
        self.level_spec = Some(crate::level::parse_ncep_level(&level));
        self.level = Some(level);
        self
    }

    /// State the parameter's WMO codes.
    #[must_use]
    pub fn with_parameter(mut self, parameter: ParameterId) -> Self {
        self.parameter = Some(parameter);
        self
    }

    /// State the reference time, as the ten digits a `d=` field carries.
    #[must_use]
    pub fn with_reference_time(mut self, reference_time: impl Into<String>) -> Self {
        self.reference_time = Some(reference_time.into());
        self
    }

    /// State the message's total length in bytes.
    #[must_use]
    pub fn with_total_length(mut self, total_length: u64) -> Self {
        self.total_length = Some(total_length);
        self
    }

    /// Check fetched bytes against the envelope this expectation implies.
    ///
    /// The cheap half of "a plan is a claim", and the half that needs no
    /// decoder: the bytes must start with the GRIB magic, state an edition this
    /// check understands, and declare a §0 total length consistent with what
    /// was actually fetched. A stale sidecar fails on the magic, because a
    /// mid-message offset lands in packed data; a short read fails on the
    /// length. Returns the declared total length so a caller that fetched
    /// open-ended knows where the message ends.
    ///
    /// Both editions are checked, not just GRIB2. §0 is the one section the two
    /// share, they disagree only about where the length sits and how wide it
    /// is, and a `.idx` beside a GRIB1 object is a perfectly ordinary thing for
    /// wgrib2 to have written.
    ///
    /// The **semantic** half — that the discipline, parameter and level the
    /// message decodes to are the ones the line promised — needs the tables, so
    /// it lives with the decoder. [`Mismatch::Field`] is its shape, declared
    /// here so both halves report a mismatch the same way.
    pub fn verify_envelope(&self, bytes: &[u8], range: &PlanRange) -> Result<u64, Mismatch> {
        if bytes.len() < GRIB1_SECTION0_LEN {
            return Err(Mismatch::TooShort {
                need: GRIB1_SECTION0_LEN,
                fetched: bytes.len(),
            });
        }
        if &bytes[..4] != GRIB_MAGIC {
            return Err(Mismatch::Magic {
                expected: "GRIB".to_string(),
                found: printable(&bytes[..4]),
            });
        }

        // Octet 8 in both editions, which is what makes one check cover both.
        let edition = bytes[7];
        let declared = match edition {
            1 => u64::from(u32::from_be_bytes([0, bytes[4], bytes[5], bytes[6]])),
            2 => {
                if bytes.len() < GRIB2_SECTION0_LEN {
                    return Err(Mismatch::TooShort {
                        need: GRIB2_SECTION0_LEN,
                        fetched: bytes.len(),
                    });
                }
                let mut octets = [0u8; 8];
                octets.copy_from_slice(&bytes[8..16]);
                u64::from_be_bytes(octets)
            }
            found => return Err(Mismatch::Edition { found }),
        };

        // A manifest that stated a length is checked against the message's own
        // before the fetch is, so a disagreement is reported against the
        // sidecar rather than against the host's `Range` arithmetic.
        if let Some(promised) = self.total_length
            && promised != declared
        {
            return Err(Mismatch::Length {
                declared,
                fetched: promised,
            });
        }

        let fetched = bytes.len() as u64;
        match range {
            // An exact range is a claim about the length, so an inequality
            // either way is a mismatch.
            PlanRange::Exact { .. } if declared != fetched => {
                Err(Mismatch::Length { declared, fetched })
            }
            // An open end may over-shoot: the host asked for everything from
            // the offset on and got the rest of the object, which is one
            // message plus whatever followed it. Only a short read is wrong.
            _ if declared > fetched => Err(Mismatch::Truncated { declared, fetched }),
            _ => Ok(declared),
        }
    }
}

/// Render bytes for an error message, escaping anything unprintable.
///
/// A stale offset lands in packed data, so the "found" half of a magic mismatch
/// is arbitrary binary; printing it raw would put control characters into a
/// host's log or a test's assertion output.
fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                (b as char).to_string()
            } else {
                format!("\\x{b:02x}")
            }
        })
        .collect()
}

/// A WMO parameter's identity, as GRIB2 numbers it.
///
/// Three octets and nothing else, because that is what a request is stored as
/// and what both sidecar vocabularies have to resolve *to* for a query written
/// once to match on either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterId {
    /// GRIB2 discipline (Code Table 0.0).
    pub discipline: u8,
    /// Parameter category within the discipline (Code Table 4.1).
    pub category: u8,
    /// Parameter number within the category (Code Table 4.2).
    pub number: u8,
}

/// One thing to fetch: which object, which bytes, and what they should be.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PlanItem {
    /// The object key the manifest describes, exactly as the caller gave it.
    /// This crate never invents one and hard-codes no bucket.
    pub key: String,
    /// The bytes to fetch.
    pub range: PlanRange,
    /// Which field *within* the message, when the message holds more than one.
    ///
    /// wgrib2 numbers those records `n.m`, and every one of them shares message
    /// `n`'s offset — so two items can carry the identical range and differ
    /// only here. 1-based, matching the `m` wgrib2 writes; `None` when the
    /// message holds a single field.
    pub sub_index: Option<u32>,
    /// What the manifest line promised about these bytes.
    pub expect: Expect,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three header forms, including the one that is deliberately absent.
    #[test]
    fn range_headers_are_the_http_forms() {
        assert_eq!(
            PlanRange::Exact {
                offset: 100,
                length: 50
            }
            .http_range_header()
            .as_deref(),
            // Inclusive on both ends: `bytes=100-149` is 50 bytes, and the
            // off-by-one here would silently fetch 51.
            Some("bytes=100-149")
        );
        assert_eq!(
            PlanRange::OpenEnded { offset: 100 }
                .http_range_header()
                .as_deref(),
            Some("bytes=100-")
        );
        assert_eq!(PlanRange::Whole.http_range_header(), None);
    }

    /// Closing is what a host does when it learns the object size, and it hands
    /// back the settled `[start, start + len)` the byte source takes. It must
    /// not silently produce a backwards range when the size it learned is
    /// smaller than the offset it planned.
    #[test]
    fn closing_an_open_end_needs_a_size_past_the_offset() {
        assert_eq!(
            PlanRange::OpenEnded { offset: 10 }.close(30).unwrap(),
            fieldglass_core::ByteRange::new(10, 20)
        );
        assert_eq!(
            PlanRange::Whole.close(30).unwrap(),
            fieldglass_core::ByteRange::new(0, 30)
        );
        assert_eq!(
            PlanRange::OpenEnded { offset: 40 }.close(30),
            Err(FetchPlanError::OffsetPastEnd {
                offset: 40,
                object_size: 30
            })
        );
    }

    /// A manifest that states a length outranks a `Content-Length`: closing
    /// must not overwrite it, even when the object is far larger.
    #[test]
    fn closing_leaves_a_stated_length_alone() {
        let exact = PlanRange::Exact {
            offset: 10,
            length: 5,
        };
        assert_eq!(
            exact.close(1_000).unwrap(),
            fieldglass_core::ByteRange::new(10, 5)
        );
    }

    /// A minimal GRIB2 §0 for a message of `len` bytes.
    fn grib2_section0(len: u64) -> Vec<u8> {
        let mut v = b"GRIB\0\0\0\x02".to_vec();
        v.extend_from_slice(&len.to_be_bytes());
        v
    }

    #[test]
    fn an_exact_range_must_match_the_declared_length() {
        let mut bytes = grib2_section0(32);
        bytes.resize(32, 0);
        let e = Expect::default();
        assert_eq!(
            e.verify_envelope(
                &bytes,
                &PlanRange::Exact {
                    offset: 0,
                    length: 32
                }
            ),
            Ok(32)
        );

        // One byte short of what the message declares.
        assert_eq!(
            e.verify_envelope(
                &bytes[..31],
                &PlanRange::Exact {
                    offset: 0,
                    length: 31
                }
            ),
            Err(Mismatch::Length {
                declared: 32,
                fetched: 31
            })
        );
    }

    /// An open-ended fetch returns the rest of the object, so it legitimately
    /// over-shoots one message. Only a short read is a mismatch.
    #[test]
    fn an_open_end_may_overshoot_but_not_undershoot() {
        let mut bytes = grib2_section0(32);
        bytes.resize(100, 0);
        let e = Expect::default();
        assert_eq!(
            e.verify_envelope(&bytes, &PlanRange::OpenEnded { offset: 0 }),
            Ok(32)
        );
        assert_eq!(
            e.verify_envelope(&bytes[..20], &PlanRange::OpenEnded { offset: 0 }),
            Err(Mismatch::Truncated {
                declared: 32,
                fetched: 20
            })
        );
    }

    /// GRIB1's length is 24 bits in octets 5..7, so the same check has to read
    /// it from a different place. A GRIB1 object with a `.idx` beside it is
    /// ordinary, and an envelope check that only knew edition 2 would reject
    /// every message in it.
    #[test]
    fn edition_one_states_its_length_in_three_octets() {
        // 0x000064 == 100 bytes, edition 1.
        let mut bytes = vec![b'G', b'R', b'I', b'B', 0x00, 0x00, 0x64, 0x01];
        bytes.resize(100, 0);
        assert_eq!(
            Expect::default().verify_envelope(
                &bytes,
                &PlanRange::Exact {
                    offset: 0,
                    length: 100
                }
            ),
            Ok(100)
        );
    }

    /// A GRIB1 message is only eight octets into its §0, so requiring sixteen
    /// would refuse a short-but-valid one. The edition decides how much is
    /// needed, and the check must ask for the larger amount only for edition 2.
    #[test]
    fn edition_two_needs_sixteen_octets_and_edition_one_does_not() {
        let short = b"GRIB\0\0\0\x02";
        assert_eq!(
            Expect::default().verify_envelope(short, &PlanRange::Whole),
            Err(Mismatch::TooShort {
                need: 16,
                fetched: 8
            })
        );
    }

    /// The stale-sidecar signal. A mid-message offset lands in packed data, so
    /// the check fails on the magic and the error carries the bytes that were
    /// there — escaped, because they are arbitrary binary.
    #[test]
    fn a_mid_message_offset_fails_on_the_magic_with_both_sides() {
        let bytes = [0x1a_u8, 0x0b, b'x', 0xff, 0, 0, 0, 0];
        let err = Expect::default()
            .verify_envelope(&bytes, &PlanRange::OpenEnded { offset: 7 })
            .unwrap_err();
        assert_eq!(
            err,
            Mismatch::Magic {
                expected: "GRIB".to_string(),
                found: "\\x1a\\x0bx\\xff".to_string(),
            }
        );
    }

    /// A length the manifest stated is checked against §0 before the fetch is,
    /// so a stale ECMWF `_length` is reported against the sidecar.
    #[test]
    fn a_promised_length_is_checked_against_the_message() {
        let mut bytes = grib2_section0(32);
        bytes.resize(32, 0);
        let e = Expect {
            total_length: Some(40),
            ..Expect::default()
        };
        assert_eq!(
            e.verify_envelope(
                &bytes,
                &PlanRange::Exact {
                    offset: 0,
                    length: 32
                }
            ),
            Err(Mismatch::Length {
                declared: 32,
                fetched: 40
            })
        );
    }

    #[test]
    fn an_unknown_edition_is_refused_by_number() {
        let bytes = b"GRIB\0\0\0\x09";
        assert_eq!(
            Expect::default().verify_envelope(bytes, &PlanRange::Whole),
            Err(Mismatch::Edition { found: 9 })
        );
    }
}
