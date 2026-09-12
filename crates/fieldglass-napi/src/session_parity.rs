//! What `fieldglass::Session` answers about a message, against what this
//! binding answers for itself (#726).
//!
//! `Grib1Handle` and `Grib2Handle` each hold their own reader and build
//! `MessageMeta` out of the format crate's types directly — two hand-written
//! mappings, ~450 lines between them, reading the WMO and CCT tables at the
//! binding layer. ADR-0006 says a host is a binding over the umbrella, so those
//! mappings should be one mapping over `Session`'s DTOs.
//!
//! **This file is the oracle for making that move, written before it.** It pairs
//! every field of `MessageMeta` that `Session::message` also answers and
//! requires them equal, for every message of every GRIB fixture in the
//! committed corpus. A field the session gets wrong is a field the migration
//! would silently change, and the napi characterisation golden would only catch
//! it if the extension happened to display it.
//!
//! It is deliberately *not* a spot check on one fixture. The interesting
//! disagreements are in the corners — a message with no §2, a template with no
//! horizontal product common, a centre absent from the CCT table, GRIB1's time
//! range 10 — and those live in single fixtures scattered through the corpus.
//!
//! What it does **not** cover: the ~55 projection fields (`lambertLatin1`,
//! `geosSubLon`, …). Those come from `Georef::geometry`, which is one value
//! compared wholesale by `place_slice.rs` and the conformance suite rather than
//! field by field here.

use super::*;
use crate::characterisation::{CORPUS, fixtures, stem};

/// One field, named, as the handle reports it and as the session does.
type Row = (&'static str, String, String);

/// Every `MessageMeta` field `Session::message` also answers.
///
/// Written as strings so one comparison covers `Option<String>`, `i32` and
/// `f64` alike, and so a mismatch prints what both sides actually said. The
/// small normalisations are where the two types differ in shape rather than in
/// fact, and each is named:
///
/// - `packing` and `reference_time` are `Option` on one side and not the other,
///   because the handle renders an absent value and the DTO reports it absent.
/// - `forecast_hours` is `Option<i32>` on the DTO and `i32` on the handle, which
///   substitutes `0` for a template that states no lead time.
/// - `total_length_bytes` is `u64` on the DTO and `f64` on the handle, which is
///   the JavaScript number a `#[napi]` object can carry.
fn rows(handle: &MessageMeta, info: &fieldglass::MessageInfo) -> Vec<Row> {
    fn show(v: &Option<String>) -> String {
        v.clone().unwrap_or_else(|| "<none>".to_string())
    }
    vec![
        (
            "messageIndex",
            handle.message_index.to_string(),
            info.index.to_string(),
        ),
        (
            "offsetBytes",
            handle.offset_bytes.to_string(),
            (info.offset_bytes as f64).to_string(),
        ),
        (
            "parameterName",
            handle.parameter_name.clone(),
            info.parameter.clone(),
        ),
        (
            "parameterUnits",
            handle.parameter_units.clone(),
            info.units.clone(),
        ),
        (
            "parameterAbbreviation",
            handle.parameter_abbreviation.clone(),
            info.abbreviation.clone(),
        ),
        ("level", handle.level.clone(), info.level.clone()),
        (
            "levelType",
            handle.level_type.clone(),
            info.level_type.clone(),
        ),
        (
            "referenceTime",
            handle.reference_time.clone(),
            info.reference_time.clone().unwrap_or_default(),
        ),
        (
            "forecastDisplay",
            handle.forecast_display.clone(),
            info.forecast.clone(),
        ),
        // Two vocabularies for one fact, and the only field of the twenty where
        // the handle does not simply echo the DTO. `Session` reports the
        // eccodes-style template identifier (`grid_second_order_SPD3`,
        // `complex_spatial_diff`); the handle renders it for a message table
        // ("Second-order (SPD-3)"). Neither is wrong — the display name is a
        // host concern and lossy on purpose, mapping `spectral_simple` and
        // `spectral_complex` alike to "Spectral (spherical harmonic)".
        //
        // What matters for the migration is that the rendering is a **pure
        // function of the identifier**, so a handle over `Session` can still
        // produce its own label with nothing but the DTO. That is the claim
        // asserted here, and it is the reason `MessageInfo` needs no display
        // string of its own.
        (
            "packing (as friendly_packing of the session's identifier)",
            show(&handle.packing),
            friendly_packing(&info.packing),
        ),
        (
            "gridSizeLabel",
            show(&handle.grid_size_label),
            show(&info.size_label),
        ),
        (
            "forecastHours",
            handle.forecast_hours.to_string(),
            info.forecast_hours.unwrap_or(0).to_string(),
        ),
        (
            "p1Octet",
            show(&handle.p1_octet.map(|v| v.to_string())),
            show(&info.p1_octet.map(|v| v.to_string())),
        ),
        (
            "originatingCentre",
            handle.originating_centre.clone(),
            info.originating_centre.clone(),
        ),
        (
            "subCentre",
            show(&handle.sub_centre),
            show(&info.sub_centre),
        ),
        (
            "edition",
            show(&handle.edition.map(|v| v.to_string())),
            show(&info.edition.map(|v| v.to_string())),
        ),
        (
            "discipline",
            show(&handle.discipline),
            show(&info.discipline),
        ),
        (
            "totalLengthBytes",
            show(&handle.total_length_bytes.map(|v| v.to_string())),
            show(&info.total_length_bytes.map(|v| (v as f64).to_string())),
        ),
        (
            "productionStatus",
            show(&handle.production_status),
            show(&info.production_status),
        ),
        ("dataType", show(&handle.data_type), show(&info.data_type)),
    ]
}

/// Every disagreement across the **whole** corpus, appended to `wrong`.
///
/// Accumulated rather than asserted per file: stopping at the first mismatching
/// fixture says a problem exists, where a full sweep says whether it is one
/// field or twenty, and which corners it reaches.
fn compare(
    file: &str,
    count: u32,
    meta: impl Fn(u32) -> MessageMeta,
    session: &fieldglass::Session,
    wrong: &mut Vec<String>,
) {
    assert_eq!(
        session.count(),
        count,
        "{file}: the session found a different number of messages"
    );
    for i in 0..count {
        let info = session
            .message(i)
            .unwrap_or_else(|e| panic!("{file}#{i}: the session refused the message: {e}"));
        for (field, handle, dto) in rows(&meta(i), &info) {
            if handle != dto {
                wrong.push(format!(
                    "  {file}#{i} {field}: handle {handle:?} vs session {dto:?}"
                ));
            }
        }
    }
}

/// What the disagreements were, grouped by field, so the report names the
/// *kinds* of divergence rather than listing one line per message.
fn report(wrong: &[String]) {
    if wrong.is_empty() {
        return;
    }
    let mut by_field: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for line in wrong {
        let field = line.split_whitespace().nth(1).unwrap_or("?");
        *by_field.entry(field).or_default() += 1;
    }
    let summary: Vec<String> = by_field.iter().map(|(f, n)| format!("{f} x{n}")).collect();
    panic!(
        "{} field(s) disagree across {} kind(s) [{}]:\n{}",
        wrong.len(),
        by_field.len(),
        summary.join(", "),
        wrong.join("\n")
    );
}

/// Every field of `MessageMeta`, as `Debug`, so a whole-struct comparison names
/// the fields that differ instead of printing two seventy-two-field blobs.
///
/// Parsed out of the derived `Debug` rather than listed by hand: a field added
/// to `MessageMeta` then enters this comparison automatically, where a hand-kept
/// list would silently stop covering it.
fn fields(meta: &MessageMeta) -> std::collections::BTreeMap<String, String> {
    let text = format!("{meta:#?}");
    let mut out = std::collections::BTreeMap::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        let line = line.trim();
        // Pretty `Debug` wraps a value across lines — `Some(`, the value, `)` —
        // so a continuation has to join the field it belongs to. Splitting per
        // line without this compares every wrapped value as the literal
        // `"Some("`, which sees a present/absent difference but never a wrong
        // number. It did exactly that until the corpus made it obvious.
        match line.split_once(": ") {
            Some((k, v)) if !k.contains(' ') && !k.contains('(') => {
                if let Some((key, value)) = current.take() {
                    out.insert(key, value);
                }
                current = Some((k.to_string(), v.trim_end_matches(',').to_string()));
            }
            _ => {
                if let Some((_, value)) = current.as_mut() {
                    value.push_str(line.trim_end_matches(','));
                }
            }
        }
    }
    if let Some((key, value)) = current {
        out.insert(key, value);
    }
    out
}

/// The whole `MessageMeta` the unified builder produces against the one a
/// hand-written builder does, appended to `wrong`.
///
/// This is the claim #726's handle migration rests on: that one builder over
/// `Session`'s DTOs reproduces both per-edition builders exactly. Compared as a
/// whole struct rather than a chosen list, so a field nobody thought about is
/// still covered.
fn compare_whole_meta(
    what: &str,
    file: &str,
    index: u32,
    from_handle: &MessageMeta,
    from_session: &MessageMeta,
    wrong: &mut Vec<String>,
    known: &mut Vec<String>,
) {
    let (a, b) = (fields(from_handle), fields(from_session));
    for (field, handle) in &a {
        let session = b.get(field).map(String::as_str).unwrap_or("<absent>");
        if handle == session {
            continue;
        }
        let line = format!("  {file}#{index} {what} {field}: handle {handle} vs session {session}");
        if KNOWN_GAPS.contains(&field.as_str()) {
            known.push(line);
        } else {
            wrong.push(line);
        }
    }
}

/// The fields one builder over `Session`'s DTOs cannot yet reproduce, and why.
///
/// **Corner coordinates** (`lat_first`, `lon_first`, `lat_last`, `lon_last`).
/// The per-edition builders read them from the format crate's own
/// `bounds()`, which for a projected family *computes* the far corner by
/// projecting the last grid point, and for a reduced grid reports the
/// **declared** octet rather than the widened raster's. `GridGeometry` carries
/// neither: a `PolarStereo` is defined by its first corner plus spacing, so it
/// has no `lat_last` to read, and a widened reduced grid's computed corner
/// differs from the declared one in the fifth decimal (357.1875 against the
/// file's 357.188).
///
/// **Row order for a family with no conventional raster** (`j_scans_positive`).
/// The handles report `Some(false)` for HEALPix and nothing for a synthesised
/// spectral grid; going through `Georef` gives the opposite on both, because a
/// synthesised raster genuinely is north-down and a HEALPix pixel list genuinely
/// has no rows.
///
/// Both are recorded rather than papered over: closing them means `Georef`
/// carrying the format's own corner pair, which is a DTO addition with its own
/// justification, not something to slip into a refactor. Until then this test
/// says exactly how far one builder gets, which is the useful fact for #726.
const KNOWN_GAPS: [&str; 5] = [
    "lat_first",
    "lon_first",
    "lat_last",
    "lon_last",
    "j_scans_positive",
];

/// Every GRIB1 fixture, every message.
#[test]
fn the_session_answers_what_grib1_handles_answer() {
    let mut files = 0;
    let mut messages = 0;
    let mut wrong = Vec::new();
    for extension in CORPUS[0].2 {
        for path in fixtures(CORPUS[0].1, extension) {
            let file = format!("grib1/{}", stem(&path));
            let bytes = std::fs::read(&path).expect("fixture bytes");
            // A fixture the reader refuses is one the session refuses too, and
            // there is nothing to compare. The characterisation golden is what
            // holds those to their parse outcome.
            let Ok(reader) = Grib1Reader::from_bytes(bytes.clone()) else {
                continue;
            };
            let count = reader.messages.len() as u32;
            let handle = Grib1Handle {
                reader,
                decoded: Mutex::new(std::collections::HashMap::new()),
                synthesized: Mutex::new(std::collections::HashMap::new()),
            };
            let session = fieldglass::Session::open(bytes).unwrap_or_else(|e| {
                panic!("{file}: the reader opened this and the session must too: {e}")
            });
            compare(
                &file,
                count,
                |i| handle.message_meta(i).expect("the handle's own metadata"),
                &session,
                &mut wrong,
            );
            files += 1;
            messages += count;
        }
    }
    assert!(
        files > 0 && messages > 0,
        "the GRIB1 corpus is present and non-empty"
    );
    report(&wrong);
    eprintln!("grib1: {files} files, {messages} messages agree");
}

/// Every GRIB2 fixture, every message.
#[test]
fn the_session_answers_what_grib2_handles_answer() {
    let mut files = 0;
    let mut messages = 0;
    let mut wrong = Vec::new();
    for path in fixtures(CORPUS[1].1, CORPUS[1].2[0]) {
        let file = format!("grib2/{}", stem(&path));
        let bytes = std::fs::read(&path).expect("fixture bytes");
        let Ok(reader) = Grib2Reader::from_bytes(bytes.clone()) else {
            continue;
        };
        let count = reader.messages.len() as u32;
        let handle = Grib2Handle {
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
            synthesized: Mutex::new(std::collections::HashMap::new()),
        };
        let session = fieldglass::Session::open(bytes).unwrap_or_else(|e| {
            panic!("{file}: the reader opened this and the session must too: {e}")
        });
        compare(
            &file,
            count,
            |i| handle.message_meta(i).expect("the handle's own metadata"),
            &session,
            &mut wrong,
        );
        files += 1;
        messages += count;
    }
    assert!(
        files > 0 && messages > 0,
        "the GRIB2 corpus is present and non-empty"
    );
    report(&wrong);
    eprintln!("grib2: {files} files, {messages} messages agree");
}

/// One builder over `Session`'s DTOs reproduces both per-edition builders,
/// field for field, for every message of every GRIB fixture.
///
/// Two placements are checked per message, because which `Georef` the builder
/// is given is the only difference between the three metas the handles build
/// today:
///
/// - **declared** — `message().grid` against the handle's `message_meta`.
/// - **resolved** — `place_message()` against the handle's `resolved_meta`,
///   which is the spectral synthesis grid where there is one and the declared
///   raster otherwise.
#[test]
fn one_builder_reproduces_both_per_edition_builders() {
    let mut wrong = Vec::new();
    let mut known = Vec::new();
    let (mut checked, mut resolved_differed) = (0, 0);

    for extension in CORPUS[0].2 {
        for path in fixtures(CORPUS[0].1, extension) {
            let file = format!("grib1/{}", stem(&path));
            let bytes = std::fs::read(&path).expect("fixture bytes");
            let Ok(reader) = Grib1Reader::from_bytes(bytes.clone()) else {
                continue;
            };
            let count = reader.messages.len() as u32;
            let handle = Grib1Handle {
                reader,
                decoded: Mutex::new(std::collections::HashMap::new()),
                synthesized: Mutex::new(std::collections::HashMap::new()),
            };
            let session = fieldglass::Session::open(bytes).expect("opens");
            for i in 0..count {
                checked += 1;
                let info = session.message(i).expect("the session's message");
                compare_whole_meta(
                    "declared",
                    &file,
                    i,
                    &handle.message_meta(i).expect("the handle's declared meta"),
                    &crate::session_meta::meta_from_session(&info, info.grid.as_ref(), "grib1"),
                    &mut wrong,
                    &mut known,
                );
                // A message `place_message` refuses has no resolved placement to
                // compare, and the handle refuses it too — `place_slice`'s own
                // corpus test in the umbrella is what holds those two together.
                if let (Ok(placed), Ok(handle_resolved)) =
                    (session.place_message(i), handle.resolved_meta(i))
                {
                    if placed.label
                        != info
                            .grid
                            .as_ref()
                            .map(|g| g.label.clone())
                            .unwrap_or_default()
                    {
                        resolved_differed += 1;
                    }
                    compare_whole_meta(
                        "resolved",
                        &file,
                        i,
                        &handle_resolved,
                        &crate::session_meta::meta_from_session(&info, Some(&placed), "grib1"),
                        &mut wrong,
                        &mut known,
                    );
                }
            }
        }
    }

    for path in fixtures(CORPUS[1].1, CORPUS[1].2[0]) {
        let file = format!("grib2/{}", stem(&path));
        let bytes = std::fs::read(&path).expect("fixture bytes");
        let Ok(reader) = Grib2Reader::from_bytes(bytes.clone()) else {
            continue;
        };
        let count = reader.messages.len() as u32;
        let handle = Grib2Handle {
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
            synthesized: Mutex::new(std::collections::HashMap::new()),
        };
        let session = fieldglass::Session::open(bytes).expect("opens");
        for i in 0..count {
            checked += 1;
            let info = session.message(i).expect("the session's message");
            compare_whole_meta(
                "declared",
                &file,
                i,
                &handle.message_meta(i).expect("the handle's declared meta"),
                &crate::session_meta::meta_from_session(&info, info.grid.as_ref(), "grib2"),
                &mut wrong,
                &mut known,
            );
            if let (Ok(placed), Ok(handle_resolved)) =
                (session.place_message(i), handle.resolved_meta(i))
            {
                if placed.label
                    != info
                        .grid
                        .as_ref()
                        .map(|g| g.label.clone())
                        .unwrap_or_default()
                {
                    resolved_differed += 1;
                }
                compare_whole_meta(
                    "resolved",
                    &file,
                    i,
                    &handle_resolved,
                    &crate::session_meta::meta_from_session(&info, Some(&placed), "grib2"),
                    &mut wrong,
                    &mut known,
                );
            }
        }
    }

    report(&wrong);
    assert!(checked > 0, "the GRIB corpus is present");
    // The other direction: if the recorded gaps stopped appearing, `KNOWN_GAPS`
    // is excusing nothing and the entries should go rather than stand as a
    // permanent apology.
    assert!(
        !known.is_empty(),
        "KNOWN_GAPS lists fields that no longer differ — remove them"
    );
    // The resolved comparison is only worth anything where resolving actually
    // changes the answer — a synthesised family. Asserted so a corpus that lost
    // its spectral fixtures could not turn half this test into a duplicate of
    // the other half.
    assert!(
        resolved_differed > 0,
        "the corpus must still hold a message whose resolved placement differs \
         from its declared one"
    );
    eprintln!(
        "{checked} messages: one builder reproduces both on every field outside \
         KNOWN_GAPS ({resolved_differed} resolved differently from declared, \
         {} recorded gaps)",
        known.len()
    );
}
