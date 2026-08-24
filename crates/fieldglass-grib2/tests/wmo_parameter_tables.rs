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

use fieldglass_grib2::{Originator, lookup_parameter};
use serde_json::Value;
use std::collections::BTreeMap;

/// The sweeps below check the WMO master set, so they resolve as a centre with
/// no local table of its own. Centre 0 is the WMO Secretariat, which will never
/// have one — the point is that nothing here routes through `tables_local`.
const MASTER_ONLY: Originator = Originator {
    centre: 0,
    sub_centre: 0,
};

const SNAPSHOT: &str = include_str!("fixtures/eccodes_parameters.ref.json");

/// A triple we intentionally word differently from eccodes, with **eccodes'
/// exact text at the time the divergence was reviewed**.
///
/// The recorded text is the assertion, not a similarity rule. What actually
/// went wrong in this table was a triple meaning something different from what
/// we thought — so pinning the authority's wording is what catches it, however
/// plausible our own label still looks. Same mechanism as
/// `wmo_code_tables.rs`.
const ACCEPTED: &[(u8, u8, u8, &str)] = &[
    // `tables.rs` keeps a shorter hand-written label for these four.
    (
        0,
        0,
        3,
        "Pseudo-adiabatic potential temperature or equivalent potential temperature",
    ),
    (0, 0, 7, "Dewpoint depression (or deficit)"),
    (0, 1, 9, "Large-scale precipitation (non-convective)"),
    (
        10,
        0,
        3,
        "Significant height of combined wind waves and swell",
    ),
    // WMO writes the micro sign where eccodes spells it `um`; ours follows WMO,
    // the authority the table is generated from.
    (3, 1, 20, "Aerosol optical thickness at 0.635 um"),
    (3, 1, 21, "Aerosol optical thickness at 0.810 um"),
    (3, 1, 22, "Aerosol optical thickness at 1.640 um"),
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
        let Some((_, ours, _)) = lookup_parameter(MASTER_ONLY, d, c, n) else {
            continue; // eccodes carries reserved / missing rows we omit
        };
        compared += 1;
        if normalize(ours) == normalize(expected) {
            continue;
        }
        // An accepted divergence only excuses the disagreement it was reviewed
        // against. If eccodes' wording changes, the triple has been reassigned
        // and the entry needs looking at again.
        match ACCEPTED
            .iter()
            .find(|&&(ed, ec, en, _)| (ed, ec, en) == (d, c, n))
        {
            Some(&(_, _, _, reviewed)) if reviewed == expected => {
                if shares_a_substantial_word(ours, expected) {
                    continue;
                }
                unexpected.push(format!(
                    "  {d}/{c}/{n}: accepted as a rewrite of eccodes {expected:?}, but \
                     ours {ours:?} shares no word with it"
                ));
                continue;
            }
            Some(&(_, _, _, reviewed)) => {
                unexpected.push(format!(
                    "  {d}/{c}/{n}: reviewed against eccodes {reviewed:?}, but eccodes \
                     now says {expected:?} — the triple has been reassigned"
                ));
                continue;
            }
            None => {}
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

/// Each accepted divergence must still be one, and must still be reviewed
/// against what eccodes currently says. Without this the list would slowly
/// become a place where a real regression could hide: an entry that stopped
/// differing, or whose oracle text moved, would keep its licence to differ.
#[test]
fn every_accepted_divergence_is_still_a_divergence() {
    let oracle = eccodes_names();
    for &(d, c, n, reviewed) in ACCEPTED {
        let (_, ours, _) = lookup_parameter(MASTER_ONLY, d, c, n)
            .unwrap_or_else(|| panic!("{d}/{c}/{n} no longer resolves"));
        let expected = oracle
            .get(&(d, c, n))
            .unwrap_or_else(|| panic!("{d}/{c}/{n} is not in the eccodes snapshot"));
        assert_eq!(
            expected, reviewed,
            "{d}/{c}/{n}: eccodes' wording changed since this divergence was reviewed"
        );
        assert_ne!(
            normalize(ours),
            normalize(expected),
            "{d}/{c}/{n} now agrees with eccodes — drop it from ACCEPTED"
        );
        assert!(
            shares_a_substantial_word(ours, expected),
            "{d}/{c}/{n}: ours {ours:?} shares no word with eccodes {expected:?} — that \
             is not a shortening, it is a different parameter"
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
                if lookup_parameter(MASTER_ONLY, d, c, n).is_some() {
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
