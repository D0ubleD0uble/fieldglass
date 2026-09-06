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
use wording::{is_a_recognisable_rewrite, keeps_wmos_word_order, normalize};

mod wording;

/// The sweeps below check the WMO master set, so they resolve as a centre with
/// no local table of its own. Centre 0 is the WMO Secretariat, which will never
/// have one — the point is that nothing here routes through `tables_local`.
const MASTER_ONLY: Originator = Originator {
    centre: 0,
    sub_centre: 0,
    local_tables_version: 0,
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

/// Accepted divergences whose rewrite reorders eccodes' own words, and so are
/// exempt from the word-order half of [`wording_survives`], each with the
/// reason.
///
/// Empty, and worth keeping as a list rather than deleting: the exemption is a
/// hole in what catches a swap between two entries differing only in word
/// order, so the next rewrite that needs one should have to write down why
/// where a reviewer sees it. Both directions are held by
/// [`every_reordering_exemption_is_still_needed`], so an entry cannot outlive
/// its reason.
const REORDERED: &[((u8, u8, u8), &str)] = &[];

/// Whether `ours` is an acceptable rewrite of eccodes' `theirs` for this
/// triple.
///
/// The rule itself lives in [`wording`], shared with `wmo_code_tables.rs`
/// because both gates need exactly it and both had their own copy until #655.
/// The word-order half is skipped for a triple on [`REORDERED`], which is why
/// that list is keyed by triple rather than by string: a label swapped *onto*
/// an exempt triple inherits the exemption, and saying so out loud is better
/// than a rule that quietly depends on which string arrived.
fn wording_survives(triple: (u8, u8, u8), ours: &str, theirs: &str) -> bool {
    is_a_recognisable_rewrite(ours, theirs)
        && (REORDERED.iter().any(|&(t, _)| t == triple) || keeps_wmos_word_order(ours, theirs))
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
                if wording_survives((d, c, n), ours, expected) {
                    continue;
                }
                unexpected.push(format!(
                    "  {d}/{c}/{n}: accepted as a rewrite of eccodes {expected:?}, but \
                     ours {ours:?} does not read as one — it shares no word, states a \
                     number eccodes does not, or reorders eccodes' words without an \
                     entry on REORDERED"
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
            wording_survives((d, c, n), ours, expected),
            "{d}/{c}/{n}: ours {ours:?} is not a recognisable rewrite of eccodes \
             {expected:?} — that is not a shortening, it is a different parameter"
        );
    }
}

/// Each word-order exemption must still be needed, and must be an accepted
/// divergence in the first place.
///
/// Held in both directions so an entry cannot outlive its reason. Vacuous while
/// [`REORDERED`] is empty, which is the intended state; the assertions exist so
/// the first entry added arrives with a check already on it.
#[test]
fn every_reordering_exemption_is_still_needed() {
    let oracle = eccodes_names();
    for &((d, c, n), why) in REORDERED {
        assert!(
            ACCEPTED
                .iter()
                .any(|&(ad, ac, an, _)| (ad, ac, an) == (d, c, n)),
            "{d}/{c}/{n} is exempt from the word-order rule but is not an accepted \
             divergence at all ({why})"
        );
        let (_, ours, _) = lookup_parameter(MASTER_ONLY, d, c, n)
            .unwrap_or_else(|| panic!("{d}/{c}/{n} no longer resolves"));
        let expected = oracle
            .get(&(d, c, n))
            .unwrap_or_else(|| panic!("{d}/{c}/{n} is not in the eccodes snapshot"));
        assert!(
            !keeps_wmos_word_order(ours, expected),
            "{d}/{c}/{n}: ours {ours:?} keeps eccodes' word order now — drop the \
             exemption rather than leaving a hole in the swap rule"
        );
    }
}

/// Two labels swapped between triples, which is the failure the rest of this
/// file cannot see.
///
/// Both strings still exist, and eccodes' recorded wording is untouched, so
/// [`every_accepted_divergence_is_still_a_divergence`] passes on the half that
/// pins the authority. Only [`wording_survives`] stands between a swap and a
/// green suite, and until #655 it asked for one shared word of five characters
/// or more, which the three aerosol optical thicknesses all satisfy against
/// each other — their labels are the same sentence with a different wavelength.
/// Rotating them relabels a 0.635 µm channel as 1.640 µm, and it passed.
///
/// A swap changes *both* entries, so catching either direction catches it.
#[test]
fn a_label_swap_between_triples_is_rejected() {
    let oracle = eccodes_names();
    let theirs = |t: (u8, u8, u8)| -> &str {
        oracle
            .get(&t)
            .unwrap_or_else(|| panic!("{t:?} is not in the eccodes snapshot"))
    };
    let ours = |t: (u8, u8, u8)| -> &'static str {
        let (_, name, _) = lookup_parameter(MASTER_ONLY, t.0, t.1, t.2)
            .unwrap_or_else(|| panic!("{t:?} no longer resolves"));
        name
    };

    for &(a, b, why) in &[
        (
            (3u8, 1u8, 20u8),
            (3u8, 1u8, 21u8),
            "a 0.635 µm aerosol channel labelled 0.810 µm",
        ),
        (
            (3, 1, 20),
            (3, 1, 22),
            "a 0.635 µm aerosol channel labelled 1.640 µm",
        ),
        (
            (3, 1, 21),
            (3, 1, 22),
            "a 0.810 µm aerosol channel labelled 1.640 µm",
        ),
    ] {
        let onto_a = wording_survives(a, ours(b), theirs(a));
        let onto_b = wording_survives(b, ours(a), theirs(b));
        assert!(
            !(onto_a && onto_b),
            "swapping {a:?} and {b:?} passes the wording rule — {why}"
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
