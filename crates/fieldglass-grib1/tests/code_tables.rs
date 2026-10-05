//! GRIB1 Code Tables 3, 4 and 5, held to the authorities that define them (#869).
//!
//! `lookup_level_type` (Table 3) was hand-written one code off for 0-9: it
//! called 1 "Cloud base level", 2 "Cloud top level" and so on, so every surface
//! field — 2 m temperature, mean sea level pressure, precipitation — read as a
//! cloud base. NCEP's local types were shifted too. Nothing compared the table
//! to anything, so nothing noticed.
//!
//! The oracles are two committed snapshots, both written by
//! `tools/gen_grib1_code_table_snapshot.py`:
//!
//! * `code_tables.eccodes.ref.json` — what eccodes 2.34.1 *decodes* for every
//!   code 0-255, from messages stamped with centres 74 (no local tables, so the
//!   WMO master table), 7 (NCEP, which eccodes also reads from the master),
//!   34 (JMA) and 98 (ECMWF), whose local tables add codes.
//! * `code_tables.on388.ref.json` — NCEP's ON388 Table 3, the only source for
//!   NCEP's local level types, which eccodes 2.34.1 does not carry.
//!
//! Three things are held, each in both directions:
//!
//! 1. **Which codes have a name.** A code the authority names and we do not,
//!    or the reverse, fails — a one-directional check skips exactly the codes
//!    nobody noticed.
//! 2. **What the name says.** Every substantial word of ours (three letters or
//!    more) and every number we state must appear in the authority's text for
//!    *that* code. Three letters is deliberate: the old code 2, "Cloud top
//!    level", shares "cloud" and "level" with the authority's "Cloud base
//!    level", and only "top" tells them apart.
//! 3. **How the value octets read** (Table 3 only). Whether octets 11-12 carry
//!    nothing, one value, or two layer bounds, against ON388's contents column
//!    and eccodes' own list of layer types.

use std::collections::{BTreeMap, BTreeSet};

use fieldglass_grib1::tables::{lookup_level_type, lookup_time_unit};
use fieldglass_grib1::{ProductDefinition, forecast_display, level_value_str};
use serde_json::Value;

const ECCODES: &str = include_str!("fixtures/code_tables.eccodes.ref.json");
const ON388: &str = include_str!("fixtures/code_tables.on388.ref.json");

const CENTRE_NCEP: u8 = 7;
/// The centres the eccodes snapshot was stamped with.
const CENTRES: [u8; 4] = [74, CENTRE_NCEP, 34, 98];

/// eccodes' decoded names for one table at one centre, code → title.
fn eccodes(centre: u8, table: &str) -> BTreeMap<u8, String> {
    let snapshot: Value = serde_json::from_str(ECCODES).expect("eccodes snapshot parses");
    assert_eq!(
        snapshot["eccodes"], "2.34.1",
        "the snapshot is from the pin"
    );
    snapshot["centres"][centre.to_string()][table]
        .as_object()
        .unwrap_or_else(|| panic!("centre {centre} table {table} in the snapshot"))
        .iter()
        .map(|(code, title)| {
            (
                code.parse().expect("code"),
                title.as_str().expect("title").to_string(),
            )
        })
        .collect()
}

/// ON388 Table 3: code → (meaning, octet-contents cells).
fn on388() -> BTreeMap<u8, (String, Vec<String>)> {
    let snapshot: Value = serde_json::from_str(ON388).expect("ON388 snapshot parses");
    snapshot["3"]
        .as_object()
        .expect("table 3")
        .iter()
        .map(|(code, row)| {
            let contents = row["contents"]
                .as_array()
                .expect("contents")
                .iter()
                .map(|c| c.as_str().expect("cell").to_string())
                .collect();
            (
                code.parse().expect("code"),
                (
                    row["meaning"].as_str().expect("meaning").to_string(),
                    contents,
                ),
            )
        })
        .collect()
}

/// What the authority calls a level type at a centre, or nothing where it
/// names nothing. A "Reserved" row is code space, not a name.
///
/// NCEP reads ON388, falling back to eccodes for the WMO code ON388 leaves out
/// (255, missing). Every other centre reads eccodes, which applies its local
/// table where it ships one. ON388 and eccodes both transcribe WMO's table, so
/// for a WMO code either wording is accepted at any centre;
/// [`the_two_authorities_assign_the_same_wmo_codes`] holds them to agreeing on
/// which codes those are. 210 is not one: eccodes' meaning is an ECMWF
/// extension, and NCEP's is its own.
fn level_authorities(code: u8, centre: u8) -> Vec<String> {
    let on388 = on388().remove(&code).map(|(meaning, _)| meaning);
    let eccodes = eccodes(centre, "3").remove(&code);
    let wmo = code <= 201;
    let mut texts: Vec<String> = if centre == CENTRE_NCEP {
        match on388 {
            Some(meaning) => std::iter::once(meaning)
                .chain(eccodes.filter(|_| wmo))
                .collect(),
            None => eccodes.into_iter().collect(),
        }
    } else {
        match eccodes {
            Some(title) => std::iter::once(title)
                .chain(on388.filter(|_| wmo))
                .collect(),
            None => Vec::new(),
        }
    };
    texts.retain(|t| !t.eq_ignore_ascii_case("reserved"));
    texts
}

/// Lower-case words, split on anything that is not a letter or digit.
fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// A word with a plural `s` taken off, so "heights" meets "height".
fn stem(w: &str) -> &str {
    w.strip_suffix('s').filter(|s| s.len() >= 3).unwrap_or(w)
}

/// Whether our label still says what the authority's text says: every
/// substantial word of ours is one of theirs, and every number of ours is a
/// number of theirs.
fn says_what_it_says(ours: &str, theirs: &str) -> bool {
    const STOP: [&str; 2] = ["the", "and"];
    let their_words: BTreeSet<String> = words(theirs).iter().map(|w| stem(w).to_string()).collect();
    let numbers = |s: &str| -> BTreeSet<String> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect()
    };
    let words_hold = words(ours)
        .iter()
        .filter(|w| {
            w.len() >= 3 && !STOP.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit())
        })
        .all(|w| their_words.contains(stem(w)));
    words_hold && numbers(ours).is_subset(&numbers(theirs))
}

#[test]
fn every_level_type_names_what_its_authority_names() {
    let mut wrong = Vec::new();
    let mut named = 0usize;
    for centre in CENTRES {
        for code in 0..=u8::MAX {
            let theirs = level_authorities(code, centre);
            match (lookup_level_type(code, centre), theirs.is_empty()) {
                (None, true) => {}
                (Some(ours), true) => wrong.push(format!(
                    "  centre {centre} code {code}: ours {ours:?}, the authority names nothing"
                )),
                (None, false) => wrong.push(format!(
                    "  centre {centre} code {code}: ours names nothing, the authority {theirs:?}"
                )),
                (Some(ours), false) => {
                    named += 1;
                    if !theirs.iter().any(|t| says_what_it_says(ours, t)) {
                        wrong.push(format!(
                            "  centre {centre} code {code}: ours {ours:?}, the authority {theirs:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} level type(s) disagree with their authority:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // A floor, so a walk that lined nothing up cannot report agreement: the
    // four centres name 199 codes between them (39 at centre 74, 77 at NCEP,
    // 42 at JMA and 41 at ECMWF).
    assert!(named > 190, "only {named} named level types were compared");
}

#[test]
fn the_two_authorities_assign_the_same_wmo_codes() {
    // Two independent transcriptions of one WMO table. Over the WMO range they
    // should assign exactly the same codes; a difference is a parse error in
    // one snapshot, or a code one authority added. Reserved code 0 is
    // eccodes-only, and 126 (isobaric level in Pa) is ON388's own addition:
    // no WMO table and no eccodes table carries it.
    let wmo: BTreeSet<u8> = eccodes(74, "3")
        .into_iter()
        .filter(|(code, title)| *code <= 201 && title != "Reserved")
        .map(|(code, _)| code)
        .collect();
    let ncep: BTreeSet<u8> = on388().into_keys().filter(|&code| code <= 201).collect();
    let only_on388: Vec<u8> = ncep.difference(&wmo).copied().collect();
    let only_eccodes: Vec<u8> = wmo.difference(&ncep).copied().collect();
    assert_eq!((only_on388, only_eccodes), (vec![126], vec![]));
    assert!(wmo.len() > 35, "only {} WMO codes lined up", wmo.len());
}

#[test]
fn eccodes_still_reads_ncep_from_the_master_table() {
    // NCEP's local level types come from ON388 because eccodes 2.34.1 has no
    // `grib1/local/kwbc/3.table`, so it decodes a centre-7 message exactly as
    // it does a centre-74 one. If a later eccodes adds that table, this fails,
    // and NCEP's arms should be held to it as well.
    assert_eq!(eccodes(CENTRE_NCEP, "3"), eccodes(74, "3"));
}

/// How a level type's octets 11-12 read, as `level_value_str` shows it.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Reads {
    Nothing,
    OneValue,
    Layer,
}

fn reads(code: u8, centre: u8) -> Reads {
    let pds = ProductDefinition {
        level_type: code,
        originating_centre: centre,
        // Nonzero, so a type with no value is shown to ignore them.
        level_value_1: 3,
        level_value_2: 7,
        ..pds()
    };
    match level_value_str(&pds) {
        None => Reads::Nothing,
        Some(s) if s.contains(" – ") => Reads::Layer,
        Some(_) => Reads::OneValue,
    }
}

#[test]
fn level_values_read_as_on388s_contents_column_says() {
    // ON388 Table 3 states the contents of octets 11 and 12 for every code:
    // "0" for none, one cell for a value, one per octet for a layer. Table 3a
    // (codes below 100, and NCEP's special levels) has no contents column, so
    // its levels carry no value except where the meaning says so in words.
    let in_words: BTreeMap<u8, Reads> = BTreeMap::from([
        // "Isothermal level (temperature in 1/100 K in octets 11 and 12)"
        (20, Reads::OneValue),
        // "Ocean Isotherm Level (1/10 deg C)"
        (235, Reads::OneValue),
        // "Layer between two depths below ocean surface depth of upper
        // surface (dam) (octet 11) depth of lower surface (dam) (octet 12)"
        (236, Reads::Layer),
        // "Ordered Sequence of Data": the value is the position in it.
        (241, Reads::OneValue),
    ]);
    let mut wrong = Vec::new();
    for (code, (meaning, contents)) in on388() {
        let expected = match contents.as_slice() {
            [] => in_words.get(&code).copied().unwrap_or(Reads::Nothing),
            cells if cells.iter().all(|c| c.starts_with('0')) => Reads::Nothing,
            [_] => Reads::OneValue,
            [_, _] => Reads::Layer,
            other => panic!("code {code}: {} contents cells", other.len()),
        };
        let ours = reads(code, CENTRE_NCEP);
        if ours != expected {
            wrong.push(format!(
                "  {code} {meaning:?} {contents:?}: reads {ours:?}, expected {expected:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn layer_types_are_eccodes_layer_types() {
    // eccodes' `grib1/section.1.def` reads octets 11 and 12 as `topLevel` and
    // `bottomLevel` for exactly these codes, and as one 16-bit `level`
    // otherwise. Held at every centre eccodes reads from its own tables.
    const ECCODES_LAYERS: [u8; 12] = [101, 104, 106, 108, 110, 112, 114, 116, 120, 121, 128, 141];
    for centre in [74, 34, 98] {
        let layers: Vec<u8> = (0..=u8::MAX)
            .filter(|&code| reads(code, centre) == Reads::Layer)
            .collect();
        assert_eq!(layers, ECCODES_LAYERS, "centre {centre}");
    }
}

#[test]
fn every_time_unit_names_what_eccodes_names() {
    // Table 4 has no local versions: `section.1.def` reads `grib1/4.table`
    // for every centre.
    for centre in CENTRES {
        assert_eq!(eccodes(centre, "4"), eccodes(74, "4"), "centre {centre}");
    }
    let theirs = eccodes(74, "4");
    let mut wrong = Vec::new();
    for code in 0..=u8::MAX {
        match (lookup_time_unit(code), theirs.get(&code)) {
            (None, None) => {}
            (Some(ours), Some(t)) if says_what_it_says(ours, t) => {}
            (ours, t) => wrong.push(format!("  {code}: ours {ours:?}, eccodes {t:?}")),
        }
    }
    assert!(
        theirs.len() > 10,
        "only {} time units lined up",
        theirs.len()
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn time_range_indicators_are_read_as_eccodes_defines_them() {
    // `forecast_display` branches on Table 5 without a name table, so what is
    // held is the branch: each code it treats specially, at every centre, is
    // the entry eccodes decodes, and the label says what the entry says.
    // Codes it does not branch on (8-9, 11-50, 113 and up, and the local
    // codes ECMWF, DWD and JMA add) print `+P1`.
    let cases: [(u8, &str, &str); 9] = [
        (1, "Initialized analysis product", "analysis"),
        (
            2,
            "valid time ranging between reference time + P1 and reference time + P2",
            "valid",
        ),
        (
            3,
            "Average (reference time + P1 to reference time + P2)",
            "average",
        ),
        (
            4,
            "Accumulation (reference time + P1 to reference time + P2)",
            "accum",
        ),
        (
            5,
            "Difference (reference time + P2 minus reference time + P1)",
            "diff",
        ),
        (
            6,
            "Average (reference time - P1 to reference time - P2)",
            "−1h to −2h average",
        ),
        (
            7,
            "Average (reference time - P1 to reference time + P2)",
            "−1h to +2h average",
        ),
        (10, "P1 occupies octets 19 and 20", "+258h"),
        (51, "Climatological Mean Value", "climatological mean"),
    ];
    for centre in CENTRES {
        let theirs = eccodes(centre, "5");
        for (code, entry, label) in cases {
            let title = theirs
                .get(&code)
                .unwrap_or_else(|| panic!("centre {centre}: no code {code}"));
            assert!(
                title.contains(entry),
                "centre {centre} code {code}: eccodes {title:?}"
            );
            let pds = ProductDefinition {
                time_range: code,
                originating_centre: centre,
                p1: 1,
                p2: 2,
                ..pds()
            };
            let shown = forecast_display(&pds);
            assert!(
                shown.contains(label),
                "code {code}: shows {shown:?}, expected {label:?}"
            );
        }
    }
}

#[test]
fn a_surface_field_reads_as_the_surface() {
    // ECMWF table 128 parameter 167, 2 m temperature, at level type 1. Its own
    // `.eccodes.ref.json` records `indicatorOfTypeOfLevel` 1.
    // Embedded, not read from disk: the wasm32-wasip1 job runs this under
    // wasmtime, which sees no absolute paths.
    let bytes = include_bytes!("fixtures/j_consecutive_latlon.grib1");
    let reader = fieldglass_grib1::Grib1Reader::from_bytes(bytes.to_vec()).expect("reader");
    let msg = reader.messages.first().expect("one message");
    assert_eq!(msg.pds.level_type, 1);
    assert_eq!(
        fieldglass_grib1::level_type_str(&msg.pds),
        "Ground or water surface"
    );
    assert_eq!(level_value_str(&msg.pds), None);
}

#[test]
fn ncep_243_is_the_convective_cloud_top_with_no_level_value() {
    let at = |centre| ProductDefinition {
        level_type: 243,
        originating_centre: centre,
        level_value_1: 3,
        level_value_2: 7,
        ..pds()
    };
    assert_eq!(
        fieldglass_grib1::level_type_str(&at(CENTRE_NCEP)),
        "Convective cloud top level"
    );
    assert_eq!(level_value_str(&at(CENTRE_NCEP)), None);
    // NCEP's local code means nothing from another centre.
    assert_eq!(fieldglass_grib1::level_type_str(&at(98)), "Level type 243");
}

#[test]
fn level_type_210_is_ncep_cloud_top_or_wmo_isobaric_pascals() {
    // The one code where NCEP's local table and eccodes' master table (an
    // ECMWF extension it applies to every centre) disagree.
    let at = |centre| ProductDefinition {
        level_type: 210,
        originating_centre: centre,
        level_value_1: 0x03,
        level_value_2: 0xe8,
        ..pds()
    };
    assert_eq!(
        fieldglass_grib1::level_type_str(&at(98)),
        "(Pa) Isobaric surface"
    );
    assert_eq!(level_value_str(&at(98)).as_deref(), Some("1000"));
    assert_eq!(
        fieldglass_grib1::level_type_str(&at(CENTRE_NCEP)),
        "Boundary layer cloud top level"
    );
    assert_eq!(level_value_str(&at(CENTRE_NCEP)), None);
}

fn pds() -> ProductDefinition {
    ProductDefinition {
        section_len: 28,
        table_version: 2,
        originating_centre: 74,
        generating_process: 0,
        grid_number: 255,
        has_gds: true,
        has_bms: false,
        parameter_id: 11,
        level_type: 1,
        level_value_1: 0,
        level_value_2: 0,
        reference_year: 24,
        reference_month: 1,
        reference_day: 1,
        reference_hour: 0,
        reference_minute: 0,
        time_unit: 1,
        p1: 0,
        p2: 0,
        time_range: 0,
        century: 21,
        sub_centre: 0,
        decimal_scale_factor: 0,
    }
}
