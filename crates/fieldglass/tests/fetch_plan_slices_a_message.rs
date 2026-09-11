//! The end of the fetch path, exercised without a network.
//!
//! `crates/fieldglass-fetchplan/tests/real_sidecars.rs` proves the planner
//! reads what the producers emit. This proves the other half: that a range it
//! plans, applied to the bytes it describes, yields a message this stack
//! decodes to the field the sidecar promised — and that a **stale** sidecar
//! does not.
//!
//! The object is built here rather than downloaded. Concatenating the committed
//! GRIB2 fixtures produces exactly the shape a NODD object has — messages laid
//! end to end, each ending where the next begins — and a `.idx` is then written
//! from their real offsets and their real metadata. That makes the whole path
//! reproducible in a fresh clone with no network and no eccodes, which is the
//! standing rule for this repo's suites; the sidecars in the planner's own
//! fixture directory are what hold the *grammar* to reality.

use fieldglass::fetchplan::{
    Expect, Manifest, MessageManifest, Mismatch, NoResolver, ParameterResolver, PlanRange, Query,
    TableResolver, Wgrib2Idx, verify_message,
};
use fieldglass::{Session, api::MessageInfo};

/// Committed single-message GRIB2 fixtures, concatenated into one object.
///
/// Three packings and two grid families, so the slicing is not accidentally
/// tuned to one message length.
const PARTS: &[&str] = &[
    "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2",
    "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
    "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2",
    "../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2",
];

/// The concatenated object, and the offset each part starts at.
fn object() -> (Vec<u8>, Vec<u64>) {
    let mut bytes = Vec::new();
    let mut offsets = Vec::new();
    for part in PARTS {
        offsets.push(bytes.len() as u64);
        // A path relative to the crate directory, not an absolute one built
        // from `CARGO_MANIFEST_DIR`: this suite also runs under
        // `wasmtime --dir=. --dir=..` for the 32-bit pointer check, and the
        // sandbox cannot open an absolute host path. Same shape as
        // `decode_and_colour.rs`.
        bytes.extend_from_slice(
            &std::fs::read(part).unwrap_or_else(|e| panic!("reading {part}: {e}")),
        );
    }
    (bytes, offsets)
}

/// Write a wgrib2-style `.idx` describing the object, from what the decoder
/// says each message is.
///
/// Generated rather than hand-written so the expectations in it are the
/// decoder's own words — which is what makes a *deliberate* corruption below
/// mean something. A hand-written sidecar that happened to disagree with the
/// decoder would fail verification for the wrong reason.
fn sidecar(offsets: &[u64], infos: &[MessageInfo]) -> String {
    offsets
        .iter()
        .zip(infos)
        .enumerate()
        .map(|(n, (offset, info))| {
            format!(
                "{}:{offset}:d={}:{}:{}:anl:\n",
                n + 1,
                info.reference_time
                    .as_deref()
                    .unwrap_or("1970010100")
                    .chars()
                    .filter(char::is_ascii_digit)
                    .take(10)
                    .collect::<String>(),
                info.abbreviation,
                info.level,
            )
        })
        .collect()
}

/// What the object's own messages are, read through the stack.
fn described() -> (Vec<u8>, Vec<u64>, Vec<MessageInfo>) {
    let (bytes, offsets) = object();
    let session = Session::open(bytes.clone()).expect("the concatenation opens");
    assert_eq!(session.count() as usize, PARTS.len());
    let infos = (0..session.count())
        .map(|i| session.message(i).expect("message"))
        .collect();
    (bytes, offsets, infos)
}

/// The headline claim: a planned range slices bytes that decode to the field
/// the sidecar promised, for every message in the object.
///
/// Each part is checked twice over — the envelope (magic, edition, §0 length)
/// and then the semantics (abbreviation, level, reference time) — because those
/// are the two halves of verification and they live in different crates.
#[test]
fn every_planned_range_slices_the_message_it_promised() {
    let (bytes, offsets, infos) = described();
    let idx = Wgrib2Idx::parse("object.grib2", &sidecar(&offsets, &infos)).expect("sidecar parses");
    let items = idx.items();
    assert_eq!(items.len(), PARTS.len());

    for (n, item) in items.iter().enumerate() {
        // What the host would do with the range: an exact one is a slice, and
        // the last is open-ended, so it takes everything from the offset on.
        let slice: &[u8] = match item.range {
            PlanRange::Exact { offset, length } => {
                &bytes[offset as usize..(offset + length) as usize]
            }
            PlanRange::OpenEnded { offset } => &bytes[offset as usize..],
            // `PlanRange` is `#[non_exhaustive]`, so a wildcard is required;
            // `Whole` is the only other shape a wgrib2 sidecar can produce.
            _ => &bytes,
        };

        let declared = item
            .expect
            .verify_envelope(slice, &item.range)
            .unwrap_or_else(|e| panic!("message {n} failed the envelope check: {e}"));

        // Every fixture here is one message, so an exact range is exactly it.
        if let Some(length) = item.range.length() {
            assert_eq!(declared, length, "message {n}");
        }

        let session = Session::open(slice[..declared as usize].to_vec())
            .unwrap_or_else(|e| panic!("message {n} did not open: {e}"));
        assert_eq!(session.count(), 1, "a sliced range holds one message");
        let info = session.message(0).expect("message");

        verify_message(&item.expect, &info)
            .unwrap_or_else(|e| panic!("message {n} is not what the sidecar promised: {e}"));
        assert_eq!(info.abbreviation, infos[n].abbreviation);
        assert_eq!(info.level, infos[n].level);
    }
}

/// A field selected by name, fetched by its planned range, and decoded — the
/// whole point of the crate, in one test.
#[test]
fn selecting_a_field_by_name_yields_the_bytes_that_decode_to_it() {
    let (bytes, offsets, infos) = described();
    let idx = Wgrib2Idx::parse("object.grib2", &sidecar(&offsets, &infos)).unwrap();

    // Whatever the second fixture's parameter is, ask for it by the sidecar's
    // own name. Read from the corpus rather than written down, so a fixture
    // swap does not silently turn this into a test of nothing.
    let wanted = infos[1].abbreviation.clone();
    let hits = idx.select(
        &Query::abbreviation(&wanted).at_level_text(&infos[1].level),
        &NoResolver,
    );
    assert_eq!(
        hits.len(),
        1,
        "{wanted} should be unambiguous in this object"
    );

    let PlanRange::Exact { offset, length } = hits[0].range else {
        panic!("an interior message has both ends: {:?}", hits[0].range)
    };
    assert_eq!(offset, offsets[1]);

    let slice = &bytes[offset as usize..(offset + length) as usize];
    hits[0]
        .expect
        .verify_envelope(slice, &hits[0].range)
        .unwrap();
    let session = Session::open(slice.to_vec()).unwrap();
    let info = session.message(0).unwrap();
    assert_eq!(info.abbreviation, wanted);
    // …and it really decodes, not just parses.
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("the sliced message decodes");
    assert!(field.stats.valid_count > 0);
}

/// A stale sidecar is the failure mode ADR-0005 decision 5 warns about: NODD
/// regenerates an object and every offset after the first change points into
/// the middle of a message. The envelope check must catch it and say what it
/// found instead of `GRIB`.
#[test]
fn a_stale_offset_pointing_mid_message_fails_with_both_sides() {
    let (bytes, offsets, infos) = described();
    let mut lines: Vec<String> = sidecar(&offsets, &infos)
        .lines()
        .map(String::from)
        .collect();

    // Shift the second record 64 bytes into its own message — the shape a
    // sidecar regenerated against a slightly different object has.
    let stale_offset = offsets[1] + 64;
    let fields: Vec<&str> = lines[1].split(':').collect();
    lines[1] = format!("{}:{stale_offset}:{}", fields[0], fields[2..].join(":"));
    let text = lines.join("\n") + "\n";

    let idx = Wgrib2Idx::parse("object.grib2", &text).expect("a stale sidecar still parses");
    let item = &idx.items()[1];
    let PlanRange::Exact { offset, length } = item.range else {
        panic!("expected an interior range")
    };
    let slice = &bytes[offset as usize..(offset + length) as usize];

    let err = item
        .expect
        .verify_envelope(slice, &item.range)
        .expect_err("a mid-message offset must not verify");
    match err {
        Mismatch::Magic { expected, found } => {
            assert_eq!(expected, "GRIB");
            assert_ne!(found, "GRIB");
            // The bytes that were actually there reach the message, which is
            // what tells a user the sidecar is stale rather than the object
            // corrupt.
            assert!(!found.is_empty());
        }
        other => panic!("expected a magic mismatch, got {other}"),
    }
}

/// The other stale shape, and the one a magic check cannot catch: the offsets
/// are still message boundaries, but the *fields* have moved, so the bytes are
/// a perfectly valid message that is simply not the one asked for. Only the
/// semantic half sees this.
#[test]
fn a_sidecar_promising_the_wrong_field_fails_the_semantic_check() {
    let (bytes, offsets, infos) = described();
    // Describe message 1's bytes with message 0's metadata — exactly what a
    // sidecar written against a reordered object would say.
    let mut swapped = infos.clone();
    swapped[1] = infos[0].clone();
    let idx = Wgrib2Idx::parse("object.grib2", &sidecar(&offsets, &swapped)).unwrap();
    let item = &idx.items()[1];

    let PlanRange::Exact { offset, length } = item.range else {
        panic!("expected an interior range")
    };
    let slice = &bytes[offset as usize..(offset + length) as usize];

    // The envelope is fine: these really are a whole, valid GRIB message.
    item.expect
        .verify_envelope(slice, &item.range)
        .expect("the bytes are a valid message");

    let session = Session::open(slice.to_vec()).unwrap();
    let info = session.message(0).unwrap();
    let err = verify_message(&item.expect, &info)
        .expect_err("the wrong field must be reported, not decoded");
    let Mismatch::Field {
        field,
        expected,
        actual,
    } = err
    else {
        panic!("expected a field mismatch, got {err}")
    };
    assert!(
        matches!(field, "abbreviation" | "level"),
        "unexpected field {field}"
    );
    assert_ne!(expected, actual);
}

/// The umbrella's resolver, over the real tables, against a plan built from
/// real bytes: a stored WMO request selects the message whose decoded parameter
/// has those codes.
#[test]
fn a_stored_wmo_request_selects_through_the_table_resolver() {
    let (_, offsets, infos) = described();
    let idx = Wgrib2Idx::parse("object.grib2", &sidecar(&offsets, &infos)).unwrap();
    let resolver = TableResolver::ncep();

    // Take the codes from the first message's own abbreviation, so the test
    // does not hard-code which parameter the fixture happens to carry.
    let codes = resolver
        .resolve(&infos[0].abbreviation, &infos[0].level)
        .unwrap_or_else(|| {
            panic!(
                "the tables should name {:?}, which the decoder itself produced",
                infos[0].abbreviation
            )
        });

    let hits = idx.select(&Query::parameter(codes), &resolver);
    assert!(
        hits.iter()
            .any(|h| h.expect.abbreviation.as_deref() == Some(infos[0].abbreviation.as_str())),
        "a stored request for {codes:?} should reach {:?}",
        infos[0].abbreviation
    );
}

/// A host that fetched bytes some other way can still state what they should be
/// and check them, because `Expect` has builders. Without them the type is
/// `#[non_exhaustive]` and constructible only from a parsed sidecar.
#[test]
fn an_expectation_can_be_built_by_hand_and_checked() {
    let (bytes, offsets, infos) = described();
    let end = offsets.get(1).copied().unwrap_or(bytes.len() as u64);
    let slice = &bytes[..end as usize];

    let expect = Expect::new()
        .with_abbreviation(&infos[0].abbreviation)
        .with_level(&infos[0].level);
    let range = PlanRange::Exact {
        offset: 0,
        length: end,
    };
    expect
        .verify_envelope(slice, &range)
        .expect("valid message");

    let session = Session::open(slice.to_vec()).unwrap();
    verify_message(&expect, &session.message(0).unwrap()).expect("it is what was stated");
}
