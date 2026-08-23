//! Every hand-written GRIB2 code table in `tables.rs`, checked against WMO.
//!
//! `tables_wmo.rs` is generated, so it cannot drift from the standard. The code
//! tables — discipline, production status, grid template, earth shape,
//! statistical process and the rest — are hand-written, because they carry
//! deliberately short labels that WMO's own verbose wording would not fit in a
//! metadata column. Hand-written means unverified, and #415 showed what that
//! costs: three parameters naming the wrong quantity, then six more here, in
//! Tables 1.3 and 4.10, where the codes had been assigned to something else
//! entirely.
//!
//! An accepted divergence records **WMO's exact wording**, not a similarity
//! rule. That is deliberate: what actually went wrong was a code meaning
//! something different from what we thought, and pinning the authority's text
//! catches exactly that — if WMO reassigns a code, the recorded text stops
//! matching and this fails, however plausible our own label still looks.

use fieldglass_grib2::{
    lookup_data_type, lookup_discipline, lookup_earth_shape, lookup_ensemble_type,
    lookup_fixed_surface, lookup_generating_process_type, lookup_grid_template,
    lookup_production_status, lookup_reference_time_significance, lookup_statistical_process,
    lookup_time_range_unit,
};
use serde_json::Value;

const SNAPSHOT: &str = include_str!("fixtures/wmo_code_tables.ref.json");

/// One curated table and the WMO table it transcribes.
struct Table {
    /// WMO table number, as keyed in the snapshot.
    wmo: &'static str,
    /// The function under test, widened to `u16` so Table 3.1 fits.
    lookup: fn(u16) -> &'static str,
    /// Whether the underlying function takes a `u8` (so codes above 255 in the
    /// snapshot are outside what it can be asked about, not a gap).
    octet: bool,
}

fn discipline(c: u16) -> &'static str {
    lookup_discipline(c as u8)
}
fn reference_time_significance(c: u16) -> &'static str {
    lookup_reference_time_significance(c as u8)
}
fn production_status(c: u16) -> &'static str {
    lookup_production_status(c as u8)
}
fn data_type(c: u16) -> &'static str {
    lookup_data_type(c as u8)
}
fn grid_template(c: u16) -> &'static str {
    lookup_grid_template(c)
}
fn earth_shape(c: u16) -> &'static str {
    lookup_earth_shape(c as u8)
}
fn generating_process_type(c: u16) -> &'static str {
    lookup_generating_process_type(c as u8)
}
fn time_range_unit(c: u16) -> &'static str {
    lookup_time_range_unit(c as u8)
}
fn fixed_surface(c: u16) -> &'static str {
    lookup_fixed_surface(c as u8)
}
fn ensemble_type(c: u16) -> &'static str {
    lookup_ensemble_type(c as u8)
}
fn statistical_process(c: u16) -> &'static str {
    lookup_statistical_process(c as u8)
}

const TABLES: &[Table] = &[
    Table {
        wmo: "0.0",
        lookup: discipline,
        octet: true,
    },
    Table {
        wmo: "1.2",
        lookup: reference_time_significance,
        octet: true,
    },
    Table {
        wmo: "1.3",
        lookup: production_status,
        octet: true,
    },
    Table {
        wmo: "1.4",
        lookup: data_type,
        octet: true,
    },
    Table {
        wmo: "3.1",
        lookup: grid_template,
        octet: false,
    },
    Table {
        wmo: "3.2",
        lookup: earth_shape,
        octet: true,
    },
    Table {
        wmo: "4.3",
        lookup: generating_process_type,
        octet: true,
    },
    Table {
        wmo: "4.4",
        lookup: time_range_unit,
        octet: true,
    },
    Table {
        wmo: "4.5",
        lookup: fixed_surface,
        octet: true,
    },
    Table {
        wmo: "4.6",
        lookup: ensemble_type,
        octet: true,
    },
    Table {
        wmo: "4.10",
        lookup: statistical_process,
        octet: true,
    },
];

/// A label we intentionally word differently from WMO, together with **WMO's
/// exact text at the time the divergence was reviewed**. If that text changes,
/// the code has been reassigned and the entry must be looked at again.
const ACCEPTED: &[(&str, u16, &str)] = &[
    // Table 3.1 — grid definition template.
    ("3.1", 100, "Triangular grid based on an icosahedron"),
    // Table 3.2 — earth shape. WMO spells out the full geodetic definition,
    // which does not fit a metadata column; the labels keep the distinguishing
    // radius or datum.
    (
        "3.2",
        0,
        "Earth assumed spherical with radius = 6 367 470.0 m",
    ),
    (
        "3.2",
        1,
        "Earth assumed spherical with radius specified (in m) by data producer",
    ),
    (
        "3.2",
        2,
        "Earth assumed oblate spheroid with size as determined by IAU in 1965 (major axis = 6 378 160.0 m, minor axis = 6 356 775.0 m, f = 1/297.0)",
    ),
    (
        "3.2",
        3,
        "Earth assumed oblate spheroid with major and minor axes specified (in km) by data producer",
    ),
    (
        "3.2",
        4,
        "Earth assumed oblate spheroid as defined in IAG-GRS80 model (major axis = 6 378 137.0 m, minor axis = 6 356 752.314 m, f = 1/298.257 222 101)",
    ),
    (
        "3.2",
        5,
        "Earth assumed represented by WGS-84 (as used by ICAO since 1998)",
    ),
    (
        "3.2",
        6,
        "Earth assumed spherical with radius of 6 371 229.0 m",
    ),
    (
        "3.2",
        7,
        "Earth assumed oblate spheroid with major or minor axes specified (in m) by data producer",
    ),
    (
        "3.2",
        8,
        "Earth model assumed spherical with radius of 6 371 200 m, but the horizontal datum of the resulting latitude/longitude field is the WGS-84 reference frame",
    ),
    (
        "3.2",
        9,
        "Earth represented by the Ordnance Survey Great Britain 1936 Datum, using the Airy 1830 Spheroid, the Greenwich meridian as 0 longitude, and the Newlyn datum as mean sea level, 0 height",
    ),
    // Table 4.5 — fixed surface. Same wording, ours carries the unit.
    ("4.5", 3, "Level of cloud tops"),
    ("4.5", 103, "Specified height level above ground"),
    (
        "4.5",
        108,
        "Level at specified pressure difference from ground to level",
    ),
    // Table 4.10 — statistical process. Ours states the same subtraction more
    // briefly.
    (
        "4.10",
        4,
        "Difference (value at the end of time range minus value at the beginning)",
    ),
    (
        "4.10",
        8,
        "Difference (value at the start of time range minus value at the end)",
    ),
];

/// Labels the lookups return for a code they do not carry. These are gaps, not
/// disagreements, so they are skipped rather than compared.
fn is_not_carried(label: &str) -> bool {
    label.starts_with("Unknown") || label == "Reserved for local use" || label == "Missing"
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether `ours` still visibly describes the same thing as `theirs`.
///
/// A recorded divergence pins the *authority's* wording, which catches a
/// reassigned code — but on its own it says nothing about our side, so a label
/// swapped for something unrelated would sail through. This is the other half:
/// some substantial word of ours must still appear in the authority's text.
/// Deliberately weak, because the accepted labels are heavy rewrites ("Oblate
/// spheroid (WGS84)" for a 60-character geodetic definition); it only has to
/// separate a rewrite from an unrelated name, and every defect found in #415
/// shared no word at all.
fn shares_a_substantial_word(ours: &str, theirs: &str) -> bool {
    let haystack = normalize(theirs);
    ours.split(|c: char| !c.is_ascii_alphanumeric())
        .map(normalize)
        .filter(|w| w.len() >= 5)
        .any(|w| haystack.contains(&w))
}

fn snapshot() -> Value {
    serde_json::from_str(SNAPSHOT).expect("snapshot parses")
}

/// Every curated code-table label either matches WMO or is a reviewed
/// divergence recorded against WMO's exact wording.
#[test]
fn every_curated_code_table_entry_agrees_with_wmo() {
    let doc = snapshot();
    let mut compared = 0usize;
    let mut wrong = Vec::new();

    for table in TABLES {
        let entries = doc["tables"][table.wmo]
            .as_object()
            .unwrap_or_else(|| panic!("table {} missing from the snapshot", table.wmo));
        for (code, expected) in entries {
            let code: u16 = code.parse().expect("numeric code");
            if table.octet && code > 255 {
                continue;
            }
            let expected = expected.as_str().expect("string meaning");
            let ours = (table.lookup)(code);
            if is_not_carried(ours) {
                continue;
            }
            compared += 1;
            let (a, b) = (normalize(ours), normalize(expected));
            if a == b || a.contains(&b) || b.contains(&a) {
                continue;
            }
            match ACCEPTED
                .iter()
                .find(|&&(t, c, _)| t == table.wmo && c == code)
            {
                // The recorded text is the assertion: WMO must still mean what
                // it meant when the divergence was reviewed.
                Some(&(_, _, reviewed)) if reviewed == expected => {
                    if !shares_a_substantial_word(ours, expected) {
                        wrong.push(format!(
                            "  {}/{code}: accepted as a rewrite of WMO {expected:?}, but \
                             ours {ours:?} shares no word with it",
                            table.wmo
                        ));
                    }
                }
                Some(&(_, _, reviewed)) => wrong.push(format!(
                    "  {}/{code}: reviewed against WMO {reviewed:?}, but WMO now says \
                     {expected:?} — the code has been reassigned",
                    table.wmo
                )),
                None => wrong.push(format!(
                    "  {}/{code}: ours {ours:?}, WMO {expected:?}",
                    table.wmo
                )),
            }
        }
    }

    assert!(
        compared > 120,
        "only {compared} code-table entries were compared; the tables are not \
         lining up, so agreement proves nothing"
    );
    assert!(
        wrong.is_empty(),
        "{} of {compared} curated code-table entries disagree with WMO:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Each accepted divergence must still be one. An entry whose wording caught up
/// with WMO would otherwise keep a licence it no longer needs, and the list
/// would slowly become a place a real disagreement could hide.
#[test]
fn every_accepted_divergence_is_still_a_divergence() {
    let doc = snapshot();
    for &(wmo, code, reviewed) in ACCEPTED {
        let table = TABLES
            .iter()
            .find(|t| t.wmo == wmo)
            .unwrap_or_else(|| panic!("{wmo} is not a table under test"));
        let expected = doc["tables"][wmo][code.to_string()]
            .as_str()
            .unwrap_or_else(|| panic!("{wmo}/{code} is not in the snapshot"));
        assert_eq!(
            expected, reviewed,
            "{wmo}/{code}: WMO's wording changed since this divergence was reviewed"
        );
        let ours = (table.lookup)(code);
        let (a, b) = (normalize(ours), normalize(expected));
        assert!(
            a != b && !a.contains(&b) && !b.contains(&a),
            "{wmo}/{code}: ours {ours:?} now matches WMO — drop it from ACCEPTED"
        );
        assert!(
            shares_a_substantial_word(ours, expected),
            "{wmo}/{code}: ours {ours:?} shares no word with WMO {expected:?} — that is \
             not a rewrite, it is a different entry"
        );
    }
}
