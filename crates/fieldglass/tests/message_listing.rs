//! `Session::message_listing` is `Session::message` without the grid's per-row
//! point counts and engine geometry (#811).
//!
//! A host lists a file with it, so the two must not drift apart in anything
//! else. That is asserted over the committed GRIB corpus, on the wire: for
//! every message of every fixture, the listing's JSON is the `MessageInfo`'s
//! JSON with `grid.geometry` and `grid.pointsPerRow` removed, key for key. A
//! field one of them gains without the other fails here.
//!
//! The reduced fixtures are what make it worth asserting: their `MessageInfo`
//! carries one point count per row, and the listing must carry none.

use fieldglass::{Error, MessageInfo, MessageListing, Session};
use serde_json::Value;

/// Every GRIB fixture in the workspace, as `(label, path)`.
fn corpus() -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    for (label, dir, extensions) in [
        (
            "grib1",
            "../fieldglass-grib1/tests/fixtures",
            &["grib", "grib1"][..],
        ),
        (
            "grib2",
            "../fieldglass-grib2/tests/fixtures",
            &["grib2"][..],
        ),
    ] {
        for extension in extensions {
            let mut paths: Vec<_> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("{dir} is the committed corpus: {e}"))
                .map(|e| e.expect("entry").path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(*extension))
                .collect();
            paths.sort();
            for path in paths {
                let stem = path
                    .file_name()
                    .expect("a file")
                    .to_string_lossy()
                    .into_owned();
                out.push((format!("{label}/{stem}"), path));
            }
        }
    }
    out
}

/// The `MessageInfo`'s JSON as a listing should carry it.
fn without_per_row_counts(mut info: Value) -> Value {
    if let Some(grid) = info.get_mut("grid").and_then(Value::as_object_mut) {
        grid.remove("geometry");
        grid.remove("pointsPerRow");
    }
    info
}

/// Whether `pointsPerRow` appears anywhere in `v`, at any depth.
fn mentions_points_per_row(v: &Value) -> bool {
    match v {
        Value::Object(o) => o
            .iter()
            .any(|(k, v)| k == "pointsPerRow" || mentions_points_per_row(v)),
        Value::Array(a) => a.iter().any(mentions_points_per_row),
        _ => false,
    }
}

#[test]
fn a_listing_is_the_message_without_its_per_row_counts() {
    let (mut messages, mut reduced) = (0usize, 0usize);
    let mut wrong = Vec::new();
    for (label, path) in corpus() {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: {e}"));
        // A fixture that does not open is another test's subject.
        let Ok(session) = Session::open(bytes) else {
            continue;
        };
        for i in 0..session.count() {
            let (info, listing) = match (session.message(i), session.message_listing(i)) {
                (Ok(info), Ok(listing)) => (info, listing),
                (Err(a), Err(b)) => {
                    if a.code() != b.code() {
                        wrong.push(format!("{label}#{i}: refused as {a:?} and as {b:?}"));
                    }
                    continue;
                }
                (a, b) => {
                    wrong.push(format!(
                        "{label}#{i}: one answered, one refused: {a:?} / {b:?}"
                    ));
                    continue;
                }
            };
            messages += 1;
            let rows = info.grid.as_ref().and_then(|g| g.points_per_row.as_ref());
            if rows.is_some_and(|r| !r.is_empty()) {
                reduced += 1;
            }
            let info_json = serde_json::to_value(&info).expect("a MessageInfo serializes");
            let listing_json = serde_json::to_value(&listing).expect("a listing serializes");
            if mentions_points_per_row(&listing_json) {
                wrong.push(format!("{label}#{i}: the listing carries pointsPerRow"));
            }
            if listing_json != without_per_row_counts(info_json) {
                wrong.push(format!(
                    "{label}#{i}: the listing is not the message less its rows"
                ));
            }
            // The conversion a host can make itself agrees with the call.
            if MessageListing::from(info) != listing {
                wrong.push(format!(
                    "{label}#{i}: From<MessageInfo> disagrees with the call"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} disagreement(s):\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // Not vacuous: the corpus has reduced grids of both editions, and plenty
    // of messages besides.
    assert!(messages >= 90, "only {messages} messages listed");
    assert!(reduced >= 8, "only {reduced} reduced-grid messages listed");
}

/// A listing does not read back as a `MessageInfo`, so a call that takes one
/// back (the fetch planner's `verify_message`, the browser's `verifyMessage`)
/// refuses it rather than checking a message it was not given.
#[test]
fn a_listing_does_not_read_as_a_message_info() {
    let bytes =
        std::fs::read("../fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2")
            .expect("the reduced Gaussian fixture is committed");
    let session = Session::open(bytes).expect("it opens");
    let listing = serde_json::to_value(session.message_listing(0).expect("it lists"))
        .expect("a listing serializes");
    let err = serde_json::from_value::<MessageInfo>(listing)
        .expect_err("a listing must not deserialize as a MessageInfo");
    assert!(err.to_string().contains("geometry"), "{err}");
    // And the message itself still does, so the refusal is the listing's.
    let info = serde_json::to_value(session.message(0).expect("it reads")).expect("serializes");
    serde_json::from_value::<MessageInfo>(info).expect("a MessageInfo reads back");
}

/// The refusals are `message`'s, naming the call that was made.
#[test]
fn a_listing_refuses_as_the_message_does() {
    let grib = std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
        .expect("a GRIB2 fixture");
    let session = Session::open(grib).expect("it opens");
    assert!(matches!(
        session.message_listing(session.count()),
        Err(Error::NoSuchMessage { .. })
    ));

    let netcdf = std::fs::read("../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc")
        .expect("a NetCDF fixture");
    let session = Session::open(netcdf).expect("it opens");
    let err = session
        .message_listing(0)
        .expect_err("a variable dataset lists no messages");
    assert_eq!(err.code(), "wrong_addressing");
    assert!(err.to_string().contains("`message_listing`"), "{err}");
}
