//! The NCEP local parameter table, cross-checked against eccodes (#426).
//!
//! Unlike ECMWF (#424) and DWD (#425), this table does not come from eccodes:
//! `tables_ncep.rs` is generated from wgrib2's `gribtable.dat`, which carries
//! 479 routable entries where eccodes' `localConcepts/kwbc` carries 313, and
//! carries NCEP's own uppercase abbreviations rather than eccodes' lowercased
//! ones.
//!
//! That makes eccodes something better than a second opinion here — it is a
//! genuinely independent transcription of the same NCO tables, through its own
//! definition files and its own concept engine. Where the two overlap they must
//! agree about *which parameter* a triple is; where they do not overlap is the
//! 166 entries this source buys.
//!
//! Regenerate the snapshot with
//! `python3 tools/gen_ncep_tables.py --crosscheck && cargo fmt`.

use fieldglass_grib2::{Grib2Reader, Originator, lookup_parameter};
use std::collections::BTreeMap;

const CROSSCHECK: &str = include_str!("fixtures/ncep_eccodes_crosscheck.json");

/// WMO Common Code Table C-11 code for NCEP.
const NCEP: u16 = 7;

/// A GRIB2 message NCEP really published, carrying a local triple: Eta on a
/// Lambert grid, parameter `(0, 3, 192)`.
const ETA: &[u8] = include_bytes!("fixtures/eta_lambert_msg0.grib2");

/// The three triples where eccodes prefers a different name for the same
/// parameter. NCO's own documentation is with wgrib2 on all three, so they are
/// pinned as known divergences rather than tolerated by a loose comparison —
/// a fourth appearing is something to look at, not to absorb.
const KNOWN_DIVERGENCES: [(&str, &str, &str); 3] = [
    ("0/1/197", "MDIV", "mconv"),
    ("0/191/194", "RTSEC", "tsec"),
    ("10/3/194", "ELEV", "elevhtml"),
];

/// `"<discipline>/<category>/<number>"` -> what eccodes answered for it.
/// `"unknown"` in every field means eccodes does not define that triple.
fn crosscheck() -> BTreeMap<String, (String, String, String)> {
    let parsed: serde_json::Value =
        serde_json::from_str(CROSSCHECK).expect("crosscheck is valid JSON");
    assert_eq!(
        parsed["eccodes"].as_str(),
        Some("2.34.1"),
        "the snapshot was built with a different eccodes than the repo pins"
    );
    assert_eq!(
        parsed["wgrib2"].as_str(),
        Some("v3.8.0"),
        "the snapshot was built against a different wgrib2 than the table"
    );
    parsed["resolved"]
        .as_object()
        .expect("resolved section")
        .iter()
        .map(|(k, v)| {
            let field = |i: usize| v[i].as_str().expect("string").to_string();
            (k.clone(), (field(0), field(1), field(2)))
        })
        .collect()
}

fn triple(key: &str) -> (u8, u8, u8) {
    let mut it = key
        .split('/')
        .map(|n| n.parse::<u8>().expect("numeric key part"));
    (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
}

/// Every triple the table ships resolves, and the shipped abbreviation matches
/// what eccodes calls the same parameter — case folded, because the two sources
/// disagree about case by convention and about nothing else.
#[test]
fn the_table_agrees_with_eccodes_where_they_overlap() {
    let snapshot = crosscheck();
    assert!(
        snapshot.len() > 400,
        "only {} entries — the snapshot is not loaded, so this proves nothing",
        snapshot.len()
    );

    let ncep = Originator::new(NCEP, 0, 1);
    let mut compared = 0usize;
    for (key, (short, _, _)) in &snapshot {
        let (discipline, category, number) = triple(key);
        let (ours, _, _) = lookup_parameter(ncep, discipline, category, number)
            .unwrap_or_else(|| panic!("{key} is in the table's own snapshot but does not resolve"));
        if short == "unknown" {
            continue; // eccodes does not define it; that is the point of wgrib2.
        }
        compared += 1;
        if let Some((_, _, eccodes_name)) = KNOWN_DIVERGENCES.iter().find(|(k, _, _)| k == key) {
            assert_eq!(
                short, eccodes_name,
                "{key} is pinned as a known divergence, but eccodes now says {short:?}"
            );
            continue;
        }
        assert_eq!(
            ours.to_lowercase(),
            short.to_lowercase(),
            "{key}: wgrib2 says {ours:?}, eccodes says {short:?}"
        );
    }
    assert!(
        compared > 300,
        "only {compared} triples were actually compared against eccodes"
    );
}

/// The 166 entries eccodes does not define are why this table comes from
/// wgrib2. Asserted, so that a source swap or an eccodes bump that quietly
/// erased the gain would fail rather than pass.
#[test]
fn wgrib2_covers_what_eccodes_does_not() {
    let snapshot = crosscheck();
    let beyond_eccodes = snapshot
        .values()
        .filter(|(short, _, _)| short == "unknown")
        .count();
    assert_eq!(
        beyond_eccodes, 166,
        "wgrib2 covers {beyond_eccodes} triples eccodes does not, expected 166"
    );
    assert_eq!(
        snapshot.len(),
        479,
        "the table should ship 479 entries at wgrib2 v3.8.0"
    );
}

/// NCEP's abbreviations are uppercase, which is what NCO publishes and what the
/// curated entries in `tables.rs` already use. Taking eccodes' lowercase forms
/// would have made a GFS file read `snohf` where every NCEP listing says
/// `SNOHF`.
#[test]
fn the_abbreviations_are_ncep_s_own_uppercase_forms() {
    let ncep = Originator::new(NCEP, 0, 1);
    let mut checked = 0usize;
    for key in crosscheck().keys() {
        let (discipline, category, number) = triple(key);
        let (short, _, _) = lookup_parameter(ncep, discipline, category, number).expect("resolves");
        assert_eq!(
            short,
            short.to_uppercase(),
            "{key} ships {short:?}, which is not NCEP's form"
        );
        assert!(!short.is_empty(), "{key} ships an empty abbreviation");
        checked += 1;
    }
    assert_eq!(checked, 479);
}

/// A spot-check with the values written out, so a reader can see what the table
/// contains without decoding a 479-entry fixture. These are fields a forecaster
/// meets in GFS, HRRR and RAP output.
#[test]
fn well_known_ncep_parameters_resolve() {
    let ncep = Originator::new(NCEP, 0, 1);
    for (discipline, category, number, expected) in [
        (
            0u8,
            3u8,
            192u8,
            ("MSLET", "MSLP (Eta model reduction)", "Pa"),
        ),
        (0, 3, 198, ("MSLMA", "MSLP (MAPS System Reduction)", "Pa")),
        (0, 1, 195, ("CSNOW", "Categorical Snow", "-")),
        (0, 16, 196, ("REFC", "Composite reflectivity", "dB")),
        (0, 0, 192, ("SNOHF", "Snow Phase Change Heat Flux", "W/m^2")),
    ] {
        assert_eq!(
            lookup_parameter(ncep, discipline, category, number),
            Some(expected),
            "{discipline}/{category}/{number}"
        );
    }
}

/// The table takes no local table version. wgrib2 records 1 for every NCEP
/// entry and every NCEP file in the tree declares 1, but that is a convention
/// rather than a rule, and eccodes' own concepts gate none of them.
#[test]
fn the_table_is_not_gated_on_a_local_table_version() {
    let at = |version| lookup_parameter(Originator::new(NCEP, 0, version), 0, 3, 192);
    let expected = at(1);
    assert!(
        expected.is_some(),
        "the version real NCEP files carry resolves to nothing"
    );
    for version in [0u8, 2, 128, 255] {
        assert_eq!(at(version), expected, "local table version {version}");
    }
}

/// NCEP's answers are NCEP's alone, and the sub-centre does not gate them —
/// NBM is sub-centre 14 under the same centre and reads the same table.
///
/// Note what this does *not* assert. Another centre may well define the same
/// triple: DWD calls `(0, 3, 192)` `PP` "Pressure perturbation" where NCEP
/// calls it `MSLET`. Two centres disagreeing about one triple is the whole
/// reason the seam is keyed on the centre, so the check is that they get
/// different answers, not that only one of them gets an answer.
#[test]
fn the_table_is_scoped_to_ncep() {
    let sample = (0u8, 3u8, 192u8);
    assert!(lookup_parameter(Originator::new(NCEP, 0, 1), sample.0, sample.1, sample.2).is_some());
    assert_eq!(
        lookup_parameter(Originator::new(NCEP, 0, 1), sample.0, sample.1, sample.2),
        lookup_parameter(Originator::new(NCEP, 14, 1), sample.0, sample.1, sample.2),
        "the sub-centre must not gate a centre-wide table"
    );
    // Not `None` for every other centre — DWD defines `(0, 3, 192)` too, as
    // `PP` "Pressure perturbation", which is exactly why local tables are keyed
    // on the centre. The property that matters is that nobody else gets *this*
    // answer.
    let ours = lookup_parameter(Originator::new(NCEP, 0, 1), sample.0, sample.1, sample.2);
    for other in [0u16, 34, 78, 98, 161] {
        assert_ne!(
            lookup_parameter(Originator::new(other, 0, 1), sample.0, sample.1, sample.2),
            ours,
            "centre {other} read NCEP's answer for {}/{}/{}",
            sample.0,
            sample.1,
            sample.2
        );
    }
}

/// End to end on a message NCEP actually published: the committed Eta fixture
/// carries `(0, 3, 192)`, which before this table showed as
/// `Parameter 0/3/192` with no name and no units.
#[test]
fn a_real_ncep_message_resolves_its_local_parameter() {
    let reader = Grib2Reader::from_bytes(ETA.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.ids.centre, NCEP, "the Eta fixture is not from NCEP");

    let common = msg.pds.common().expect("a horizontal product");
    let resolved = lookup_parameter(
        msg.ids.originator(),
        msg.is.discipline,
        common.parameter_category,
        common.parameter_number,
    )
    .expect("the message's own triple resolves");
    assert_eq!(
        resolved,
        ("MSLET", "MSLP (Eta model reduction)", "Pa"),
        "the fixture carries {}/{}/{}",
        msg.is.discipline,
        common.parameter_category,
        common.parameter_number
    );
}
