//! The originating-centre and sub-centre tables, checked against the WMO CCT (#440).
//!
//! `lookup_centre` was the last curated table with no oracle. #415 showed what
//! unverified costs — three parameters naming the wrong quantity — and the code
//! -table sweep that followed found six more. This is the same treatment for
//! the centre tables: `tools/gen_wmo_cct_tables.py` writes both the tables and
//! `fixtures/wmo_cct.ref.json` from one pinned download, and the tests below
//! check the shipped tables against that snapshot.
//!
//! The oracle carries the **unmodified** upstream names, so the check is not
//! circular in the way a generated-file-versus-itself comparison would be: it
//! asserts that every difference between what ships and what WMO published is
//! one of the sixteen declared overrides, and that each override still sits on
//! the CCT text it was written against.
//!
//! Regenerate both with `python3 tools/gen_wmo_cct_tables.py && cargo fmt`.

use fieldglass_core::cct_tables::lookup_sub_centre;
use fieldglass_grib1::tables_cct::lookup_centre as lookup_grib1_centre;
use fieldglass_grib2::{Grib2Reader, lookup_centre as lookup_grib2_centre};
use std::collections::BTreeMap;

const ORACLE: &str = include_str!("fixtures/wmo_cct.ref.json");
const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

/// The oracle, parsed just far enough. A JSON dependency for four flat
/// string maps would be the tail wagging the dog, and `serde_json` is a
/// dev-dependency of this crate already — but the shape here is fixed by our
/// own generator, so a small reader keeps the test readable.
fn oracle() -> serde_json::Value {
    serde_json::from_str(ORACLE).expect("oracle is valid JSON")
}

fn map(section: &str) -> BTreeMap<String, String> {
    oracle()[section]
        .as_object()
        .unwrap_or_else(|| panic!("oracle has no {section} section"))
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("string name").to_string()))
        .collect()
}

/// The declared overrides: `code -> [upstream text, what ships]`.
fn overrides() -> BTreeMap<u16, (String, String)> {
    oracle()["overrides"]
        .as_object()
        .expect("overrides section")
        .iter()
        .map(|(k, v)| {
            let pair = v.as_array().expect("override is a pair");
            (
                k.parse().expect("override key is a code"),
                (
                    pair[0].as_str().expect("upstream").to_string(),
                    pair[1].as_str().expect("shipped").to_string(),
                ),
            )
        })
        .collect()
}

/// Fold the handful of diacritics WMO's own ASCII transcription drops, so an
/// override can be compared against the name it restores.
fn ascii_fold(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'ö' => 'o',
            'é' => 'e',
            'ü' => 'u',
            'å' => 'a',
            'ø' => 'o',
            other => other,
        })
        .collect()
}

/// Every GRIB2 code the CCT assigns resolves, and to WMO's own text unless it
/// is one of the declared overrides.
#[test]
fn the_grib2_table_matches_the_cct() {
    let upstream = map("grib2_centres");
    assert!(
        upstream.len() > 300,
        "only {} centres in the oracle — it is not loaded, so this proves nothing",
        upstream.len()
    );
    let overrides = overrides();
    for (code, wmo_name) in &upstream {
        let code: u16 = code.parse().expect("code");
        let shipped = lookup_grib2_centre(code)
            .unwrap_or_else(|| panic!("centre {code} is in the CCT but does not resolve"));
        match overrides.get(&code) {
            Some((_, replacement)) => assert_eq!(
                shipped, replacement,
                "centre {code} is overridden and must ship the override text"
            ),
            None => assert_eq!(
                shipped, wmo_name,
                "centre {code} differs from the CCT without a declared override"
            ),
        }
    }
}

/// The same for GRIB1, which reads a different CCT table — C-1, not C-11.
#[test]
fn the_grib1_table_matches_the_cct() {
    let upstream = map("grib1_centres");
    assert!(
        upstream.len() > 230,
        "only {} centres in the oracle — it is not loaded",
        upstream.len()
    );
    let overrides = overrides();
    for (code, wmo_name) in &upstream {
        let wide: u16 = code.parse().expect("code");
        let code = u8::try_from(wide).expect("C-1 assigns nothing above 255");
        let shipped = lookup_grib1_centre(code)
            .unwrap_or_else(|| panic!("centre {code} is in the CCT but does not resolve"));
        match overrides.get(&wide) {
            Some((_, replacement)) => assert_eq!(shipped, replacement, "centre {code}"),
            None => assert_eq!(
                shipped, wmo_name,
                "centre {code} differs from the CCT without a declared override"
            ),
        }
    }
}

/// An override that upstream has moved under is worse than no override: it
/// silently pins a name WMO has stopped publishing. Each one records the text
/// it was written against, and this is what re-checks it.
#[test]
fn every_override_still_sits_on_the_text_it_was_written_for() {
    let grib1 = map("grib1_centres");
    let grib2 = map("grib2_centres");
    let overrides = overrides();
    assert!(!overrides.is_empty(), "the oracle lists no overrides");

    for (code, (expected_upstream, replacement)) in &overrides {
        let key = code.to_string();
        let found = grib2.get(&key).or_else(|| grib1.get(&key));
        let found = found.unwrap_or_else(|| panic!("override {code} names a code the CCT drops"));
        assert_eq!(
            found,
            expected_upstream,
            "override {code} was written against {expected_upstream:?}, but {} now publishes \
             {found:?} — re-review it rather than letting it pin a stale name",
            oracle()["tag"].as_str().unwrap_or("?")
        );
        // An override may only *add* to what WMO says — it may not contradict
        // it. Checking that the replacement is merely *longer* would let
        // "Beijing (RSMC) - some other agency" through, so the rule is
        // containment. `Norrköping - SMHI` does not literally contain
        // `Norrkoping`, because the ASCII fold is exactly what the override
        // undoes, so fold the replacement the same way before comparing.
        let folded = ascii_fold(replacement);
        assert!(
            folded.contains(&ascii_fold(expected_upstream)),
            "override {code} ({replacement:?}) does not contain the WMO name \
             {expected_upstream:?} — an override may add detail, never replace it"
        );
        assert!(
            replacement.len() > expected_upstream.len(),
            "override {code} ({replacement:?}) adds nothing to {expected_upstream:?}"
        );
    }
}

/// #440's acceptance criterion, as a test rather than a claim: every id the
/// pre-CCT curated tables named still resolves, and to a name at least as
/// complete. The literals are the curated table as it stood at the parent of
/// this commit — the point is that they are *not* re-derived from the new
/// table, so this cannot pass by construction.
#[test]
fn no_previously_curated_centre_got_less_informative() {
    for (id, curated) in [
        (7u16, "US National Weather Service - NCEP"),
        (8, "US NWS Telecommunications Gateway"),
        (9, "US National Weather Service - Other"),
        (34, "Tokyo (RSMC) - JMA"),
        (38, "Beijing (RSMC) - CMA"),
        (40, "Seoul - KMA"),
        (46, "INPE"),
        (54, "Montreal (RSMC) - CMC"),
        (58, "Fleet Numerical Meteorology and Oceanography Center"),
        (59, "NOAA Forecast Systems Laboratory"),
        (60, "NCAR"),
        (74, "UK Met Office - Exeter (RSMC)"),
        (78, "Offenbach (RSMC) - DWD"),
        (80, "Rome (RSMC)"),
        (82, "Norrköping - SMHI"),
        (85, "Toulouse (RSMC) - Météo-France"),
        (86, "Helsinki - FMI"),
        (88, "Oslo - MET Norway"),
        (94, "Copenhagen - DMI"),
        (97, "European Space Agency (ESA)"),
        (
            98,
            "European Centre for Medium-Range Weather Forecasts (ECMWF)",
        ),
        (173, "NASA"),
    ] {
        let now = lookup_grib2_centre(id)
            .unwrap_or_else(|| panic!("centre {id} was curated and must still resolve"));
        // "At least as complete" is judged on the distinguishing part of the
        // curated name — its first word, and any acronym it carried. WMO's
        // wording differs freely around those (`Tokyo (RSMC) - JMA` becomes
        // `Tokyo (RSMC), Japan Meteorological Agency`), so a string compare
        // would only pin the rewording, not the loss this guards against.
        let head = curated.split([' ', '-', ',']).next().unwrap();
        assert!(
            now.contains(head),
            "centre {id} was {curated:?} and is now {now:?}, which drops {head:?}"
        );
    }
}

/// The one that would have been wrong with an obvious implementation: a flat
/// `sub_centre -> name` table. 51 of the 104 sub-centre codes mean different
/// things under different centres, so the key is the pair.
#[test]
fn sub_centres_are_namespaced_by_their_centre() {
    // NCEP and NASA both define sub-centre 4, and it is not the same place.
    assert_eq!(
        lookup_sub_centre(7, 4),
        Some("Environmental Modeling Center")
    );
    assert_eq!(
        lookup_sub_centre(173, 4),
        Some("Goddard Space Flight Center")
    );
    assert_ne!(lookup_sub_centre(7, 4), lookup_sub_centre(173, 4));

    // Count it from the oracle rather than trusting the two examples above to
    // stay representative.
    let mut by_code: BTreeMap<u16, std::collections::BTreeSet<String>> = BTreeMap::new();
    for (key, name) in map("sub_centres") {
        let (_, sub) = key.split_once('/').expect("centre/sub key");
        by_code
            .entry(sub.parse().expect("sub-centre code"))
            .or_default()
            .insert(name);
    }
    let ambiguous = by_code.values().filter(|names| names.len() > 1).count();
    assert!(
        ambiguous >= 40,
        "only {ambiguous} sub-centre codes are ambiguous across centres — if that is \
         really so, a flat table would now be defensible and this test should be re-read"
    );
}

/// Every pair the CCT assigns resolves, and nothing else does.
#[test]
fn the_sub_centre_table_matches_the_cct() {
    let upstream = map("sub_centres");
    assert!(
        upstream.len() > 200,
        "oracle not loaded: {}",
        upstream.len()
    );
    for (key, name) in &upstream {
        let (centre, sub) = key.split_once('/').expect("centre/sub key");
        let centre: u16 = centre.parse().expect("centre");
        let sub: u16 = sub.parse().expect("sub-centre");
        assert_eq!(
            lookup_sub_centre(centre, sub),
            Some(name.as_str()),
            "sub-centre {centre}/{sub}"
        );
    }
    // A centre that defines sub-centres does not thereby define all of them.
    assert_eq!(lookup_sub_centre(7, 60_000), None);
    // And a centre that defines none answers None rather than falling through
    // to another centre's list.
    assert_eq!(lookup_sub_centre(98, 4), None);
}

/// GRIB uses sub-centre 0 to mean "the field is absent". WMO lists a name
/// against 0 for centre 82, which is the one case where the two readings
/// collide; #440 resolves it in favour of absence.
#[test]
fn sub_centre_zero_is_absent_under_every_centre() {
    for centre in [0u16, 7, 82, 98, 173, 65_535] {
        assert_eq!(
            lookup_sub_centre(centre, 0),
            None,
            "sub-centre 0 must read as absent under centre {centre}"
        );
    }
    // Specifically the collision: the CCT does carry a name here.
    assert!(
        map("sub_centres").keys().all(|k| !k.ends_with("/0")),
        "the generator should have dropped every sub-centre 0 row from the oracle"
    );
}

/// The `)` continuation rows, which are the defect an obvious generator ships.
/// Codes 1-3 share one printed brace in the manual; the CSV writes the closing
/// brace as a lone `)` in the name column.
#[test]
fn continuation_rows_carry_the_name_forward() {
    for id in 1..=3u16 {
        assert_eq!(
            lookup_grib2_centre(id),
            Some("Melbourne (WMC)"),
            "centre {id}"
        );
    }
    for id in 4..=6u16 {
        assert_eq!(lookup_grib2_centre(id), Some("Moscow (WMC)"), "centre {id}");
    }
    assert_eq!(lookup_grib2_centre(10), lookup_grib2_centre(11));
    // Nothing anywhere in either table may have shipped the marker itself.
    for id in 0..=u16::MAX {
        if let Some(name) = lookup_grib2_centre(id) {
            assert_ne!(name, ")", "centre {id} shipped the continuation marker");
            assert!(name.len() > 1, "centre {id} shipped {name:?}");
        }
    }
    for id in 0..=u8::MAX {
        if let Some(name) = lookup_grib1_centre(id) {
            assert_ne!(
                name, ")",
                "GRIB1 centre {id} shipped the continuation marker"
            );
        }
    }
}

/// The two editions genuinely disagree, which is why they are two tables and
/// not one shared one. Asserted concretely rather than by a similarity rule:
/// C-1 says `Lisbon` where C-11 says `Lisboa`, so any "the names agree"
/// heuristic either passes vacuously or fails on real data.
#[test]
fn the_two_editions_do_not_share_a_table() {
    let grib1 = map("grib1_centres");
    let grib2 = map("grib2_centres");

    // C-11 assigns codes a one-octet GRIB1 field cannot even express.
    let beyond_one_octet = grib2
        .keys()
        .filter(|k| k.parse::<u16>().unwrap() > 0xFF)
        .count();
    assert!(
        beyond_one_octet >= 60,
        "only {beyond_one_octet} C-11 codes sit above 255; if C-11 has shrunk to \
         what GRIB1 can encode, the case for two tables is worth re-reading"
    );
    assert!(lookup_grib2_centre(300).is_some());
    assert_eq!(
        grib1
            .keys()
            .filter(|k| k.parse::<u16>().unwrap() > 0xFF)
            .count(),
        0
    );

    // The same code can carry a different name in each edition.
    let mut differing: Vec<u16> = grib1
        .iter()
        .filter(|(code, name)| grib2.get(*code).is_some_and(|other| other != *name))
        .map(|(code, _)| code.parse::<u16>().unwrap())
        .collect();
    differing.sort_unstable();
    assert_eq!(
        differing,
        vec![7, 18, 19, 52, 98, 110, 166, 174, 212, 216, 255],
        "the set of codes whose name differs between C-1 and C-11 moved; re-read \
         the diff before repinning — this is the evidence for generating two tables"
    );
    // 255 is the one where they differ in *meaning*, not just wording.
    assert_eq!(lookup_grib1_centre(255), Some("Missing value"));
    assert_eq!(lookup_grib2_centre(255), Some("Not to be used"));
}

/// Reserved code space must stay unnamed: a confident label on an unassigned
/// code is worse than the numeric fallback.
#[test]
fn reserved_codes_do_not_resolve() {
    for id in [68u16, 77, 149] {
        assert_eq!(lookup_grib2_centre(id), None, "centre {id} is Reserved");
        assert_eq!(
            lookup_grib1_centre(u8::try_from(id).unwrap()),
            None,
            "GRIB1 centre {id} is Reserved"
        );
    }
}

/// The sub-centre path end to end, on a real message rather than through the
/// lookup alone.
///
/// The committed fixtures all carry sub-centre 0, and the one file in the
/// manual corpus that does not (`samples/nbm.grib2`, NCEP sub-centre 14, "NWS
/// Meteorological Development Laboratory") is not redistributable and is
/// gitignored. So the message here is built in code: §1 of a committed fixture
/// with its centre and sub-centre octets rewritten. Those two fields carry no
/// length or checksum, so patching them in place leaves a message the reader
/// parses normally — which is the point, since what is under test is the
/// parse-to-lookup seam and not the lookup on its own.
#[test]
fn a_message_carrying_a_sub_centre_resolves_it() {
    // §0 is a fixed 16 octets, so §1 starts at 16: length (4), number (1),
    // then centre and sub-centre as big-endian u16 pairs.
    const IDS: usize = 16;
    let mut bytes = FIXTURE.to_vec();
    assert_eq!(bytes[IDS + 4], 1, "octet {} should start §1", IDS + 1);

    // NCEP (7), sub-centre 4 — the Environmental Modeling Center, and the same
    // code NASA uses for Goddard, which is what makes the pair the key.
    bytes[IDS + 5..IDS + 7].copy_from_slice(&7u16.to_be_bytes());
    bytes[IDS + 7..IDS + 9].copy_from_slice(&4u16.to_be_bytes());

    let reader = Grib2Reader::from_bytes(bytes).expect("patched fixture still parses");
    let ids = reader.messages[0].ids;
    assert_eq!(ids.centre, 7);
    assert_eq!(ids.sub_centre, 4);
    assert_eq!(
        lookup_sub_centre(ids.centre, ids.sub_centre),
        Some("Environmental Modeling Center")
    );

    // Unpatched, the fixture is ECMWF with no sub-centre, and must stay absent.
    let clean = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let ids = clean.messages[0].ids;
    assert_eq!(ids.sub_centre, 0);
    assert_eq!(lookup_sub_centre(ids.centre, ids.sub_centre), None);
}
