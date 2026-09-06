//! Every hand-written GRIB2 code table in `tables.rs`, checked against WMO.
//!
//! `tables_wmo.rs` is generated, so it cannot drift from the standard. The code
//! tables — discipline, production status, grid template, earth shape,
//! statistical process and the rest — are hand-written, because a handful of
//! them carry deliberately short labels that WMO's own verbose wording would
//! not fit in a metadata column. Two are hybrids: `lookup_time_range_unit` and
//! `lookup_fixed_surface` curate the common codes and fall through to the
//! generated module for the rest, so a good share of Table 4.5's entries come
//! from the source that cannot drift. Sweeping those anyway costs nothing and
//! checks the curated arms that shadow it.
//!
//! Hand-written means unverified, and #415 showed what that
//! costs: three parameters naming the wrong quantity, then six more here, in
//! Tables 1.3 and 4.10, where the codes had been assigned to something else
//! entirely.
//!
//! An accepted divergence records **WMO's exact wording**, not a similarity
//! rule. That is deliberate: what actually went wrong was a code meaning
//! something different from what we thought, and pinning the authority's text
//! catches exactly that — if WMO reassigns a code, the recorded text stops
//! matching and this fails, however plausible our own label still looks.
//!
//! The other half of the gate is that a code we do **not** name has to be
//! declared. Comparing only the codes our lookups answer for makes a code WMO
//! assigns and we never noticed neither a pass nor a failure — it is skipped,
//! and nothing reports it. That hid 42 assigned codes, one of which
//! (discipline 191, "Computational parameters") was found by a check on the
//! fetch planner's reverse index instead, because this file was structurally
//! unable to find it (#653). So [`every_code_wmo_assigns_is_named`] holds the
//! set of unnamed codes equal to [`DELIBERATELY_UNNAMED`], and a code WMO adds
//! later fails here until someone either names it or records why not.

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
    (
        "3.2",
        11,
        "Sun assumed spherical with radius = 695 990 000 m (Allen, C.W., Astrophysical Quantities, 3rd ed.; Athlone: London, 1976) and Stonyhurst latitude and longitude system with origin at the intersection of the solar central meridian (as seen from Earth) and the solar equator (Thompson, W., Coordinate systems for solar image data, Astron. Astrophys. 2006, 449, 791-803)",
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

/// Codes WMO assigns that our lookups deliberately do not name, each with the
/// reason. A code that our lookups answer `Unknown…` for and that is **not**
/// listed here fails [`every_code_wmo_assigns_is_named`].
///
/// Empty is the intended state, and is what #653 left behind: every code the
/// eleven tables assign is named. It is not empty because there is nothing to
/// say — it is empty because each of the 42 that used to sit here silently was
/// looked at and named. An entry is the escape hatch for a code that genuinely
/// should stay unnamed; write down which and why, so the next reader sees a
/// decision rather than an oversight.
const DELIBERATELY_UNNAMED: &[(&str, u16, &str)] = &[];

/// Whether the lookup answered with no name at all.
///
/// The `Unknown…` fallback arms, and only those. `"Missing"` and `"Reserved
/// for local use"` are *names* — WMO's own, for the missing sentinel and the
/// local range — so a lookup returning one has carried the code and is
/// compared like any other. (They were once treated as gaps here, which is
/// what let the nine `255 => "Missing"` arms that existed go unchecked — and
/// hid that Tables 3.1 and 3.2 had no missing-sentinel arm at all: #653.)
fn lookup_has_no_name(label: &str) -> bool {
    label.starts_with("Unknown")
}

/// Whether WMO's own text assigns the code no meaning to name.
///
/// In practice that is `Reserved`, and only `Reserved`: `Missing` is a meaning
/// — every lookup here names it — and a code whose text is anything else is
/// something WMO has assigned. `Reserved for local use` matches the same
/// prefix but never reaches the snapshot, because WMO publishes the local
/// range as a single row (`192-254`) and
/// `tools/gen_wmo_code_table_snapshot.py` keeps only rows whose code is one
/// number.
///
/// When WMO later assigns one of these, the snapshot's text changes and the
/// code stops being exempt, which is exactly the notification this gate exists
/// to give.
fn wmo_assigns_no_meaning(wmo: &str) -> bool {
    wmo.starts_with("Reserved")
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
            // The last silent skip this file had. `every_code_wmo_assigns_is_named`
            // proves the branch is unreachable — only 3.1 carries codes above
            // 255 and its lookup takes a `u16` — so reaching it means WMO
            // widened a table, and dropping four comparisons quietly is how a
            // gate stops measuring what it says it measures.
            assert!(
                !(table.octet && code > 255),
                "table {}: code {code} is above what its `u8` lookup can be asked \
                 about — widen the lookup rather than skipping the code",
                table.wmo
            );
            let expected = expected.as_str().expect("string meaning");
            let ours = (table.lookup)(code);
            if lookup_has_no_name(ours) {
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

    // A floor, raised from 120 to the number #653's naming pass left behind
    // minus a little slack. The point is not the exact count — it is that a
    // walk which silently lined nothing up cannot report agreement.
    assert!(
        compared > 240,
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

/// Every code WMO assigns is named, or is recorded on [`DELIBERATELY_UNNAMED`]
/// with a reason.
///
/// [`every_curated_code_table_entry_agrees_with_wmo`] compares only the codes
/// our lookups answer for, which is the right rule for *that* question and the
/// wrong one for this: a code WMO assigns and we never noticed is skipped
/// there, so it is neither a pass nor a failure and nothing counts it. This is
/// the counting. The set of unnamed codes is held **equal** to the recorded
/// set, so it fails in both directions — a new WMO assignment we have not
/// named, and an entry on the list that has since been named and should come
/// off it.
#[test]
fn every_code_wmo_assigns_is_named() {
    let doc = snapshot();
    // A table the snapshot carries and `TABLES` omits would be skipped whole,
    // with nothing to report it — the same defect one level up. The per-table
    // `panic!` below covers only the other direction.
    let mut in_snapshot: Vec<&str> = doc["tables"]
        .as_object()
        .expect("snapshot has a tables section")
        .keys()
        .map(String::as_str)
        .collect();
    let mut under_test: Vec<&str> = TABLES.iter().map(|t| t.wmo).collect();
    in_snapshot.sort_unstable();
    under_test.sort_unstable();
    assert_eq!(
        under_test, in_snapshot,
        "the tables under test and the tables in the snapshot are not the same set"
    );
    let mut unnamed: Vec<(&str, u16, &str)> = Vec::new();
    let mut report = Vec::new();
    let (mut total_assigned, mut total_unassigned) = (0usize, 0usize);

    for table in TABLES {
        let entries = doc["tables"][table.wmo]
            .as_object()
            .unwrap_or_else(|| panic!("table {} missing from the snapshot", table.wmo));
        let (mut assigned, mut named, mut unassigned) = (0usize, 0usize, 0usize);
        let mut out_of_range = 0usize;
        for (code, expected) in entries {
            let code: u16 = code.parse().expect("numeric code");
            // Outside what a `u8` lookup can be asked about. Counted, and
            // asserted zero below: it is the one skip left in this walk, and a
            // skip nothing asserts on is the defect this test exists for. No
            // table reaches it today — only 3.1 carries codes above 255 and its
            // lookup takes a `u16` — so a non-zero count means WMO widened a
            // table and the lookup's argument needs widening with it.
            if table.octet && code > 255 {
                out_of_range += 1;
                continue;
            }
            let expected = expected.as_str().expect("string meaning");
            if wmo_assigns_no_meaning(expected) {
                unassigned += 1;
                continue;
            }
            assigned += 1;
            if lookup_has_no_name((table.lookup)(code)) {
                unnamed.push((table.wmo, code, expected));
            } else {
                named += 1;
            }
        }
        // The skips, reported rather than swallowed. Visible with
        // `cargo test -p fieldglass-grib2 --test wmo_code_tables -- --nocapture`.
        report.push(format!(
            "  {:>4}: {named}/{assigned} assigned codes named, {unassigned} reserved, \
             {out_of_range} outside the lookup's width",
            table.wmo
        ));
        assert!(
            named > 0,
            "table {} named none of its {assigned} assigned codes — the lookup is not \
             wired to the table it is being checked against",
            table.wmo
        );
        assert_eq!(
            out_of_range, 0,
            "table {} carries {out_of_range} code(s) above 255 that its `u8` lookup \
             cannot be asked about — widen the lookup rather than leaving them \
             permanently unreachable",
            table.wmo
        );
        total_assigned += assigned;
        total_unassigned += unassigned;
    }

    println!(
        "WMO code-table coverage ({} assigned, {total_unassigned} reserved):\n{}",
        total_assigned,
        report.join("\n")
    );
    // Same floor, and the same reason, as the sibling test's: a walk that lined
    // nothing up must not be able to report full coverage.
    assert!(
        total_assigned > 240,
        "only {total_assigned} assigned codes were found in the snapshot; it is not \
         loading, so coverage proves nothing"
    );

    let missing = unrecorded(&unnamed, DELIBERATELY_UNNAMED);
    assert!(
        missing.is_empty(),
        "{} code(s) WMO assigns are neither named nor recorded on \
         DELIBERATELY_UNNAMED:\n{}",
        missing.len(),
        missing.join("\n")
    );

    let stale = stale_records(&unnamed, DELIBERATELY_UNNAMED);
    assert!(
        stale.is_empty(),
        "{} DELIBERATELY_UNNAMED entr(y/ies) no longer describe an unnamed assigned \
         code — the code is named now, or WMO stopped assigning it, so drop the \
         entry rather than leaving a licence nothing uses:\n{}",
        stale.len(),
        stale.join("\n")
    );
}

/// Unnamed codes that no entry on `recorded` accounts for.
///
/// Factored out of the test, together with its twin below, so both directions
/// of the equality can be exercised on a synthetic list. `DELIBERATELY_UNNAMED`
/// is empty in this tree — which is the point of the naming pass — and an empty
/// list leaves both of these unreached, so the allowlist mechanism would
/// otherwise ship with no coverage at all.
fn unrecorded(unnamed: &[(&str, u16, &str)], recorded: &[(&str, u16, &str)]) -> Vec<String> {
    unnamed
        .iter()
        .filter(|(t, c, _)| !recorded.iter().any(|&(rt, rc, _)| rt == *t && rc == *c))
        .map(|(t, c, wmo)| format!("  {t}/{c}: WMO assigns {wmo:?}, we answer nothing"))
        .collect()
}

/// Entries on `recorded` that no longer describe an unnamed code.
fn stale_records(unnamed: &[(&str, u16, &str)], recorded: &[(&str, u16, &str)]) -> Vec<String> {
    recorded
        .iter()
        .filter(|(t, c, _)| !unnamed.iter().any(|&(ut, uc, _)| ut == *t && uc == *c))
        .map(|(t, c, why)| format!("  {t}/{c}: recorded as unnamed ({why})"))
        .collect()
}

/// The allowlist mechanism itself, on a synthetic list — the coverage an empty
/// `DELIBERATELY_UNNAMED` cannot give it.
#[test]
fn the_unnamed_allowlist_fails_in_both_directions() {
    let unnamed = [(
        "4.6",
        9u16,
        "Initial conditions and model physics perturbations",
    )];
    let recorded = [("4.6", 9u16, "a recorded reason")];

    // The matched pair: neither direction complains.
    assert!(unrecorded(&unnamed, &recorded).is_empty());
    assert!(stale_records(&unnamed, &recorded).is_empty());
    // An unnamed code nothing records.
    assert_eq!(unrecorded(&unnamed, &[]).len(), 1);
    // A record naming a code that is not unnamed.
    assert_eq!(stale_records(&unnamed, &[("4.6", 8, "why")]).len(), 1);
    // The table is part of the key, not just the code.
    assert_eq!(unrecorded(&unnamed, &[("4.3", 9, "why")]).len(), 1);
    assert_eq!(stale_records(&unnamed, &[("4.3", 9, "why")]).len(), 1);
}

/// No two codes in one table share a label.
///
/// The wording check accepts a label that is a substring of WMO's, which is
/// what lets `TIGGE` stand for `THORPEX Interactive Grand Global Ensemble
/// (TIGGE)`. The cost is that an over-broad label also passes: `Mercator` for
/// §3.13, "Mercator with modelling subdomains definition", is a substring of
/// WMO's text and would sail through — while colliding with §3.10, which is
/// plainly *the* Mercator. Two codes reading the same in a metadata column is
/// the damaging half of that, so it is checked directly.
///
/// Uniqueness is a property of our own table, not of WMO's, so this sweeps the
/// lookup's whole argument range rather than the snapshot's key set. Not
/// tidiness: Tables 4.4 and 4.5 answer most codes out of the generated
/// `tables_wmo` module, whose entries a snapshot walk would never visit, and a
/// collision between a curated arm and a generated one reads exactly as badly
/// as any other.
#[test]
fn no_two_codes_in_a_table_share_a_label() {
    for table in TABLES {
        let highest: u32 = if table.octet {
            u8::MAX as u32
        } else {
            u16::MAX as u32
        };
        let mut seen: Vec<(u16, &'static str)> = Vec::new();
        for code in 0..=highest {
            let code = code as u16;
            let ours = (table.lookup)(code);
            // The three catch-all answers are shared by construction.
            if lookup_has_no_name(ours) || ours == "Missing" || ours == "Reserved for local use" {
                continue;
            }
            if let Some((first, _)) = seen.iter().find(|(_, label)| *label == ours) {
                panic!(
                    "{}/{first} and {}/{code} both read {ours:?} — a reader cannot tell \
                     them apart",
                    table.wmo, table.wmo
                );
            }
            seen.push((code, ours));
        }
        assert!(
            seen.len() > 1,
            "table {} contributed {} label(s), so uniqueness proves nothing",
            table.wmo,
            seen.len()
        );
    }
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
