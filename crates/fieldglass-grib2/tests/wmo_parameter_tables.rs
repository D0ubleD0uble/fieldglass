//! The generated WMO master parameter table (#415), cross-checked against
//! eccodes.
//!
//! `tables_wmo.rs` is generated from the WMO CSVs; `eccodes_parameters.ref.json`
//! is the same WMO Code Table 4.2 as eccodes transcribes it. Two independent
//! transcriptions of one standard agreeing is worth much more than either
//! alone — and this comparison is what found three curated entries whose
//! triples were GRIB1 ON388 codes copied onto GRIB2 discipline/category/number.
//!
//! The snapshot is committed, so this needs no eccodes at runtime; regenerate
//! it with `tools/gen_eccodes_parameter_snapshot.py` after an eccodes upgrade.

use fieldglass_grib2::lookup_parameter;
use serde_json::Value;
use std::collections::BTreeMap;

const SNAPSHOT: &str = include_str!("fixtures/eccodes_parameters.ref.json");

/// Triples where a difference from eccodes is intended. Every one is listed
/// individually with its reason: a blanket tolerance would have hidden the
/// three real defects this comparison exists to catch.
const EXPECTED_DIFFERENCES: &[(u8, u8, u8, &str)] = &[
    // `tables.rs` keeps a shorter hand-written label for these four, which is
    // what the metadata table has always shown.
    (
        0,
        0,
        3,
        "curated short form of 'or equivalent potential temperature'",
    ),
    (
        0,
        0,
        7,
        "curated 'Dew-point depression' vs eccodes 'Dewpoint depression (or deficit)'",
    ),
    (
        0,
        1,
        9,
        "curated abbreviates '(non-convective)' to '(non-conv.)'",
    ),
    (
        10,
        0,
        3,
        "curated abbreviates 'wind waves and swell' to 'wind+swell'",
    ),
    // WMO writes the micro sign; eccodes spells it `um` in ASCII. Ours follows
    // WMO, which is the authority the table is generated from.
    (3, 1, 20, "'0.635 μm' vs ASCII 'um'"),
    (3, 1, 21, "'0.810 μm' vs ASCII 'um'"),
    (3, 1, 22, "'1.640 μm' vs ASCII 'um'"),
];

/// Names compare on letters and digits only: the two sources differ freely on
/// hyphens, case, and spacing (`Dew-point` / `dewpoint`) without disagreeing
/// about which parameter a triple names.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn eccodes_names() -> BTreeMap<(u8, u8, u8), String> {
    let doc: Value = serde_json::from_str(SNAPSHOT).expect("snapshot parses");
    let params = doc["parameters"].as_object().expect("parameters object");
    let mut out = BTreeMap::new();
    for (key, entry) in params {
        let parts: Vec<&str> = key.split('/').collect();
        assert_eq!(parts.len(), 3, "malformed key {key}");
        let (Ok(d), Ok(c), Ok(n)) = (
            parts[0].parse::<u16>(),
            parts[1].parse::<u16>(),
            parts[2].parse::<u16>(),
        ) else {
            continue;
        };
        // eccodes carries a few keys outside the octet range; skip rather than
        // truncate them into a different triple.
        if d > 255 || c > 255 || n > 255 {
            continue;
        }
        let name = entry["name"].as_str().expect("name is a string");
        out.insert((d as u8, c as u8, n as u8), name.to_string());
    }
    out
}

/// Every triple the two sources share must name the same parameter.
#[test]
fn every_shared_triple_agrees_with_eccodes() {
    let oracle = eccodes_names();
    assert!(
        oracle.len() > 1_000,
        "snapshot looks truncated: {}",
        oracle.len()
    );

    let mut compared = 0usize;
    let mut unexpected = Vec::new();
    for (&(d, c, n), expected) in &oracle {
        let Some((_, ours, _)) = lookup_parameter(d, c, n) else {
            continue; // eccodes carries reserved / missing rows we omit
        };
        compared += 1;
        if normalize(ours) == normalize(expected) {
            continue;
        }
        if EXPECTED_DIFFERENCES
            .iter()
            .any(|&(ed, ec, en, _)| (ed, ec, en) == (d, c, n))
        {
            continue;
        }
        unexpected.push(format!(
            "  {d}/{c}/{n}: ours {ours:?}, eccodes {expected:?}"
        ));
    }

    assert!(
        compared > 1_300,
        "only {compared} triples were actually compared — the tables are not \
         lining up, so agreement proves nothing"
    );
    assert!(
        unexpected.is_empty(),
        "{} of {compared} triples name a different parameter than eccodes:\n{}",
        unexpected.len(),
        unexpected.join("\n")
    );
}

/// Each listed exception must still *be* a difference. Without this the list
/// would quietly become a place where a real regression could hide: an entry
/// that stopped differing would keep its licence to differ.
#[test]
fn every_expected_difference_is_still_a_difference() {
    let oracle = eccodes_names();
    for &(d, c, n, why) in EXPECTED_DIFFERENCES {
        let (_, ours, _) = lookup_parameter(d, c, n)
            .unwrap_or_else(|| panic!("{d}/{c}/{n} no longer resolves ({why})"));
        let expected = oracle
            .get(&(d, c, n))
            .unwrap_or_else(|| panic!("{d}/{c}/{n} is not in the eccodes snapshot ({why})"));
        assert_ne!(
            normalize(ours),
            normalize(expected),
            "{d}/{c}/{n} now agrees with eccodes — drop it from \
             EXPECTED_DIFFERENCES ({why})"
        );
    }
}

/// The table is the size it should be. A generator that silently emitted a
/// fraction of the tables would still pass the agreement test above, since
/// that only checks the triples that do resolve.
#[test]
fn the_master_table_is_the_expected_size() {
    let mut resolved = 0usize;
    for d in 0..=255u8 {
        for c in 0..=255u8 {
            for n in 0..=255u8 {
                if lookup_parameter(d, c, n).is_some() {
                    resolved += 1;
                }
            }
        }
    }
    // 1387 from WMO v37 plus the curated entries that sit outside it. Held as a
    // floor rather than an equality so adding parameters doesn't fail the
    // build, but losing a table does.
    assert!(
        resolved >= 1_380,
        "only {resolved} parameters resolve; the master table looks incomplete"
    );
}
