//! Fetch planning, with the tables a planner is not allowed to link.
//!
//! [`fieldglass_fetchplan`] reads a cloud-native sidecar and returns byte
//! ranges. It depends on no format crate on purpose — a planner that linked a
//! decoder would drag GRIB2's four codecs into a host that only wanted to know
//! which bytes to ask for — so the two things that *do* need a decoder live
//! here, where one already is:
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

use std::collections::HashMap;
use std::sync::OnceLock;

pub use fieldglass_fetchplan::{
    Address, Candidate, Dialect, EcmwfIndex, Expect, FetchPlanError, LevelSpec, Manifest,
    MessageManifest, Mismatch, NoResolver, ParameterId, ParameterResolver, PlanItem, PlanRange,
    Query, SourceSpec, Surface, Wgrib2Idx, candidates, parse_ecmwf_level, parse_ncep_level,
};

use crate::api::MessageInfo;

/// Disciplines to scan when building the reverse index.
///
/// Read from [`fieldglass_grib2::lookup_discipline`] rather than written down:
/// WMO Code Table 0.0 assigns eight, and scanning all 255 costs 879 ms of
/// `lookup_parameter` calls against ~25 ms for these — thirty-six times the
/// work, for code space no table defines. A discipline WMO adds later is picked
/// up when that table gains it, without this list being touched.
///
/// The filter reads a *string*, because `lookup_discipline` reports an
/// unassigned code as `"Unknown discipline"` rather than as `None`.
/// `scan_bounds::no_parameter_lives_outside_the_named_disciplines` is what
/// stops that being a silent assumption — and it earned its place immediately,
/// by catching discipline 191, which carries six parameters and which Code
/// Table 0.0 assigns but `lookup_discipline` did not name.
fn disciplines() -> Vec<u8> {
    (0u8..=254)
        .filter(|&d| {
            !matches!(
                fieldglass_grib2::lookup_discipline(d),
                "Unknown discipline" | "Missing"
            )
        })
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
/// Comparison is case-insensitive and ignores surrounding whitespace, and
/// nothing else: the sidecar's `2 m above ground` and the decoder's
/// `2 m above ground` are the same string because both come from the same WMO
/// surface table, and normalising harder would let a real disagreement through.
pub fn verify_message(expect: &Expect, info: &MessageInfo) -> Result<(), Mismatch> {
    if let Some(promised) = expect.abbreviation.as_deref()
        // A numeric fallback name is not an abbreviation the decoder can match;
        // the `parameter` check below is the one that applies to it.
        && !promised.starts_with("var ")
        && !equivalent(promised, &info.abbreviation)
    {
        return Err(Mismatch::Field {
            field: "abbreviation",
            expected: promised.to_string(),
            actual: info.abbreviation.clone(),
        });
    }

    if let Some(promised) = expect.level.as_deref()
        && !equivalent(promised, &info.level)
    {
        return Err(Mismatch::Field {
            field: "level",
            expected: promised.to_string(),
            actual: info.level.clone(),
        });
    }

    if let Some(promised) = &expect.reference_time
        && let Some(actual) = &info.reference_time
        // The sidecar writes `YYYYMMDDHH`; the message states RFC 3339. Compared
        // as the ten digits both can produce, so this is a real check and not a
        // format comparison that always fails.
        && let Some(actual) = ten_digit_reference_time(actual)
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

    fn info(abbrev: &str, level: &str) -> MessageInfo {
        MessageInfo {
            index: 0,
            offset_bytes: 0,
            parameter: "Temperature".into(),
            abbreviation: abbrev.into(),
            units: "K".into(),
            level: level.into(),
            level_type: "Specified height level above ground".into(),
            reference_time: Some("2026-09-04T00:00:00Z".into()),
            forecast: "analysis".into(),
            packing: "grid_simple".into(),
            grid: None,
            size_label: None,
        }
    }

    /// The check that catches a stale sidecar pointing at a valid but wrong
    /// message: the bytes decode fine, and they are not the promised field.
    #[test]
    fn a_wrong_field_is_reported_with_both_sides() {
        let expect = Expect::new()
            .with_abbreviation("TMP")
            .with_level("2 m above ground");
        assert_eq!(
            verify_message(&expect, &info("TMP", "2 m above ground")),
            Ok(())
        );

        assert_eq!(
            verify_message(&expect, &info("RH", "2 m above ground")),
            Err(Mismatch::Field {
                field: "abbreviation",
                expected: "TMP".into(),
                actual: "RH".into()
            })
        );
        assert_eq!(
            verify_message(&expect, &info("TMP", "850 mb")),
            Err(Mismatch::Field {
                field: "level",
                expected: "2 m above ground".into(),
                actual: "850 mb".into()
            })
        );
    }

    /// An expectation that promises nothing checks nothing, rather than
    /// failing because the fields are absent.
    #[test]
    fn an_empty_expectation_passes() {
        assert_eq!(
            verify_message(&Expect::default(), &info("TMP", "surface")),
            Ok(())
        );
    }

    /// A numeric fallback name is not an abbreviation and must not be compared
    /// as one — every such message would otherwise fail verification.
    #[test]
    fn a_numeric_fallback_name_is_not_compared_as_an_abbreviation() {
        let expect = Expect::new()
            .with_abbreviation("var discipline=0 center=7 local_table=1 parmcat=16 parm=201");
        assert_eq!(
            verify_message(&expect, &info("REFC", "entire atmosphere")),
            Ok(())
        );
    }

    /// The two reference-time spellings are compared as the ten digits both can
    /// produce. A naive string comparison would fail on every message.
    #[test]
    fn reference_times_are_compared_across_their_two_spellings() {
        let expect = Expect::new().with_reference_time("2026090400");
        assert_eq!(verify_message(&expect, &info("TMP", "surface")), Ok(()));

        let stale = Expect::new().with_reference_time("2026090312");
        assert_eq!(
            verify_message(&stale, &info("TMP", "surface")),
            Err(Mismatch::Field {
                field: "referenceTime",
                expected: "2026090312".into(),
                actual: "2026090400".into()
            })
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
    /// the filter reads a *string* out of `lookup_discipline` and a reworded
    /// fallback would silently narrow or widen the scan.
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
