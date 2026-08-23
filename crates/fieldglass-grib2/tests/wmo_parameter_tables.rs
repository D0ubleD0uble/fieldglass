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

/// Why a triple is allowed to differ from eccodes. Not free text: the kind is
/// *checked*, so an exception cannot quietly come to cover a different
/// disagreement than the one it was granted for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Why {
    /// `tables.rs` keeps a shorter hand-written label. It must still be
    /// recognisably the same parameter: the three defects this comparison
    /// found were entirely unrelated names, which the prefix rule below
    /// excludes.
    Shortening,
    /// Same text, different character set — WMO writes the micro sign where
    /// eccodes spells it `um`. Ours follows WMO, the authority we generate from.
    MicroSign,
}

/// Triples where a difference from eccodes is intended, each with a checked
/// reason. A blanket tolerance here would have hidden the three real defects
/// this comparison exists to catch.
const EXPECTED_DIFFERENCES: &[(u8, u8, u8, Why)] = &[
    (0, 0, 3, Why::Shortening),  // drops "or equivalent potential temperature"
    (0, 0, 7, Why::Shortening),  // "Dew-point depression" / "...(or deficit)"
    (0, 1, 9, Why::Shortening),  // "(non-conv.)" / "(non-convective)"
    (10, 0, 3, Why::Shortening), // "wind+swell" / "wind waves and swell"
    (3, 1, 20, Why::MicroSign),  // 0.635 um
    (3, 1, 21, Why::MicroSign),  // 0.810 um
    (3, 1, 22, Why::MicroSign),  // 1.640 um
];

/// Shortest normalized prefix two names must share for one to pass as a
/// shortening of the other. The four real shortenings share at least 18
/// characters and the three defects shared none, so this sits well clear of
/// both populations.
const SHORTENING_PREFIX: usize = 12;

fn shared_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Whether `ours` differs from `theirs` only in the way `why` licenses.
fn difference_is_licensed(why: Why, ours: &str, theirs: &str) -> bool {
    match why {
        Why::MicroSign => normalize(&ours.replace('\u{3bc}', "u")) == normalize(theirs),
        Why::Shortening => {
            let (ours, theirs) = (normalize(ours), normalize(theirs));
            ours.len() <= theirs.len() && shared_prefix(&ours, &theirs) >= SHORTENING_PREFIX
        }
    }
}

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
        // An exception only excuses the disagreement it was granted for. A
        // curated shortening that silently became a different parameter — which
        // is exactly what 10/1/2 was — no longer slips through.
        if let Some(&(_, _, _, why)) = EXPECTED_DIFFERENCES
            .iter()
            .find(|&&(ed, ec, en, _)| (ed, ec, en) == (d, c, n))
        {
            if difference_is_licensed(why, ours, expected) {
                continue;
            }
            unexpected.push(format!(
                "  {d}/{c}/{n}: listed as {why:?} but ours {ours:?} is not that \
                 kind of difference from eccodes {expected:?}"
            ));
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
            .unwrap_or_else(|| panic!("{d}/{c}/{n} no longer resolves ({why:?})"));
        let expected = oracle
            .get(&(d, c, n))
            .unwrap_or_else(|| panic!("{d}/{c}/{n} is not in the eccodes snapshot ({why:?})"));
        assert_ne!(
            normalize(ours),
            normalize(expected),
            "{d}/{c}/{n} now agrees with eccodes — drop it from \
             EXPECTED_DIFFERENCES ({why:?})"
        );
        assert!(
            difference_is_licensed(why, ours, expected),
            "{d}/{c}/{n} is listed as {why:?}, but ours {ours:?} is not that kind \
             of difference from eccodes {expected:?}"
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
