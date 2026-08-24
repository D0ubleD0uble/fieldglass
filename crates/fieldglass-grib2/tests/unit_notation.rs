//! Unit-notation normalisation over the whole GRIB2 parameter table (#432).
//!
//! The table mixes notations by construction: the curated entries carry typeset
//! units (`m s⁻¹`), the 1,387 generated from the WMO master tables carry WMO's
//! own ASCII (`m/s`, `kg m-2`). `normalize_units` reconciles them at the display
//! seam, leaving the generated file byte-identical to its pinned upstream.
//!
//! The snapshot below is the deliverable, not scaffolding. Normalisation is the
//! kind of transform where a rule that looks obviously right mangles a handful
//! of real inputs, so every distinct unit string the table can produce is
//! pinned with its exact output. A rule change shows up as a diff across all of
//! them, which is the only way to see what it did to the strings you were not
//! thinking about.
//!
//! Regenerate with `UPDATE_UNIT_SNAPSHOT=1 cargo test -p fieldglass-grib2
//! --test unit_notation` and read the diff before committing it.

use fieldglass_core::units::normalize_units;
use fieldglass_grib2::{Originator, lookup_parameter};
use std::collections::BTreeSet;

/// The centres the sweeps below resolve as. Centre 0 is the WMO Secretariat,
/// which has no local table and so reaches the master set; 98 is ECMWF, whose
/// 2,826 local parameters (#424) carry units in eccodes' Fortran notation; 78
/// is DWD, whose 213 (#425) carry a third notation again — bare negative
/// exponents, `kg kg-1` — and one string, `Pa-3h`, that is not a product of
/// units at all; 7 is NCEP, whose 479 (#426) come from wgrib2 rather than
/// eccodes and use a *fourth*, `kg/m^2/s`. Both ECMWF table versions appear
/// because thirteen of its entries are gated on one; DWD and NCEP gate
/// nothing.
///
/// Sweeping the master set alone would have left the entire ECMWF unit
/// vocabulary unpinned — 59 of its 69 distinct strings — which is exactly the
/// silent gap this snapshot exists to close.
const SWEPT: [Originator; 5] = [
    Originator {
        centre: 0,
        sub_centre: 0,
        local_tables_version: 0,
    },
    Originator {
        centre: 98,
        sub_centre: 0,
        local_tables_version: 0,
    },
    Originator {
        centre: 98,
        sub_centre: 0,
        local_tables_version: 1,
    },
    Originator {
        centre: 78,
        sub_centre: 0,
        local_tables_version: 0,
    },
    Originator {
        centre: 7,
        sub_centre: 0,
        local_tables_version: 1,
    },
];
const SNAPSHOT: &str = include_str!("fixtures/unit_notation.snapshot.txt");
const SNAPSHOT_PATH: &str = "tests/fixtures/unit_notation.snapshot.txt";

/// Every distinct unit string the parameter table can return.
fn distinct_units() -> BTreeSet<String> {
    let mut units = BTreeSet::new();
    for discipline in 0..=255u8 {
        for category in 0..=255u8 {
            for number in 0..=255u8 {
                for originator in SWEPT {
                    if let Some((_, _, unit)) =
                        lookup_parameter(originator, discipline, category, number)
                    {
                        units.insert(unit.to_string());
                    }
                }
            }
        }
    }
    units
}

fn render_snapshot(units: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for unit in units {
        // Debug-quoted on both sides. Two reasons: the empty unit would
        // otherwise render as a line ending in a tab, which the repo's
        // trailing-whitespace hook strips out from under the snapshot; and
        // upstream really does ship `(m2 s sr )-1` with a stray internal
        // space, which quoting makes visible instead of mysterious.
        out.push_str(&format!("{unit:?}\t{:?}\n", normalize_units(unit)));
    }
    out
}

#[test]
fn the_whole_table_normalises_as_pinned() {
    let units = distinct_units();
    assert!(
        units.len() > 150,
        "only {} distinct units — the table is not loaded, so this proves nothing",
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

/// The strings a structural rule mangles. Each of these is real — it appears in
/// WMO Code Table 4.2 or in a centre-local table — and each would be corrupted
/// by an obvious-looking implementation, so they are pinned separately from the
/// bulk snapshot where they are easy to miss.
#[test]
fn the_strings_that_look_like_units_but_are_not_survive_untouched() {
    for input in [
        // A character encoding. Any "letter then digit means exponent" rule
        // renders this `CCITT IA⁵`.
        "CCITT IA5",
        // A *fractional* exponent, m^(2/3). Reading `/` as division gives
        // nonsense.
        "m2/3 s-1",
        // Grouped, with a solidus inside the group.
        "(m2 s sr eV/nuc)-1",
        "(m2 s sr)-1",
        // A cross-reference to another code table, not a unit.
        "Code table 4.253",
        "(Code table 4.201)",
        // Prose that WMO uses where a quantity is dimensionless.
        "Numeric",
        "Proportion",
        "Bites per day per person",
        "",
        // DWD's contributions (#425). `Pa-3h` is a pressure change *over three
        // hours*, so the trailing `-3h` is a period and not an exponent;
        // `10-7 s-2` leads with a scale factor the way ECMWF's `10**-6 …` does;
        // `Pa(O3)` names the species in parentheses; and `Km kg-1 s-1` is the
        // frontogenesis function, where upstream ran two symbols together and
        // `Km` could as easily be kilometre as kelvin-metre. Guessing at any of
        // them buys a prettier string and risks a wrong one.
        "Pa-3h",
        "10-7 s-2",
        "Pa(O3)",
        "Km kg-1 s-1",
        // Prefixed forms outside the allow-list. Nothing would change if they
        // were added — neither takes an exponent anywhere in the table — so
        // they stay out rather than growing the list for nothing.
        "klux",
        "Degree",
        // NCEP's contributions (#426), from wgrib2 rather than eccodes. Its
        // notation is the caret form, which `normalize_units` now reads, but
        // the same table carries three families that it must not:
        //
        //  * `*` as an explicit product. `K*m/s` is kelvin-metres per second
        //    and `J/m^2*K` is joules per square metre per kelvin, but nothing
        //    else in any corpus writes a product that way, and reading `*` as
        //    one would collide with the `**` exponent operator eccodes uses.
        "K*m/s",
        "Pa*Pa",
        "kg/kg*Pa/s",
        "J/m^2*K",
        //  * A leading scale factor, and logarithms of one.
        "10^-6g/m^3",
        "log10(kg/m^3)",
        "ln(kPa)",
        //  * Prose for a dimensionless or coded quantity. `-` and `non-dim`
        //    are NCEP's spellings of what WMO writes as `Numeric`, and
        //    `Integer(0-13)` names a code range.
        "-",
        "non-dim",
        "Categorical",
        "Integer(0-13)",
        "ppbV",
    ] {
        assert_eq!(
            normalize_units(input),
            input,
            "{input:?} must pass through untouched"
        );
    }
}

/// Applying it twice changes nothing, which is what lets the curated entries —
/// already typeset — go through the same path as the generated ones.
#[test]
fn normalisation_is_idempotent_over_the_whole_table() {
    for unit in distinct_units() {
        let once = normalize_units(&unit).into_owned();
        let twice = normalize_units(&once).into_owned();
        assert_eq!(once, twice, "{unit:?} is not stable under normalisation");
    }
}

/// The point of the exercise: after normalisation the table speaks one
/// notation. Nothing that was rewritten may still carry the ASCII forms.
#[test]
fn no_rewritten_unit_still_uses_ascii_notation() {
    for unit in distinct_units() {
        let normalised = normalize_units(&unit).into_owned();
        if normalised == unit {
            continue; // deliberately left alone; covered by the snapshot
        }
        // `*` as well as `/` since #441: a future WMO tag could introduce the
        // Fortran `**` form the ECMWF and NCEP tables are written in, and a
        // half-stripped `*` would be as wrong here as a leftover solidus.
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
