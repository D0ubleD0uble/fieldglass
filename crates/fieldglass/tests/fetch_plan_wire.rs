//! The fetch planner as a host receives it (#1081), recorded so the browser
//! host can be held to the same answers.
//!
//! `fetch_plan_slices_a_message.rs` proves a plan slices the message it
//! promised. This proves the **wire form** of the same path, over a real
//! sidecar and the real bytes one of its records points at: a query as a host
//! sends it (JSON), the plan items and `Range` headers it gets back, the
//! envelope check on the fetched bytes, and the run discovery in front of it.
//!
//! Every answer is written to [`RECORDING`]. `crates/fieldglass-wasm/tests/
//! node/fetchplan.mjs` reads the same file, asks the built browser module the
//! same questions, and compares. That is the Rust-versus-wasm parity check: one
//! recording, two runners, the pattern the conformance suite uses (ADR-0006
//! decision 3). The queries live in the recording too, so the two runners
//! cannot drift apart by asking different things.
//!
//! The recording is compared exactly, and rewritten when
//! [`UPDATE_ENV`] is set — and the run then **fails**, because a run that
//! re-recorded has verified nothing.
//!
//! The inputs are `include_str!` / `include_bytes!` rather than read from disk,
//! so the planning half needs no filesystem under `wasm32-wasip1`; the
//! recording is read by a path relative to this crate, which the runner's
//! `--dir=.` grants.

use fieldglass::Session;
use fieldglass::fetchplan::{
    CandidateOptions, EcmwfIndex, Expect, MAX_CANDIDATES, Manifest, MessageManifest, PlanItem,
    PlanRange, Query, SourceSpec, TableResolver, Wgrib2Idx, candidates_at, verify_envelope,
    verify_message,
};
use serde_json::{Value, json};

/// GFS 0.25°, 2026-09-04 00Z analysis: 696 records, all exact but the last.
const GFS_IDX: &str = include_str!("../../fieldglass-fetchplan/tests/fixtures/gfs_0p25.idx");
/// The object key that sidecar describes, as a host would name it.
const GFS_KEY: &str = "gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f000";
/// Record 107 of [`GFS_IDX`], fetched from that object: `RH:2 mb:anl`.
const GFS_RH_2MB: &[u8] = include_bytes!("fixtures/gfs_0p25_rh_2mb.grib2");

/// ECMWF IFS 0.25°, the same cycle: every range exact.
const ECMWF_INDEX: &str =
    include_str!("../../fieldglass-fetchplan/tests/fixtures/ecmwf_ifs_0p25_oper.index");
const ECMWF_KEY: &str = "20260904/00z/ifs/0p25/oper/20260904000000-0h-oper-fc.grib2";

/// Relative to this crate's directory, which cargo makes the working directory.
const RECORDING: &str = "tests/fetch_plan_wire.json";
/// Set to rewrite [`RECORDING`] rather than compare against it.
const UPDATE_ENV: &str = "FIELDGLASS_UPDATE_PLAN";

/// The queries asked of each sidecar, as a host sends them.
///
/// The GFS pair is the point of the file: the sidecar's own words, and then the
/// meaning — WMO codes through the table-backed resolver, and a level through
/// the level grammar — selecting the same record.
fn gfs_queries() -> Vec<Value> {
    vec![
        json!({ "abbreviation": "RH", "levelText": "2 mb" }),
        json!({
            "parameter": { "discipline": 0, "category": 1, "number": 1 },
            "level": { "surface": "isobaric", "value": 2.0, "value2": null }
        }),
        json!({ "abbreviation": "TMP", "levelText": "2 m above ground" }),
        // The same record by meaning, as the browser host's README spells it.
        json!({
            "parameter": { "discipline": 0, "category": 0, "number": 0 },
            "level": { "surface": "height_above_ground", "value": 2.0, "value2": null }
        }),
        // The last record, the one range a `.idx` leaves open.
        json!({ "abbreviation": "VWSH", "levelText": "PV=-2e-06 (Km^2/kg/s) surface" }),
        // Nothing has this name: an empty plan, not an error.
        json!({ "abbreviation": "NOTAPARAMETER" }),
    ]
}

fn ecmwf_queries() -> Vec<Value> {
    vec![
        json!({ "abbreviation": "2t" }),
        json!({
            "abbreviation": "t",
            "level": { "surface": "isobaric", "value": 500.0, "value2": null }
        }),
    ]
}

/// One catalog entry, the run discovery's input, with a clock set five and a
/// half hours after the sidecars' cycle.
fn gfs_source() -> Value {
    json!({
        "keyPattern": "gfs.{yyyy}{mm}{dd}/{HH}/atmos/gfs.t{HH}z.pgrb2.0p25.f{fff}",
        "cycleHours": [0, 6, 12, 18],
        "latencyMinutes": 240,
        "maxCandidates": 3
    })
}

fn wire<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a wire type serialises")
}

fn query(value: &Value) -> Query {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("the query {value} does not read: {e}"))
}

/// One sidecar's answers: its size, its first and last item, and each query's
/// plan with the `Range` header of every item in it.
fn record_manifest(
    manifest: &dyn MessageManifest,
    queries: &[Value],
    resolver: &TableResolver,
) -> Value {
    let items = manifest.items();
    let selections: Vec<Value> = queries
        .iter()
        .map(|q| {
            let plan = manifest.select(&query(q), resolver);
            let headers: Vec<Option<String>> = plan
                .iter()
                .map(|i| i.range.http_range_header().unwrap())
                .collect();
            json!({ "query": q, "items": wire(&plan), "rangeHeaders": headers })
        })
        .collect();
    json!({
        "itemCount": items.len(),
        "messageCount": manifest.messages().len(),
        "first": wire(&items[0]),
        "last": wire(items.last().expect("the sidecar is not empty")),
        "selections": selections,
    })
}

/// Everything the recording holds, computed now.
fn recording() -> Value {
    let resolver = TableResolver::ncep();
    let gfs = Wgrib2Idx::parse(GFS_KEY, GFS_IDX).expect("the GFS sidecar parses");
    let ecmwf = EcmwfIndex::parse(ECMWF_KEY, ECMWF_INDEX).expect("the ECMWF index parses");

    let rh = &gfs.select(&query(&gfs_queries()[0]), &resolver)[0];
    let declared = verify_envelope(rh, GFS_RH_2MB).expect("the fetched bytes are the record");
    let mut stale = GFS_RH_2MB.to_vec();
    stale[0] = b'X';

    let source: SourceSpec = serde_json::from_value(gfs_source()).expect("the source reads");
    let options = CandidateOptions::new(1_788_500_000);
    let mut no_cycles = gfs_source();
    no_cycles["cycleHours"] = json!([]);
    let mut too_many = gfs_source();
    too_many["maxCandidates"] = json!(1025);
    let mut bad_pattern = gfs_source();
    bad_pattern["keyPattern"] = json!("gfs.{yyyy}{zz}/gfs.t{HH}z");

    let code = |r: Result<u64, fieldglass::Error>| r.expect_err("refused").code();
    let source_code = |v: &Value| {
        let s: SourceSpec = serde_json::from_value(v.clone()).expect("the source reads");
        candidates_at(&s, &options)
            .expect_err("refused")
            .code()
            .to_string()
    };

    json!({
        "description": "Recorded by `FIELDGLASS_UPDATE_PLAN=1 cargo test -p fieldglass --test \
            fetch_plan_wire`; do not edit by hand. crates/fieldglass-wasm/tests/node/fetchplan.mjs \
            asks the browser module the same questions and compares (#1081). Paths are from the \
            repository root.",
        "wgrib2": {
            "path": "crates/fieldglass-fetchplan/tests/fixtures/gfs_0p25.idx",
            "key": GFS_KEY,
            "plan": record_manifest(&gfs, &gfs_queries(), &resolver),
        },
        "ecmwf": {
            "path": "crates/fieldglass-fetchplan/tests/fixtures/ecmwf_ifs_0p25_oper.index",
            "key": ECMWF_KEY,
            "plan": record_manifest(&ecmwf, &ecmwf_queries(), &resolver),
        },
        "envelope": {
            "path": "crates/fieldglass/tests/fixtures/gfs_0p25_rh_2mb.grib2",
            "item": wire(rh),
            "declaredLength": declared,
            "truncatedTo": 100,
            "truncatedCode": code(verify_envelope(rh, &GFS_RH_2MB[..100])),
            "staleCode": code(verify_envelope(rh, &stale)),
        },
        "sources": {
            "source": gfs_source(),
            "options": wire(&options),
            "candidates": wire(&candidates_at(&source, &options).expect("the source is valid")),
            "noCycleHours": { "source": no_cycles, "code": source_code(&no_cycles) },
            "tooManyCandidates": { "source": too_many, "code": source_code(&too_many) },
            "unknownPlaceholder": { "source": bad_pattern, "code": source_code(&bad_pattern) },
        },
    })
}

/// The recording is what the planner answers now.
#[test]
fn the_recorded_plan_is_what_the_planner_answers() {
    let now = recording();
    if std::env::var_os(UPDATE_ENV).is_some() {
        let text = serde_json::to_string_pretty(&now).expect("serialises") + "\n";
        std::fs::write(RECORDING, text).expect("the recording is writable");
        panic!("{RECORDING} re-recorded; unset {UPDATE_ENV} and run again to verify it");
    }
    let text = std::fs::read_to_string(RECORDING)
        .unwrap_or_else(|e| panic!("reading {RECORDING}: {e}; record it with {UPDATE_ENV}=1"));
    let recorded: Value = serde_json::from_str(&text).expect("the recording is JSON");
    assert_eq!(
        recorded, now,
        "the planner's answers moved from {RECORDING}; if that is intended, re-record with \
         {UPDATE_ENV}=1 and say why in the commit"
    );
}

/// The headline, asserted in full rather than only recorded: both GFS queries
/// select record 107, its range is exact and states the `Range` header the
/// object was fetched with, the bytes pass the envelope, and they decode to the
/// field the sidecar promised.
///
/// A recording only says the answer did not move. This says it is right.
#[test]
fn one_gfs_field_is_planned_fetched_and_verified() {
    let resolver = TableResolver::ncep();
    let gfs = Wgrib2Idx::parse(GFS_KEY, GFS_IDX).expect("the GFS sidecar parses");
    let queries = gfs_queries();

    let by_words = gfs.select(&query(&queries[0]), &resolver);
    let by_meaning = gfs.select(&query(&queries[1]), &resolver);
    assert_eq!(by_words.len(), 1, "one record is RH at 2 mb");
    assert_eq!(
        by_words, by_meaning,
        "the sidecar's words and the WMO codes select the same record"
    );

    // 2 m temperature, likewise, both ways.
    let tmp_words = gfs.select(&query(&queries[2]), &resolver);
    assert_eq!(tmp_words.len(), 1);
    assert_eq!(tmp_words, gfs.select(&query(&queries[3]), &resolver));

    let item: &PlanItem = &by_words[0];
    assert_eq!(item.key, GFS_KEY);
    assert_eq!(
        item.range,
        serde_json::from_value::<PlanRange>(
            json!({ "kind": "exact", "offset": 76_032_983, "length": 210 })
        )
        .expect("a range reads")
    );
    assert_eq!(
        item.range.http_range_header().unwrap().as_deref(),
        Some("bytes=76032983-76033192"),
        "the header the fixture was fetched with"
    );
    assert_eq!(verify_envelope(item, GFS_RH_2MB), Ok(210));

    let session = Session::open(GFS_RH_2MB.to_vec()).expect("the fetched message opens");
    let info = session.message(0).expect("one message");
    // The semantic half, asked of the parameter only. `verify_message` itself
    // is not asserted here: it compares the sidecar's level wording (`2 mb`)
    // with `MessageInfo::level`, which is the bare value in the surface's own
    // unit (`200`, pascals), so it refuses this correct message. That is a
    // defect in `verify_message` against real sidecars, reported separately
    // rather than worked around here.
    assert_eq!(
        info.abbreviation.as_deref(),
        item.expect.abbreviation.as_deref()
    );
    let promised = Expect::new().with_abbreviation("RH");
    assert_eq!(verify_message(&promised, &info), Ok(()));
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("decodes");
    assert_eq!((field.ni, field.nj), (1440, 721));
    // eccodes 2.34.1 reads the field as zero everywhere (NOTICE.md).
    assert_eq!((field.stats.min, field.stats.max), (Some(0.0), Some(0.0)));

    // The run discovery names the very object this sidecar describes, as its
    // newest candidate five and a half hours after the cycle.
    let source: SourceSpec = serde_json::from_value(gfs_source()).expect("the source reads");
    let runs = candidates_at(&source, &CandidateOptions::new(1_788_500_000)).expect("valid");
    assert_eq!(runs[0].key, GFS_KEY);
}

/// A fetch that came back short is `short_read`, which a host retries; bytes
/// that are not the message are `decode`, which a host does not. The two call
/// for opposite responses, which is why they are not one code.
#[test]
fn a_short_fetch_and_a_stale_sidecar_are_told_apart() {
    let resolver = TableResolver::ncep();
    let gfs = Wgrib2Idx::parse(GFS_KEY, GFS_IDX).expect("the GFS sidecar parses");
    let item = &gfs.select(&query(&gfs_queries()[0]), &resolver)[0];

    for short in [&GFS_RH_2MB[..4], &GFS_RH_2MB[..100], &GFS_RH_2MB[..209]] {
        let err = verify_envelope(item, short).expect_err("short");
        assert_eq!(err.code(), "short_read", "{} bytes: {err:?}", short.len());
    }

    let mut stale = GFS_RH_2MB.to_vec();
    stale[0] = b'X';
    assert_eq!(
        verify_envelope(item, &stale).map_err(|e| e.code()),
        Err("decode")
    );

    // More bytes than an exact range asked for is the sidecar and the object
    // disagreeing, not a short transfer.
    let mut long = GFS_RH_2MB.to_vec();
    long.extend_from_slice(b"GRIB");
    assert_eq!(
        verify_envelope(item, &long).map_err(|e| e.code()),
        Err("decode")
    );

    // The last record of a `.idx` is the open-ended one.
    let last = gfs.items().pop().expect("not empty");
    assert!(matches!(last.range, PlanRange::OpenEnded { .. }));

    // An open range may over-shoot: everything from the offset on is one
    // message and whatever followed it. Too few bytes for the length the
    // message declares, or for its indicator section, is a short transfer.
    let open = item_with(json!({ "kind": "openEnded", "offset": 76_032_983 }));
    let mut over = GFS_RH_2MB.to_vec();
    over.extend_from_slice(b"GRIB\0\0\0\x02");
    assert_eq!(verify_envelope(&open, &over), Ok(210));
    assert_eq!(
        verify_envelope(&open, &GFS_RH_2MB[..100]),
        Err(fieldglass::Error::ShortRead {
            at: 76_032_983,
            got: 100,
            wanted: 210
        })
    );
    assert_eq!(
        verify_envelope(&open, &GFS_RH_2MB[..4]).map_err(|e| e.code()),
        Err("short_read")
    );

    // An exact range that arrived whole but is too short to be a message is
    // the sidecar's mistake, not the transfer's: retrying would fetch the same
    // ten bytes again.
    let tiny = item_with(json!({ "kind": "exact", "offset": 76_032_983, "length": 10 }));
    assert_eq!(
        verify_envelope(&tiny, &GFS_RH_2MB[..10]).map_err(|e| e.code()),
        Err("decode")
    );
}

/// A plan item over `range` that promises nothing, as a host would send one
/// back.
fn item_with(range: Value) -> PlanItem {
    serde_json::from_value(json!({
        "key": GFS_KEY,
        "range": range,
        "address": { "kind": "message", "index": 106, "subIndex": null },
        "expect": {
            "abbreviation": null, "parameter": null, "level": null, "levelSpec": null,
            "forecast": null, "referenceTime": null, "totalLength": null, "qualifiers": []
        }
    }))
    .expect("a plan item reads")
}

/// A catalog entry the planner cannot expand is the caller's to fix, so it is
/// `invalid_option`; a manifest that does not parse is `decode`.
#[test]
fn planner_failures_take_the_codes_a_host_already_branches_on() {
    let mut no_cycles = gfs_source();
    no_cycles["cycleHours"] = json!([]);
    let source: SourceSpec = serde_json::from_value(no_cycles).expect("reads");
    assert_eq!(
        candidates_at(&source, &CandidateOptions::new(0)).map_err(|e| e.code()),
        Err("invalid_option")
    );

    // A count above the cap is the same kind of catalog fault, and is refused
    // rather than answered with an empty list.
    let mut too_many = gfs_source();
    too_many["maxCandidates"] = json!(MAX_CANDIDATES + 1);
    let source: SourceSpec = serde_json::from_value(too_many).expect("reads");
    let err = candidates_at(&source, &CandidateOptions::new(0)).expect_err("refused");
    assert_eq!(err.code(), "invalid_option");
    assert!(err.message().contains("1025"), "{}", err.message());

    let err: fieldglass::Error =
        Wgrib2Idx::parse("k", "1:notanumber:d=2026090400:TMP:surface:anl:")
            .expect_err("refused")
            .into();
    assert_eq!(err.code(), "decode");
    assert!(err.message().contains("line 1"), "{}", err.message());
}

/// A query key the planner does not know is refused, not ignored: a misspelt
/// `levelTxt` dropped silently would widen the query to every level.
#[test]
fn a_misspelt_query_key_is_refused() {
    let err = serde_json::from_value::<Query>(json!({ "abbreviation": "TMP", "levelTxt": "2 mb" }))
        .expect_err("refused");
    assert!(err.to_string().contains("levelTxt"), "{err}");

    // The same inside the structs a query nests: a misspelt `valeu` would read
    // as "no value", every level on the surface, and a misspelt octet as a
    // missing one.
    let err = serde_json::from_value::<Query>(json!({
        "level": { "surface": "isobaric", "valeu": 2.0, "value2": null }
    }))
    .expect_err("refused");
    assert!(err.to_string().contains("valeu"), "{err}");
    let err = serde_json::from_value::<Query>(json!({
        "parameter": { "discipline": 0, "category": 1, "number": 1, "numbr": 2 }
    }))
    .expect_err("refused");
    assert!(err.to_string().contains("numbr"), "{err}");
    let err =
        serde_json::from_value::<CandidateOptions>(json!({ "nowUnixSecs": 0, "stepHour": 6 }))
            .expect_err("refused");
    assert!(err.to_string().contains("stepHour"), "{err}");

    assert_eq!(
        serde_json::from_value::<Query>(json!({})).expect("empty reads"),
        Query::default(),
        "a key left out is unstated"
    );
}
