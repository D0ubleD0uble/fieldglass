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
//!
//! Two more blind spots closed in #655, both about what a gate cannot see
//! rather than what it compares:
//!
//! * **A swap.** Exchanging two labels inside one table left every test green:
//!   the strings all still exist, are still distinct, and WMO's recorded
//!   wording still matches, so only the rule on *our* side could catch it and
//!   it looked only for one shared word. [`wording_survives`] now also holds
//!   our numbers to WMO's and our word order to WMO's, and
//!   [`a_label_swap_inside_a_table_is_rejected`] exercises the swaps that used
//!   to pass — including §4.10 codes 4 and 8, a sign inversion in a displayed
//!   statistic. The rule itself moved to [`wording`], because
//!   `wmo_parameter_tables.rs` had its own identical copy and would otherwise
//!   have kept the weakness. What the rule *still* cannot see is counted rather
//!   than argued: [`every_swap_this_gate_cannot_see_is_recorded`] walks every
//!   pair of labels in every table and holds the ones that could trade places
//!   equal to [`UNDETECTED_SWAPS`]. The hand-written version of that claim was
//!   wrong about which pairs were left, which is why it is computed.
//! * **A span.** WMO writes unassigned code space as one row (`18-191`), which
//!   the snapshot generator dropped silently, so a code assigned inside a span
//!   could never enter the snapshot and never fail the naming check. The
//!   generator records spans now, and
//!   [`every_span_wmo_publishes_is_unassigned`] holds each one meaningless —
//!   and holds the converse, that we name nothing inside a span WMO reserves.

use fieldglass_grib2::{
    lookup_data_type, lookup_discipline, lookup_earth_shape, lookup_ensemble_type,
    lookup_fixed_surface, lookup_generating_process_type, lookup_grid_template,
    lookup_production_status, lookup_reference_time_significance, lookup_statistical_process,
    lookup_time_range_unit,
};
use serde_json::Value;
use wording::{is_a_recognisable_rewrite, keeps_wmos_word_order, normalize};

mod wording;

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

/// Accepted divergences whose rewrite reorders WMO's own words, and so is
/// exempt from [`keeps_wmos_word_order`].
///
/// The order rule is what catches a swap between two entries that differ only
/// in operand order, so an exemption is a real hole and each one is written
/// down with what fills it. English puts the head noun in a different place
/// from WMO's "Level of X" construction, and a rewrite that reads naturally in
/// a metadata column is the reason these labels exist at all.
const REORDERED: &[(&str, u16, &str)] = &[(
    "4.5",
    3,
    "\"Cloud top level\" fronts the noun where WMO's \"Level of cloud tops\" \
     trails it, and pairs with code 2's \"Cloud base level\". The swap this \
     would otherwise catch — 2 against 3 — is caught anyway: code 2 matches \
     WMO exactly, so a label landing on it that is not WMO's own text is not \
     on ACCEPTED and fails outright.",
)];

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
/// prefix, and reaches this function through the snapshot's `ranges` block:
/// WMO publishes the local range as one span row (`192-254`), which the
/// generator used to drop and now records (#655).
///
/// When WMO later assigns one of these, the snapshot's text changes and the
/// code stops being exempt, which is exactly the notification this gate exists
/// to give.
fn wmo_assigns_no_meaning(wmo: &str) -> bool {
    wmo.starts_with("Reserved")
}

/// Whether `ours` is an acceptable rewrite of WMO's `theirs` for this code.
///
/// The word-order half is skipped for an entry on [`REORDERED`], which is why
/// that list is keyed by code rather than by string: a label swapped *onto* an
/// exempt code inherits the exemption, and saying so out loud is better than a
/// rule that quietly depends on which string arrived.
fn wording_survives(table: &str, code: u16, ours: &str, theirs: &str) -> bool {
    is_a_recognisable_rewrite(ours, theirs)
        && (REORDERED.iter().any(|&(t, c, _)| t == table && c == code)
            || keeps_wmos_word_order(ours, theirs))
}

/// Why a label is, or is not, acceptable for a code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Verdict {
    /// It is WMO's own text, or one string contains the other — the rule that
    /// lets `TIGGE` stand for WMO's spelt-out name, and lets ours add the unit
    /// WMO leaves to its own column.
    WmosText,
    /// It is a recorded divergence the wording rule accepts as a rewrite.
    Rewrite,
    /// Neither.
    No,
}

/// The whole decision [`every_curated_code_table_entry_agrees_with_wmo`] makes,
/// in one place, and *why* it made it.
///
/// Lifted out so [`every_swap_this_gate_cannot_see_is_recorded`] can ask the
/// same question about a label that is not the one the source carries, without
/// mutating the source or restating the rule and letting the two drift. The
/// reason comes back because the two branches have different owners: the
/// wording rule is what #655 hardened, and containment is an older, broader
/// weakness this file already records.
fn verdict(table: &str, code: u16, ours: &str, expected: &str) -> Verdict {
    let (a, b) = (normalize(ours), normalize(expected));
    if a == b || a.contains(&b) || b.contains(&a) {
        return Verdict::WmosText;
    }
    // The recorded text is the assertion: WMO must still mean what it meant
    // when the divergence was reviewed.
    let recorded = matches!(
        ACCEPTED.iter().find(|&&(t, c, _)| t == table && c == code),
        Some(&(_, _, reviewed)) if reviewed == expected
    );
    if recorded && wording_survives(table, code, ours, expected) {
        Verdict::Rewrite
    } else {
        Verdict::No
    }
}

/// Whether `ours` is an acceptable label for `table`/`code`.
fn label_is_acceptable(table: &str, code: u16, ours: &str, expected: &str) -> bool {
    verdict(table, code, ours, expected) != Verdict::No
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
            if label_is_acceptable(table.wmo, code, ours, expected) {
                continue;
            }
            match ACCEPTED
                .iter()
                .find(|&&(t, c, _)| t == table.wmo && c == code)
            {
                // The recorded text is the assertion: WMO must still mean what
                // it meant when the divergence was reviewed.
                Some(&(_, _, reviewed)) if reviewed == expected => {
                    if !wording_survives(table.wmo, code, ours, expected) {
                        wrong.push(format!(
                            "  {}/{code}: accepted as a rewrite of WMO {expected:?}, but \
                             ours {ours:?} does not read as one — it shares no word, \
                             states a number WMO does not, or reorders WMO's words \
                             without an entry on REORDERED",
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
            wording_survives(wmo, code, ours, expected),
            "{wmo}/{code}: ours {ours:?} is not a recognisable rewrite of WMO \
             {expected:?} — that is not a rewrite, it is a different entry"
        );
    }
}

/// Each word-order exemption must still be needed, and must be an accepted
/// divergence in the first place.
///
/// An exemption is a hole in the rule that catches operand-order swaps, so an
/// entry left behind after its label was reworded is a hole nothing fills. Held
/// in both directions, like [`DELIBERATELY_UNNAMED`]: an entry whose label now
/// keeps WMO's order comes off the list.
#[test]
fn every_reordering_exemption_is_still_needed() {
    let doc = snapshot();
    for &(wmo, code, why) in REORDERED {
        assert!(
            ACCEPTED.iter().any(|&(t, c, _)| t == wmo && c == code),
            "{wmo}/{code} is exempt from the word-order rule but is not an accepted \
             divergence at all ({why})"
        );
        let table = TABLES
            .iter()
            .find(|t| t.wmo == wmo)
            .unwrap_or_else(|| panic!("{wmo} is not a table under test"));
        let expected = doc["tables"][wmo][code.to_string()]
            .as_str()
            .unwrap_or_else(|| panic!("{wmo}/{code} is not in the snapshot"));
        let ours = (table.lookup)(code);
        assert!(
            !keeps_wmos_word_order(ours, expected),
            "{wmo}/{code}: ours {ours:?} keeps WMO's word order now — drop the \
             exemption rather than leaving a hole in the swap rule"
        );
    }
}

/// Two labels swapped inside one table, which is the failure the rest of this
/// file cannot see.
///
/// Both strings still exist and are still distinct, so
/// [`no_two_codes_in_a_table_share_a_label`] passes; both are still recorded
/// divergences with WMO's exact wording unchanged, so
/// [`every_accepted_divergence_is_still_a_divergence`] passes on the half that
/// pins the authority. Only [`wording_survives`] stands between a
/// swap and a green suite, and until #655 it did not: every pair below was
/// verified green on `master` at 9ea139c with the two arms exchanged.
///
/// A swap changes *both* arms, so catching either direction catches the swap.
#[test]
fn a_label_swap_inside_a_table_is_rejected() {
    let doc = snapshot();
    let wmo_text = |table: &str, code: u16| -> String {
        doc["tables"][table][code.to_string()]
            .as_str()
            .unwrap_or_else(|| panic!("{table}/{code} is not in the snapshot"))
            .to_string()
    };
    let ours = |table: &str, code: u16| -> &'static str {
        let t = TABLES
            .iter()
            .find(|t| t.wmo == table)
            .unwrap_or_else(|| panic!("{table} is not a table under test"));
        (t.lookup)(code)
    };

    for &(table, a, b, why) in &[
        // The radius family: `spherical` and `radius` anchor both, and the
        // metres are the only difference. Caught by the number rule.
        (
            "3.2",
            0u16,
            6u16,
            "a 6 367 470 m sphere labelled 6 371 229 m",
        ),
        // `IAU`, and `1965` with it, is what makes code 2 the IAU spheroid;
        // `GRS80` is what makes code 4 the IAG-GRS80 one. Both are identifiers
        // rather than prose, which is why the rule keeps any token carrying a
        // digit and not only pure digit runs — 3 <-> 4 and 4 <-> 7 passed
        // while it kept only the latter, found in review.
        ("3.2", 2, 3, "the IAU 1965 spheroid labelled custom axes"),
        ("3.2", 2, 7, "the IAU 1965 spheroid labelled custom axes"),
        (
            "3.2",
            3,
            4,
            "the IAG-GRS80 spheroid labelled custom axes, km",
        ),
        (
            "3.2",
            4,
            7,
            "the IAG-GRS80 spheroid labelled custom axes, m",
        ),
        // The sharp one: opposite quantities, and a sign inversion in anything
        // that displays the statistic. Caught by the order rule.
        (
            "4.10",
            4,
            8,
            "a difference with its operands the wrong way round",
        ),
    ] {
        let swapped_onto_a = wording_survives(table, a, ours(table, b), &wmo_text(table, a));
        let swapped_onto_b = wording_survives(table, b, ours(table, a), &wmo_text(table, b));
        assert!(
            !(swapped_onto_a && swapped_onto_b),
            "{table}: swapping codes {a} and {b} passes the wording rule — {why}"
        );
    }
}

/// The swaps the **wording rule** still cannot see, enumerated rather than
/// argued.
///
/// [`a_label_swap_inside_a_table_is_rejected`] proves the rule catches the
/// swaps #655 named. This is the other half, and the more useful one:
/// [`every_swap_this_gate_cannot_see_is_recorded`] walks every pair of codes in
/// every table and reports each pair that could trade labels with the whole
/// suite green. Held equal to this list, so a change that opens a hole fails,
/// and one that closes an old hole fails too until the entry comes off.
///
/// Written this way because the hand-written version was wrong: it claimed
/// §3.2/3 ↔ §3.2/7 was the only pair left, and review found three more. A blind
/// spot argued from the outside is a blind spot about blind spots.
const UNDETECTED_SWAPS: &[(&str, u16, u16, &str)] = &[
    (
        "3.2",
        3,
        7,
        "\"Oblate spheroid (custom axes, km)\" and \"…, m)\" differ by `km` \
         against `m`, and WMO separates them by `and` against `or` and \
         `(in km)` against `(in m)`. Nothing there carries a digit, and no \
         token floor that keeps `km` is defensible — `TIGGE` is a legitimate \
         five-character label, so the floor cannot drop on words either.",
    ),
    (
        "4.5",
        3,
        108,
        "\"Cloud top level\" and \"Level at specified pressure difference from \
         ground (Pa)\" share only `level`, which a third of Table 4.5 uses. \
         Both are heavy rewrites of long WMO text, so one generic word is all \
         there is to compare; code 3's word-order exemption is not what opens \
         this, because 108's single matching word cannot be out of order \
         either way.",
    ),
];

/// The recorded blind spots are exactly the real ones.
///
/// Two kinds of pair come out of this walk, and they have different owners.
///
/// A pair where at least one direction is accepted as a **rewrite** is the
/// wording rule's business — the rule #655 hardened — and each is named on
/// [`UNDETECTED_SWAPS`] with what would be needed to close it.
///
/// A pair where both directions are accepted because one string **contains**
/// the other is older and broader: "Latitude/longitude" is a substring of
/// "Rotated latitude/longitude", so §3.1 codes 0 and 1 can trade places. That
/// rule is what lets `TIGGE` stand for WMO's spelt-out name and lets ours carry
/// a unit WMO puts in another column, and tightening it would turn twenty
/// ordinary labels into recorded divergences — a design decision, not a fix, so
/// this counts them and holds the count in a band rather than pretending they
/// are not there.
#[test]
fn every_swap_this_gate_cannot_see_is_recorded() {
    let doc = snapshot();
    let mut undetected: Vec<(&str, u16, u16)> = Vec::new();
    let (mut pairs, mut by_containment) = (0usize, 0usize);
    for table in TABLES {
        let entries = doc["tables"][table.wmo]
            .as_object()
            .unwrap_or_else(|| panic!("table {} missing from the snapshot", table.wmo));
        // Only codes we actually name can trade labels; an `Unknown…` answer is
        // the naming gate's business, not this one. Sorted by code, because the
        // snapshot's keys are strings and `"108"` sorts before `"3"`.
        let mut named: Vec<(u16, &str, &'static str)> = entries
            .iter()
            .filter_map(|(code, wmo)| {
                let code: u16 = code.parse().expect("numeric code");
                if table.octet && code > 255 {
                    return None;
                }
                let ours = (table.lookup)(code);
                (!lookup_has_no_name(ours))
                    .then(|| (code, wmo.as_str().expect("string meaning"), ours))
            })
            .collect();
        named.sort_unstable_by_key(|&(code, _, _)| code);
        for (i, &(a, wmo_a, ours_a)) in named.iter().enumerate() {
            for &(b, wmo_b, ours_b) in &named[i + 1..] {
                pairs += 1;
                let onto_a = verdict(table.wmo, a, ours_b, wmo_a);
                let onto_b = verdict(table.wmo, b, ours_a, wmo_b);
                if onto_a == Verdict::No || onto_b == Verdict::No {
                    continue;
                }
                if onto_a == Verdict::Rewrite || onto_b == Verdict::Rewrite {
                    undetected.push((table.wmo, a, b));
                } else {
                    by_containment += 1;
                }
            }
        }
    }

    println!(
        "{pairs} label pairs tried; {} swap(s) the wording rule cannot see, \
         {by_containment} the containment rule cannot see",
        undetected.len()
    );
    // A floor on the walk itself, the same one every test in this file carries.
    // Table 4.5 alone contributes over four thousand pairs.
    assert!(
        pairs > 4_000,
        "only {pairs} label pairs were tried; the walk is not lining up"
    );
    let recorded: Vec<(&str, u16, u16)> = UNDETECTED_SWAPS
        .iter()
        .map(|&(t, a, b, _)| (t, a, b))
        .collect();
    let fresh: Vec<String> = undetected
        .iter()
        .filter(|p| !recorded.contains(p))
        .map(|(t, a, b)| format!("  {t}/{a} and {t}/{b} can trade labels"))
        .collect();
    assert!(
        fresh.is_empty(),
        "{} swap(s) the wording rule cannot see are not on UNDETECTED_SWAPS — close \
         the hole, or record it with what would fill it:\n{}",
        fresh.len(),
        fresh.join("\n")
    );
    let closed: Vec<String> = UNDETECTED_SWAPS
        .iter()
        .filter(|&&(t, a, b, _)| !undetected.contains(&(t, a, b)))
        .map(|(t, a, b, why)| format!("  {t}/{a} and {t}/{b} are caught now ({why})"))
        .collect();
    assert!(
        closed.is_empty(),
        "{} recorded blind spot(s) no longer exist — drop them rather than leaving a \
         licence nothing uses:\n{}",
        closed.len(),
        closed.join("\n")
    );
    // A band, not a ceiling: a jump means a label got broader and started
    // swallowing its neighbours, and a collapse means the containment rule
    // changed shape and this count stopped meaning what it means here. 62 at
    // WMO v37.
    assert!(
        (50..=75).contains(&by_containment),
        "{by_containment} swaps pass because one label contains the other; the \
         containment rule has moved and this inventory needs re-reading"
    );
}

/// Every span WMO publishes is code space it has assigned nothing in.
///
/// WMO writes an unassigned run as one row (`18-191`, `192-254`), and the
/// snapshot generator used to drop those rows with a bare `continue` — silently
/// and uncounted, so a code WMO later assigned *inside* a span could never
/// enter the snapshot and could never fail [`every_code_wmo_assigns_is_named`].
/// That is the structural blindness #653 closed, one level up in the generator
/// (#655). The generator now records every span; this holds them all
/// unassigned, so the day one gains a meaning it fails here and someone has to
/// expand it into codes rather than never hearing about it.
///
/// The other direction too: a code inside a span WMO plainly reserves must not
/// be one we name, or we are inventing a meaning for code space nobody has
/// defined. `Reserved for local use` is excluded from that half — it is
/// delegated space, and naming a centre's code in it is the point (Table 4.5
/// codes 200 and 201 are NCEP's).
#[test]
fn every_span_wmo_publishes_is_unassigned() {
    let doc = snapshot();
    let ranges = doc["ranges"]
        .as_object()
        .expect("snapshot has a ranges section");
    let mut in_snapshot: Vec<&str> = ranges.keys().map(String::as_str).collect();
    let mut under_test: Vec<&str> = TABLES.iter().map(|t| t.wmo).collect();
    in_snapshot.sort_unstable();
    under_test.sort_unstable();
    assert_eq!(
        under_test, in_snapshot,
        "the tables under test and the tables with spans are not the same set"
    );

    let (mut spans, mut local, mut out_of_range) = (0usize, 0usize, 0usize);
    let mut report = Vec::new();
    let mut wrong = Vec::new();
    for table in TABLES {
        let codes = doc["tables"][table.wmo]
            .as_object()
            .unwrap_or_else(|| panic!("table {} missing from the snapshot", table.wmo));
        let entries = ranges[table.wmo]
            .as_object()
            .unwrap_or_else(|| panic!("table {} has no spans", table.wmo));
        assert!(
            !entries.is_empty(),
            "table {} records no spans; every WMO code table reserves something, so \
             the block is not loading",
            table.wmo
        );
        for (span, meaning) in entries {
            let (lo, hi) = span
                .split_once('-')
                .unwrap_or_else(|| panic!("{}: {span:?} is not a span", table.wmo));
            let (lo, hi): (u32, u32) = (
                lo.parse().expect("span start"),
                hi.parse().expect("span end"),
            );
            assert!(lo < hi, "{}: {span:?} is not increasing", table.wmo);
            let meaning = meaning.as_str().expect("string meaning");
            spans += 1;
            if !wmo_assigns_no_meaning(meaning) {
                wrong.push(format!(
                    "  {}/{span}: WMO assigns {meaning:?} across the whole span — expand \
                     it into codes in the generator rather than leaving every code in it \
                     invisible to this gate",
                    table.wmo
                ));
                continue;
            }
            let delegated = meaning.contains("local use");
            local += usize::from(delegated);
            for code in lo..=hi {
                // The two blocks partition the table: a code is one or the
                // other, never both.
                assert!(
                    !codes.contains_key(&code.to_string()),
                    "{}: code {code} is both a span member ({span}) and an entry",
                    table.wmo
                );
                if table.octet && code > 255 {
                    // Counted, not skipped — the same rule as the sibling
                    // test's `out_of_range`, and asserted zero below. A span
                    // reaching above 255 in an octet table means WMO widened
                    // it, and the codes in it would otherwise drop out of this
                    // check with nothing to say so.
                    out_of_range += 1;
                    continue;
                }
                if delegated {
                    continue;
                }
                let ours = (table.lookup)(code as u16);
                if !lookup_has_no_name(ours) {
                    wrong.push(format!(
                        "  {}/{code}: we answer {ours:?}, but WMO {meaning:?} this span \
                         ({span}) — we are naming code space nobody has defined",
                        table.wmo
                    ));
                }
            }
        }
        report.push(format!("  {:>4}: {} span(s)", table.wmo, entries.len()));
    }

    println!(
        "WMO code-table spans ({spans} in all, {out_of_range} member(s) outside a \
         lookup's width):\n{}",
        report.join("\n")
    );
    // A floor, for the same reason the sibling tests carry one: a walk that
    // lined nothing up must not be able to report agreement. Eleven tables
    // reserve 44 spans at v37.
    assert!(
        spans > 40,
        "only {spans} span(s) were checked; the ranges block is not loading"
    );
    // Every table delegates 192-254 (or 32768-65534) to centres, so the
    // `Reserved for local use` half of `wmo_assigns_no_meaning` is reached
    // here — it was unreachable while the generator dropped span rows.
    assert!(
        local >= TABLES.len(),
        "only {local} local-use span(s) of {} tables; the delegated ranges are missing",
        TABLES.len()
    );
    assert_eq!(
        out_of_range, 0,
        "{out_of_range} span member(s) sit above 255 in a table whose `u8` lookup \
         cannot be asked about them — widen the lookup rather than leaving them \
         permanently unreachable"
    );
    assert!(
        wrong.is_empty(),
        "{} span problem(s):\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
