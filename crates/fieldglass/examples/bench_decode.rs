//! Native decode timings for the wasm benchmark to be measured against.
//!
//!     cargo run --release -p fieldglass --example bench_decode > native.json
//!
//! The corpus is the three committed real-producer GRIB2 fixtures, so this runs
//! from a clean clone with no network and no `samples/` (issue #462 asked for
//! `samples/`, which is git-ignored — a benchmark that skips when the corpus is
//! absent measures nothing, and one that downloads a live model run measures a
//! different file every day).
//!
//! One number per fixture: the median of `Session::decode`, which is the whole
//! unpack (section 7 through the mask and the `Values` buffer) and nothing else.
//! `Session::open` is timed separately because it only walks the section
//! headers, so folding it in would dilute the figure the browser cares about.
//!
//! The output is pretty-printed JSON and nothing else: it is what
//! `crates/fieldglass-wasm/tests/node/bench.mjs` reads to put the wasm column
//! beside this one, and it is legible enough on its own that a second
//! human-readable mode would only have been a flag to get wrong. Reading one off
//! `std::env::args()` also trips `rust.lang.security.args.args`, and this repo
//! keeps zero semgrep suppressions.

use std::path::{Path, PathBuf};
use std::time::Instant;

use fieldglass::{DecodeOptions, Session};

/// Median of this many timed decodes, after one untimed warm-up.
const ITERATIONS: usize = 5;

/// `(fixture file name, label)`. Every entry is a real operational message with
/// an eccodes value oracle beside it; see the grib2 fixtures' `NOTICE.md`.
const CORPUS: &[(&str, &str)] = &[
    (
        "ecmwf_ccsds_latlon.grib2",
        "ECMWF IFS 0.25 deg, CCSDS (5.42)",
    ),
    (
        "hrrr_complex_spd_lambert.grib2",
        "HRRR 3 km Lambert, complex+spd (5.3)",
    ),
    (
        "rap_jpeg2000_lambert.grib2",
        "RAP 13 km Lambert, JPEG 2000 (5.40)",
    ),
];

fn fixtures_dir() -> PathBuf {
    // `crates/fieldglass` -> `crates/fieldglass-grib2/tests/fixtures`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fieldglass-grib2/tests/fixtures")
        .canonicalize()
        .expect("the grib2 fixture directory is committed next to this crate")
}

/// Median in milliseconds. `samples` is consumed sorted.
fn median_ms(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let dir = fixtures_dir();
    let mut rows = Vec::new();

    for (file, label) in CORPUS {
        let path = dir.join(file);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("committed fixture {} is unreadable: {e}", path.display()));

        let open_start = Instant::now();
        let session = Session::open(bytes).expect("a committed fixture must open");
        let open_ms = open_start.elapsed().as_secs_f64() * 1e3;

        let options = DecodeOptions::default();
        let warm = session
            .decode(0, &options)
            .expect("a committed fixture must decode");
        let points = warm.values.len();
        drop(warm);

        let mut timings = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let start = Instant::now();
            let field = session
                .decode(0, &options)
                .expect("decode is deterministic");
            timings.push(start.elapsed().as_secs_f64() * 1e3);
            // Dropping inside the loop keeps the allocator state comparable
            // between iterations rather than growing a Vec of fields.
            drop(field);
        }
        rows.push((*file, *label, points, open_ms, median_ms(&mut timings)));
    }

    assert!(
        !rows.is_empty(),
        "the corpus is empty, so nothing was measured"
    );

    // serde_json rather than hand-written braces: the labels are prose, and a
    // future one with a quote in it would emit a file the reader parses wrong
    // rather than a file it rejects.
    let fields: Vec<serde_json::Value> = rows
        .iter()
        .map(|(file, label, points, open_ms, ms)| {
            serde_json::json!({
                "file": file,
                "label": label,
                "points": points,
                "openMs": open_ms,
                "decodeMs": ms,
            })
        })
        .collect();
    let report = serde_json::json!({ "iterations": ITERATIONS, "fields": fields });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("plain values serialise")
    );
}
