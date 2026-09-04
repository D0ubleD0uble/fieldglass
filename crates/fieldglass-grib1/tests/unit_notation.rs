//! Unit-notation normalisation over the whole GRIB1 parameter table (#441).
//!
//! The GRIB1 side carries two ASCII notations, from two generated sources.
//! WMO ON388 Table 2 writes products with solidi and chains them —
//! `kg/m2`, `kg/m2/s`, `W/m3/sr` — while the ECMWF local tables 128/129 are
//! generated from eccodes, which writes exponents Fortran-style: `kg m**-2`,
//! `K m**2 kg**-1 s**-1`. Both files are reproduced from their upstream, so
//! `normalize_units` reconciles them at the display seam instead
//! (`fieldglass-napi/src/lib.rs`, where `parameter_units` is filled in).
//!
//! The snapshot below is the deliverable, not scaffolding — same reasoning as
//! the GRIB2 one it mirrors. Normalisation is the kind of transform where a
//! rule that looks obviously right mangles a handful of real inputs, so every
//! distinct unit string the tables can produce is pinned with its exact output.
//! A rule change shows up as a diff across all of them, which is the only way
//! to see what it did to the strings you were not thinking about.
//!
//! Regenerate with `UPDATE_UNIT_SNAPSHOT=1 cargo test -p fieldglass-grib1
//! --test unit_notation` and read the diff before committing it.

use fieldglass_core::units::normalize_units;
use fieldglass_grib1::tables::lookup_parameter;
use std::collections::BTreeSet;

const SNAPSHOT: &str = include_str!("fixtures/unit_notation.snapshot.txt");
const SNAPSHOT_PATH: &str = "tests/fixtures/unit_notation.snapshot.txt";

/// WMO originating-centre code for ECMWF, the one centre with local tables here.
const CENTRE_ECMWF: u8 = 98;
/// A centre with no local tables, so `lookup_parameter` resolves against ON388.
const CENTRE_WMO: u8 = 0;

/// Every distinct unit string the GRIB1 tables can return.
///
/// Versions 1-3 are the ON388 international table and 128/129 the ECMWF local
/// ones. Only three of the ten combinations carry units of their own — the
/// international table is centre-independent, and a local version resolves to
/// nothing at all for a centre whose table this crate does not ship (#547) —
/// so enumerating every combination is redundant today. Which is the point: if
/// that routing ever changes, the snapshot widens rather than silently going on
/// testing a narrower table than its own doc comment claims.
fn distinct_units() -> BTreeSet<String> {
    let mut units = BTreeSet::new();
    for centre in [CENTRE_WMO, CENTRE_ECMWF] {
        for version in [1, 2, 3, 128, 129] {
            for id in 0..=255u8 {
                units.insert(lookup_parameter(id, version, centre).units.to_string());
            }
        }
    }
    units
}

fn render_snapshot(units: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for unit in units {
        // Debug-quoted on both sides, so the empty unit does not render as a
        // line ending in a tab — which the repo's trailing-whitespace hook
        // would strip out from under the snapshot.
        out.push_str(&format!("{unit:?}\t{:?}\n", normalize_units(unit)));
    }
    out
}

#[test]
fn the_whole_table_normalises_as_pinned() {
    let units = distinct_units();
    assert!(
        units.len() > 50,
        "only {} distinct units — the tables are not loaded, so this proves nothing",
        units.len()
    );

    let rendered = render_snapshot(&units);
    if std::env::var("UPDATE_UNIT_SNAPSHOT").is_ok() {
        std::fs::write(SNAPSHOT_PATH, &rendered).expect("write snapshot");
        return;
    }
    assert_eq!(
        rendered, SNAPSHOT,
        "unit normalisation changed. Re-read the diff, then regenerate with \
         UPDATE_UNIT_SNAPSHOT=1"
    );
}

/// The Fortran `**` form is why this test exists, so assert it is actually
/// present in the table rather than trusting the snapshot to have covered it.
/// If a future eccodes regeneration drops the notation, this fails loudly
/// instead of quietly turning the snapshot into a test of nothing.
#[test]
fn the_ecmwf_table_really_is_written_with_fortran_exponents() {
    let fortran = distinct_units()
        .into_iter()
        .filter(|u| u.contains("**"))
        .count();
    assert!(
        fortran >= 8,
        "only {fortran} distinct units use the `**` form — the ECMWF table is \
         not loaded, or eccodes changed notation"
    );
    // And the ON388 chain form, for the same reason.
    let chained = distinct_units()
        .into_iter()
        .filter(|u| u.matches('/').count() >= 2 && !u.contains('('))
        .count();
    assert!(
        chained >= 5,
        "only {chained} distinct units chain solidi — the ON388 table is not loaded"
    );
}

/// Applying it twice changes nothing, which is what lets an already-typeset
/// string go through the same path.
#[test]
fn normalisation_is_idempotent_over_the_whole_table() {
    for unit in distinct_units() {
        let once = normalize_units(&unit).into_owned();
        let twice = normalize_units(&once).into_owned();
        assert_eq!(once, twice, "{unit:?} is not stable under normalisation");
    }
}

/// The point of the exercise: after normalisation the table speaks one
/// notation. Nothing that was rewritten may still carry an ASCII form.
#[test]
fn no_rewritten_unit_still_uses_ascii_notation() {
    for unit in distinct_units() {
        let normalised = normalize_units(&unit).into_owned();
        if normalised == unit {
            continue; // deliberately left alone; covered by the snapshot
        }
        for ascii in ['/', '*'] {
            assert!(
                !normalised.contains(ascii),
                "{unit:?} normalised to {normalised:?}, which still has {ascii:?}"
            );
        }
        assert!(
            !normalised
                .as_bytes()
                .windows(2)
                .any(|w| w[0] == b'-' && w[1].is_ascii_digit()),
            "{unit:?} normalised to {normalised:?}, which still has an ASCII exponent"
        );
    }
}

/// GRIB1 prose units — the tables use words where a quantity is dimensionless,
/// and ranges where it is bounded. None is a product of units, and each would
/// be corrupted by a structural rule, so they are pinned separately from the
/// bulk snapshot where they are easy to miss.
#[test]
fn the_strings_that_look_like_units_but_are_not_survive_untouched() {
    for input in [
        // ECMWF writes bounded quantities as a range.
        "(0 - 1)",
        "(-1 to 1)",
        // Prose, from both tables.
        "m of water equivalent",
        "dimensionless",
        "non-dim",
        "numeric",
        "integer",
        "fraction",
        "proportion",
        "deg true",
        "radians",
        "Dobson",
        // A lone hyphen is ON388's "no unit", not an exponent sign.
        "-",
        // Functions and juxtaposed groups.
        "ln(kPa)",
        "log10(kg/m3)",
        "(kg/m3)(m/s)",
        // A single `*` is ON388's multiplication, which this module does not
        // model — one entry uses it, and half-converting it would be worse.
        "K*m/s",
        "Millimetres*100 + number of stations",
        "",
    ] {
        assert_eq!(
            normalize_units(input),
            input,
            "{input:?} must pass through untouched"
        );
    }
}
