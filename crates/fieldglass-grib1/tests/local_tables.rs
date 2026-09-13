//! Every GRIB1 local parameter this crate names, against eccodes' decode (#601).
//!
//! `src/tables_local.rs` is generated from eccodes' table *files*. The oracle
//! here is not: `tools/gen_grib1_local_tables.py` stamps a real GRIB1 message
//! with each centre, sub-centre and table version, and records what eccodes
//! 2.34.1 *decodes* for every id — which is the table eccodes selects as well as
//! the row it reads. So a generator that misread a file, and a lookup that chose
//! the wrong table, both fail here.
//!
//! Two differences from eccodes are deliberate and handled below: its unset
//! marker `~` is an empty string here, and units that carry their own
//! parentheses (`Cloud fraction ((0 - 1))`) are split at the last balanced
//! group, where eccodes splits at the last `(`.

use std::collections::BTreeSet;

use fieldglass_grib1::tables::{ParameterEntry, lookup_parameter};
use serde_json::Value;

const ORACLE: &str = include_str!("fixtures/local_tables.eccodes.ref.json");
const CENTRE_ECMWF: u8 = 98;

fn tables() -> serde_json::Map<String, Value> {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle parses");
    oracle["tables"].as_object().expect("tables").clone()
}

fn key(key: &str) -> (u8, u8, u8) {
    let parts: Vec<u8> = key.split('/').map(|p| p.parse().expect("code")).collect();
    (parts[0], parts[1], parts[2])
}

/// What this crate should answer for eccodes' `[abbreviation, name, units]`,
/// as `(abbreviation, name, units)`, and whether the nested-parenthesis
/// difference applied.
fn expected(row: &Value) -> ((String, String, String), bool) {
    let field = |i: usize| {
        let s = row[i].as_str().expect("string field");
        if s == "~" {
            String::new()
        } else {
            s.to_string()
        }
    };
    let (abbreviation, name, units) = (field(0), field(1), field(2));
    // eccodes' title runs to the last `(`, so a unit written `((0 - 1))` leaves
    // the title ending in `(` and the units ending in `)`.
    match name.strip_suffix('(') {
        Some(title) => (
            (
                abbreviation,
                title.trim_end().to_string(),
                format!("({units}"),
            ),
            true,
        ),
        None => ((abbreviation, name, units), false),
    }
}

fn as_owned(p: ParameterEntry) -> (String, String, String) {
    (
        p.abbreviation.to_string(),
        p.name.to_string(),
        p.units.to_string(),
    )
}

#[test]
fn every_local_parameter_matches_eccodes_decode() {
    let mut compared = 0usize;
    let mut nested = 0usize;
    for (label, rows) in tables() {
        let (centre, sub_centre, version) = key(&label);
        let rows = rows.as_object().expect("rows");
        for id in 0..=255u8 {
            let ours = lookup_parameter(id, version, centre, sub_centre);
            match (rows.get(&id.to_string()), ours) {
                (None, None) => {}
                (Some(row), Some(ours)) => {
                    let (want, was_nested) = expected(row);
                    assert_eq!(as_owned(ours), want, "{label} id {id}");
                    compared += 1;
                    nested += usize::from(was_nested);
                }
                (row, ours) => panic!("{label} id {id}: eccodes {row:?}, this crate {ours:?}"),
            }
        }
    }
    // The ECMWF tables alone hold thousands of entries; far fewer means the
    // oracle or the tables did not load, and this proved nothing.
    assert!(compared > 3_000, "compared only {compared} entries");
    assert!(
        nested > 0,
        "no nested-parenthesis unit was compared, so that branch is untested"
    );
}

/// The oracle covers exactly the ECMWF tables this crate carries: a table added
/// to the generator without its oracle, or dropped from it, fails here rather
/// than going unchecked.
#[test]
fn the_oracle_covers_every_carried_table() {
    let in_oracle: BTreeSet<u8> = tables()
        .keys()
        .map(|k| key(k))
        .filter(|&(centre, sub_centre, _)| centre == CENTRE_ECMWF && sub_centre == 0)
        .map(|(_, _, version)| version)
        .collect();
    let carried: BTreeSet<u8> = (128..=255u8)
        .filter(|&v| (0..=255u8).any(|id| lookup_parameter(id, v, CENTRE_ECMWF, 0).is_some()))
        .collect();
    assert_eq!(in_oracle, carried);
    assert!(carried.len() > 20, "only {} ECMWF tables", carried.len());
}

/// The selection rule's cases are in the oracle, and say what the rule says:
/// another centre's message with ECMWF as sub-centre reads ECMWF's table, and
/// the same centre without it reads nothing.
#[test]
fn the_oracle_holds_the_sub_centre_rule() {
    let all = tables();
    let rows = |k: &str| all[k].as_object().expect(k).len();
    assert_eq!(rows("80/98/128"), rows("98/0/128"));
    assert_eq!(rows("7/98/128"), rows("98/0/128"));
    assert_eq!(rows("80/0/128"), 0);
}
