//! Fetch planning, with the tables a planner is not allowed to link.
//!
//! [`fieldglass_fetchplan`] reads a cloud-native manifest and returns a chunk
//! plan: which bytes to fetch, and which chunk or message each range is. It
//! answers in two addressing shapes, and both are reachable from here:
//!
//! * **A query over messages.** A GRIB sidecar — a wgrib2 `.idx` or an ECMWF
//!   `.index` — lists the messages of one object, and a request is "which
//!   field" ([`MessageManifest::select`] with a [`Query`]).
//! * **A chunk index over an array.** A kerchunk reference document maps the
//!   chunks of Zarr arrays onto byte ranges of other objects, and a request is
//!   "which chunk" ([`KerchunkRefs::chunk_at`], through the array's
//!   [`ArrayMetadata`] and its [`ChunkGrid`]).
//!
//! ```
//! use fieldglass::fetchplan::{Address, ChunkGrid, KerchunkRefs, Manifest};
//!
//! let doc = r#"{
//!   "version": 1,
//!   "templates": {"u": "s3://bucket/archive.nc"},
//!   "refs": {
//!     "temp/.zarray": "{\"zarr_format\":2,\"shape\":[4,6],\"chunks\":[2,3],\"dtype\":\"<f4\"}",
//!     "temp/1.1": ["{{u}}", 4096, 24]
//!   }
//! }"#;
//! let refs = KerchunkRefs::parse(doc)?;
//! let meta = refs.array("temp")?;
//! let grid: &ChunkGrid = meta.grid();
//! assert_eq!(grid.grid_shape(), vec![2, 2]);
//!
//! let item = refs.chunk_at("temp", &meta, &[1, 1])?.expect("the document addresses it");
//! assert_eq!(item.key, "s3://bucket/archive.nc");
//! assert_eq!(item.range.http_range_header().as_deref(), Some("bytes=4096-4119"));
//! assert_eq!(item.address, Address::Chunk { array: "temp".into(), index: vec![1, 1] });
//!
//! // The same chunk is the whole of the plan the document states.
//! assert_eq!(refs.items(), vec![item]);
//! # Ok::<(), fieldglass::fetchplan::FetchPlanError>(())
//! ```
//!
//! The planner depends on no format crate on purpose — a planner that linked a
//! decoder would drag GRIB2's four codecs into a host that only wanted to know
//! which bytes to ask for — so the two things the message half needs a decoder
//! for live here, where one already is:
//!
//! * [`TableResolver`] resolves a sidecar's `TMP` / `2 m above ground` to WMO
//!   codes through the GRIB2 parameter tables (#426), so a request stored as
//!   (discipline, category, number, level, value) selects without the caller
//!   knowing NCEP's spelling.
//! * [`verify_message`] is the semantic half of "a plan is a claim": the
//!   parameter and level the message actually decodes to must be the ones the
//!   sidecar promised. The syntactic half —
//!   [`Expect::verify_envelope`](fieldglass_fetchplan::Expect::verify_envelope),
//!   the magic and the §0 length — needs no tables and stays in the planner, so
//!   a host can run it before it has a decoder in hand at all.
//!
//! ```no_run
//! use fieldglass::fetchplan::{MessageManifest, Query, TableResolver, Wgrib2Idx};
//!
//! # fn fetch(_key: &str, _header: Option<String>) -> Vec<u8> { unimplemented!() }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let idx = Wgrib2Idx::parse("gfs.t00z.pgrb2.0p25.f000", &fetch_text()?)?;
//! let item = &idx.select(&Query::abbreviation("TMP"), &TableResolver::ncep())[0];
//!
//! let bytes = fetch(&item.key, item.range.http_range_header());
//! item.expect.verify_envelope(&bytes, &item.range)?;   // syntax: magic, §0 length
//!
//! let session = fieldglass::Session::open(bytes)?;
//! let info = session.message(0)?;
//! fieldglass::fetchplan::verify_message(&item.expect, &info)?;  // semantics
//! # Ok(()) }
//! # fn fetch_text() -> Result<String, std::io::Error> { unimplemented!() }
//! ```

//! # From a host
//!
//! The plan crosses to a host as it is (#1081): [`PlanItem`], [`PlanRange`],
//! [`Address`], [`Expect`], [`Query`], [`SourceSpec`] and [`Candidate`] are
//! serde types with a camelCase wire form, so a binding hands them over and
//! reads them back without a DTO of its own. Two things here exist for a host
//! rather than for a Rust caller: [`verify_envelope`], which is
//! [`Expect::verify_envelope`] answering in this crate's [`Error`], and
//! [`CandidateOptions`], the clock and step a host sends to [`candidates`].

use std::collections::HashMap;
use std::sync::OnceLock;

pub use fieldglass_fetchplan::{
    Address, Candidate, Dialect, EcmwfIndex, Expect, FetchPlanError, LevelSpec, Manifest,
    MessageManifest, Mismatch, NoResolver, ParameterId, ParameterResolver, PlanItem, PlanRange,
    Query, SourceSpec, Surface, Wgrib2Idx, candidates, parse_ecmwf_level, parse_ncep_level,
};
// The chunk half (#687): the reference-document dialect, the one reader of an
// array's metadata, and the chunk-grid arithmetic it spells keys with. The last
// three are `fieldglass-core`'s and `fieldglass-zarr`'s, taken through the
// planner rather than directly, so a host naming them here names the very types
// the planner's own signatures carry.
pub use fieldglass_fetchplan::{
    ArrayError, ArrayMetadata, ChunkGrid, ChunkKeyEncoding, KerchunkRefs,
};

use crate::api::{MessageIdentity, MessageInfo, SurfaceCodes};
use crate::error::Error;

/// When to ask [`candidates`] about, as a host sends it.
///
/// The clock is the host's (the planner reads none), so it is the one thing a
/// host has to state. The step defaults to the analysis.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct CandidateOptions {
    /// Now, as whole Unix seconds. A browser's `Date.now()` is milliseconds,
    /// so pass `Math.floor(Date.now() / 1000)`: a fractional second is not an
    /// integer and is refused.
    pub now_unix_secs: i64,
    /// The forecast step to expand each key for, in hours. `0`, the analysis,
    /// when left out.
    #[serde(default)]
    pub step_hours: u32,
}

impl CandidateOptions {
    /// The analysis step, at `now_unix_secs`.
    pub fn new(now_unix_secs: i64) -> Self {
        Self {
            now_unix_secs,
            step_hours: 0,
        }
    }
}

/// The runs of `source` that should have posted by the options' clock, newest
/// first, as [`candidates`] answers it but in this crate's [`Error`].
pub fn candidates_at(
    source: &SourceSpec,
    options: &CandidateOptions,
) -> Result<Vec<Candidate>, Error> {
    Ok(candidates(
        source,
        options.now_unix_secs,
        options.step_hours,
    )?)
}

/// Check fetched bytes against the envelope a plan item promised:
/// [`Expect::verify_envelope`] over the item's own range, in this crate's
/// [`Error`]. Returns the message's declared total length.
///
/// A host passes the item it was handed back whole, so the expectation and the
/// range cannot come from two different items.
///
/// Fewer bytes than the item asked for, or than the message declares, is
/// [`Error::ShortRead`], which is worth retrying. Anything else — the bytes are
/// not GRIB, or not the length the sidecar said, which is what a stale sidecar
/// looks like — is [`Error::Decode`].
pub fn verify_envelope(item: &PlanItem, bytes: &[u8]) -> Result<u64, Error> {
    let short = |wanted: u64| Error::ShortRead {
        at: item.range.offset(),
        got: bytes.len() as u64,
        wanted,
    };
    let asked = item.range.length();
    item.expect
        .verify_envelope(bytes, &item.range)
        .map_err(|m| match m {
            // A range that stated its length says how many bytes the fetch
            // should have returned, so fewer is a short transfer whatever the
            // bytes then fail on. Read off the buffer and not the mismatch:
            // `Length` also reports a sidecar whose stated length disagrees
            // with the message's, and then its `fetched` is the stated length.
            _ if asked.is_some_and(|asked| (bytes.len() as u64) < asked) => {
                short(asked.unwrap_or_default())
            }
            // An open range has no length to hold the fetch to, so too few
            // bytes for the indicator section, or for the length the message
            // declares, is the transfer's. An exact range that arrived whole
            // and is still too short is the sidecar's, and falls through.
            Mismatch::TooShort { need, .. } if asked.is_none() => short(need as u64),
            Mismatch::Truncated { declared, .. } => short(declared),
            other => Error::Decode {
                detail: format!("the fetched bytes are not what the plan promised: {other}"),
            },
        })
}

/// Disciplines to scan when building the reverse index.
///
/// Read from [`fieldglass_grib2::lookup_discipline`] rather than written down:
/// WMO Code Table 0.0 assigns eight, and scanning all 255 costs 879 ms of
/// `lookup_parameter` calls against ~25 ms for these — thirty-six times the
/// work, for code space no table defines. A discipline WMO adds later is picked
/// up when that table gains it, without this list being touched.
///
/// `lookup_discipline` answers `None` for an unassigned or local-use code
/// (#774), and the range stops short of 255, the one code it names `"Missing"`.
/// `scan_bounds::no_parameter_lives_outside_the_named_disciplines` is what
/// stops that being a silent assumption — and it earned its place immediately,
/// by catching discipline 191, which carries six parameters and which Code
/// Table 0.0 assigns but `lookup_discipline` did not name.
fn disciplines() -> Vec<u8> {
    (0u8..=254)
        .filter(|&d| fieldglass_grib2::lookup_discipline(d).is_some())
        .collect()
}

/// A [`ParameterResolver`] over this build's GRIB2 parameter tables.
///
/// # Why it is an index and not a lookup
///
/// The tables answer *codes to name*; a sidecar hands us a *name*. There is no
/// reverse function to call, so the first `resolve` walks the code space
/// through [`fieldglass_grib2::lookup_parameter`] and builds the inverse map
/// once — a few hundred thousand lookups, tens of milliseconds — and every
/// record of the sidecar after that is a hash hit. Resolving per record without
/// the index would be that scan times the number of records, which on the
/// 696-record GFS sidecar is not a slow path but a hung one.
///
/// The index is per [`Originator`](fieldglass_grib2::Originator), because the
/// local code space (192–254 in any of the three octets) resolves against the
/// originating centre's own table.
///
/// # What it does not resolve
///
/// **ECMWF's MARS short names.** An ECMWF `.index` says `param: 2t`, which is a
/// MARS name and not a GRIB2 abbreviation; no table in this repo carries one,
/// so `2t` resolves to `None` here and an ECMWF source is selected by its own
/// vocabulary until such a table exists. The planner's
/// [`ParameterResolver`] seam takes any implementation, so that is a table to
/// add rather than a design to change.
///
/// **An ambiguous name.** Where two triples share an abbreviation the first in
/// scan order wins, which is the lowest triple — deterministic, and the WMO
/// master entry rather than a local override, since local space starts at 192.
/// Guessing between two *different* parameters would be worse than either.
#[derive(Debug)]
#[non_exhaustive]
pub struct TableResolver {
    originator: fieldglass_grib2::Originator,
    index: OnceLock<HashMap<String, ParameterId>>,
}

impl TableResolver {
    /// A resolver for one originating centre's tables.
    pub fn new(originator: fieldglass_grib2::Originator) -> Self {
        Self {
            originator,
            index: OnceLock::new(),
        }
    }

    /// A resolver for NCEP (centre 7), which is what every NOAA `.idx` sidecar
    /// is written in.
    pub fn ncep() -> Self {
        Self::new(fieldglass_grib2::Originator::new(7, 0, 1))
    }

    /// The reverse index, built on first use.
    fn index(&self) -> &HashMap<String, ParameterId> {
        self.index.get_or_init(|| {
            let mut map = HashMap::new();
            for discipline in disciplines() {
                for category in 0u8..=254 {
                    for number in 0u8..=254 {
                        if let Some((short, _, _)) = fieldglass_grib2::lookup_parameter(
                            self.originator,
                            discipline,
                            category,
                            number,
                        ) && !short.is_empty()
                        {
                            // `or_insert`, so the lowest triple wins and the
                            // map does not depend on iteration order.
                            map.entry(short.to_ascii_uppercase())
                                .or_insert(ParameterId {
                                    discipline,
                                    category,
                                    number,
                                });
                        }
                    }
                }
            }
            map
        })
    }
}

impl ParameterResolver for TableResolver {
    fn resolve(&self, abbrev: &str, _level: &str) -> Option<ParameterId> {
        // wgrib2's numeric fallback (`var discipline=0 …`) is read by the
        // planner itself and never reaches here; anything else is a short name.
        self.index().get(&abbrev.to_ascii_uppercase()).copied()
    }
}

/// Check a decoded message against what the manifest line promised.
///
/// The semantic half of verification. [`Expect::verify_envelope`] has already
/// established that the bytes are a GRIB message of the right length; this
/// establishes that they are *the* message — because a sidecar regenerated
/// against a newer object can point at a perfectly valid message that is simply
/// the wrong field.
///
/// Only fields the manifest actually promised are checked; an [`Expect`] with
/// nothing in it passes, which is correct rather than lax — it is what a
/// caller gets from a manifest that stated nothing to check.
///
/// # What is compared with what
///
/// A sidecar's words and a message's codes are compared as **values**, never
/// as two strings (#1093). A wgrib2 `.idx` writes `2 mb` for a message whose
/// §4 states isobaric surface 200 Pa, and comparing either rendering with the
/// other refuses every real record.
///
/// * **Parameter codes**, where the line states them (wgrib2's
///   `var discipline=… parmcat=… parm=…` fallback name): equal to the GRIB2
///   message's discipline, category and number.
/// * **Abbreviation**, on a wgrib2 `.idx`: equal, ignoring case and
///   surrounding whitespace, to the short name the message's own codes have in
///   its centre's table — the forward lookup, so two codes sharing a name both
///   pass.
/// * **Level**: the line's [`LevelSpec`] against one built from the message's
///   fixed surfaces (GRIB2 Code Table 4.5, GRIB1 Table 3), in the line's units
///   — hectopascals for pressure, so `2 mb` and 200 Pa are the same level. A
///   value matches to six significant figures, the precision wgrib2 prints
///   with. A level the grammar kept as [`Surface::Named`] was not understood
///   and is not compared; a level it did understand and the message is not on
///   is refused, including a message on a surface this check does not model.
///   An ECMWF `sfc` is every single-level field (2-metre temperature is
///   `sfc`), so it is refused only for a message on a surface ECMWF lists
///   levels of (`pl`, `ml`, `pt`, `pv`, `sol`).
/// * **Reference time**: the ten digits `YYYYMMDDHH` both spellings carry.
///
/// **Two records get no parameter check at all.** An ECMWF `.index` line's
/// `param` is a MARS short name (`2t`), no table here maps those to WMO codes,
/// and the line states no codes of its own. And a wgrib2 line whose message's
/// codes have no short name in this build's tables has nothing to compare its
/// name with: resolving the name to codes instead would refuse a correct
/// record whenever the name also belongs to other codes (`REFD` is both
/// 0.16.4 and NCEP's 0.16.195). For both, only the level and the reference
/// time are compared, so a stale sidecar pointing at another field on the same
/// level passes.
///
/// **A message on a product template the reader does not parse** (the
/// chemistry and aerosol templates 4.40–4.48, such as GEFS-aerosol's 4.48
/// `AOTK` and `PMTC` records) is checked on its reference time alone. Its
/// parameter codes, short name and fixed surfaces are all unread, so the level
/// is not compared, as for a level the grammar did not understand. Every
/// message on a parsed template is compared on its surfaces as above.
///
/// The forecast step and an ensemble qualifier (`ENS=+1`) are not compared.
pub fn verify_message(expect: &Expect, info: &MessageInfo) -> Result<(), Mismatch> {
    let ecmwf = expect.dialect == Some(Dialect::EcmwfIndex);

    if let Some(promised) = expect.parameter
        && let Some(actual) = grib2_parameter(&info.identity)
        && promised != actual
    {
        return Err(Mismatch::Field {
            field: "parameter",
            expected: codes(promised),
            actual: codes(actual),
        });
    }

    if !ecmwf
        && let Some(promised) = expect.abbreviation.as_deref()
        // A numeric fallback name is not an abbreviation the decoder can match;
        // the `parameter` check above is the one that applies to it.
        && !promised.starts_with("var ")
        // No short name means this build's tables do not name the message's
        // codes, so there is nothing to compare with (see above).
        && let Some(actual) = info.abbreviation.as_deref()
        && !equivalent(promised, actual)
    {
        return Err(Mismatch::Field {
            field: "abbreviation",
            expected: promised.to_string(),
            actual: actual.to_string(),
        });
    }

    if let Some(promised) = promised_level(expect, ecmwf)
        && !level_holds(&promised, ecmwf, &info.identity)
    {
        return Err(Mismatch::Field {
            field: "level",
            expected: expect
                .level
                .clone()
                .unwrap_or_else(|| format!("{promised:?}")),
            actual: message_level_prose(info),
        });
    }

    if let Some(promised) = &expect.reference_time
        // The sidecar writes `YYYYMMDDHH`; the message states RFC 3339. Compared
        // as the ten digits both can produce, so this is a real check and not a
        // format comparison that always fails.
        && let Some(actual) = ten_digit_reference_time(&info.reference_time)
        && promised.len() == 10
        && *promised != actual
    {
        return Err(Mismatch::Field {
            field: "referenceTime",
            expected: promised.clone(),
            actual,
        });
    }

    Ok(())
}

/// Case-insensitive, whitespace-trimmed equality.
fn equivalent(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// `2008-02-06T12:00:00Z` to `2008020612`, or `None` if it is not that shape.
fn ten_digit_reference_time(rfc3339: &str) -> Option<String> {
    let digits: String = rfc3339.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 10).then(|| digits[..10].to_string())
}

/// `discipline.category.number`, for a mismatch message.
fn codes(p: ParameterId) -> String {
    format!("{}.{}.{}", p.discipline, p.category, p.number)
}

/// A GRIB2 message's parameter codes. `None` for GRIB1, whose codes are not
/// GRIB2's, and for a product template the reader does not parse.
fn grib2_parameter(identity: &MessageIdentity) -> Option<ParameterId> {
    match identity {
        MessageIdentity::Grib2 {
            discipline,
            parameter_category: Some(category),
            parameter_number: Some(number),
            ..
        } => Some(ParameterId {
            discipline: *discipline,
            category: *category,
            number: *number,
        }),
        _ => None,
    }
}

/// The level the line promised, parsed.
///
/// Both parsers fill [`Expect::level_spec`]; an expectation that came over
/// the wire with only the text is parsed here, by its dialect's grammar, so
/// the text is never compared as a string.
fn promised_level(expect: &Expect, ecmwf: bool) -> Option<LevelSpec> {
    if let Some(spec) = &expect.level_spec {
        return Some(spec.clone());
    }
    let text = expect.level.as_deref()?;
    Some(if ecmwf {
        // `to_item` writes `levtype` alone or `levtype levelist`.
        match text.trim().split_once(' ') {
            Some((levtype, levelist)) => parse_ecmwf_level(levtype, Some(levelist)),
            None => parse_ecmwf_level(text, None),
        }
    } else {
        parse_ncep_level(text)
    })
}

/// Whether the message is on the level `promised` names.
fn level_holds(promised: &LevelSpec, ecmwf: bool, identity: &MessageIdentity) -> bool {
    match &promised.surface {
        // Kept verbatim because the grammar did not understand it, so there is
        // no surface to compare. Comparing the text with the message's would
        // refuse every such record, which is the defect this replaces.
        Surface::Named(_) => true,
        // The reader did not parse the message's product template (the
        // chemistry and aerosol templates 4.40-4.48, say), so its surfaces
        // are unknown and there is nothing to compare with either (#1126).
        _ if surfaces_unknown(identity) => true,
        Surface::Surface if ecmwf => !on_an_ecmwf_level_list(identity),
        _ => message_level(identity).is_some_and(|have| same_level(promised, &have)),
    }
}

/// Whether the message is GRIB2 on a product template the reader does not
/// parse, which leaves its fixed surfaces unread. A parsed template always
/// states its first surface, so this is not a message on no surface.
fn surfaces_unknown(identity: &MessageIdentity) -> bool {
    matches!(
        identity,
        MessageIdentity::Grib2 {
            first_surface: None,
            ..
        }
    )
}

/// How far apart, relative to the larger, a sidecar's value and the message's
/// may be and still be one value.
///
/// wgrib2 prints a level with `%g`, six significant figures, so the text is
/// within half a unit in the sixth figure of the value: at most 5e-6 of it.
/// It rounds an `f32`, which adds up to `f32::EPSILON` of the value more.
/// Anything wider would let a different level through; `1.00001` and `1` are
/// two levels at this precision.
const PRINTED_PRECISION: f64 = 5e-6 + f32::EPSILON as f64;

/// Whether `have`, the message's level, is the level `want` names.
///
/// [`LevelSpec::matches`] is the query rule and would accept a layer for a
/// single level; here a stated single level must be a single level.
fn same_level(want: &LevelSpec, have: &LevelSpec) -> bool {
    if want.surface != have.surface {
        return false;
    }
    // A surface the line names without a value (`surface`, `tropopause`)
    // says nothing about one.
    if want.value.is_none() {
        return true;
    }
    let close = |w: Option<f64>, h: Option<f64>| match (w, h) {
        (None, None) => true,
        (Some(w), Some(h)) => w == h || (w - h).abs() <= PRINTED_PRECISION * w.abs().max(h.abs()),
        _ => false,
    };
    close(want.value, have.value) && close(want.value2, have.value2)
}

/// Whether the message sits on a surface an ECMWF `.index` gives a `levtype`
/// with a `levelist` (`pl`, `ml`, `pt`, `pv`, `sol`), which is what `sfc` is
/// not.
fn on_an_ecmwf_level_list(identity: &MessageIdentity) -> bool {
    match identity {
        MessageIdentity::Grib2 {
            first_surface: Some(first),
            ..
        } => matches!(first.surface_type, 100 | 105 | 107 | 109 | 151),
        // Table 3: isobaric, hybrid, isentropic, potential vorticity. Not the
        // depth layer 112: ECMWF's GRIB1 soil fields (`swvl1`, `stl1`) are
        // `sfc` on it.
        MessageIdentity::Grib1 { level_type, .. } => {
            matches!(level_type, 100 | 109 | 113 | 117)
        }
        _ => false,
    }
}

/// The level, rendered for a mismatch message: the decoder's own value and
/// surface type.
fn message_level_prose(info: &MessageInfo) -> String {
    match (info.level.as_deref(), info.level_type.as_deref()) {
        (Some(level), Some(kind)) if level != kind => format!("{level} ({kind})"),
        (Some(level), _) => level.to_string(),
        (None, Some(kind)) => kind.to_string(),
        (None, None) => String::new(),
    }
}

/// How a surface's value becomes the sidecar's.
#[derive(Clone, Copy)]
enum Value {
    /// The surface takes no value in the sidecar's wording.
    None,
    /// The wording states one value whatever the message codes (`0C
    /// isotherm` is GRIB2 type 4, which codes none).
    Fixed(f64),
    /// The coded value divided by `10^shift`: 2 for pascals to hectopascals,
    /// 0 for a value already in the sidecar's unit.
    Shift(i32),
}

/// A GRIB2 fixed surface type (Code Table 4.5) as a [`Surface`], with how its
/// value reads. `None` for a type this check does not model.
///
/// The pairing is wgrib2's own: the wording `src/Level.c` prints for each
/// type is the wording [`parse_ncep_level`] reads back. 200, 215 and 220 are
/// NCEP's local entries, which wgrib2 names for an NCEP message only.
fn grib2_surface(code: u8) -> Option<(Surface, Value)> {
    Some(match code {
        1 => (Surface::Surface, Value::None),
        2 => (Surface::CloudBase, Value::None),
        3 => (Surface::CloudTop, Value::None),
        4 => (Surface::Isotherm, Value::Fixed(0.0)),
        6 => (Surface::MaxWind, Value::None),
        7 => (Surface::Tropopause, Value::None),
        8 => (Surface::TopOfAtmosphere, Value::None),
        10 | 200 => (Surface::EntireAtmosphere, Value::None),
        20 => (Surface::Isothermal, Value::Shift(0)),
        100 => (Surface::Isobaric, Value::Shift(2)),
        101 => (Surface::MeanSeaLevel, Value::None),
        102 => (Surface::HeightAboveMeanSeaLevel, Value::Shift(0)),
        103 => (Surface::HeightAboveGround, Value::Shift(0)),
        104 => (Surface::SigmaLevel, Value::Shift(0)),
        105 => (Surface::HybridLevel, Value::Shift(0)),
        106 => (Surface::DepthBelowLand, Value::Shift(0)),
        107 => (Surface::PotentialTemperature, Value::Shift(0)),
        108 => (Surface::PressureAboveGround, Value::Shift(2)),
        215 => (Surface::CloudCeiling, Value::None),
        220 => (Surface::PlanetaryBoundaryLayer, Value::None),
        _ => return None,
    })
}

/// `scaled · 10^-(scale + shift)`, the one division that rounds correctly, so
/// `995` at scale 3 is the `0.995` a sidecar's text parses to.
fn scaled(value: Option<i64>, scale: Option<i8>, shift: i32) -> Option<f64> {
    let exponent = i32::from(scale?) + shift;
    let value = value? as f64;
    Some(if exponent >= 0 {
        value / 10f64.powi(exponent)
    } else {
        value * 10f64.powi(-exponent)
    })
}

/// The message's level, as a [`LevelSpec`] in the units a sidecar writes.
/// `None` when the message has no surface, or one this check does not model.
fn message_level(identity: &MessageIdentity) -> Option<LevelSpec> {
    match identity {
        MessageIdentity::Grib2 {
            first_surface: Some(first),
            second_surface,
            ..
        } => {
            let (surface, how) = grib2_surface(first.surface_type)?;
            let read = |s: &SurfaceCodes| match how {
                Value::None => None,
                Value::Fixed(v) => Some(v),
                Value::Shift(shift) => scaled(s.scaled_value, s.scale_factor, shift),
            };
            let value2 = match second_surface {
                None => None,
                Some(second) if second.surface_type == 255 => None,
                // A layer between two of the same surface, `30-0 mb above
                // ground`. Two different surfaces are a layer wgrib2 writes
                // as two levels joined by ` - `, which the grammar keeps
                // named, so a modelled level never names one.
                // A surface that takes no value names no layer either, so a
                // second surface of the same type adds nothing to compare:
                // `surface` is the ground whatever §4 put beside it.
                Some(second)
                    if second.surface_type == first.surface_type
                        && !matches!(how, Value::Shift(_)) =>
                {
                    None
                }
                Some(second) if second.surface_type == first.surface_type => Some(read(second)?),
                Some(_) => return None,
            };
            let mut level = LevelSpec::named(surface);
            level.value = read(first);
            level.value2 = value2;
            Some(level)
        }
        MessageIdentity::Grib1 {
            level_type,
            level_octets: [a, b],
            ..
        } => grib1_level(*level_type, *a, *b),
        _ => None,
    }
}

/// A GRIB1 level (Table 3, PDS octets 10–12) in a sidecar's units.
fn grib1_level(code: u8, a: u8, b: u8) -> Option<LevelSpec> {
    let both = f64::from(u16::from_be_bytes([a, b]));
    let (a, b) = (f64::from(a), f64::from(b));
    Some(match code {
        1 => LevelSpec::named(Surface::Surface),
        2 => LevelSpec::named(Surface::CloudBase),
        3 => LevelSpec::named(Surface::CloudTop),
        4 => LevelSpec::at(Surface::Isotherm, 0.0),
        6 => LevelSpec::named(Surface::MaxWind),
        7 => LevelSpec::named(Surface::Tropopause),
        8 => LevelSpec::named(Surface::TopOfAtmosphere),
        100 => LevelSpec::at(Surface::Isobaric, both),
        // Top and bottom in kilopascals.
        101 => LevelSpec::between(Surface::Isobaric, a * 10.0, b * 10.0),
        102 => LevelSpec::named(Surface::MeanSeaLevel),
        103 => LevelSpec::at(Surface::HeightAboveMeanSeaLevel, both),
        105 => LevelSpec::at(Surface::HeightAboveGround, both),
        // Top and bottom in hectometres.
        106 => LevelSpec::between(Surface::HeightAboveGround, a * 100.0, b * 100.0),
        107 => LevelSpec::at(Surface::SigmaLevel, both / 10_000.0),
        108 => LevelSpec::between(Surface::SigmaLevel, a / 100.0, b / 100.0),
        109 => LevelSpec::at(Surface::HybridLevel, both),
        110 => LevelSpec::between(Surface::HybridLevel, a, b),
        // Centimetres.
        111 => LevelSpec::at(Surface::DepthBelowLand, both / 100.0),
        112 => LevelSpec::between(Surface::DepthBelowLand, a / 100.0, b / 100.0),
        113 => LevelSpec::at(Surface::PotentialTemperature, both),
        116 => LevelSpec::between(Surface::PressureAboveGround, a, b),
        200 => LevelSpec::named(Surface::EntireAtmosphere),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names every NOAA sidecar in the corpus leans on, resolved through
    /// the real tables. `TMP` is the one a stored 2-metre-temperature request
    /// turns into, so it is the one the browser host's first query depends on.
    #[test]
    fn the_ncep_index_resolves_the_names_the_sidecars_use() {
        let r = TableResolver::ncep();
        assert_eq!(
            r.resolve("TMP", "2 m above ground"),
            Some(ParameterId {
                discipline: 0,
                category: 0,
                number: 0
            })
        );
        assert_eq!(
            r.resolve("HGT", "500 mb"),
            Some(ParameterId {
                discipline: 0,
                category: 3,
                number: 5
            })
        );
        assert_eq!(
            r.resolve("PRMSL", "mean sea level"),
            Some(ParameterId {
                discipline: 0,
                category: 3,
                number: 1
            })
        );
        // Case-insensitive, because a sidecar's spelling is not a contract.
        assert_eq!(r.resolve("tmp", ""), r.resolve("TMP", ""));
    }

    /// An NCEP local parameter — code space 192+ — resolves too, which is what
    /// #426's table is for. A resolver that only saw the WMO master table would
    /// silently answer `None` for every CONUS-model-specific field.
    #[test]
    fn a_local_ncep_parameter_resolves_through_the_centres_table() {
        let r = TableResolver::ncep();
        // Local-only: `TTRAD` is in NCEP's table and in no WMO one, so it can
        // only have come from the centre's tables.
        assert_eq!(
            r.resolve("TTRAD", "surface"),
            Some(ParameterId {
                discipline: 0,
                category: 0,
                number: 193
            })
        );
    }

    /// An abbreviation both tables carry resolves to the **WMO master** triple,
    /// not the centre's local one.
    ///
    /// `CRAIN` is the case: NCEP defines it at (0, 1, 192) and WMO has since
    /// standardised it at (0, 1, 33). Both are "categorical rain", so either
    /// answer decodes something sensible — which is exactly why the choice has
    /// to be pinned rather than left to `HashMap` iteration order. The lowest
    /// triple wins, and local code space starts at 192, so the standard entry
    /// is the one a stored request gets.
    #[test]
    fn a_name_both_tables_carry_resolves_to_the_wmo_entry() {
        assert_eq!(
            TableResolver::ncep().resolve("CRAIN", "surface"),
            Some(ParameterId {
                discipline: 0,
                category: 1,
                number: 33
            })
        );
    }

    /// ECMWF MARS names are not in any table here, and the resolver says so
    /// rather than inventing a triple. Written down because it is a limitation
    /// a reader will otherwise rediscover from a silent empty selection.
    #[test]
    fn an_ecmwf_mars_name_does_not_resolve() {
        assert_eq!(TableResolver::ncep().resolve("2t", "sfc"), None);
    }

    /// A name nothing defines resolves to nothing, which is what keeps a
    /// parameter query from matching everything.
    #[test]
    fn an_unknown_name_resolves_to_nothing() {
        assert_eq!(TableResolver::ncep().resolve("NOTAPARAMETER", ""), None);
    }

    /// One GRIB2 surface as §4 codes it: type, scale factor, scaled value.
    type Codes = (u8, i8, i64);

    fn surface((surface_type, scale, value): Codes) -> crate::api::SurfaceCodes {
        crate::api::SurfaceCodes {
            surface_type,
            scale_factor: Some(scale),
            scaled_value: Some(value),
        }
    }

    /// A GRIB2 message with `abbrev` for its short name, on `first` (and on
    /// `second` for a layer). The parameter codes are temperature's whatever
    /// the name, which only the parameter-code test reads.
    fn info_on(abbrev: &str, first: Codes, second: Option<Codes>) -> MessageInfo {
        MessageInfo {
            index: 0,
            offset_bytes: 0,
            parameter: Some("Temperature".into()),
            abbreviation: Some(abbrev.into()),
            units: Some("K".into()),
            // What the decoder renders: the bare coded value, which is what a
            // sidecar's wording must not be compared with as text.
            level: Some(first.2.to_string()),
            level_type: Some(format!("Surface type {}", first.0)),
            reference_time: "2026-09-04T00:00:00Z".into(),
            forecast: Some("analysis".into()),
            packing: Some("grid_simple".into()),
            grid: None,
            placement: crate::api::Placement::NoRaster,
            reprojectable: false,
            supports: crate::api::GridSupport::default(),
            size_label: None,
            truncation: None,
            // Identification `verify_message` does not read.
            forecast_hours: None,
            originating_centre: String::new(),
            sub_centre: None,
            identification: crate::api::Identification::Grib2 {
                discipline: String::new(),
                production_status: String::new(),
                data_type: String::new(),
            },
            // The parameter and the surfaces are what it reads (#1093).
            identity: crate::api::MessageIdentity::Grib2 {
                discipline: 0,
                product_template: 0,
                product_octets: None,
                ensemble_type: None,
                ensemble_member: None,
                derived_forecast: None,
                local_centre: None,
                parameter_category: Some(0),
                parameter_number: Some(0),
                first_surface: Some(surface(first)),
                second_surface: Some(second.map_or(
                    crate::api::SurfaceCodes {
                        surface_type: 255,
                        scale_factor: None,
                        scaled_value: None,
                    },
                    surface,
                )),
                reference_time: "2026-09-04T00:00:00Z".into(),
                forecast_time_unit: None,
                forecast_time: None,
                range_end: None,
                statistics: Vec::new(),
                grid_template: 0,
                dimensions: None,
                data_points: 0,
            },
            total_length_bytes: 0,
            uv_relative_to_grid: None,
        }
    }

    /// Temperature on one surface.
    fn info(abbrev: &str, first: Codes) -> MessageInfo {
        info_on(abbrev, first, None)
    }

    const TWO_METRES: Codes = (103, 0, 2);

    fn level(text: &str) -> Expect {
        Expect::new().with_level(text)
    }

    fn holds(text: &str, first: Codes, second: Option<Codes>) -> bool {
        verify_message(&level(text), &info_on("TMP", first, second)).is_ok()
    }

    /// The check that catches a stale sidecar pointing at a valid but wrong
    /// message: the bytes decode fine, and they are not the promised field.
    #[test]
    fn a_wrong_field_is_reported_with_both_sides() {
        let expect = Expect::new()
            .with_abbreviation("TMP")
            .with_level("2 m above ground");
        assert_eq!(verify_message(&expect, &info("TMP", TWO_METRES)), Ok(()));

        assert_eq!(
            verify_message(&expect, &info("RH", TWO_METRES)),
            Err(Mismatch::Field {
                field: "abbreviation",
                expected: "TMP".into(),
                actual: "RH".into()
            })
        );
        assert_eq!(
            verify_message(&expect, &info("TMP", (100, 0, 85_000))),
            Err(Mismatch::Field {
                field: "level",
                expected: "2 m above ground".into(),
                actual: "85000 (Surface type 100)".into()
            })
        );
    }

    /// wgrib2 writes an isobaric level in millibars and §4 codes it in
    /// pascals; the two are one value in two units (#1093). Includes the
    /// stratospheric levels GFS publishes below 1 mb.
    #[test]
    fn an_isobaric_level_is_compared_in_hectopascals() {
        assert!(holds("2 mb", (100, 0, 200), None));
        assert!(holds("500 mb", (100, 0, 50_000), None));
        assert!(holds("0.01 mb", (100, 0, 1), None));
        assert!(holds("0.4 mb", (100, 0, 40), None));
        // A scale factor and the unit shift combine.
        assert!(holds("2 mb", (100, -2, 2), None));
        assert!(!holds("3 mb", (100, 0, 200), None));
        assert!(!holds("2 mb", (108, 0, 200), None));
    }

    /// A scaled value is the decimal the sidecar wrote, and a layer is two of
    /// them, in order.
    #[test]
    fn scaled_values_and_layers_are_compared_as_numbers() {
        assert!(holds("0.995 sigma level", (104, 3, 995), None));
        assert!(holds("1 hybrid level", (105, 0, 1), None));
        assert!(holds(
            "0-0.1 m below ground",
            (106, 0, 0),
            Some((106, 1, 1))
        ));
        assert!(holds(
            "30-0 mb above ground",
            (108, 0, 3000),
            Some((108, 0, 0))
        ));
        assert!(holds(
            "6000-0 m above ground",
            (103, 0, 6000),
            Some((103, 0, 0))
        ));
        // The bounds in the other order are another layer.
        assert!(!holds(
            "0-30 mb above ground",
            (108, 0, 3000),
            Some((108, 0, 0))
        ));
        // A layer is not the single level at its top, nor the reverse.
        assert!(!holds(
            "30 mb above ground",
            (108, 0, 3000),
            Some((108, 0, 0))
        ));
        assert!(!holds("30-0 mb above ground", (108, 0, 3000), None));
    }

    /// The surfaces wgrib2 names without a value, each against the code
    /// `Level.c` names that way.
    #[test]
    fn named_surfaces_are_compared_by_their_codes() {
        assert!(holds("surface", (1, 0, 0), None));
        assert!(holds("mean sea level", (101, 0, 0), None));
        assert!(holds("entire atmosphere", (10, 0, 0), None));
        assert!(holds(
            "entire atmosphere (considered as a single layer)",
            (200, 0, 0),
            None
        ));
        assert!(holds("cloud ceiling", (215, 0, 0), None));
        assert!(holds("planetary boundary layer", (220, 0, 0), None));
        assert!(holds("tropopause", (7, 0, 0), None));
        // Type 4 codes no value; the wording's `0` is the surface's.
        assert!(holds("0C isotherm", (4, 0, 0), None));
        // Type 20 is a temperature, not the isentropic 107.
        assert!(holds("263 K level", (20, 0, 263), None));
        assert!(!holds("263 K level", (107, 0, 263), None));
        assert!(holds("320 K isentropic level", (107, 0, 320), None));

        assert!(!holds("surface", TWO_METRES, None));
        assert!(!holds("tropopause", (6, 0, 0), None));
    }

    /// A level the grammar understood is refused on a surface this check does
    /// not model, rather than waved through: the line said where the field is,
    /// and the message is somewhere else.
    #[test]
    fn a_modelled_level_is_refused_on_an_unmodelled_surface() {
        assert!(!holds("2 m above ground", (109, 6, 2), None));
    }

    /// A level the grammar kept verbatim was not understood, so there is
    /// nothing to compare it with and it is not compared.
    #[test]
    fn a_level_the_grammar_did_not_understand_is_not_compared() {
        assert!(holds("PV=2e-06 (Km^2/kg/s) surface", (109, 6, 2), None));
        assert!(holds(
            "lowest level of the wet bulb zero",
            (245, 0, 0),
            None
        ));
    }

    /// A message whose product template the reader does not parse (the
    /// chemistry and aerosol templates 4.40–4.48) has no known surfaces, so a
    /// level, in either dialect, is not compared with it (#1126). A message
    /// whose surfaces are known is still refused on a different one.
    #[test]
    fn a_level_is_not_compared_with_an_unparsed_template() {
        let mut unparsed = info("AOTK", (10, 0, 0));
        unparsed.abbreviation = None;
        unparsed.level = None;
        unparsed.level_type = None;
        if let crate::api::MessageIdentity::Grib2 {
            product_template,
            parameter_category,
            parameter_number,
            first_surface,
            second_surface,
            ..
        } = &mut unparsed.identity
        {
            *product_template = 48;
            *parameter_category = None;
            *parameter_number = None;
            *first_surface = None;
            *second_surface = None;
        }
        for text in ["entire atmosphere", "surface", "2 mb", "2 m above ground"] {
            assert_eq!(verify_message(&level(text), &unparsed), Ok(()), "{text}");
        }
        for record in [
            r#"{"param": "r", "levtype": "pl", "levelist": "50", "_offset": 0, "_length": 1}"#,
            r#"{"param": "2t", "levtype": "sfc", "_offset": 0, "_length": 1}"#,
        ] {
            assert_eq!(
                verify_message(&ecmwf(record), &unparsed),
                Ok(()),
                "{record}"
            );
        }

        assert!(!holds("surface", (10, 0, 0), None));
        assert!(!holds("2 mb", (1, 0, 0), None));
    }

    /// An expectation that promises nothing checks nothing, rather than
    /// failing because the fields are absent.
    #[test]
    fn an_empty_expectation_passes() {
        assert_eq!(
            verify_message(&Expect::default(), &info("TMP", (1, 0, 0))),
            Ok(())
        );
    }

    /// A numeric fallback name is not an abbreviation and is not compared as
    /// one; the codes it states are compared with the message's instead.
    #[test]
    fn a_numeric_fallback_name_is_checked_by_its_codes() {
        let expect = Expect::new()
            .with_abbreviation("var discipline=0 center=7 local_table=1 parmcat=16 parm=201")
            .with_parameter(ParameterId {
                discipline: 0,
                category: 16,
                number: 201,
            });
        let mut refc = info("REFC", (10, 0, 0));
        let crate::api::MessageIdentity::Grib2 {
            parameter_category,
            parameter_number,
            ..
        } = &mut refc.identity
        else {
            unreachable!()
        };
        *parameter_category = Some(16);
        *parameter_number = Some(201);
        assert_eq!(verify_message(&expect, &refc), Ok(()));

        assert_eq!(
            verify_message(&expect, &info("TMP", (10, 0, 0))),
            Err(Mismatch::Field {
                field: "parameter",
                expected: "0.16.201".into(),
                actual: "0.0.0".into()
            })
        );
    }

    /// A message whose codes this build's tables do not name has no short name
    /// to compare with, so its name is not compared. Resolving the promised
    /// name to codes instead would refuse a correct record whose name another
    /// triple also carries.
    #[test]
    fn a_message_without_a_short_name_is_not_compared_by_name() {
        let mut unnamed = info("TMP", TWO_METRES);
        unnamed.abbreviation = None;
        assert_eq!(
            verify_message(&Expect::new().with_abbreviation("RH"), &unnamed),
            Ok(())
        );
    }

    /// Six significant figures, the precision wgrib2 prints with, and no
    /// wider: a value that differs in the sixth figure is another level.
    #[test]
    fn a_value_matches_to_the_precision_it_was_printed_at() {
        // 0.995 as an `f32`, which is what wgrib2 formats, is not 0.995.
        assert!(holds("0.995 sigma level", (104, 9, 995_000_005), None));
        // `%g` of 1000.004 hPa is `1000`.
        assert!(holds("1000 mb", (100, 1, 1_000_004), None));
        assert!(!holds("1.00001 sigma level", (104, 0, 1), None));
        assert!(!holds("1 sigma level", (104, 5, 100_001), None));
    }

    /// A surface that takes no value names no layer, so a second surface of
    /// the same type beside it does not make it something else.
    #[test]
    fn a_valueless_surface_beside_itself_is_still_that_surface() {
        assert!(holds("surface", (1, 0, 0), Some((1, 0, 0))));
        assert!(holds("entire atmosphere", (10, 0, 0), Some((10, 0, 0))));
        // A different second surface is a layer wgrib2 names otherwise.
        assert!(!holds("surface", (1, 0, 0), Some((8, 0, 0))));
    }

    /// An ECMWF `.index` line, through the real parser so the expectation
    /// carries its dialect.
    fn ecmwf(record: &str) -> Expect {
        let idx = EcmwfIndex::parse("o", record).expect("one record");
        idx.items()[0].expect.clone()
    }

    /// MARS vocabulary: `2t` is not compared with the WMO `TMP`, and `sfc` is
    /// any single-level surface but not one ECMWF lists levels of.
    #[test]
    fn an_ecmwf_line_is_read_in_its_own_vocabulary() {
        let sfc = ecmwf(r#"{"param": "2t", "levtype": "sfc", "_offset": 0, "_length": 1}"#);
        assert_eq!(verify_message(&sfc, &info("TMP", TWO_METRES)), Ok(()));
        assert_eq!(verify_message(&sfc, &info("MSL", (101, 0, 0))), Ok(()));
        assert!(verify_message(&sfc, &info("TMP", (100, 0, 50_000))).is_err());
        assert!(verify_message(&sfc, &info("TMP", (105, 0, 137))).is_err());

        let pl = ecmwf(
            r#"{"param": "t", "levtype": "pl", "levelist": "500", "_offset": 0, "_length": 1}"#,
        );
        assert_eq!(verify_message(&pl, &info("TMP", (100, 0, 50_000))), Ok(()));
        assert!(verify_message(&pl, &info("TMP", (100, 0, 85_000))).is_err());

        let ml = ecmwf(
            r#"{"param": "t", "levtype": "ml", "levelist": "137", "_offset": 0, "_length": 1}"#,
        );
        assert_eq!(verify_message(&ml, &info("TMP", (105, 0, 137))), Ok(()));
    }

    /// An expectation that crossed the wire with its level text and no parsed
    /// level is parsed here rather than compared as a string.
    #[test]
    fn a_level_sent_as_text_alone_is_parsed() {
        let expect: Expect = serde_json::from_value(serde_json::json!({
            "abbreviation": null,
            "parameter": null,
            "level": "2 mb",
            "levelSpec": null,
            "forecast": null,
            "referenceTime": null,
            "totalLength": null,
            "qualifiers": [],
            "dialect": "wgrib2_idx",
        }))
        .expect("an expectation");
        assert_eq!(verify_message(&expect, &info("RH", (100, 0, 200))), Ok(()));
        assert!(verify_message(&expect, &info("RH", (100, 0, 300))).is_err());
    }

    /// The two reference-time spellings are compared as the ten digits both can
    /// produce. A naive string comparison would fail on every message.
    #[test]
    fn reference_times_are_compared_across_their_two_spellings() {
        let expect = Expect::new().with_reference_time("2026090400");
        assert_eq!(verify_message(&expect, &info("TMP", (1, 0, 0))), Ok(()));

        let stale = Expect::new().with_reference_time("2026090312");
        assert_eq!(
            verify_message(&stale, &info("TMP", (1, 0, 0))),
            Err(Mismatch::Field {
                field: "referenceTime",
                expected: "2026090312".into(),
                actual: "2026090400".into()
            })
        );
    }

    /// A GRIB1 message's level reads through Table 3 in the same units.
    #[test]
    fn a_grib1_level_reads_in_the_sidecars_units() {
        assert_eq!(
            grib1_level(100, 0x01, 0xf4),
            Some(LevelSpec::at(Surface::Isobaric, 500.0))
        );
        assert_eq!(
            grib1_level(105, 0, 2),
            Some(LevelSpec::at(Surface::HeightAboveGround, 2.0))
        );
        assert_eq!(
            grib1_level(112, 0, 10),
            Some(LevelSpec::between(Surface::DepthBelowLand, 0.0, 0.1))
        );
        assert_eq!(
            grib1_level(116, 30, 0),
            Some(LevelSpec::between(Surface::PressureAboveGround, 30.0, 0.0))
        );
        assert_eq!(grib1_level(117, 0, 2), None);
    }
}

#[cfg(test)]
mod scan_bounds {
    use super::*;

    /// The index scans only the disciplines WMO Code Table 0.0 names, which is
    /// a 36× saving (879 ms of `lookup_parameter` calls becomes ~25 ms) and an
    /// assumption: that no parameter table defines a triple under a discipline
    /// the code table does not name. Asserted rather than relied on, because
    /// the filter is only as good as `lookup_discipline`'s idea of which codes
    /// it names, and a change there would silently narrow or widen the scan.
    ///
    /// The full sweep is the slow part of this crate's test run and is worth
    /// it: without it, a local discipline added to a centre's table would drop
    /// out of every parameter query with no failure anywhere.
    #[test]
    fn no_parameter_lives_outside_the_named_disciplines() {
        let originator = fieldglass_grib2::Originator::new(7, 0, 1);
        let named = disciplines();
        let mut missed = Vec::new();
        for discipline in 0u8..=254 {
            if named.contains(&discipline) {
                continue;
            }
            for category in 0u8..=254 {
                for number in 0u8..=254 {
                    if fieldglass_grib2::lookup_parameter(originator, discipline, category, number)
                        .is_some()
                    {
                        missed.push((discipline, category, number));
                    }
                }
            }
        }
        assert!(
            missed.is_empty(),
            "parameters outside the scanned disciplines would never resolve: {:?}",
            &missed[..missed.len().min(10)]
        );
        assert_eq!(
            named,
            vec![0, 1, 2, 3, 4, 10, 20, 191],
            "the eight disciplines WMO Code Table 0.0 assigns"
        );
    }

    /// The index is not empty and not implausibly small — a filter that
    /// excluded everything would make every parameter query silently match
    /// nothing, and every test above it would still pass by asserting `None`.
    #[test]
    fn the_index_covers_the_whole_table() {
        let n = TableResolver::ncep().index().len();
        assert!(n > 1_000, "the reverse index holds only {n} names");
    }
}
