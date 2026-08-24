//! The DWD/ICON local parameter table, cross-checked against eccodes (#425).
//!
//! Same construction as `ecmwf_local_parameters.rs` — see `local_concepts/mod.rs`
//! — against `definitions/grib2/localConcepts/edzw/`.
//!
//! Regenerate with
//! `python3 tools/gen_localconcepts_tables.py edzw --oracle && cargo fmt`.
//!
//! # What this table does and does not cover
//!
//! DWD writes most of its parameter database against *standard* WMO triples and
//! separates the collisions with §4 keys, so only the genuinely local part of it
//! reaches this seam: 213 of the 1,704 concept blocks at eccodes 2.34.1, after
//! a further 254 `DUMMY_n` placeholders are dropped. The headline ICON fields —
//! `T_2M`, `TOT_PREC`, `PMSL`, `W_SO` — are all in the rest, and keep resolving
//! against the WMO master set, which names them correctly without DWD's
//! abbreviation. Nine of the 129 parameter groups DWD publishes for ICON-D2 are
//! named by this table; the rest were already named. That is the trade the seam
//! makes: names we do not gain, never names we get wrong.

mod local_concepts;

use fieldglass_grib2::{Originator, lookup_parameter};
use local_concepts::Oracle;

const ORACLE: &str = include_str!("fixtures/localconcepts_edzw.ref.json");

/// WMO Common Code Table C-11 code for Offenbach (RSMC) - DWD.
const DWD: u16 = 78;

/// A centre with no local table, so lookups reach the WMO master set.
const MASTER_ONLY: Originator = Originator {
    centre: 0,
    sub_centre: 0,
    local_tables_version: 0,
};

fn oracle() -> Oracle {
    let oracle = Oracle::load(ORACLE);
    assert_eq!(
        oracle.centre_code, DWD,
        "the snapshot was collected for a different centre than this table dispatches on"
    );
    oracle
}

#[test]
fn every_entry_matches_what_eccodes_reports() {
    oracle().assert_every_entry_matches(200);
}

#[test]
fn the_oracle_contains_no_unresolved_entries() {
    oracle().assert_nothing_unresolved();
}

#[test]
fn placeholder_names_are_not_shipped() {
    oracle().assert_no_placeholder_names();
}

/// The table only ever answers for DWD, and the sub-centre does not gate it.
/// 98 is in the "other centres" list on purpose: ECMWF has its own table, and
/// `(0, 1, 203)` means something different there.
#[test]
fn the_table_is_scoped_to_dwd() {
    oracle().assert_scoped_to_its_centre((0, 1, 203), &[0, 7, 34, 98, 173]);
}

/// A spot-check with the values written out, so a reader can see what the table
/// contains without decoding a 213-entry fixture. `FRESHSNW` and `CLCT_MOD` are
/// the triples carried by the real ICON-D2 open-data files this was validated
/// against; the rest are ICON diagnostics across two disciplines.
#[test]
fn well_known_icon_parameters_resolve() {
    let dwd = Originator::new(DWD, 0, 0);
    for (discipline, category, number, expected) in [
        (
            0u8,
            1u8,
            203u8,
            (
                "FRESHSNW",
                "Fresh snow factor (weighting function for albedo indicating freshness of snow)",
                "Numeric",
            ),
        ),
        (
            0,
            6,
            199,
            ("CLCT_MOD", "Modified cloud cover for media", "Numeric"),
        ),
        (
            0,
            0,
            192,
            ("DT_CON", "Temperature tendency due to convection", "K s-1"),
        ),
        (
            0,
            7,
            193,
            (
                "SDI_2",
                "Supercell detection index 2 (only rot. updrafts)",
                "s-1",
            ),
        ),
        (
            2,
            3,
            196,
            (
                "SOILTYP",
                "Soil type (1...9, local soilType.table)",
                "Numeric",
            ),
        ),
    ] {
        assert_eq!(
            lookup_parameter(dwd, discipline, category, number),
            Some(expected),
            "{discipline}/{category}/{number}"
        );
    }
}

/// DWD gates nothing on `localTablesVersion`, so the table has to answer the
/// same at every version — and it has to actually be exercised at the version
/// real files carry. Every ICON-D2 open-data message declares
/// `localTablesVersion = 1`, so a table that only answered at 0 would name
/// nothing in practice while passing every other test here.
#[test]
fn the_table_is_not_gated_on_a_local_table_version() {
    let at = |version| lookup_parameter(Originator::new(DWD, 255, version), 0, 1, 203);
    let expected = at(1);
    assert!(
        expected.is_some(),
        "the version and sub-centre a real ICON-D2 file carries resolve to nothing"
    );
    for version in [0u8, 2, 128, 255] {
        assert_eq!(at(version), expected, "local table version {version}");
    }
}

/// The 983 concept blocks DWD writes against standard triples are deliberately
/// not shipped, so those parameters keep the WMO master name rather than DWD's
/// abbreviation. Asserted against three fields a real ICON file carries, because
/// "we drop them" is only safe if what is left is still correct.
#[test]
fn standard_triples_still_resolve_to_the_master_entry() {
    let dwd = Originator::new(DWD, 255, 1);
    for (discipline, category, number, dwd_short_name) in [
        (0u8, 3u8, 1u8, "PMSL"),
        (0, 1, 1, "RELHUM"),
        (0, 6, 1, "CLCT"),
    ] {
        let local = lookup_parameter(dwd, discipline, category, number);
        assert_eq!(
            local,
            lookup_parameter(MASTER_ONLY, discipline, category, number),
            "{discipline}/{category}/{number} resolves differently for DWD than for the \
             master set, so the local table is shadowing a standard parameter"
        );
        let (short, _, _) = local.unwrap_or_else(|| {
            panic!("{discipline}/{category}/{number} resolves to nothing at all")
        });
        assert_ne!(
            short, dwd_short_name,
            "{discipline}/{category}/{number} picked up DWD's own abbreviation, which the \
             ≥192 rule is supposed to keep out of this seam"
        );
    }
}
