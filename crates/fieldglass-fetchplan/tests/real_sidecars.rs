//! The parser against real, unmodified sidecars from five NCEP products and
//! ECMWF.
//!
//! The unit tests in `src/` pin behaviour against hand-written lines, which is
//! the right shape for an edge case but proves nothing about what the producers
//! actually emit. These run over the committed corpus — see
//! [`fixtures/NOTICE.md`](fixtures/NOTICE.md) for each file's provenance — and
//! assert the invariants a host depends on:
//!
//! * every record parses, with no line skipped and none invented;
//! * `messages()` ranges are strictly ascending and non-overlapping;
//! * `items()` ranges are non-decreasing, and any two that are *equal* are
//!   sub-messages of one message and carry distinct sub-indices;
//! * one stored request selects the same field on an NCEP source and an ECMWF
//!   one.

use fieldglass_fetchplan::{
    EcmwfIndex, LevelSpec, Manifest, NoResolver, ParameterId, ParameterResolver, PlanItem,
    PlanRange, Query, Surface, Wgrib2Idx,
};

/// Read a committed fixture.
///
/// A path relative to the crate directory, not an absolute one built from
/// `CARGO_MANIFEST_DIR`: this suite also runs under `wasmtime --dir=. --dir=..`
/// for the 32-bit pointer check, and the sandbox cannot open an absolute host
/// path.
fn fixture(name: &str) -> String {
    let path = format!("tests/fixtures/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// The five NCEP sidecars, by fixture name.
const NCEP: &[&str] = &[
    "gfs_0p25.idx",
    "hrrr_conus_wrfsfc.idx",
    "nam_awip12.idx",
    "nbm_core_co.idx",
    "gefs_p01_0p50.idx",
];

/// Every non-blank line becomes exactly one record.
///
/// The count is the check that matters most on a real file: a parser that
/// quietly skipped a line it did not understand would still produce a plausible
/// plan, with one field silently missing from it and every neighbouring range
/// too long.
#[test]
fn every_line_of_every_sidecar_becomes_one_record() {
    for name in NCEP {
        let text = fixture(name);
        let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
        let idx = Wgrib2Idx::parse(*name, &text).expect(name);
        assert_eq!(idx.items().len(), lines, "{name}");
        assert_eq!(idx.key(), *name);
    }

    let text = fixture("ecmwf_ifs_0p25_oper.index");
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let idx = EcmwfIndex::parse("ifs.grib2", &text).expect("ecmwf");
    assert_eq!(idx.items().len(), lines);
    assert_eq!(lines, 187);
}

/// Whole-message ranges must be strictly ascending and must not overlap — the
/// invariant a host schedules fetches against, and the one the sub-message
/// records deliberately break.
#[test]
fn message_ranges_ascend_and_never_overlap() {
    let mut checked = 0;
    for name in NCEP {
        let idx = Wgrib2Idx::parse(*name, &fixture(name)).expect(name);
        assert_ascending_disjoint(&idx.messages(), name);
        checked += 1;
    }
    let idx = EcmwfIndex::parse("ifs.grib2", &fixture("ecmwf_ifs_0p25_oper.index")).unwrap();
    assert_ascending_disjoint(&idx.messages(), "ecmwf");
    assert_eq!(checked, 5);
}

fn assert_ascending_disjoint(items: &[PlanItem], name: &str) {
    assert!(items.len() > 1, "{name}: too few items to be a real check");
    for pair in items.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(
            b.range.offset() > a.range.offset(),
            "{name}: offsets are not strictly ascending: {a:?} then {b:?}"
        );
        if let Some(end) = a.range.end_exclusive() {
            assert!(
                end <= b.range.offset(),
                "{name}: {a:?} overlaps {b:?} (ends at {end})"
            );
        }
        // A collapsed representative addresses a whole message, so it must
        // never claim to be one field of it.
        assert_eq!(a.sub_index, None, "{name}");
    }
    // Only the final record of a wgrib2 sidecar may be open-ended; every other
    // one is bounded by its successor.
    let open: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i.range, PlanRange::OpenEnded { .. }))
        .map(|(n, _)| n)
        .collect();
    assert!(
        open.is_empty() || open == [items.len() - 1],
        "{name}: open-ended ranges at {open:?}"
    );
}

/// The record-level list may repeat a range, and when it does the repeat is a
/// sub-message: same bytes, different field, distinct index.
#[test]
fn only_sub_messages_share_a_range_and_they_are_numbered_apart() {
    let idx = Wgrib2Idx::parse("nam", &fixture("nam_awip12.idx")).unwrap();
    let items = idx.items();

    let mut shared = 0;
    for pair in items.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(
            b.range.offset() >= a.range.offset(),
            "record offsets must not descend"
        );
        if a.range == b.range {
            shared += 1;
            assert!(
                a.sub_index.is_some() && b.sub_index.is_some(),
                "{a:?} {b:?}"
            );
            assert_ne!(a.sub_index, b.sub_index, "{a:?} {b:?}");
        }
    }
    // NAM pairs UGRD/VGRD and USTM/VSTM: ten pairs, twenty records.
    assert_eq!(
        shared, 10,
        "expected ten sub-message pairs in the NAM sidecar"
    );
    assert_eq!(items.len() - idx.messages().len(), 10);
}

/// A real sub-message pair is one fetch and two fields, and the second field is
/// reachable only through its index.
#[test]
fn a_nam_wind_pair_is_one_fetch_and_two_fields() {
    let idx = Wgrib2Idx::parse("nam", &fixture("nam_awip12.idx")).unwrap();
    let hits = idx.select(
        &Query::abbreviation("VGRD").at_level_text("10 m above ground"),
        &NoResolver,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].sub_index, Some(2));

    let u = idx.select(
        &Query::abbreviation("UGRD").at_level_text("10 m above ground"),
        &NoResolver,
    );
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].sub_index, Some(1));
    // Same bytes; the host fetches once and decodes the field it wants.
    assert_eq!(u[0].range, hits[0].range);
    assert!(matches!(u[0].range, PlanRange::Exact { .. }));
}

/// NBM publishes a plain deterministic field *and* five probabilistic ones
/// under one abbreviation and level. An under-specified query must return all
/// six rather than pick one — and both halves of the set must then be
/// reachable, which is what `unqualified()` is for. Taking the first match
/// would return the deterministic record here and so look correct by accident;
/// the same shortcut on `TMP` with `ENS=` members returns member 1 and calls it
/// the forecast.
#[test]
fn an_ambiguous_nbm_request_returns_every_match() {
    let idx = Wgrib2Idx::parse("nbm", &fixture("nbm_core_co.idx")).unwrap();
    let ceiling = Query::abbreviation("CEIL").at_level_text("cloud ceiling");

    let all = idx.select(&ceiling, &NoResolver);
    assert_eq!(all.len(), 6, "{all:#?}");
    let probabilistic = all
        .iter()
        .filter(|i| i.expect.qualifiers.iter().any(|q| q.starts_with("prob ")))
        .count();
    assert_eq!(
        probabilistic, 5,
        "five thresholds and one deterministic field"
    );

    // Narrowing by threshold reaches one of the five.
    let one = idx.select(&ceiling.clone().with_qualifier("prob <304.8"), &NoResolver);
    assert_eq!(one.len(), 1);
    assert!(
        one[0]
            .expect
            .qualifiers
            .contains(&"prob fcst 3/7".to_string())
    );

    // …and `unqualified` reaches the sixth, which nothing else can name.
    let plain = idx.select(&ceiling.unqualified(), &NoResolver);
    assert_eq!(plain.len(), 1);
    assert!(plain[0].expect.qualifiers.is_empty());
    assert_ne!(plain[0].range, one[0].range);
}

/// Every GEFS record carries its member tag, and the product writes no trailing
/// colon — so a parser that assumed one would put an empty string in every
/// qualifier list.
#[test]
fn every_gefs_record_carries_its_member_qualifier() {
    let idx = Wgrib2Idx::parse("gefs", &fixture("gefs_p01_0p50.idx")).unwrap();
    let items = idx.items();
    assert!(
        items
            .iter()
            .all(|i| i.expect.qualifiers == ["ENS=+1".to_string()]),
        "expected exactly one qualifier per record"
    );
    assert_eq!(
        idx.select(&Query::default().with_qualifier("ENS=+1"), &NoResolver)
            .len(),
        items.len()
    );
}

/// HRRR's numeric fallback name is read into codes off the real file, not just
/// off a hand-written line.
#[test]
fn the_hrrr_sidecar_states_wmo_codes_for_one_parameter() {
    let idx = Wgrib2Idx::parse("hrrr", &fixture("hrrr_conus_wrfsfc.idx")).unwrap();
    let numeric: Vec<_> = idx
        .items()
        .into_iter()
        .filter(|i| i.expect.parameter.is_some())
        .collect();
    assert!(
        !numeric.is_empty(),
        "the HRRR fixture should carry at least one `var discipline=…` record"
    );
    assert_eq!(
        numeric[0].expect.parameter,
        Some(ParameterId {
            discipline: 0,
            category: 16,
            number: 201
        })
    );
}

/// The whole ECMWF object is described contiguously: each record begins where
/// the last one ended. That is not required by the format, but it is what the
/// producer emits, and a parser that mis-read `_offset` or `_length` would
/// break it immediately.
#[test]
fn the_ecmwf_records_tile_the_object_without_gaps() {
    let idx = EcmwfIndex::parse("ifs.grib2", &fixture("ecmwf_ifs_0p25_oper.index")).unwrap();
    let items = idx.items();
    let mut cursor = items[0].range.offset();
    for item in &items {
        assert_eq!(item.range.offset(), cursor, "gap or overlap at {item:?}");
        cursor = item
            .range
            .end_exclusive()
            .expect("every ECMWF range states an end");
        // The dialect promises a length, so the expectation carries it and a
        // consumer can check §0 against the sidecar as well as against the
        // fetch.
        assert_eq!(item.expect.total_length, item.range.length());
    }
    assert!(cursor > 100_000_000, "the object should be large: {cursor}");
}

/// A resolver over both vocabularies, standing in for the umbrella's.
struct BothDialects;

impl ParameterResolver for BothDialects {
    fn resolve(&self, abbrev: &str, _level: &str) -> Option<ParameterId> {
        // 2-metre temperature: `TMP` at NCEP, `2t` at ECMWF.
        matches!(abbrev, "TMP" | "2t").then_some(ParameterId {
            discipline: 0,
            category: 0,
            number: 0,
        })
    }
}

/// The point of the level grammar and the resolver together: one stored
/// request — WMO codes plus a surface — selects the right field on an NCEP
/// source and an ECMWF one, without the caller knowing either spelling.
#[test]
fn one_stored_request_selects_on_both_sources() {
    let temperature = ParameterId {
        discipline: 0,
        category: 0,
        number: 0,
    };

    let ncep = Wgrib2Idx::parse("gfs", &fixture("gfs_0p25.idx")).unwrap();
    let hits = ncep.select(
        &Query::parameter(temperature).at_level(LevelSpec::at(Surface::HeightAboveGround, 2.0)),
        &BothDialects,
    );
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].expect.abbreviation.as_deref(), Some("TMP"));
    assert_eq!(hits[0].expect.level.as_deref(), Some("2 m above ground"));

    let ecmwf = EcmwfIndex::parse("ifs", &fixture("ecmwf_ifs_0p25_oper.index")).unwrap();
    let hits = ecmwf.select(
        &Query::parameter(temperature).at_level(LevelSpec::named(Surface::Surface)),
        &BothDialects,
    );
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].expect.abbreviation.as_deref(), Some("2t"));

    // The same query with no resolver finds nothing on either, which is what
    // makes the two assertions above about the resolver rather than about the
    // abbreviation happening to match.
    assert!(
        ncep.select(&Query::parameter(temperature), &NoResolver)
            .is_empty()
    );
    assert!(
        ecmwf
            .select(&Query::parameter(temperature), &NoResolver)
            .is_empty()
    );
}

/// A plan crosses a host boundary as JSON — the wasm binding hands it to
/// JavaScript — so it has to survive the round trip whole, including the range
/// shape, which is a tagged enum and the easiest part to get wrong.
#[test]
fn a_plan_item_survives_a_json_round_trip() {
    let idx = Wgrib2Idx::parse("nbm", &fixture("nbm_core_co.idx")).unwrap();
    for item in idx.items().iter().take(20).chain(idx.items().last()) {
        let json = serde_json::to_string(item).expect("serialises");
        let back: PlanItem = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(&back, item);
    }
}

/// Every fixture's plan closes against a plausible object size, and closing
/// yields the `[start, len)` the byte-source seam takes.
#[test]
fn an_open_ended_plan_closes_onto_the_byte_source_type() {
    let idx = Wgrib2Idx::parse("gfs", &fixture("gfs_0p25.idx")).unwrap();
    let messages = idx.messages();
    let last = messages.last().unwrap();
    assert!(matches!(last.range, PlanRange::OpenEnded { .. }));

    // A size the host learned from a HEAD, or from the bucket listing.
    let object_size = last.range.offset() + 1_000_000;
    let closed = last.range.close(object_size).unwrap();
    assert_eq!(closed.start, last.range.offset());
    assert_eq!(closed.len, 1_000_000);
    assert_eq!(closed.end(), Some(object_size));
}
