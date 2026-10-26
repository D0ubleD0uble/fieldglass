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
//! assert_eq!(item.range.http_range_header()?.as_deref(), Some("bytes=4096-4119"));
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
//!   parameter, level and forecast window the message actually decodes to,
//!   and its probability, ensemble member, derived forecast, percentile, and
//!   chemical or aerosol constituent and ranges, must be the ones the sidecar
//!   promised. The syntactic half —
//!   [`Expectation::verify_envelope`](fieldglass_fetchplan::Expectation::verify_envelope),
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
//! let bytes = fetch(&item.key, item.range.http_range_header()?);
//! item.expectation.verify_envelope(&bytes, &item.range)?;   // syntax: magic, §0 length
//!
//! let session = fieldglass::Session::open(bytes)?;
//! let info = session.message(0)?;
//! fieldglass::fetchplan::verify_message(&item.expectation, &info)?;  // semantics
//! # Ok(()) }
//! # fn fetch_text() -> Result<String, std::io::Error> { unimplemented!() }
//! ```

//! # From a host
//!
//! The plan crosses to a host as it is (#1081): [`PlanItem`], [`PlanRange`],
//! [`Address`], [`Expectation`], [`Query`], [`SourceSpec`] and [`Candidate`] are
//! serde types with a camelCase wire form, so a binding hands them over and
//! reads them back without a DTO of its own. Two things here exist for a host
//! rather than for a Rust caller: [`verify_envelope`], which is
//! [`Expectation::verify_envelope`] answering in this crate's [`Error`], and
//! [`CandidateOptions`], the clock and step a host sends to [`candidates`].

use std::collections::HashMap;
use std::sync::OnceLock;

pub use fieldglass_fetchplan::{
    Address, Candidate, Dialect, EcmwfIndex, Expectation, FetchPlanError, LevelSpec,
    MAX_CANDIDATES, Manifest, MessageManifest, Mismatch, NoResolver, ParameterId,
    ParameterResolver, PlanItem, PlanRange, Query, SourceSpec, Surface, Wgrib2Idx, candidates,
    parse_ecmwf_level, parse_ncep_level,
};
// The chunk half (#687): the reference-document dialect, the one reader of an
// array's metadata, and the chunk-grid arithmetic it spells keys with. The last
// three are `fieldglass-core`'s and `fieldglass-zarr`'s, taken through the
// planner rather than directly, so a host naming them here names the very types
// the planner's own signatures carry.
pub use fieldglass_fetchplan::{
    ArrayError, ArrayMetadata, ChunkGrid, ChunkKeyEncoding, KerchunkRefs,
};

use crate::api::{MessageFingerprint, MessageInfo, StatisticalStep, SurfaceCodes};
use crate::error::Error;
use fieldglass_core::bits::sign_magnitude_to_i64;

mod mars_short_names;

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
/// [`Expectation::verify_envelope`] over the item's own range, in this crate's
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
    item.expectation
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
/// MARS name and not a GRIB2 abbreviation, so `2t` resolves to `None` here and
/// an ECMWF source is selected by its own vocabulary. [`verify_message`]
/// checks a MARS name against the message through a table of its own (#1116),
/// but that table maps a name to eccodes' concept blocks, several of which
/// share one triple (`2t`, `mx2t3` and `mn2t3` are all 0.0.0), so it is not a
/// resolver: a query by codes would select all of them. The planner's
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
/// The semantic half of verification. [`Expectation::verify_envelope`] has already
/// established that the bytes are a GRIB message of the right length; this
/// establishes that they are *the* message — because a sidecar regenerated
/// against a newer object can point at a perfectly valid message that is simply
/// the wrong field.
///
/// Only fields the manifest actually promised are checked; an [`Expectation`] with
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
///   pass. A message whose codes the tables do not name is checked the other
///   way only where the tables are complete (see below).
/// * **MARS short name**, on an ECMWF `.index` (#1116): the message must match
///   one of the eccodes concept blocks for the name, on the parameter codes,
///   the statistical process and the length of its time range. `mx2t3` and
///   `mn2t3` share discipline 0, category 0 and number 0, and the process is
///   what tells them apart. `2t` shares them too and states no process, so a
///   `2t` line is not refused on `mx2t3` bytes at the same step. The table
///   covers ECMWF's open-data parameters; another name is not compared.
/// * **Level**: the line's [`LevelSpec`] against one built from the message's
///   fixed surfaces (GRIB2 Code Table 4.5, GRIB1 Table 3), in the line's units
///   — hectopascals for pressure, so `2 mb` and 200 Pa are the same level. A
///   value matches to six significant figures, the precision wgrib2 prints
///   with. A level the grammar kept as [`Surface::Named`] was not understood
///   and is not compared; a level it did understand and the message is not on
///   is refused, including a message on a surface this check does not model.
///   A wgrib2 line's level is not compared with a template 4.44 message,
///   because wgrib2 reads the surface of 4.44's four-octet form from the
///   wrong octets.
///   An ECMWF `sfc` is every single-level field (2-metre temperature is
///   `sfc`), so it is refused only for a message on a surface ECMWF lists
///   levels of (`pl`, `ml`, `pt`, `pv`, `sol`), unless eccodes' `levtype`
///   definitions call that message `sfc` too: AIFS medium and high cloud
///   cover are isobaric layers (800 to 450 hPa, 450 hPa to the top of the
///   atmosphere) and `sfc` (#1323). An ECMWF `sol N` is soil
///   level N (type 151), or the soil layer whose bottom is level N, which is
///   how eccodes numbers one.
/// * **Reference time**: the ten digits `YYYYMMDDHH` both spellings carry.
/// * **Forecast window** (#1117): a wgrib2 `anl`, `N hour fcst` or
///   `a-b hour acc fcst` against the message's forecast time and statistical
///   process, in seconds, so `0-0 day` and `0-0 hour` are one window; an
///   ECMWF `step` against the end of the message's window, which is what MARS
///   states for an accumulation or an extreme (`mx2t3` at step 3 is hours 0 to
///   3). Only the minute, hour, day and second units are read, and a window
///   wgrib2 writes in another form (`0-1 hour acc@(fcst,dt=1 hour)`, a
///   climatology) is not compared, and neither is a wgrib2 window for
///   template 4.44, whose two-octet forecast time wgrib2 reads as four.
/// * **Qualifiers** of a wgrib2 line (#1117): `prob >0.254` and its kin
///   against the probability block of templates 4.5 and 4.9, `ENS=+1` and
///   `ENS=low-res ctl` against the ensemble block of 4.1 and 4.11, `ens mean`
///   and the other derived forecasts against 4.2 and 4.12, and `10% level`
///   against the percentile of 4.6 and 4.10. wgrib2 prints each of these for
///   every message on those templates, so a line from a real `.idx` that
///   carries none of them is refused for a message that has one. A derived
///   forecast and a percentile are words a newer wgrib2 can add to, so a line
///   carrying a qualifier this does not read is not refused for lacking
///   those two. A line built by hand (no [`Expectation::dialect`]) is not refused
///   for leaving one out. An
///   ECMWF `number` is compared with the ensemble member.
/// * **Constituent and ranges** of a chemistry or aerosol message (templates
///   4.40–4.48, #1130): `aerosol=Dust dry` or `chemical=Ozone` against the
///   constituent type, by its WMO Common Code Table C-14 name or a number, and
///   `aerosol_size <1e-05` and `aerosol_wavelength >=5.45e-07,<=5.65e-07`
///   against the size and wavelength ranges, to the precision wgrib2 prints.
///   A line carrying one is refused for a message that has none. A line
///   leaving one out is not refused, because which of them wgrib2 prints, and
///   in which words, has changed between its releases; nor is a constituent
///   name C-14 does not hold.
///
/// **Some names are not compared.** A MARS name outside the open-data table
/// is not, and neither is a wgrib2 line's name when the message's codes have
/// no short name in this build's tables, unless those codes are where the
/// tables are complete: a discipline the tables cover, a category and a
/// number below 192, the local-use range, and a triple the WMO master table
/// does not define at all. There, a name the tables give to other codes is
/// refused. Elsewhere, resolving the name to codes would refuse a correct
/// record, since the tables do not hold every code a producer names: MRMS
/// publishes `VIL` under its own discipline 209, NCEP publishes `REFD` as both
/// 0.16.4 and its local 0.16.195, and 167 WMO parameters have no short name
/// here that a newer wgrib2 may give one.
///
/// **A message on a product template the reader does not parse** (a
/// satellite product, 4.31, or a template newer than the reader; the
/// chemistry and aerosol templates were among these until #1130) is checked
/// on its reference time alone. Its parameter codes, short name, fixed
/// surfaces and forecast time are all unread, so none of them is compared, as
/// for a level the grammar did not understand. A chemistry or aerosol
/// template too short for its own layout is read the same way. A GRIB1
/// message's forecast window and qualifiers are not compared either.
pub fn verify_message(expect: &Expectation, info: &MessageInfo) -> Result<(), Mismatch> {
    let ecmwf = expect.dialect == Some(Dialect::EcmwfIndex);

    if let Some(promised) = expect.parameter
        && let Some(actual) = grib2_parameter(&info.fingerprint)
        && promised != actual
    {
        return Err(Mismatch::Field {
            field: "parameter",
            expected: codes(promised),
            actual: codes(actual),
        });
    }

    if let Some(promised) = expect.abbreviation.as_deref()
        && !name_holds(promised, ecmwf, info)
    {
        return Err(Mismatch::Field {
            field: "abbreviation",
            expected: promised.to_string(),
            actual: parameter_prose(info, ecmwf),
        });
    }

    if let Some(promised) = promised_level(expect, ecmwf)
        && (ecmwf || !wgrib2_misreads_4_44(&info.fingerprint))
        && !level_holds(&promised, ecmwf, &info.fingerprint)
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

    if let Some(promised) = expect.forecast.as_deref()
        && (ecmwf || !wgrib2_misreads_4_44(&info.fingerprint))
        && let Some(actual) = message_window(&info.fingerprint)
        && !window_holds(promised, ecmwf, &actual)
    {
        return Err(Mismatch::Field {
            field: "forecast",
            expected: promised.to_string(),
            actual: actual.to_string(),
        });
    }

    qualifiers_hold(expect, ecmwf, &info.fingerprint)
}

/// Whether this is a template 4.44 message, whose forecast time and level a
/// wgrib2 line can state wrongly, so neither is compared with a wgrib2 line.
///
/// * **Forecast time.** WMO writes 4.44's in two octets. wgrib2 knows that
///   (`forecast_time_in_units_location` sets the size to 2 for 4.44) but
///   `forecast_time_in_units` reads four octets for either size, taking two
///   octets of the first fixed surface with it.
/// * **Level.** wgrib2 reads 4.44's first fixed surface at octet 34, which is
///   right for WMO's 45-octet form but falls inside the forecast time of the
///   four-octet form eccodes once wrote (GRIB-29), which `fieldglass-grib2`
///   also reads. [`MessageFingerprint`] does not say which form a message is in,
///   so the level of every 4.44 is left uncompared on a wgrib2 line; an ECMWF
///   line's is still compared. 4.44 is deprecated in favor of 4.48.
fn wgrib2_misreads_4_44(fingerprint: &MessageFingerprint) -> bool {
    matches!(
        fingerprint,
        MessageFingerprint::Grib2 {
            product_template: 44,
            ..
        }
    )
}

/// Whether the message is the parameter `promised` names, in the line's
/// dialect. `true` wherever the name cannot be checked.
fn name_holds(promised: &str, ecmwf: bool, info: &MessageInfo) -> bool {
    if ecmwf {
        return mars_name_holds(promised, &info.fingerprint);
    }
    // A numeric fallback name is not an abbreviation the decoder can match;
    // the `parameter` check is the one that applies to it.
    if promised.starts_with("var ") {
        return true;
    }
    match info.abbreviation.as_deref() {
        Some(actual) => equivalent(promised, actual),
        // The tables do not name the message's codes. Where they are complete,
        // that says the codes have no short name, so a name the tables give to
        // other codes is another parameter (#1116).
        None => {
            !(codes_where_tables_are_complete(&info.fingerprint)
                && ncep_names().resolve(promised.trim(), "").is_some())
        }
    }
}

/// The NCEP name index, built once per process for [`name_holds`].
fn ncep_names() -> &'static TableResolver {
    static NAMES: OnceLock<TableResolver> = OnceLock::new();
    NAMES.get_or_init(TableResolver::ncep)
}

/// Whether the message's codes are where this build's tables are complete, so
/// that their naming nothing means no parameter is there: a discipline WMO
/// Code Table 0.0 assigns and the tables cover, a category and a number
/// outside the local-use range 192–254, and a triple the WMO master table
/// (v37) does not define at all.
///
/// A triple WMO defines but gives no short name is not complete: 167 of
/// them are (0.16.6–8, the layer-maximum reflectivities, among them), and a
/// newer wgrib2 can name one after a centre's local parameter it replaces,
/// which our index resolves to the local codes. A record for it would be
/// refused while right.
///
/// The §1 master tables version is not in [`MessageFingerprint`], so a message
/// written against a newer WMO version is read against v37 all the same. A
/// parameter added after v37 is then undefined here, and a newer wgrib2 name
/// for it that this build gives to other codes is refused.
fn codes_where_tables_are_complete(fingerprint: &MessageFingerprint) -> bool {
    matches!(
        fingerprint,
        MessageFingerprint::Grib2 {
            discipline,
            local_centre: None,
            parameter_category: Some(category),
            parameter_number: Some(number),
            ..
        } if *category < 192
            && *number < 192
            && disciplines().contains(discipline)
            // Below 192 no centre's table applies, so NCEP's originator
            // reads the WMO master table alone.
            && fieldglass_grib2::lookup_parameter(
                fieldglass_grib2::Originator::new(7, 0, 1),
                *discipline,
                *category,
                *number,
            )
            .is_none()
    )
}

/// Whether a GRIB2 message is one eccodes may name `promised`, the MARS name
/// an ECMWF record gives it (#1116, #1298). `true` for a name the table does
/// not carry, and for a message whose parameter is unread (GRIB1, or a
/// template the reader does not parse).
///
/// eccodes names a message after the concept block, among those whose every
/// condition holds, that states the most conditions (`concept_evaluate`). So
/// `2t`'s block, which states the codes and the 2 m surface, matches `mx2t3`'s
/// bytes but loses to `mx2t3`'s, which also states the process and the range.
/// The record passes when, in some pool of concept files, a block of
/// `promised` may match and no block of another name in that pool certainly
/// matches with more conditions: another listed name, or one of the rival
/// blocks the table keeps (eccodes 2.45's `max_2t`, which from tables version
/// 34 is what `mx2t3` was). A block certainly matches when every
/// condition it states is one the table keeps and the fingerprint carries, and
/// each holds. A condition either side cannot read leaves its block possible
/// but never certain, so it can widen what a name accepts and never refuses
/// one (see `tools/gen_mars_short_names.py`).
fn mars_name_holds(promised: &str, fingerprint: &MessageFingerprint) -> bool {
    let promised = promised.trim();
    let Some(own) = mars_short_names::blocks(promised) else {
        return true;
    };
    let Some(facts) = ConceptFacts::of(fingerprint) else {
        return true;
    };
    let rivals = || {
        mars_short_names::NAMES
            .iter()
            .filter(|(name, _)| *name != promised)
            .flat_map(|(_, blocks)| blocks.iter())
            .chain(mars_short_names::RIVALS)
    };
    facts.concept_may_give(mars_short_names::POOLS, own, rivals)
}

/// Whether a concept condition holds for a message, `Maybe` where it cannot be
/// read from the block or from the message. Ordered so that a block holds as
/// well as its weakest condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Holds {
    /// It does not.
    No,
    /// It cannot be told.
    Maybe,
    /// It does.
    Yes,
}

impl Holds {
    /// `Yes` when `have` is `want`: two values, or two `missing()`s, compared
    /// as eccodes compares a concept's integer keys.
    fn equal(want: Option<i64>, have: Option<i64>) -> Self {
        if want == have { Self::Yes } else { Self::No }
    }
}

/// What a concept block's conditions are compared with, read once from a
/// GRIB2 message's fingerprint.
struct ConceptFacts<'a> {
    /// Discipline, category, number.
    codes: [u8; 3],
    /// The product template.
    template: u16,
    /// The first fixed surface, `None` when unread.
    first: Option<SurfaceCodes>,
    /// The second fixed surface, `None` when unread.
    second: Option<SurfaceCodes>,
    /// The statistical processes, outermost first.
    statistics: &'a [StatisticalStep],
    /// The probability block of 4.5 and 4.9 (§4 octets 35-47), `None` for
    /// another template or a block the fingerprint does not carry.
    probability: Option<Vec<u8>>,
}

impl<'a> ConceptFacts<'a> {
    /// The facts of a GRIB2 message whose parameter the reader parsed.
    fn of(fingerprint: &'a MessageFingerprint) -> Option<Self> {
        let MessageFingerprint::Grib2 {
            discipline,
            product_template,
            product_octets,
            parameter_category: Some(category),
            parameter_number: Some(number),
            first_surface,
            second_surface,
            statistics,
            ..
        } = fingerprint
        else {
            return None;
        };
        Some(Self {
            codes: [*discipline, *category, *number],
            template: *product_template,
            first: *first_surface,
            second: *second_surface,
            statistics,
            probability: matches!(product_template, 5 | 9)
                .then(|| product_octets.as_deref().and_then(from_hex))
                .flatten(),
        })
    }

    /// Whether eccodes may give the message the value whose blocks are `own`
    /// in a concept whose other values' blocks are `rivals()`, ranked within
    /// each of `pools` pools: some block of `own` may match, and no rival in
    /// that pool certainly matches stating more conditions.
    fn concept_may_give<'b, I>(
        &self,
        pools: u32,
        own: &[mars_short_names::Block],
        rivals: impl Fn() -> I,
    ) -> bool
    where
        I: Iterator<Item = &'b mars_short_names::Block>,
    {
        (0..pools).any(|pool| {
            let in_pool = |block: &&mars_short_names::Block| block.pools & (1 << pool) != 0;
            let rival = rivals()
                .filter(in_pool)
                .filter(|block| self.block(block) == Holds::Yes)
                .map(|block| block.conditions)
                .max();
            own.iter()
                .filter(in_pool)
                .filter(|block| self.block(block) != Holds::No)
                .any(|block| rival.is_none_or(|rival| rival <= block.conditions))
        })
    }

    /// Whether every condition of `block` holds: its weakest condition, and
    /// at most `Maybe` when it states one the table does not keep.
    fn block(&self, block: &mars_short_names::Block) -> Holds {
        let unkept = if block.read.len() < usize::from(block.conditions) {
            Holds::Maybe
        } else {
            Holds::Yes
        };
        block
            .read
            .iter()
            .map(|&(key, want)| self.condition(block, key, want.map(i64::from)))
            .fold(unkept, Ord::min)
    }

    /// Whether the condition `key` = `want` (`None` for `missing()`) holds.
    fn condition(
        &self,
        block: &mars_short_names::Block,
        key: mars_short_names::Key,
        want: Option<i64>,
    ) -> Holds {
        use mars_short_names::Key;
        let is = |have: i64| Holds::equal(want, Some(have));
        let surface = |surface: Option<SurfaceCodes>, part: fn(SurfaceCodes) -> Option<i64>| {
            surface.map_or(Holds::Maybe, |s| Holds::equal(want, part(s)))
        };
        match key {
            Key::Discipline => is(self.codes[0].into()),
            Key::Category => is(self.codes[1].into()),
            Key::Number => is(self.codes[2].into()),
            Key::ProductTemplate => is(self.template.into()),
            Key::FirstSurfaceType => surface(self.first, |s| Some(s.surface_type.into())),
            Key::FirstScaleFactor => surface(self.first, |s| s.scale_factor.map(i64::from)),
            Key::FirstScaledValue => surface(self.first, |s| s.scaled_value),
            Key::SecondSurfaceType => surface(self.second, |s| Some(s.surface_type.into())),
            Key::SecondScaleFactor => surface(self.second, |s| s.scale_factor.map(i64::from)),
            Key::SecondScaledValue => surface(self.second, |s| s.scaled_value),
            Key::StatisticalProcess => self.statistic(|step| is(step.process.into())),
            Key::TimeRangeUnit => {
                self.statistic(|step| is(step.length_unit.into()).max(same_length(block, step)))
            }
            Key::TimeRangeLength => {
                self.statistic(|step| is(step.length.into()).max(same_length(block, step)))
            }
            Key::ProbabilityType => self.probability(want, |b| Some(Some(i64::from(*b.get(2)?)))),
            // Sign and magnitude, all ones coded missing, as eccodes reads a
            // `signed` key that `can_be_missing`.
            Key::LowerLimitScaleFactor => self.probability(want, |b| {
                let octet = *b.get(3)?;
                Some((octet != 0xff).then(|| sign_magnitude_to_i64(octet.into(), 8)))
            }),
            Key::LowerLimitScaledValue => self.probability(want, |b| {
                let raw = u32::from_be_bytes(b.get(4..8)?.try_into().ok()?);
                Some((raw != u32::MAX).then(|| sign_magnitude_to_i64(raw, 32)))
            }),
        }
    }

    /// A statistical condition. A template with no statistical block (4.0,
    /// 4.1, 4.2, 4.5, 4.6, 4.7) has none of its keys, so it does not hold, as
    /// it does not for eccodes. A message whose statistics the fingerprint does
    /// not carry as one range (4.15, or several nested ranges) is not
    /// compared.
    fn statistic(&self, holds: impl Fn(&StatisticalStep) -> Holds) -> Holds {
        match self.statistics {
            [] if matches!(self.template, 0 | 1 | 2 | 5 | 6 | 7) => Holds::No,
            [step] => holds(step),
            _ => Holds::Maybe,
        }
    }

    /// A probability condition against `want`, `value` reading the message's
    /// side from its 13-octet block (`Some(None)` for a value coded missing,
    /// `None` for a block too short to hold it). A template the reader parses
    /// without a probability block has none of its keys; one whose block is
    /// not carried is not compared.
    fn probability(
        &self,
        want: Option<i64>,
        value: impl Fn(&[u8]) -> Option<Option<i64>>,
    ) -> Holds {
        if !matches!(self.template, 5 | 9) {
            return if matches!(self.template, 0..=2 | 6..=8 | 10..=12 | 15 | 40..=48) {
                Holds::No
            } else {
                Holds::Maybe
            };
        }
        match self.probability.as_deref().and_then(value) {
            Some(have) => Holds::equal(want, have),
            None => Holds::Maybe,
        }
    }
}

/// `Maybe` where a block's time range differs from `step`'s only in how it is
/// written: the same length in other units, or a unit without a fixed length
/// on either side. eccodes compares the unit and the length as written, so
/// such a block would not match for it; it is left possible here, as before
/// #1298, rather than refused. `No` for a block without both keys.
fn same_length(block: &mars_short_names::Block, step: &StatisticalStep) -> Holds {
    use mars_short_names::Key;
    let stated = |key: Key| {
        block
            .read
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| *v)
    };
    let (Some(unit), Some(length)) = (stated(Key::TimeRangeUnit), stated(Key::TimeRangeLength))
    else {
        return Holds::No;
    };
    let want = u8::try_from(unit)
        .ok()
        .and_then(|unit| seconds(unit, i64::from(length)));
    match (want, seconds(step.length_unit, i64::from(step.length))) {
        (Some(want), Some(have)) if want != have => Holds::No,
        _ => Holds::Maybe,
    }
}

/// The message's parameter, for a mismatch message: its short name where the
/// tables give one, else its codes, and for an ECMWF line, whose name is
/// checked against codes, the codes and the statistical process as well.
fn parameter_prose(info: &MessageInfo, ecmwf: bool) -> String {
    let codes = grib2_parameter(&info.fingerprint).map(codes);
    let mut prose = info
        .abbreviation
        .clone()
        .or_else(|| codes.clone())
        .unwrap_or_default();
    if ecmwf && let Some(codes) = codes {
        prose.push_str(&format!(" ({codes}"));
        if let MessageFingerprint::Grib2 { statistics, .. } = &info.fingerprint
            && let Some(step) = statistics.first()
        {
            prose.push_str(&format!(", statistical process {}", step.process));
        }
        prose.push(')');
    }
    prose
}

/// A forecast time in a Code Table 4.4 unit as seconds, for the units that
/// have a fixed length. `None` for a month, a year and the longer units, and
/// for a code the table does not assign.
fn seconds(unit: u8, value: i64) -> Option<i64> {
    let per = match unit {
        0 => 60,
        1 => 3_600,
        2 => 86_400,
        10 => 3 * 3_600,
        11 => 6 * 3_600,
        12 => 12 * 3_600,
        13 => 1,
        _ => return None,
    };
    value.checked_mul(per)
}

/// When a message is valid, in seconds from its reference time.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Window {
    /// At one instant.
    Instant(i64),
    /// Over a range, with what was done over it (Code Table 4.10).
    Range {
        /// The start.
        start: i64,
        /// The end.
        end: i64,
        /// The statistical process.
        process: u8,
    },
}

impl std::fmt::Display for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hours = |s: i64| s as f64 / 3_600.0;
        match self {
            Self::Instant(t) => write!(f, "{} h", hours(*t)),
            Self::Range {
                start,
                end,
                process,
            } => write!(
                f,
                "{}-{} h, statistical process {process}",
                hours(*start),
                hours(*end)
            ),
        }
    }
}

/// The message's window. `None` for GRIB1, for a template the reader does not
/// parse, for a unit without a fixed length, and for nested time ranges.
fn message_window(fingerprint: &MessageFingerprint) -> Option<Window> {
    let MessageFingerprint::Grib2 {
        forecast_time_unit: Some(unit),
        forecast_time: Some(time),
        statistics,
        ..
    } = fingerprint
    else {
        return None;
    };
    let start = seconds(*unit, *time)?;
    match statistics.as_slice() {
        [] => Some(Window::Instant(start)),
        [step] => Some(Window::Range {
            start,
            end: start.checked_add(seconds(step.length_unit, i64::from(step.length))?)?,
            process: step.process,
        }),
        _ => None,
    }
}

/// Whether the line's forecast field names the message's window. `true` for a
/// field the grammar does not read.
fn window_holds(promised: &str, ecmwf: bool, actual: &Window) -> bool {
    if ecmwf {
        return ecmwf_step_holds(promised, actual);
    }
    match parse_wgrib2_window(promised) {
        None => true,
        Some(Wgrib2Window::Instant(t)) => *actual == Window::Instant(t),
        Some(Wgrib2Window::Range {
            start,
            end,
            process,
        }) => matches!(
            actual,
            Window::Range { start: s, end: e, process: p }
                if *s == start && *e == end && process.is_none_or(|want| want == *p)
        ),
    }
}

/// An ECMWF `step` against the message: the instant, or the end of a range,
/// in hours. A `start-end` step is a whole range. `true` for a step this does
/// not read.
fn ecmwf_step_holds(promised: &str, actual: &Window) -> bool {
    let hours = |s: &str| {
        s.trim()
            .parse::<i64>()
            .ok()
            .and_then(|h| h.checked_mul(3_600))
    };
    match promised.split_once('-') {
        None => match hours(promised) {
            None => true,
            Some(step) => match actual {
                Window::Instant(t) => *t == step,
                Window::Range { end, .. } => *end == step,
            },
        },
        Some((a, b)) => match (hours(a), hours(b)) {
            (Some(a), Some(b)) => match actual {
                Window::Instant(t) => a == b && *t == b,
                Window::Range { start, end, .. } => *start == a && *end == b,
            },
            _ => true,
        },
    }
}

/// A wgrib2 forecast field, as far as this reads it.
#[derive(Debug, PartialEq)]
enum Wgrib2Window {
    /// `anl`, or `N unit fcst`.
    Instant(i64),
    /// `a-b unit [process] fcst`, with the process when it is one of the
    /// names `Ftime2.c` prints.
    Range {
        start: i64,
        end: i64,
        process: Option<u8>,
    },
}

/// Parse a wgrib2 forecast field (`src/Ftime2.c`): `anl`, `N unit fcst`, or
/// `a-b unit process fcst`, where unit is `min`, `hour`, `day` or `sec`.
/// `None` for anything else, which is then not compared.
fn parse_wgrib2_window(text: &str) -> Option<Wgrib2Window> {
    let text = text.trim();
    if text == "anl" {
        return Some(Wgrib2Window::Instant(0));
    }
    let rest = text.strip_suffix(" fcst")?;
    let (number, rest) = rest.split_once(' ')?;
    let (unit, process) = match rest.split_once(' ') {
        Some((unit, process)) => (unit, Some(process)),
        None => (rest, None),
    };
    let unit = match unit {
        "min" => 0,
        "hour" => 1,
        "day" => 2,
        "sec" => 13,
        _ => return None,
    };
    let value = |s: &str| -> Option<i64> {
        // Digits only: wgrib2 prints these with `%d`, and a sign here would be
        // the separator of a range.
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        seconds(unit, s.parse().ok()?)
    };
    match number.split_once('-') {
        None if process.is_none() => Some(Wgrib2Window::Instant(value(number)?)),
        None => None,
        Some((a, b)) => Some(Wgrib2Window::Range {
            start: value(a)?,
            end: value(b)?,
            // A word this does not know leaves the process uncompared.
            process: process.and_then(process_code),
        }),
    }
}

/// The line's qualifiers against the message's §4 blocks: probability,
/// ensemble member, derived forecast and percentile on a wgrib2 line, the
/// member on an ECMWF one (#1117).
///
/// Nothing is compared for GRIB1, or for a template the reader does not parse.
/// A qualifier is read only in the words wgrib2 prints it in (`Prob.c`,
/// `Ensemble.c`, `Code_Values.c`), and one it does not read is not compared,
/// but a recognizable kind of qualifier (a `prob …`, an `ENS=…`) still says
/// the message has that block.
fn qualifiers_hold(
    expect: &Expectation,
    ecmwf: bool,
    fingerprint: &MessageFingerprint,
) -> Result<(), Mismatch> {
    let MessageFingerprint::Grib2 {
        product_template,
        product_octets,
        ensemble_type,
        ensemble_member,
        derived_forecast,
        parameter_category: Some(_),
        ..
    } = fingerprint
    else {
        return Ok(());
    };
    let refuse = |field: &'static str, expected: &str, actual: String| {
        Err(Mismatch::Field {
            field,
            expected: expected.to_string(),
            actual,
        })
    };
    let ensemble = ensemble_type.zip(*ensemble_member);

    if ecmwf {
        // MARS `number` is the perturbation number of a member forecast. A
        // message without an ensemble block can still be the control, number
        // 0, which a producer may write on template 4.0.
        for q in &expect.qualifiers {
            if let Some(n) = q.strip_prefix("number=")
                && let Ok(n) = n.trim().parse::<u8>()
                && ensemble.map_or(n != 0, |(_, member)| member != n)
            {
                return refuse("ensemble", q, ensemble_prose(ensemble));
            }
        }
        return Ok(());
    }

    // A real `.idx` line carries every qualifier wgrib2 printed, so one that
    // is missing says the message has no such block. A hand-built expectation
    // states only what its caller chose to.
    let whole_line = expect.dialect == Some(Dialect::Wgrib2Idx);
    let octets = product_octets.as_deref().and_then(from_hex);
    let qualifiers = || expect.qualifiers.iter().map(|q| q.trim());
    // A derived forecast and a percentile are written in a closed vocabulary
    // of words, which a newer wgrib2 can add to. A qualifier this does not
    // read may be one of them, so it leaves their absence unproven. A
    // probability and a member are always recognizable (`prob `, `ENS=`), so
    // their absence holds whatever else the line says.
    let every_qualifier_read = qualifiers().all(qualifier_is_read);

    // Probability: templates 4.5 and 4.9.
    let message_probability =
        matches!(product_template, 5 | 9).then(|| probability_of(octets.as_deref()));
    let stated = qualifiers().find(|q| q.starts_with("prob ") && !q.starts_with("prob fcst "));
    match (stated, &message_probability) {
        (Some(q), None) => return refuse("probability", q, "none".into()),
        (Some(q), Some(have)) => {
            if let (Some(want), Some(have)) = (parse_probability(q), have)
                && !want.same_as(have)
            {
                return refuse("probability", q, have.to_string());
            }
        }
        (None, Some(have)) if whole_line => {
            return refuse(
                "probability",
                "none",
                have.as_ref()
                    .map_or_else(|| "a probability".into(), ToString::to_string),
            );
        }
        _ => {}
    }

    // Ensemble member: templates 4.1 and 4.11.
    let stated = qualifiers().find(|q| q.contains("ENS="));
    match (stated, ensemble) {
        (Some(q), None) => return refuse("ensemble", q, "none".into()),
        (Some(q), Some((kind, member))) => {
            // ECMWF writes members with the type missing (`Ensemble.c`).
            if kind != 255
                && let Some((want_kind, want_member)) = parse_ensemble(q)
                && (want_kind != kind || want_member.is_some_and(|m| m != member))
            {
                return refuse("ensemble", q, ensemble_prose(ensemble));
            }
        }
        // wgrib2 prints the member of 4.1, 4.11, 4.41 and 4.43 but not of the
        // aerosol members 4.45 and 4.47 (`code_table_4_6_location`), so only
        // the first four lack one on a real line because they have none.
        (None, Some(_)) if whole_line && matches!(product_template, 1 | 11 | 41 | 43) => {
            return refuse("ensemble", "none", ensemble_prose(ensemble));
        }
        _ => {}
    }

    // Derived forecast: templates 4.2 and 4.12.
    let stated = qualifiers().find_map(|q| derived_code(q).map(|code| (q, code)));
    match (stated, derived_forecast) {
        (Some((q, _)), None) => return refuse("derivedForecast", q, "none".into()),
        (Some((q, Some(want))), Some(have)) if want != *have => {
            return refuse("derivedForecast", q, format!("Code Table 4.7 {have}"));
        }
        (None, Some(have)) if whole_line && every_qualifier_read => {
            return refuse("derivedForecast", "none", format!("Code Table 4.7 {have}"));
        }
        _ => {}
    }

    // Percentile: templates 4.6 and 4.10.
    let message_percentile = matches!(product_template, 6 | 10)
        .then(|| octets.as_deref().and_then(|o| o.first().copied()));
    let stated = qualifiers().find_map(|q| parse_percentile(q).map(|p| (q, p)));
    match (stated, message_percentile) {
        (Some((q, _)), None) => return refuse("percentile", q, "none".into()),
        (Some((q, want)), Some(Some(have))) if want != have => {
            return refuse("percentile", q, format!("{have}% level"));
        }
        (None, Some(have)) if whole_line && every_qualifier_read => {
            return refuse(
                "percentile",
                "none",
                have.map_or_else(|| "a percentile".into(), |p| format!("{p}% level")),
            );
        }
        _ => {}
    }

    // Chemistry and aerosol: templates 4.40-4.48 (#1130). wgrib2 names the
    // constituent `chemical=` (4.40-4.43) or `aerosol=` (4.44-4.48) and prints
    // the ranges as `aerosol_size` and `aerosol_wavelength` (`ExtName.c`,
    // `Aerosol.c`). Which of these a release prints, for which templates, and
    // in which words has changed over wgrib2's history (its `Changes`: sizes
    // for 4.44 alone until v0.2.0.6c, `chemical=` for a 4.48 for a while, Code
    // Table 4.233's own names until it was folded into C-14), so a line is
    // refused for one it carries that the message contradicts or lacks, never
    // for leaving one out. A name C-14 does not hold is not compared.
    let constituent = constituent_of(*product_template, octets.as_deref());
    let quals: Vec<&str> = qualifiers().collect();
    let stated = quals.iter().find_map(|q| {
        q.strip_prefix("aerosol=")
            .or_else(|| q.strip_prefix("chemical="))
            .map(|name| (*q, name))
    });
    match (stated, &constituent) {
        (Some((q, _)), None) => return refuse("constituent", q, "none".into()),
        (Some((q, name)), Some(have)) => {
            if let Some(want) = constituent_code(name)
                && want != have.code
            {
                return refuse("constituent", q, constituent_prose(have.code));
            }
        }
        _ => {}
    }
    for (name, have) in [
        ("aerosol_size", constituent.as_ref().and_then(|c| c.size)),
        (
            "aerosol_wavelength",
            constituent.as_ref().and_then(|c| c.wavelength),
        ),
    ] {
        let Some(text) = range_qualifier(&quals, name) else {
            continue;
        };
        let Some(have) = have else {
            return refuse(range_field(name), text, "none".into());
        };
        if parse_interval(text).same_as(&have) == Some(false) {
            return refuse(range_field(name), text, have.to_string());
        }
    }

    Ok(())
}

/// The mismatch field for a range qualifier.
fn range_field(name: &str) -> &'static str {
    if name == "aerosol_size" {
        "aerosolSize"
    } else {
        "aerosolWavelength"
    }
}

/// Whether a wgrib2 qualifier is one this reads: a probability, a member, a
/// derived forecast, a percentile, a constituent or a size or wavelength
/// range, or one of the fixed words `ExtName.c` prints for the generating
/// process.
fn qualifier_is_read(q: &str) -> bool {
    q.starts_with("prob ")
        || q.contains("ENS=")
        || derived_code(q).is_some()
        || parse_percentile(q).is_some()
        || q.starts_with("aerosol=")
        || q.starts_with("chemical=")
        || q.starts_with("aerosol_size ")
        || q.starts_with("aerosol_wavelength ")
        || matches!(
            q,
            "probability forecast"
                | "analysis/forecast error"
                | "climatological"
                | "Confidence Indicator"
                | "Neighborhood Probability"
        )
        || q.starts_with("process=")
}

/// Lowercase hex, as [`MessageFingerprint`] carries unread octets, to bytes.
fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// An ensemble block for a mismatch message.
fn ensemble_prose(ensemble: Option<(u8, u8)>) -> String {
    match ensemble {
        Some((kind, member)) => format!("Code Table 4.6 {kind}, member {member}"),
        None => "none".into(),
    }
}

/// What a probability field is the probability of, as wgrib2 prints it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Probability {
    /// `prob <x`.
    Below(f64),
    /// `prob >x`.
    Above(f64),
    /// `prob >=x <y`.
    Between(f64, f64),
    /// `prob =x`.
    Equal(f64),
    /// `prob above normal` (6), `near normal` (7), `below normal` (8).
    Normal(u8),
}

impl Probability {
    /// The same probability, its limits to the precision wgrib2 prints.
    fn same_as(&self, other: &Self) -> bool {
        let close =
            |a: f64, b: f64| a == b || (a - b).abs() <= PRINTED_PRECISION * a.abs().max(b.abs());
        // `Prob.c` prints a type 2 range as `prob =x` when its limits are
        // equal as the `f32`s `scaled2flt` returns, which two limits coded at
        // different scales can be while their `f64`s differ.
        let settled = |p: &Self| match *p {
            Self::Between(a, b) if a as f32 == b as f32 => Self::Equal(a),
            other => other,
        };
        match (&settled(self), &settled(other)) {
            (Self::Below(a), Self::Below(b))
            | (Self::Above(a), Self::Above(b))
            | (Self::Equal(a), Self::Equal(b)) => close(*a, *b),
            (Self::Between(a, b), Self::Between(c, d)) => close(*a, *c) && close(*b, *d),
            (Self::Normal(a), Self::Normal(b)) => a == b,
            _ => false,
        }
    }
}

impl std::fmt::Display for Probability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Below(x) => write!(f, "prob <{x}"),
            Self::Above(x) => write!(f, "prob >{x}"),
            Self::Between(x, y) => write!(f, "prob >={x} <{y}"),
            Self::Equal(x) => write!(f, "prob ={x}"),
            Self::Normal(6) => f.write_str("prob above normal"),
            Self::Normal(7) => f.write_str("prob near normal"),
            Self::Normal(_) => f.write_str("prob below normal"),
        }
    }
}

/// A `prob …` qualifier, in the forms `Prob.c` prints. `None` for another.
fn parse_probability(q: &str) -> Option<Probability> {
    let rest = q.strip_prefix("prob ")?.trim();
    let number = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    Some(match rest {
        "above normal" => Probability::Normal(6),
        "near normal" => Probability::Normal(7),
        "below normal" => Probability::Normal(8),
        _ => {
            if let Some(range) = rest.strip_prefix(">=") {
                let (low, high) = range.split_once(" <")?;
                Probability::Between(number(low)?, number(high)?)
            } else if let Some(x) = rest.strip_prefix('>') {
                Probability::Above(number(x)?)
            } else if let Some(x) = rest.strip_prefix('<') {
                Probability::Below(number(x)?)
            } else {
                Probability::Equal(number(rest.strip_prefix('=')?)?)
            }
        }
    })
}

/// The probability a template 4.5 or 4.9 message states, from its 13-octet
/// probability block (§4 octets 35–47: forecast number and count, Code Table
/// 4.9 type, then the lower and upper limits as scale factor and scaled
/// value), read as `Prob.c` reads it. `None` for a type or limit this does not
/// read.
fn probability_of(block: Option<&[u8]>) -> Option<Probability> {
    let block = block?;
    let (lower, upper) = (|| scaled_limit(block, 3), || scaled_limit(block, 8));
    Some(match *block.get(2)? {
        0 => Probability::Below(lower()?),
        1 => Probability::Above(upper()?),
        2 => {
            let (low, high) = (lower()?, upper()?);
            if low as f32 == high as f32 {
                Probability::Equal(low)
            } else {
                Probability::Between(low, high)
            }
        }
        3 => Probability::Above(lower()?),
        4 => Probability::Below(upper()?),
        5 => Probability::Equal(lower()?),
        kind @ 6..=8 => Probability::Normal(kind),
        _ => return None,
    })
}

/// The limit at `block[at..at + 5]`: a sign-magnitude scale factor and scaled
/// value, as a probability block and a size or wavelength range write one.
/// `None` when either is coded missing or `block` is too short.
fn scaled_limit(block: &[u8], at: usize) -> Option<f64> {
    let scale = *block.get(at)?;
    let value = block.get(at + 1..at + 5)?;
    let raw = u32::from_be_bytes(value.try_into().ok()?);
    if scale == 0xff || raw == u32::MAX {
        return None;
    }
    let magnitude = f64::from(raw & 0x7fff_ffff);
    let value = if raw & 0x8000_0000 != 0 {
        -magnitude
    } else {
        magnitude
    };
    let scale = i32::from(scale & 0x7f) * if scale & 0x80 != 0 { -1 } else { 1 };
    Some(value / 10f64.powi(scale))
}

/// What a chemistry or aerosol message (templates 4.40-4.48) is a quantity
/// of, read from its [`MessageFingerprint`] `product_octets`: the constituent
/// type, then for 4.44-4.48 the size range, then for 4.48 the wavelength
/// range, as `fieldglass-grib2` keeps them (#1130).
struct Constituent {
    /// WMO Common Code Table C-14 (GRIB2 Code Tables 4.230 and 4.233).
    code: u16,
    /// The size range as wgrib2 prints it, `None` for a template without one
    /// or a range coded missing (Code Table 4.91 type 255), which wgrib2 does
    /// not print.
    size: Option<Interval>,
    /// The wavelength range, likewise.
    wavelength: Option<Interval>,
}

/// The [`Constituent`] of a message on `template`, `None` for any template but
/// 4.40-4.48, or octets too short for it.
fn constituent_of(template: u16, octets: Option<&[u8]>) -> Option<Constituent> {
    if !(40..=48).contains(&template) {
        return None;
    }
    let octets = octets?;
    let code = u16::from_be_bytes([*octets.first()?, *octets.get(1)?]);
    let range = |at: usize| octets.get(at..at + 11).and_then(interval_of);
    Some(Constituent {
        code,
        size: (template >= 44).then(|| range(2)).flatten(),
        wavelength: (template == 48).then(|| range(13)).flatten(),
    })
}

/// A size or wavelength range as wgrib2 prints it (`prt_code_table_4_91` in
/// `CodeTable.c`), which writes the limit a Code Table 4.91 type uses and not
/// the type: `<x` is "below the first limit" (type 0) and "below the second"
/// (4) alike.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Interval {
    /// `<x`, types 0 and 4.
    Below(f64),
    /// `>x`, types 1 and 3.
    Above(f64),
    /// `<=x`, types 5 and 9.
    AtMost(f64),
    /// `>=x`, types 6 and 8.
    AtLeast(f64),
    /// `>=x,<y`, type 2.
    HalfOpen(f64, f64),
    /// `>=x,<=y`, type 7.
    Closed(f64, f64),
    /// `>x,<=y`, type 10.
    OpenClosed(f64, f64),
    /// `=x`, type 11.
    Equal(f64),
    /// A type or a limit this does not read: never compared.
    Unread,
}

impl Interval {
    /// The same range, its limits to the precision wgrib2 prints (`%lg`).
    /// `None` when either side is [`Interval::Unread`].
    fn same_as(&self, other: &Self) -> Option<bool> {
        let close =
            |a: f64, b: f64| a == b || (a - b).abs() <= PRINTED_PRECISION * a.abs().max(b.abs());
        Some(match (self, other) {
            (Self::Unread, _) | (_, Self::Unread) => return None,
            (Self::Below(a), Self::Below(b))
            | (Self::Above(a), Self::Above(b))
            | (Self::AtMost(a), Self::AtMost(b))
            | (Self::AtLeast(a), Self::AtLeast(b))
            | (Self::Equal(a), Self::Equal(b)) => close(*a, *b),
            (Self::HalfOpen(a, b), Self::HalfOpen(c, d))
            | (Self::Closed(a, b), Self::Closed(c, d))
            | (Self::OpenClosed(a, b), Self::OpenClosed(c, d)) => close(*a, *c) && close(*b, *d),
            _ => false,
        })
    }
}

impl std::fmt::Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = printed_g;
        match self {
            Self::Below(x) => write!(f, "<{}", g(*x)),
            Self::Above(x) => write!(f, ">{}", g(*x)),
            Self::AtMost(x) => write!(f, "<={}", g(*x)),
            Self::AtLeast(x) => write!(f, ">={}", g(*x)),
            Self::HalfOpen(x, y) => write!(f, ">={},<{}", g(*x), g(*y)),
            Self::Closed(x, y) => write!(f, ">={},<={}", g(*x), g(*y)),
            Self::OpenClosed(x, y) => write!(f, ">{},<={}", g(*x), g(*y)),
            Self::Equal(x) => write!(f, "={}", g(*x)),
            Self::Unread => f.write_str("a range this does not read"),
        }
    }
}

/// `x` as C's `%lg` prints it, which is how wgrib2 writes a range: six
/// significant figures, trailing zeros dropped, and an exponent (at least two
/// digits) below 1e-4 or from 1e6, so a refusal reads `<1e-05` beside a line
/// that says `<1e-05`.
fn printed_g(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    // Rounded to six significant figures first, so 999999.5 takes the
    // exponent its rounding gives it, as `%g` decides.
    let sci = format!("{x:.5e}");
    let (mantissa, exponent) = sci.split_once('e').expect("`{:e}` writes an exponent");
    let exponent: i32 = exponent.parse().expect("`{:e}` writes an integer exponent");
    let trim = |s: &str| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exponent.unsigned_abs())
    } else {
        let decimals = usize::try_from(5 - exponent).unwrap_or(0);
        trim(&format!("{x:.decimals$}"))
    }
}

/// The 11-octet range at the start of `block` (Code Table 4.91 type, then the
/// first and second limits) as wgrib2 prints it. `None` for type 255, which
/// wgrib2 does not print at all.
fn interval_of(block: &[u8]) -> Option<Interval> {
    let kind = *block.first()?;
    if kind == 255 {
        return None;
    }
    let (first, second) = (scaled_limit(block, 1), scaled_limit(block, 6));
    let read = |interval: Option<Interval>| interval.unwrap_or(Interval::Unread);
    Some(read(match kind {
        0 => first.map(Interval::Below),
        1 => second.map(Interval::Above),
        2 => first.zip(second).map(|(a, b)| Interval::HalfOpen(a, b)),
        3 => first.map(Interval::Above),
        4 => second.map(Interval::Below),
        5 => first.map(Interval::AtMost),
        6 => second.map(Interval::AtLeast),
        7 => first.zip(second).map(|(a, b)| Interval::Closed(a, b)),
        8 => first.map(Interval::AtLeast),
        9 => second.map(Interval::AtMost),
        10 => first.zip(second).map(|(a, b)| Interval::OpenClosed(a, b)),
        11 => first.map(Interval::Equal),
        _ => None,
    }))
}

/// A range in the words `prt_code_table_4_91` prints, the forms its inverse
/// `scan_code_table_4_91` reads. [`Interval::Unread`] for another form
/// (`??? code_table_4.91=N`).
fn parse_interval(text: &str) -> Interval {
    let number = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    let pair = |rest: &str, upper: &str, make: fn(f64, f64) -> Interval| {
        let (a, b) = rest.split_once(upper)?;
        Some(make(number(a)?, number(b)?))
    };
    let text = text.trim();
    let parsed = if let Some(rest) = text.strip_prefix(">=") {
        if rest.contains(",<=") {
            pair(rest, ",<=", Interval::Closed)
        } else if rest.contains(",<") {
            pair(rest, ",<", Interval::HalfOpen)
        } else {
            number(rest).map(Interval::AtLeast)
        }
    } else if let Some(rest) = text.strip_prefix("<=") {
        number(rest).map(Interval::AtMost)
    } else if let Some(rest) = text.strip_prefix('>') {
        if rest.contains(",<=") {
            pair(rest, ",<=", Interval::OpenClosed)
        } else {
            number(rest).map(Interval::Above)
        }
    } else if let Some(rest) = text.strip_prefix('<') {
        number(rest).map(Interval::Below)
    } else if let Some(rest) = text.strip_prefix('=') {
        number(rest).map(Interval::Equal)
    } else {
        None
    };
    parsed.unwrap_or(Interval::Unread)
}

/// The text after `name ` in the qualifier that carries it, up to where
/// another range starts: wgrib2's `ExtName.c` writes a 4.48 message's size and
/// wavelength with no `:` between them
/// (`aerosol_size <2e-05aerosol_wavelength >=…`), where the inventory that
/// wrote GEFS-Aerosols' sidecar separates them.
fn range_qualifier<'a>(qualifiers: &[&'a str], name: &str) -> Option<&'a str> {
    let prefix = format!("{name} ");
    qualifiers.iter().find_map(|q| {
        let start = q.find(&prefix)? + prefix.len();
        let rest = &q[start..];
        let end = ["aerosol_size ", "aerosol_wavelength "]
            .iter()
            .filter_map(|other| rest.find(other))
            .min()
            .unwrap_or(rest.len());
        Some(&rest[..end])
    })
}

/// The constituent a `chemical=` or `aerosol=` qualifier names: a code, or a
/// C-14 name. `None` for a name C-14 does not hold, which is not compared.
fn constituent_code(name: &str) -> Option<u16> {
    let name = name.trim();
    name.parse()
        .ok()
        .or_else(|| fieldglass_grib2::tables_constituent::constituent_type_code(name))
}

/// A constituent for a mismatch message.
fn constituent_prose(code: u16) -> String {
    match fieldglass_grib2::tables_constituent::lookup_constituent_type(code) {
        Some(name) => format!("{name} ({code})"),
        None => format!("Code Table 4.230 {code}"),
    }
}

/// An `ENS=…` qualifier as `(Code Table 4.6 type, member)`, the member `None`
/// for a control. `None` for a form `Ensemble.c` prints otherwise.
fn parse_ensemble(q: &str) -> Option<(u8, Option<u8>)> {
    let rest = q.strip_prefix("ENS=")?;
    if rest.starts_with("hi-res ctl") {
        return Some((0, None));
    }
    if rest.starts_with("low-res ctl") {
        return Some((1, None));
    }
    let kind = match rest.as_bytes().first()? {
        b'+' => 3,
        b'-' => 2,
        _ => return None,
    };
    let digits = rest[1..].split(' ').next()?;
    Some((kind, Some(digits.parse().ok()?)))
}

/// A derived-forecast qualifier as its Code Table 4.7 code, in the words
/// `Ensemble.c` prints: `Some(None)` for `unknown derived fcst`, which names
/// no code, and `None` for a qualifier that is not one of these.
fn derived_code(q: &str) -> Option<Option<u8>> {
    Some(Some(match q {
        "ens mean" => 0,
        "wt ens mean" => 1,
        "ens std dev" | "cluster std dev" => 2,
        "normalized ens std dev" | "normalized cluster std dev" => 3,
        "ens spread" => 4,
        "ens large anom index" => 5,
        "unwt ens mean" | "unwt cluster mean" => 6,
        "25%-75% range" => 7,
        "min all members" => 8,
        "max all members" => 9,
        "unwt mode all members" => 192,
        "10% all members" => 193,
        "50% all members" => 194,
        "90% all members" => 195,
        "stat. weight for each members" => 196,
        "percentile from climate distribution" => 197,
        "deviation of ens mean from daily climo" => 198,
        "extreme forecast index" => 199,
        "equally weighted mean" => 200,
        "5% all members" => 201,
        "25% all members" => 202,
        "75% all members" => 203,
        "95% all members" => 204,
        "unknown derived fcst" => return Some(None),
        _ => return None,
    }))
}

/// A `N% level` qualifier, as `ExtName.c` prints a percentile.
fn parse_percentile(q: &str) -> Option<u8> {
    let digits = q.strip_suffix("% level")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A Code Table 4.10 process by the name wgrib2 prints for it
/// (`src/CodeTable_4.10.dat`).
fn process_code(name: &str) -> Option<u8> {
    Some(match name {
        "ave" => 0,
        "acc" => 1,
        "max" => 2,
        "min" => 3,
        "last-first" => 4,
        "RMS" => 5,
        "StdDev" => 6,
        "covar" => 7,
        "first-last" => 8,
        "ratio" => 9,
        "standardized anomaly" => 10,
        "summation" => 11,
        "return period" => 12,
        "median" => 13,
        "severity" => 100,
        "mode" => 101,
        "index processing" => 102,
        _ => return None,
    })
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
fn grib2_parameter(fingerprint: &MessageFingerprint) -> Option<ParameterId> {
    match fingerprint {
        MessageFingerprint::Grib2 {
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
/// Both parsers fill [`Expectation::level_spec`]; an expectation that came over
/// the wire with only the text is parsed here, by its dialect's grammar, so
/// the text is never compared as a string.
fn promised_level(expect: &Expectation, ecmwf: bool) -> Option<LevelSpec> {
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
fn level_holds(promised: &LevelSpec, ecmwf: bool, fingerprint: &MessageFingerprint) -> bool {
    match &promised.surface {
        // Kept verbatim because the grammar did not understand it, so there is
        // no surface to compare. Comparing the text with the message's would
        // refuse every such record, which is the defect this replaces.
        Surface::Named(_) => true,
        // The reader did not parse the message's product template (the
        // chemistry and aerosol templates 4.40-4.48, say), so its surfaces
        // are unknown and there is nothing to compare with either (#1126).
        _ if surfaces_unknown(fingerprint) => true,
        Surface::Surface if ecmwf => {
            !on_an_ecmwf_level_list(fingerprint) || eccodes_levtype_is_sfc(fingerprint)
        }
        _ => message_level(fingerprint).is_some_and(|have| same_level(promised, &have)),
    }
}

/// Whether the message is GRIB2 on a product template the reader does not
/// parse, which leaves its fixed surfaces unread.
///
/// The fingerprint states that as the parameter and the first surface both
/// absent: they are read from the one common block every parsed template
/// has, so a parsed template states both and an unparsed one neither. An
/// fingerprint missing only one of them is not that message, and its level is
/// still compared (and refused, having no surface to match).
fn surfaces_unknown(fingerprint: &MessageFingerprint) -> bool {
    matches!(
        fingerprint,
        MessageFingerprint::Grib2 {
            parameter_category: None,
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
/// not, bar the messages [`eccodes_levtype_is_sfc`] finds. The GRIB2 types
/// are `LEVEL_LISTS` in `tools/gen_mars_short_names.py`, which keeps the
/// `levtype` blocks on them; change both together.
fn on_an_ecmwf_level_list(fingerprint: &MessageFingerprint) -> bool {
    match fingerprint {
        MessageFingerprint::Grib2 {
            first_surface: Some(first),
            ..
        } => matches!(first.surface_type, 100 | 105 | 107 | 109 | 151),
        // Table 3: isobaric, hybrid, isentropic, potential vorticity. Not the
        // depth layer 112: ECMWF's GRIB1 soil fields (`swvl1`, `stl1`) are
        // `sfc` on it.
        MessageFingerprint::Grib1 { level_type, .. } => {
            matches!(level_type, 100 | 109 | 113 | 117)
        }
        _ => false,
    }
}

/// Whether eccodes' `marsLevtype` concept may call a message on a surface
/// ECMWF lists levels of `sfc` (#1323). Medium cloud cover is the layer from
/// 800 hPa to 450 hPa and high cloud cover from 450 hPa to the top of the
/// atmosphere, both isobaric, and an AIFS `.index` gives both `sfc`, as
/// eccodes does: the concept's `sfc` block for each layer states more
/// conditions than `pl`'s. eccodes 2.45 adds three isentropic levels
/// (type 107, θ = 273.15, 274.15 and 274.65 K) to `sfc`. Their parameters
/// (`h0thg`, `h1thg`, `h1p5thg`) are heights of the 0, 1 and 1.5 °C
/// isotherms, so type 107 there is likely an eccodes slip for the isothermal
/// type 20, but this mirrors eccodes, which is what writes a record's
/// `levtype`. The table is a union across eccodes versions, so an `sfc` line
/// on θ = 273.15 K passes although 2.34.1 reads that message as `pt`. The
/// concept is parameter-blind; the name check is what holds the parameter to
/// its layer.
fn eccodes_levtype_is_sfc(fingerprint: &MessageFingerprint) -> bool {
    ConceptFacts::of(fingerprint).is_some_and(|facts| {
        facts.concept_may_give(
            mars_short_names::LEVTYPE_POOLS,
            mars_short_names::SFC_ON_LEVEL_LISTS,
            || mars_short_names::LEVTYPES_ON_LEVEL_LISTS.iter(),
        )
    })
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
        4 => (Surface::IsothermZero, Value::Fixed(0.0)),
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
        151 => (Surface::SoilLevel, Value::Shift(0)),
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
fn message_level(fingerprint: &MessageFingerprint) -> Option<LevelSpec> {
    match fingerprint {
        MessageFingerprint::Grib2 {
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
            // A layer between two soil levels is numbered by its bottom, which
            // is the `levelist` an ECMWF `sol` record states (eccodes aliases
            // `level` to `bottomLevel` for it; ECC-854).
            if first.surface_type == 151
                && let Some(second) = second_surface
                && second.surface_type == 151
            {
                let mut level = LevelSpec::named(surface);
                level.value = read(second);
                return Some(level);
            }
            let value2 = match second_surface {
                None => None,
                Some(second) if second.surface_type == 255 => None,
                // A layer between two of the same surface, `30-0 mb above
                // ground`. Two different surfaces are a layer wgrib2 writes
                // as two levels joined by ` - `, which the grammar keeps
                // named, so a modeled level never names one.
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
        MessageFingerprint::Grib1 {
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
        4 => LevelSpec::at(Surface::IsothermZero, 0.0),
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
    /// standardized it at (0, 1, 33). Both are "categorical rain", so either
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

    /// ECMWF MARS names are not in the resolver's tables, and the resolver says
    /// so rather than inventing a triple. Written down because it is a limitation
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
            ensemble_member: None,
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
            fingerprint: crate::api::MessageFingerprint::Grib2 {
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

    fn level(text: &str) -> Expectation {
        Expectation::new().with_level(text)
    }

    fn holds(text: &str, first: Codes, second: Option<Codes>) -> bool {
        verify_message(&level(text), &info_on("TMP", first, second)).is_ok()
    }

    /// The check that catches a stale sidecar pointing at a valid but wrong
    /// message: the bytes decode fine, and they are not the promised field.
    #[test]
    fn a_wrong_field_is_reported_with_both_sides() {
        let expect = Expectation::new()
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
    fn a_modeled_level_is_refused_on_an_unmodeled_surface() {
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

    /// A message whose product template the reader does not parse (here 4.31,
    /// a satellite product; before #1130 also the chemistry and aerosol
    /// templates) has no known surfaces, so a level, in either dialect, is not
    /// compared with it (#1126). A message whose surfaces are known is still
    /// refused on a different one.
    #[test]
    fn a_level_is_not_compared_with_an_unparsed_template() {
        let mut unparsed = info("AOTK", (10, 0, 0));
        unparsed.abbreviation = None;
        unparsed.level = None;
        unparsed.level_type = None;
        if let crate::api::MessageFingerprint::Grib2 {
            product_template,
            parameter_category,
            parameter_number,
            first_surface,
            second_surface,
            ..
        } = &mut unparsed.fingerprint
        {
            *product_template = 31;
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

        // A fingerprint with parameter codes but no surface is not an unparsed
        // template, so its level is still compared, and has nothing to match.
        let mut no_surface = unparsed.clone();
        if let crate::api::MessageFingerprint::Grib2 {
            parameter_category, ..
        } = &mut no_surface.fingerprint
        {
            *parameter_category = Some(0);
        }
        assert!(verify_message(&level("surface"), &no_surface).is_err());

        assert!(!holds("surface", (10, 0, 0), None));
        assert!(!holds("2 mb", (1, 0, 0), None));
    }

    /// An expectation that promises nothing checks nothing, rather than
    /// failing because the fields are absent.
    #[test]
    fn an_empty_expectation_passes() {
        assert_eq!(
            verify_message(&Expectation::default(), &info("TMP", (1, 0, 0))),
            Ok(())
        );
    }

    /// A numeric fallback name is not an abbreviation and is not compared as
    /// one; the codes it states are compared with the message's instead.
    #[test]
    fn a_numeric_fallback_name_is_checked_by_its_codes() {
        let expect = Expectation::new()
            .with_abbreviation("var discipline=0 center=7 local_table=1 parmcat=16 parm=201")
            .with_parameter(ParameterId {
                discipline: 0,
                category: 16,
                number: 201,
            });
        let mut refc = info("REFC", (10, 0, 0));
        let crate::api::MessageFingerprint::Grib2 {
            parameter_category,
            parameter_number,
            ..
        } = &mut refc.fingerprint
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

    /// A message on `discipline.category.number` that this build's tables do
    /// not name.
    fn unnamed(discipline: u8, category: u8, number: u8, local: Option<u16>) -> MessageInfo {
        let mut unnamed = info("TMP", TWO_METRES);
        unnamed.abbreviation = None;
        if let crate::api::MessageFingerprint::Grib2 {
            discipline: d,
            parameter_category,
            parameter_number,
            local_centre,
            ..
        } = &mut unnamed.fingerprint
        {
            *d = discipline;
            *parameter_category = Some(category);
            *parameter_number = Some(number);
            *local_centre = local;
        }
        unnamed
    }

    /// A message whose codes this build's tables do not name is not compared
    /// by name where the tables are incomplete (#1116): MRMS publishes `VIL`
    /// under its own discipline 209, and a centre's local codes hold names the
    /// tables may not. Resolving the name to codes there would refuse a
    /// correct record.
    #[test]
    fn an_unnamed_message_outside_the_complete_tables_is_not_compared_by_name() {
        let vil = Expectation::new().with_abbreviation("VIL");
        assert_eq!(verify_message(&vil, &unnamed(209, 3, 0, None)), Ok(()));
        // NCEP local space, which `REFD` at 0.16.195 is in.
        assert_eq!(
            verify_message(
                &Expectation::new().with_abbreviation("REFD"),
                &unnamed(0, 16, 250, Some(7))
            ),
            Ok(())
        );
        // A name the tables do not know at all says nothing either way.
        assert_eq!(
            verify_message(
                &Expectation::new().with_abbreviation("NOTAPARAMETER"),
                &unnamed(0, 0, 150, None)
            ),
            Ok(())
        );
    }

    /// Where the tables are complete (a WMO discipline they cover, codes below
    /// 192), a message they do not name has no short name, so a name they give
    /// to other codes is another parameter (#1116).
    #[test]
    fn an_unnamed_message_inside_the_complete_tables_is_refused_another_name() {
        assert_eq!(
            verify_message(
                &Expectation::new().with_abbreviation("RH"),
                &unnamed(0, 0, 150, None)
            ),
            Err(Mismatch::Field {
                field: "abbreviation",
                expected: "RH".into(),
                actual: "0.0.150".into()
            })
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
    fn ecmwf(record: &str) -> Expectation {
        let idx = EcmwfIndex::parse("o", record).expect("one record");
        idx.items()[0].expectation.clone()
    }

    /// MARS vocabulary: `2t` is compared with eccodes' definition of it (the
    /// codes 0.0.0 every message here has, at 2 m), not with the WMO short
    /// name `TMP`, and `sfc` is any single-level surface but not one ECMWF
    /// lists levels of.
    #[test]
    fn an_ecmwf_line_is_read_in_its_own_vocabulary() {
        let sfc = ecmwf(r#"{"param": "2t", "levtype": "sfc", "_offset": 0, "_length": 1}"#);
        assert_eq!(verify_message(&sfc, &info("TMP", TWO_METRES)), Ok(()));
        // 0.0.0 at mean sea level is not `2t`, whose block states 2 m (#1298).
        assert_eq!(
            refused(&sfc, &info("TMP", (101, 0, 0))),
            Some("abbreviation")
        );
        // The level alone, from a name the table does not carry.
        let level_only = ecmwf(r#"{"param": "zz", "levtype": "sfc", "_offset": 0, "_length": 1}"#);
        assert_eq!(
            verify_message(&level_only, &info("MSL", (101, 0, 0))),
            Ok(())
        );
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

    /// eccodes' `marsLevtype` concept calls some isobaric and isentropic
    /// messages `sfc` (#1323): the medium cloud layer (800 to 450 hPa), the
    /// high cloud layer (450 hPa to the top of the atmosphere) and, from
    /// eccodes 2.45, the isentropic levels θ = 273.15, 274.15 and 274.65 K
    /// (type 107, as eccodes writes them). An ECMWF `sfc`
    /// line holds on exactly those, and still not on the isobaric or
    /// isentropic levels and layers next to them.
    #[test]
    fn an_ecmwf_sfc_line_holds_where_eccodes_calls_the_level_sfc() {
        // A name the table does not carry, so only the level is compared.
        let sfc = ecmwf(r#"{"param": "zz", "levtype": "sfc", "_offset": 0, "_length": 1}"#);
        let on = |first: Codes, second: Option<Codes>| {
            verify_message(&sfc, &info_on("ZZ", first, second)).is_ok()
        };
        // The medium cloud layer, and the high one up to surface 8.
        assert!(on((100, 0, 80_000), Some((100, 0, 45_000))));
        assert!(on((100, 0, 45_000), Some((8, 0, 0))));
        // The same pressures as single levels, the layers reversed, and
        // another isobaric layer are `pl`.
        assert!(!on((100, 0, 80_000), None));
        assert!(!on((100, 0, 45_000), None));
        assert!(!on((100, 0, 45_000), Some((100, 0, 80_000))));
        assert!(!on((100, 0, 80_000), Some((100, 0, 50_000))));
        // 80000 Pa written at another scale is another pair of conditions.
        assert!(!on((100, 1, 800_000), Some((100, 0, 45_000))));
        // The isentropic levels eccodes lists: θ = 273.15 K is `sfc`, 300 K
        // is `pt`.
        assert!(on((107, 2, 27_315), None));
        assert!(on((107, 2, 27_465), None));
        assert!(!on((107, 2, 30_000), None));
        assert!(!on((107, 0, 273), None));
    }

    /// The generator keeps the `levtype` blocks on its `LEVEL_LISTS`
    /// surfaces, and [`on_an_ecmwf_level_list`] decides when they are
    /// consulted. A surface the generator has and this side lacks would make
    /// its blocks dead, so every block's first surface must be one this side
    /// lists.
    #[test]
    fn every_levtype_block_is_on_a_surface_the_level_check_lists() {
        use mars_short_names::{Key, LEVTYPES_ON_LEVEL_LISTS, SFC_ON_LEVEL_LISTS};
        assert!(!SFC_ON_LEVEL_LISTS.is_empty());
        for block in SFC_ON_LEVEL_LISTS.iter().chain(LEVTYPES_ON_LEVEL_LISTS) {
            let first = block
                .read
                .iter()
                .find(|(key, _)| *key == Key::FirstSurfaceType)
                .and_then(|&(_, value)| value)
                .expect("every block states its first surface");
            let surface_type = u8::try_from(first).expect("a Code Table 4.5 type");
            let fingerprint = info_on("ZZ", (surface_type, 0, 0), None).fingerprint;
            assert!(on_an_ecmwf_level_list(&fingerprint), "{block:?}");
        }
    }

    /// An expectation that crossed the wire with its level text and no parsed
    /// level is parsed here rather than compared as a string.
    #[test]
    fn a_level_sent_as_text_alone_is_parsed() {
        let expect: Expectation = serde_json::from_value(serde_json::json!({
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
        let expect = Expectation::new().with_reference_time("2026090400");
        assert_eq!(verify_message(&expect, &info("TMP", (1, 0, 0))), Ok(()));

        let stale = Expectation::new().with_reference_time("2026090312");
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

    /// Temperature at 2 m on product template `template`, with its unread
    /// octets as hex, and the forecast time `time` in Code Table 4.4 `unit`.
    fn on_template(template: u16, octets: Option<&str>, unit: u8, time: i64) -> MessageInfo {
        let mut message = info("TMP", TWO_METRES);
        let crate::api::MessageFingerprint::Grib2 {
            product_template,
            product_octets,
            forecast_time_unit,
            forecast_time,
            ..
        } = &mut message.fingerprint
        else {
            unreachable!()
        };
        *product_template = template;
        *product_octets = octets.map(str::to_string);
        *forecast_time_unit = Some(unit);
        *forecast_time = Some(time);
        message
    }

    /// The same, statistically processed: `process` over `length` of `unit`.
    fn over(mut message: MessageInfo, process: u8, unit: u8, length: u32) -> MessageInfo {
        if let crate::api::MessageFingerprint::Grib2 { statistics, .. } = &mut message.fingerprint {
            *statistics = vec![StatisticalStep {
                process,
                length_unit: unit,
                length,
            }];
        }
        message
    }

    /// A member forecast: Code Table 4.6 `kind`, member `member`.
    fn member(mut message: MessageInfo, kind: u8, number: u8) -> MessageInfo {
        if let crate::api::MessageFingerprint::Grib2 {
            ensemble_type,
            ensemble_member,
            ..
        } = &mut message.fingerprint
        {
            *ensemble_type = Some(kind);
            *ensemble_member = Some(number);
        }
        message.ensemble_member = Some(number);
        message
    }

    /// A derived forecast of Code Table 4.7 `code`.
    fn derived(mut message: MessageInfo, code: u8) -> MessageInfo {
        if let crate::api::MessageFingerprint::Grib2 {
            derived_forecast, ..
        } = &mut message.fingerprint
        {
            *derived_forecast = Some(code);
        }
        message
    }

    /// A wgrib2 line's expectation as the parser builds it from a real `.idx`:
    /// forecast `forecast`, then `qualifiers`.
    fn line(forecast: &str, qualifiers: &[&str]) -> Expectation {
        let mut text = format!("1:0:d=2026090400:TMP:2 m above ground:{forecast}");
        for q in qualifiers {
            text.push(':');
            text.push_str(q);
        }
        Wgrib2Idx::parse("o", &text).expect("one record").items()[0]
            .expectation
            .clone()
    }

    /// The field a refusal names, or `None` for a pass.
    fn refused(expect: &Expectation, message: &MessageInfo) -> Option<&'static str> {
        match verify_message(expect, message) {
            Ok(()) => None,
            Err(Mismatch::Field { field, .. }) => Some(field),
            Err(other) => panic!("not a field mismatch: {other:?}"),
        }
    }

    /// The forecast field wgrib2 writes (`Ftime2.c`), as far as it is read,
    /// in seconds; every other wording is left unread (#1117).
    #[test]
    fn the_wgrib2_forecast_grammar() {
        use Wgrib2Window::{Instant, Range};
        let range = |start, end, process| Range {
            start,
            end,
            process,
        };
        assert_eq!(parse_wgrib2_window("anl"), Some(Instant(0)));
        assert_eq!(parse_wgrib2_window("6 hour fcst"), Some(Instant(21_600)));
        assert_eq!(parse_wgrib2_window("90 min fcst"), Some(Instant(5_400)));
        assert_eq!(
            parse_wgrib2_window("0-1 hour acc fcst"),
            Some(range(0, 3_600, Some(1)))
        );
        assert_eq!(
            parse_wgrib2_window("0-0 day acc fcst"),
            Some(range(0, 0, Some(1)))
        );
        assert_eq!(
            parse_wgrib2_window("174-180 hour standardized anomaly fcst"),
            Some(range(174 * 3_600, 180 * 3_600, Some(10)))
        );
        // A process wgrib2 names `missing`, and NCEP's TMAX with none, are
        // windows whose process is not compared.
        assert_eq!(
            parse_wgrib2_window("1-2 hour missing fcst"),
            Some(range(3_600, 7_200, None))
        );
        assert_eq!(
            parse_wgrib2_window("0-6 hour fcst"),
            Some(range(0, 21_600, None))
        );
        for unread in [
            "0-1 hour acc@(fcst,dt=1 hour),missing=0",
            "240@3 hour ave(anl),missing=0",
            "3 month fcst",
            "-3 hour fcst",
            "6 hour",
            "fcst",
            "",
            "1 hour fcst 2",
        ] {
            assert_eq!(parse_wgrib2_window(unread), None, "{unread:?}");
        }
    }

    /// A wgrib2 window against the message's, in seconds, whatever unit each
    /// side states it in.
    #[test]
    fn a_wgrib2_window_is_compared_with_the_messages() {
        let at = |unit, time| on_template(0, None, unit, time);
        let acc = |time, length| over(on_template(8, None, 1, time), 1, 1, length);

        assert_eq!(refused(&line("anl", &[]), &at(1, 0)), None);
        assert_eq!(refused(&line("anl", &[]), &at(1, 6)), Some("forecast"));
        assert_eq!(refused(&line("6 hour fcst", &[]), &at(1, 6)), None);
        assert_eq!(refused(&line("6 hour fcst", &[]), &at(0, 360)), None);
        assert_eq!(refused(&line("6 hour fcst", &[]), &at(11, 1)), None);
        assert_eq!(
            refused(&line("6 hour fcst", &[]), &at(1, 7)),
            Some("forecast")
        );
        assert_eq!(refused(&line("0-1 hour acc fcst", &[]), &acc(0, 1)), None);
        assert_eq!(refused(&line("0-0 day acc fcst", &[]), &acc(0, 0)), None);
        assert_eq!(
            refused(&line("0-1 hour acc fcst", &[]), &acc(1, 1)),
            Some("forecast")
        );
        assert_eq!(
            refused(&line("0-1 hour max fcst", &[]), &acc(0, 1)),
            Some("forecast")
        );
        // An instant is not a window, nor the reverse.
        assert_eq!(
            refused(&line("1 hour fcst", &[]), &acc(0, 1)),
            Some("forecast")
        );
        assert_eq!(
            refused(&line("0-1 hour acc fcst", &[]), &at(1, 1)),
            Some("forecast")
        );
        // Unread wording, and a message in months, are not compared.
        assert_eq!(
            refused(
                &line("0-1 hour acc@(fcst,dt=1 hour),missing=0", &[]),
                &at(1, 9)
            ),
            None
        );
        assert_eq!(refused(&line("anl", &[]), &at(3, 1)), None);
    }

    /// An ECMWF `step` is an instant, or the end of a window (#1117).
    #[test]
    fn an_ecmwf_step_is_compared_with_the_end_of_the_window() {
        let step = |step: &str| {
            let mut expect = ecmwf(r#"{"param": "zz", "_offset": 0, "_length": 1}"#);
            expect.forecast = Some(step.to_string());
            expect
        };
        let max = over(on_template(8, None, 1, 0), 2, 1, 3);
        let at = |time| on_template(0, None, 1, time);

        assert_eq!(refused(&step("3"), &max), None);
        assert_eq!(refused(&step("0-3"), &max), None);
        assert_eq!(refused(&step("6"), &max), Some("forecast"));
        assert_eq!(refused(&step("3-6"), &max), Some("forecast"));
        assert_eq!(refused(&step("0"), &at(0)), None);
        assert_eq!(refused(&step("0"), &at(6)), Some("forecast"));
        assert_eq!(refused(&step("6-6"), &at(6)), None);
        assert_eq!(refused(&step("90m"), &at(6)), None);
    }

    /// A size or wavelength range as hex: Code Table 4.91 `kind`, then two
    /// `(scale factor, scaled value)` limits.
    fn range_block(kind: u8, first: (u8, u32), second: (u8, u32)) -> String {
        let mut octets = vec![kind, first.0];
        octets.extend(first.1.to_be_bytes());
        octets.push(second.0);
        octets.extend(second.1.to_be_bytes());
        octets.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `aerosol_size` and `aerosol_wavelength` in the forms
    /// `prt_code_table_4_91` prints, which name the limit a Code Table 4.91
    /// type uses and not the type (#1130).
    #[test]
    fn the_wgrib2_range_grammar() {
        use Interval::*;
        for (text, want) in [
            ("<1e-05", Below(1e-5)),
            (">2e-07", Above(2e-7)),
            ("<=3", AtMost(3.0)),
            (">=3", AtLeast(3.0)),
            (">=2e-07,<2e-06", HalfOpen(2e-7, 2e-6)),
            (">=3.38e-07,<=3.42e-07", Closed(3.38e-7, 3.42e-7)),
            (">2,<=3", OpenClosed(2.0, 3.0)),
            ("=5", Equal(5.0)),
        ] {
            assert_eq!(parse_interval(text), want, "{text}");
            // Printed back, it reads the same again.
            assert_eq!(parse_interval(&want.to_string()), want, "{text}");
        }
        // Printed as C's `%lg`, the way wgrib2 wrote the line.
        assert_eq!(Below(1e-5).to_string(), "<1e-05");
        assert_eq!(
            Closed(3.38e-7, 3.42e-7).to_string(),
            ">=3.38e-07,<=3.42e-07"
        );
        assert_eq!(printed_g(2.5e-6), "2.5e-06");
        assert_eq!(printed_g(1e-4), "0.0001");
        assert_eq!(printed_g(500.0), "500");
        assert_eq!(printed_g(0.125), "0.125");
        assert_eq!(printed_g(1_234_567.0), "1.23457e+06");
        assert_eq!(printed_g(999_999.6), "1e+06");
        assert_eq!(printed_g(-2e-7), "-2e-07");
        for unread in ["??? code_table_4.91=12", "", "<", ">=1,<", "<=x", "1e-05"] {
            assert_eq!(parse_interval(unread), Unread, "{unread:?}");
        }

        // Each Code Table 4.91 type, from octets, prints as wgrib2 prints it.
        let first = (6, 10); // 10 × 10⁻⁶
        let second = (5, 2); // 2 × 10⁻⁵
        let block = |kind| {
            let hex = range_block(kind, first, second);
            interval_of(&from_hex(&hex).unwrap())
        };
        assert_eq!(block(0), Some(Below(1e-5)));
        assert_eq!(block(1), Some(Above(2e-5)));
        assert_eq!(block(2), Some(HalfOpen(1e-5, 2e-5)));
        assert_eq!(block(3), Some(Above(1e-5)));
        assert_eq!(block(4), Some(Below(2e-5)));
        assert_eq!(block(5), Some(AtMost(1e-5)));
        assert_eq!(block(6), Some(AtLeast(2e-5)));
        assert_eq!(block(7), Some(Closed(1e-5, 2e-5)));
        assert_eq!(block(8), Some(AtLeast(1e-5)));
        assert_eq!(block(9), Some(AtMost(2e-5)));
        assert_eq!(block(10), Some(OpenClosed(1e-5, 2e-5)));
        assert_eq!(block(11), Some(Equal(1e-5)));
        assert_eq!(block(12), Some(Unread), "a type 4.91 does not define");
        assert_eq!(block(255), None, "missing: wgrib2 prints no range");
        // A limit the type uses coded missing is not read; one it does not
        // use may be.
        let missing = from_hex(&range_block(0, (0xff, u32::MAX), second)).unwrap();
        assert_eq!(interval_of(&missing), Some(Unread));
        let unused = from_hex(&range_block(0, first, (0xff, u32::MAX))).unwrap();
        assert_eq!(interval_of(&unused), Some(Below(1e-5)));
        // A negative scale factor (sign bit) multiplies.
        let large = from_hex(&range_block(11, (0x82, 5), second)).unwrap();
        assert_eq!(interval_of(&large), Some(Equal(500.0)));
        assert_eq!(interval_of(&[0, 6, 0]), Some(Unread), "too short");

        assert_eq!(Below(1e-5).same_as(&Below(1.000_000_1e-5)), Some(true));
        assert_eq!(Below(1e-5).same_as(&Below(1.1e-5)), Some(false));
        assert_eq!(Below(1e-5).same_as(&AtMost(1e-5)), Some(false));
        assert_eq!(Unread.same_as(&Below(1e-5)), None);
    }

    /// A chemistry or aerosol message on `template`: constituent `code`, then
    /// for 4.44-4.48 a size range of type 0 below 10 × 10⁻⁶ m, then for 4.48 a
    /// wavelength range of type 7, 545 to 565 × 10⁻⁹ m.
    fn constituent_message(template: u16, code: u16) -> MessageInfo {
        let mut hex = format!("{code:04x}");
        if template >= 44 {
            hex += &range_block(0, (6, 10), (0, 0));
        }
        if template == 48 {
            hex += &range_block(7, (9, 545), (9, 565));
        }
        on_template(template, Some(&hex), 1, 0)
    }

    /// `aerosol=`, `chemical=`, `aerosol_size` and `aerosol_wavelength`
    /// against the octets a chemistry or aerosol template keeps (#1130). Each
    /// is refused when it contradicts the message or the message has no such
    /// block, and never for being left out, which wgrib2's history allows.
    #[test]
    fn a_constituent_and_its_ranges_are_compared() {
        let anl = |qualifiers: &[&str]| line("anl", qualifiers);
        let dust = constituent_message(48, 62001);
        // The record as GEFS-Aerosols' sidecar writes it, and as wgrib2's
        // `ExtName.c` runs the two ranges together.
        for q in [
            &["aerosol=Dust dry", "aerosol_size <1e-05"][..],
            &[
                "aerosol=DUST DRY",
                "aerosol_size <1e-05:aerosol_wavelength >=5.45e-07,<=5.65e-07",
            ],
            &[
                "aerosol=62001",
                "aerosol_size <1e-05aerosol_wavelength >=5.45e-07,<=5.65e-07",
            ],
            &[],
            &["aerosol=Unknown type"],
            &["aerosol_size ??? code_table_4.91=12"],
        ] {
            assert_eq!(refused(&anl(q), &dust), None, "{q:?}");
        }
        assert_eq!(
            refused(&anl(&["aerosol=Total aerosol"]), &dust),
            Some("constituent")
        );
        assert_eq!(
            refused(&anl(&["aerosol=62000"]), &dust),
            Some("constituent")
        );
        assert_eq!(
            refused(&anl(&["aerosol_size <2.5e-06"]), &dust),
            Some("aerosolSize")
        );
        assert_eq!(
            refused(
                &anl(&["aerosol_size <1e-05aerosol_wavelength >=3.38e-07,<=3.42e-07"]),
                &dust
            ),
            Some("aerosolWavelength")
        );

        // 4.44: a size range and no wavelength range.
        let sized = constituent_message(44, 62008);
        assert_eq!(
            refused(
                &anl(&["aerosol=Sea salt dry", "aerosol_size <1e-05"]),
                &sized
            ),
            None
        );
        assert_eq!(
            refused(&anl(&["aerosol_wavelength >=5.45e-07,<=5.65e-07"]), &sized),
            Some("aerosolWavelength")
        );

        // 4.40: a chemical constituent and no ranges.
        let ozone = constituent_message(40, 0);
        assert_eq!(refused(&anl(&["chemical=Ozone"]), &ozone), None);
        assert_eq!(
            refused(&anl(&["chemical=Methane"]), &ozone),
            Some("constituent")
        );
        assert_eq!(
            refused(&anl(&["aerosol_size <1e-05"]), &ozone),
            Some("aerosolSize")
        );

        // A size range coded missing is not one: wgrib2 prints none.
        let no_size = on_template(
            44,
            Some(&format!("f231{}", range_block(255, (0, 0), (0, 0)))),
            1,
            0,
        );
        assert_eq!(
            refused(&anl(&["aerosol_size <1e-05"]), &no_size),
            Some("aerosolSize")
        );
        assert_eq!(refused(&anl(&["aerosol=Dust dry"]), &no_size), None);

        // wgrib2 prints the member of 4.41 and 4.43, not of 4.45 or 4.47
        // (`code_table_4_6_location`): a real line leaving it out is refused
        // only for the first two.
        let chemical_member = member(constituent_message(41, 0), 3, 4);
        assert_eq!(refused(&anl(&[]), &chemical_member), Some("ensemble"));
        assert_eq!(refused(&anl(&["ENS=+4"]), &chemical_member), None);
        let aerosol_member = member(constituent_message(45, 62008), 3, 4);
        assert_eq!(refused(&anl(&[]), &aerosol_member), None);
        assert_eq!(
            refused(&anl(&["ENS=+5"]), &aerosol_member),
            Some("ensemble")
        );

        // wgrib2 reads 4.44's two-octet forecast time as four octets, so its
        // window is not compared; 4.48's is.
        let at_six = |template| {
            let mut message = constituent_message(template, 62001);
            if let crate::api::MessageFingerprint::Grib2 { forecast_time, .. } =
                &mut message.fingerprint
            {
                *forecast_time = Some(6);
            }
            message
        };
        assert_eq!(refused(&line("anl", &[]), &at_six(44)), None);
        assert_eq!(refused(&line("anl", &[]), &at_six(48)), Some("forecast"));
        assert_eq!(refused(&line("6 hour fcst", &[]), &at_six(48)), None);

        // Nor its level: wgrib2 reads it at octet 34, inside the forecast time
        // of the four-octet form, and the fingerprint does not say which form.
        // The message is on 2 m above ground; a line for 2 mb is refused on
        // 4.48 and not compared on 4.44.
        assert_eq!(
            refused(&level("2 mb"), &constituent_message(48, 62001)),
            Some("level")
        );
        assert_eq!(
            refused(&level("2 mb"), &constituent_message(44, 62001)),
            None
        );

        // A message on any other template has no constituent at all.
        let plain = on_template(0, None, 1, 0);
        assert_eq!(
            refused(&anl(&["aerosol=Dust dry"]), &plain),
            Some("constituent")
        );
        assert_eq!(
            refused(&anl(&["aerosol_size <1e-05"]), &plain),
            Some("aerosolSize")
        );
    }

    /// A 13-octet probability block as hex: Code Table 4.9 `kind`, then the
    /// lower and upper limits as (scale factor octet, scaled value).
    fn probability_block(kind: u8, lower: (u8, u32), upper: (u8, u32)) -> String {
        let mut octets = vec![0, 255, kind, lower.0];
        octets.extend(lower.1.to_be_bytes());
        octets.push(upper.0);
        octets.extend(upper.1.to_be_bytes());
        octets.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `prob …` against templates 4.5 and 4.9, in `Prob.c`'s forms, which
    /// print type 1 and type 3 alike, and type 0 and type 4 (#1117).
    #[test]
    fn a_probability_is_compared_with_the_messages_block() {
        const MISSING: (u8, u32) = (0xff, u32::MAX);
        let prob =
            |kind, lower, upper| on_template(9, Some(&probability_block(kind, lower, upper)), 1, 0);
        let check = |q: &str, message: &MessageInfo| {
            refused(&line("anl", &[q, "prob fcst 255/255"]), message)
        };

        assert_eq!(check("prob >0.254", &prob(1, MISSING, (3, 254))), None);
        assert_eq!(check("prob >0.254", &prob(3, (3, 254), MISSING)), None);
        assert_eq!(check("prob <0.254", &prob(0, (3, 254), MISSING)), None);
        assert_eq!(check("prob <0.254", &prob(4, MISSING, (3, 254))), None);
        assert_eq!(check("prob >=1 <2", &prob(2, (0, 1), (0, 2))), None);
        assert_eq!(check("prob =3", &prob(2, (0, 3), (0, 3))), None);
        assert_eq!(check("prob =3", &prob(5, (0, 3), MISSING)), None);
        assert_eq!(check("prob above normal", &prob(6, MISSING, MISSING)), None);
        // A negative scale factor multiplies.
        assert_eq!(check("prob >2500", &prob(1, MISSING, (0x82, 25))), None);
        // `%g` keeps six figures of an `f32`.
        assert_eq!(
            check("prob >0.1", &prob(1, MISSING, (9, 100_000_001))),
            None
        );

        assert_eq!(
            check("prob >0.1016", &prob(1, MISSING, (3, 254))),
            Some("probability")
        );
        assert_eq!(
            check("prob <0.254", &prob(1, MISSING, (3, 254))),
            Some("probability")
        );
        assert_eq!(
            check("prob near normal", &prob(6, MISSING, MISSING)),
            Some("probability")
        );
        // A form this does not read, or a type it does not, is not compared.
        assert_eq!(
            check("prob ?? (code table 4.9=9)", &prob(1, MISSING, (3, 254))),
            None
        );
        assert_eq!(check("prob >0.254", &prob(9, MISSING, (3, 254))), None);
        assert_eq!(check("prob >0.254", &prob(1, MISSING, MISSING)), None);

        // A probability line on a message with no probability block, and a
        // real line without one on a message with it.
        let plain = on_template(0, None, 1, 0);
        assert_eq!(check("prob >0.254", &plain), Some("probability"));
        let block = prob(1, MISSING, (3, 254));
        assert_eq!(refused(&line("anl", &[]), &block), Some("probability"));
        // A hand-built expectation states only what it chose to.
        assert_eq!(
            refused(&Expectation::new().with_abbreviation("TMP"), &block),
            None
        );
    }

    /// `ENS=…` against the ensemble block of 4.1 and 4.11 (#1117).
    #[test]
    fn an_ensemble_member_is_compared_with_the_messages() {
        let template1 = |kind, number| member(on_template(1, None, 1, 0), kind, number);
        let check = |q: &str, message: &MessageInfo| refused(&line("anl", &[q]), message);

        assert_eq!(check("ENS=+1", &template1(3, 1)), None);
        assert_eq!(check("ENS=-2", &template1(2, 2)), None);
        assert_eq!(check("ENS=low-res ctl", &template1(1, 0)), None);
        assert_eq!(check("ENS=hi-res ctl", &template1(0, 0)), None);
        assert_eq!(check("ENS=+1", &template1(3, 2)), Some("ensemble"));
        assert_eq!(check("ENS=+1", &template1(2, 1)), Some("ensemble"));
        assert_eq!(check("ENS=low-res ctl", &template1(0, 0)), Some("ensemble"));
        // ECMWF's members with the type missing, and NCEP's type 192
        // (`ENS=5`), are not compared beyond being members.
        assert_eq!(check("ENS=+3", &template1(255, 3)), None);
        assert_eq!(check("ENS=5", &template1(192, 5)), None);

        assert_eq!(
            check("ENS=+1", &on_template(0, None, 1, 0)),
            Some("ensemble")
        );
        assert_eq!(
            refused(&line("anl", &[]), &template1(3, 1)),
            Some("ensemble")
        );
        assert_eq!(
            refused(
                &Expectation::new().with_abbreviation("TMP"),
                &template1(3, 1)
            ),
            None
        );
    }

    /// A derived forecast (`ens mean`) against 4.2 and 4.12, and a percentile
    /// (`10% level`) against 4.6 and 4.10 (#1117).
    #[test]
    fn derived_forecasts_and_percentiles_are_compared() {
        let mean = derived(on_template(2, None, 1, 0), 0);
        let spread = derived(on_template(2, None, 1, 0), 2);
        let check = |q: &str, message: &MessageInfo| refused(&line("anl", &[q]), message);

        assert_eq!(check("ens mean", &mean), None);
        assert_eq!(check("ens std dev", &spread), None);
        assert_eq!(check("ens mean", &spread), Some("derivedForecast"));
        assert_eq!(
            check(
                "unknown derived fcst",
                &derived(on_template(2, None, 1, 0), 50)
            ),
            None
        );
        assert_eq!(
            check("ens mean", &on_template(0, None, 1, 0)),
            Some("derivedForecast")
        );
        assert_eq!(refused(&line("anl", &[]), &mean), Some("derivedForecast"));

        let tenth = on_template(6, Some("0a"), 1, 0);
        assert_eq!(check("10% level", &tenth), None);
        assert_eq!(check("50% level", &tenth), Some("percentile"));
        assert_eq!(
            check("10% level", &on_template(0, None, 1, 0)),
            Some("percentile")
        );
        assert_eq!(refused(&line("anl", &[]), &tenth), Some("percentile"));
        assert_eq!(
            refused(&Expectation::new().with_abbreviation("TMP"), &tenth),
            None
        );
    }

    /// A qualifier the grammar does not read may be a derived forecast or a
    /// percentile in wording a newer wgrib2 added, so a line carrying one is
    /// not refused for lacking them. A probability or a member is always
    /// recognizable, so its absence is still refused.
    #[test]
    fn an_unread_qualifier_leaves_a_missing_one_unproven() {
        let mean = derived(on_template(2, None, 1, 0), 0);
        let tenth = on_template(6, Some("0a"), 1, 0);
        let novel = line("anl", &["ens median of all members"]);
        assert_eq!(refused(&novel, &mean), None);
        assert_eq!(refused(&novel, &tenth), None);
        // Words this reads still prove the absence.
        let known = line("anl", &["probability forecast"]);
        assert_eq!(refused(&known, &mean), Some("derivedForecast"));
        assert_eq!(refused(&known, &tenth), Some("percentile"));

        let prob = on_template(
            9,
            Some(&probability_block(1, (0xff, u32::MAX), (3, 254))),
            1,
            0,
        );
        assert_eq!(refused(&novel, &prob), Some("probability"));
        let member1 = member(on_template(1, None, 1, 0), 3, 1);
        assert_eq!(refused(&novel, &member1), Some("ensemble"));
    }

    /// `Prob.c` prints a type 2 range as `prob =x` when its limits are equal as
    /// `f32`s, which two limits at different scales can be while their `f64`s
    /// differ.
    #[test]
    fn a_range_with_limits_equal_as_f32_is_an_equality() {
        let block = probability_block(2, (0, 1), (9, 1_000_000_001));
        let message = on_template(9, Some(&block), 1, 0);
        let check = |q: &str| refused(&line("anl", &[q, "prob fcst 255/255"]), &message);
        assert_eq!(check("prob =1"), None);
        assert_eq!(check("prob =2"), Some("probability"));
    }

    /// An unnamed triple the WMO master table defines is not where the tables
    /// are complete: NCEP's local `MAXREF` (0.16.198) may be published at
    /// WMO's layer-maximum reflectivity 0.16.8, which has no short name here,
    /// under the name a newer wgrib2 gives it (#1116 review).
    #[test]
    fn an_unnamed_wmo_parameter_is_not_compared_by_name() {
        assert_eq!(
            verify_message(
                &Expectation::new().with_abbreviation("MAXREF"),
                &unnamed(0, 16, 8, None)
            ),
            Ok(())
        );
    }

    /// An ECMWF `number` is the member a message is.
    #[test]
    fn an_ecmwf_number_is_the_member() {
        let record = |number: &str| {
            ecmwf(&format!(
                r#"{{"param": "zz", "number": "{number}", "_offset": 0, "_length": 1}}"#
            ))
        };
        let pf = |n| member(on_template(1, None, 1, 0), 3, n);
        assert_eq!(refused(&record("3"), &pf(3)), None);
        assert_eq!(refused(&record("3"), &pf(4)), Some("ensemble"));
        assert_eq!(
            refused(&record("3"), &on_template(0, None, 1, 0)),
            Some("ensemble")
        );
        // The control may be written without an ensemble block.
        assert_eq!(refused(&record("0"), &on_template(0, None, 1, 0)), None);
        assert_eq!(refused(&record("0"), &pf(0)), None);
        assert_eq!(
            refused(
                &ecmwf(r#"{"param": "zz", "_offset": 0, "_length": 1}"#),
                &pf(3)
            ),
            None
        );
    }

    /// The MARS table against message shapes the real pairs do not have: a
    /// process the template cannot carry, a length in other units, a name the
    /// table does not hold, and the gap of a block without a process (#1116).
    #[test]
    fn a_mars_name_is_matched_on_its_blocks() {
        let mars = |name: &str| {
            ecmwf(&format!(
                r#"{{"param": "{name}", "_offset": 0, "_length": 1}}"#
            ))
        };
        let instant = on_template(0, None, 1, 0);
        let max_3h = over(on_template(8, None, 1, 0), 2, 1, 3);

        assert_eq!(refused(&mars("mx2t3"), &max_3h), None);
        assert_eq!(
            refused(&mars("mx2t3"), &over(on_template(8, None, 1, 0), 2, 0, 180)),
            None
        );
        assert_eq!(refused(&mars("mx2t6"), &max_3h), Some("abbreviation"));
        assert_eq!(refused(&mars("mx2t3"), &instant), Some("abbreviation"));
        // `2t` states no process, so its block matches a maximum too, but
        // `mx2t3`'s states more and is the one eccodes names it after
        // (#1298). From tables version 34 eccodes 2.45 names it `max_2t`,
        // which also outranks `2t`.
        assert_eq!(refused(&mars("2t"), &max_3h), Some("abbreviation"));
        assert_eq!(refused(&mars("2t"), &instant), None);
        // Nested time ranges are not compared.
        let mut nested = max_3h.clone();
        if let crate::api::MessageFingerprint::Grib2 { statistics, .. } = &mut nested.fingerprint {
            statistics.push(statistics[0]);
        }
        assert_eq!(refused(&mars("mn2t3"), &nested), None);
        assert_eq!(refused(&mars("notamarsname"), &instant), None);
        assert_eq!(refused(&mars("r"), &instant), Some("abbreviation"));
    }

    /// An ECMWF line naming `param` and nothing else.
    fn mars(name: &str) -> Expectation {
        ecmwf(&format!(
            r#"{{"param": "{name}", "_offset": 0, "_length": 1}}"#
        ))
    }

    /// A message on `codes` at `first`, on product template `template` with
    /// its unread octets `octets`.
    fn coded(
        codes: (u8, u8, u8),
        first: crate::api::SurfaceCodes,
        template: u16,
        octets: Option<&str>,
    ) -> MessageInfo {
        let mut message = on_template(template, octets, 1, 0);
        if let crate::api::MessageFingerprint::Grib2 {
            discipline,
            parameter_category,
            parameter_number,
            first_surface,
            ..
        } = &mut message.fingerprint
        {
            *discipline = codes.0;
            *parameter_category = Some(codes.1);
            *parameter_number = Some(codes.2);
            *first_surface = Some(first);
        }
        message
    }

    /// A surface of type `kind` with its value coded missing.
    fn bare(kind: u8) -> crate::api::SurfaceCodes {
        crate::api::SurfaceCodes {
            surface_type: kind,
            scale_factor: None,
            scaled_value: None,
        }
    }

    /// Names that share parameter codes are told apart by the surface, as
    /// eccodes tells them, and the most specific block wins (#1298): `10u`,
    /// `100u` and `u` are all 0.2.2, `msl` and `sp` both 0.3.0, `str` and
    /// `ttr` both 0.5.5 accumulated.
    #[test]
    fn mars_names_sharing_codes_are_told_apart_by_the_surface() {
        let u = (0, 2, 2);
        let at_10m = coded(u, surface((103, 0, 10)), 0, None);
        let at_100m = coded(u, surface((103, 0, 100)), 0, None);
        let at_500hpa = coded(u, surface((100, 0, 50_000)), 0, None);
        assert_eq!(refused(&mars("10u"), &at_10m), None);
        assert_eq!(refused(&mars("100u"), &at_10m), Some("abbreviation"));
        // `u` states the codes alone, so `10u`'s block outranks it.
        assert_eq!(refused(&mars("u"), &at_10m), Some("abbreviation"));
        assert_eq!(refused(&mars("100u"), &at_100m), None);
        assert_eq!(refused(&mars("10u"), &at_100m), Some("abbreviation"));
        assert_eq!(refused(&mars("u"), &at_500hpa), None);
        assert_eq!(refused(&mars("10u"), &at_500hpa), Some("abbreviation"));

        let pressure = (0, 3, 0);
        let msl = coded(pressure, bare(101), 0, None);
        let ground = coded(pressure, bare(1), 0, None);
        assert_eq!(refused(&mars("msl"), &msl), None);
        assert_eq!(refused(&mars("sp"), &msl), Some("abbreviation"));
        assert_eq!(refused(&mars("sp"), &ground), None);
        assert_eq!(refused(&mars("msl"), &ground), Some("abbreviation"));

        let radiation = (0, 5, 5);
        let str_ = over(coded(radiation, bare(1), 8, None), 1, 1, 6);
        let ttr = over(coded(radiation, bare(8), 8, None), 1, 1, 6);
        assert_eq!(refused(&mars("str"), &str_), None);
        assert_eq!(refused(&mars("ttr"), &str_), Some("abbreviation"));
        assert_eq!(refused(&mars("ttr"), &ttr), None);
        assert_eq!(refused(&mars("str"), &ttr), Some("abbreviation"));
    }

    /// A probability of precipitation (template 4.9) is named after its
    /// probability block, so `tp` is not a `tpg25`, and one threshold is not
    /// another (#1298). A block the fingerprint does not carry is not compared.
    #[test]
    fn a_mars_probability_is_named_after_its_threshold() {
        let precipitation = (0, 1, 52);
        // Code Table 4.9 type 3, above the lower limit of 25 (scale 0).
        let above_25 = probability_block(3, (0, 25), (0xff, u32::MAX));
        let tpg25 = over(coded(precipitation, bare(1), 9, Some(&above_25)), 1, 1, 24);
        assert_eq!(refused(&mars("tpg25"), &tpg25), None);
        assert_eq!(refused(&mars("tp"), &tpg25), Some("abbreviation"));
        assert_eq!(refused(&mars("tpg50"), &tpg25), Some("abbreviation"));
        // The same limit at another scale is another key value to eccodes.
        let rescaled = probability_block(3, (1, 250), (0xff, u32::MAX));
        let mut other_scale = tpg25.clone();
        if let crate::api::MessageFingerprint::Grib2 { product_octets, .. } =
            &mut other_scale.fingerprint
        {
            *product_octets = Some(rescaled);
        }
        assert_eq!(refused(&mars("tpg25"), &other_scale), Some("abbreviation"));

        // The amount: template 4.8 has no probability block.
        let amount = over(coded(precipitation, bare(1), 8, None), 1, 1, 24);
        assert_eq!(refused(&mars("tp"), &amount), None);
        assert_eq!(refused(&mars("tpg25"), &amount), Some("abbreviation"));

        // A 4.9 whose block is not carried: the probability names are only
        // possible, so they cannot refuse `tp`, and are not refused.
        let unread = over(coded(precipitation, bare(1), 9, None), 1, 1, 24);
        assert_eq!(refused(&mars("tp"), &unread), None);
        assert_eq!(refused(&mars("tpg25"), &unread), None);

        // `tpl01` (eccodes 2.45) states its surface value `missing()`, and a
        // value coded missing is what matches it: below 0.1 (type 0, scale 1).
        let below_01 = probability_block(0, (1, 1), (0xff, u32::MAX));
        let tpl01 = over(coded(precipitation, bare(1), 9, Some(&below_01)), 1, 1, 24);
        assert_eq!(refused(&mars("tpl01"), &tpl01), None);
        // eccodes 2.34.1 has no WMO block for `tpl01`, so it names these
        // bytes `tp`, and either version's name passes.
        assert_eq!(refused(&mars("tp"), &tpl01), None);
        assert_eq!(refused(&mars("tpg25"), &tpl01), Some("abbreviation"));
        let mut valued = tpl01.clone();
        if let crate::api::MessageFingerprint::Grib2 { first_surface, .. } = &mut valued.fingerprint
        {
            *first_surface = Some(surface((1, 0, 0)));
        }
        assert_eq!(refused(&mars("tpl01"), &valued), Some("abbreviation"));
    }

    /// A block stating a condition the table does not keep (ECMWF's local
    /// `tp`, 0.1.193, states `localTablesVersion`) is possible, never
    /// certain: its own name passes, and it refuses no other.
    #[test]
    fn a_block_with_an_unkept_condition_is_only_possible() {
        let local_tp = over(coded((0, 1, 193), bare(1), 8, None), 1, 1, 24);
        assert_eq!(refused(&mars("tp"), &local_tp), None);
        // eccodes 2.45's local `tprate` is 0.1.193 too, with `localTablesVersion`
        // and an average: not certain either, and not this message.
        assert_eq!(refused(&mars("tprate"), &local_tp), Some("abbreviation"));
    }

    /// eccodes 2.48 dropped the mean-sea-level surface from `swhg2`..`swhg8`,
    /// so a producer on 2.48 may write a wave-height probability on another
    /// surface and still call it `swhg2`. The table reads 2.48 too, so that
    /// record passes, as it did before #1298; the threshold still counts.
    #[test]
    fn a_name_the_newest_eccodes_loosened_still_passes() {
        let wave_height = (10, 0, 3);
        // Code Table 4.9 type 3, above 2 m (scale 0).
        let above_2 = probability_block(3, (0, 2), (0xff, u32::MAX));
        for surface in [bare(1), bare(101), surface((103, 0, 10))] {
            let message = coded(wave_height, surface, 5, Some(&above_2));
            assert_eq!(refused(&mars("swhg2"), &message), None, "{surface:?}");
            assert_eq!(
                refused(&mars("swhg4"), &message),
                Some("abbreviation"),
                "{surface:?}"
            );
        }
    }

    /// `sol N` against soil level N, and against the soil layer whose bottom
    /// is level N, which is how eccodes numbers it (#1116).
    #[test]
    fn an_ecmwf_soil_level_is_compared_by_number() {
        let sol = |n: &str| {
            ecmwf(&format!(
                r#"{{"param": "zz", "levtype": "sol", "levelist": "{n}", "_offset": 0, "_length": 1}}"#
            ))
        };
        let single = info_on("SOILL", (151, 0, 3), None);
        let layer = info_on("SOILL", (151, 0, 2), Some((151, 0, 3)));
        assert_eq!(refused(&sol("3"), &single), None);
        assert_eq!(refused(&sol("3"), &layer), None);
        assert_eq!(refused(&sol("2"), &layer), Some("level"));
        assert_eq!(
            refused(&sol("3"), &info_on("TSOIL", (106, 0, 3), None)),
            Some("level")
        );
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
