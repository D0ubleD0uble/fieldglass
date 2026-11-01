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
//!   34 (JMA) and 98 (ECMWF), whose local tables add codes, and 78 (DWD),
//!   whose local Table 5 does.
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
const CENTRES: [u8; 5] = [74, CENTRE_NCEP, 34, 98, 78];

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
    // five centres name 238 codes between them (39 at centre 74 and at DWD,
    // which has no local Table 3, 77 at NCEP, 42 at JMA and 41 at ECMWF).
    assert!(named > 230, "only {named} named level types were compared");
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

/// What `forecast_display` shows for each Table 5 code eccodes names, at
/// P1 = 6 and P2 = 18 hours, keyed by which centre's table holds the code.
///
/// The P1 and P2 are distinct and do not divide each other, so a display that
/// swaps them, drops one or uses the wrong interval reads differently.
fn expected_time_range(centre: u8, code: u8) -> Option<&'static str> {
    const ECMWF: u8 = 98;
    const JMA: u8 = 34;
    const DWD: u8 = 78;
    Some(match (code, centre) {
        (0, _) => "+6h",
        (1, _) => "analysis",
        (2, _) => "6–18h valid",
        (3, _) => "6–18h average",
        (4, _) => "6–18h accum",
        (5, _) => "6–18h diff",
        (6, _) => "−6h to −18h average",
        (7, _) => "−6h to +18h average",
        // 10 reads octets 19-20 as one P1: 6 × 256 + 18.
        (10, _) => "+1554h",
        (51, _) => "climatological mean",
        (113, _) => "average of +6h forecasts issued every 18h",
        (114, _) => "accum of +6h forecasts issued every 18h",
        (115, _) => "average of +6h, +24h, … forecasts of one run",
        (116, _) => "accum of +6h, +24h, … forecasts of one run",
        (117, _) => "average of forecasts valid at +6h, issued every 18h",
        (118, _) => "variance of initialized analyses issued every 18h",
        (119, _) => "std dev of +6h, +24h, … forecasts of one run",
        (123, _) => "average of analyses every 18h",
        (124, _) => "accum of analyses every 18h",
        (125, _) => "std dev of the tendency of +6h, +24h, … forecasts of one run",
        (11, DWD) => "+6h initialized (IDFI)",
        (13, DWD) => "analysis",
        (14, DWD) => "+6h (IFS on GME grid)",
        (128, ECMWF | JMA) => "average of 6–18h products issued every 24h",
        (130, ECMWF | JMA) => "average of 6–18h products issued every 12h",
        (133, ECMWF) => "average of products every 6h from +6h to +18h",
        (129, JMA) => "variance of 6–18h products issued every 24h",
        (131, JMA) => "variance of 6–18h products issued every 12h",
        (132, JMA) => "variance of +6h forecasts issued every 18h",
        _ => return None,
    })
}

/// The word a display must carry for what eccodes' title says the product
/// is, so an expected string above cannot drift from the oracle unnoticed.
fn kind_word(title: &str) -> Option<&'static str> {
    let t = title.to_lowercase();
    [
        ("standard deviation", "std dev"),
        ("variance", "variance"),
        ("average", "average"),
        ("accumulation", "accum"),
        ("difference", "diff"),
        ("climatological", "climatological"),
        ("initialized forecast", "initialized"),
        ("analys", "analys"),
        ("valid time ranging", "valid"),
    ]
    .into_iter()
    .find(|(theirs, _)| t.contains(theirs))
    .map(|(_, ours)| ours)
}

/// The committed `j_consecutive_latlon.grib1` with its PDS restamped and read
/// back through the reader, so the test goes through the parse a file takes.
fn stamped(centre: u8, time_range: u8, p1: u8, p2: u8) -> ProductDefinition {
    // Section 0 is 8 octets, so PDS octet n sits at 8 + n - 1.
    let mut bytes = include_bytes!("fixtures/j_consecutive_latlon.grib1").to_vec();
    bytes[8 + 5 - 1] = centre;
    bytes[8 + 19 - 1] = p1;
    bytes[8 + 20 - 1] = p2;
    bytes[8 + 21 - 1] = time_range;
    let reader = fieldglass_grib1::Grib1Reader::from_bytes(bytes).expect("reader");
    let pds = reader.messages.first().expect("one message").pds;
    assert_eq!(pds.time_unit, 1, "the fixture counts in hours");
    pds
}

#[test]
fn every_time_range_eccodes_names_is_shown_as_what_it_describes() {
    // Table 5 is read from `grib1/local/<centre>/5.table` before the master,
    // as Table 3 is, so ECMWF, JMA and DWD each add codes. Both directions
    // are held at every centre: a code eccodes names must get its own display
    // (not the unknown form), and a code it does not name must not get one.
    // Before #876 every code past 51 printed `+P1`, so a 10-day mean read as a
    // plain forecast.
    let mut wrong = Vec::new();
    let mut named = 0usize;
    for centre in CENTRES {
        let theirs = eccodes(centre, "5");
        for code in 0..=u8::MAX {
            let pds = stamped(centre, code, 6, 18);
            let shown = forecast_display(&pds);
            let unknown = format!("time range {code}, P1 6h, P2 18h");
            match (theirs.get(&code), expected_time_range(centre, code)) {
                (None, None) => {
                    if shown != unknown {
                        wrong.push(format!(
                            "  centre {centre} code {code}: eccodes names nothing, shows {shown:?}"
                        ));
                    }
                    if fieldglass_grib1::forecast_hours(&pds).is_some() {
                        wrong.push(format!(
                            "  centre {centre} code {code}: unnamed, but has a lead in hours"
                        ));
                    }
                }
                (Some(title), Some(want)) => {
                    named += 1;
                    if shown != want {
                        wrong.push(format!(
                            "  centre {centre} code {code}: shows {shown:?}, expected {want:?}"
                        ));
                    }
                    if let Some(word) = kind_word(title)
                        && !shown.contains(word)
                    {
                        wrong.push(format!(
                            "  centre {centre} code {code}: {shown:?} lacks {word:?} for {title:?}"
                        ));
                    }
                }
                (title, want) => wrong.push(format!(
                    "  centre {centre} code {code}: eccodes {title:?}, expected here {want:?}"
                )),
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} time range(s) disagree with eccodes:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // A count, so a walk that lined nothing up cannot report agreement: 20 at
    // centres 74 and 7, 25 at JMA, 23 at ECMWF and DWD.
    assert_eq!(named, 111, "named time ranges compared");
}

#[test]
fn time_ranges_over_analyses_say_analyses_when_p1_is_zero() {
    // 113, 114 and JMA's 132 cover analyses as well as forecasts: WMO's text
    // says P1 = 0 for the analyses.
    let at = |centre, code| forecast_display(&stamped(centre, code, 0, 12));
    assert_eq!(
        at(74, 113),
        "average of initialized analyses issued every 12h"
    );
    assert_eq!(
        at(74, 114),
        "accum of initialized analyses issued every 12h"
    );
    assert_eq!(at(34, 132), "variance of analyses issued every 12h");
}

#[test]
fn a_local_time_range_is_read_only_at_its_own_centre() {
    // 133 is ECMWF's, 129 JMA's and 11 DWD's. The same code from another
    // centre is not theirs to read, so it shows as an unknown code.
    assert_eq!(
        forecast_display(&stamped(34, 133, 6, 18)),
        "time range 133, P1 6h, P2 18h"
    );
    assert_eq!(
        forecast_display(&stamped(98, 129, 6, 18)),
        "time range 129, P1 6h, P2 18h"
    );
    assert_eq!(
        forecast_display(&stamped(74, 11, 6, 18)),
        "time range 11, P1 6h, P2 18h"
    );
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
