//! What a reduced-resolution JPEG 2000 decode costs, against the full one
//! (#463).
//!
//!     cargo run --release -p fieldglass-grib2 --example bench_reduce
//!
//! Release, always: the entropy decode is the whole measurement and a debug
//! build times the borrow checker's leftovers rather than the codec.
//!
//! The corpus is the committed `rap_jpeg2000_lambert.grib2` — a real NCEP RAP
//! surface field, 451×337 on a Lambert grid — so this runs from a clean clone
//! with no network and no `samples/`. It prints one row per pyramid level and
//! the ratio against level 0, which is the number to read: absolute
//! milliseconds move about 10 % run to run, the ratio does not.
//!
//! Not a test. A timing assertion in the suite would be measuring the machine
//! CI happened to schedule; `crates/fieldglass-grib2/tests/decode_reduced.rs`
//! asserts the deterministic half — the shape, the placement and the mean — and
//! this reports the half that is a measurement.

use std::path::Path;
use std::time::Instant;

use fieldglass_grib2::{DecodeOptions, Grib2Reader};

/// Median of this many timed decodes, after one untimed warm-up.
const ITERATIONS: usize = 7;

/// Median in milliseconds. `samples` is consumed sorted.
fn median_ms(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rap_jpeg2000_lambert.grib2");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("committed fixture {} is unreadable: {e}", path.display()));
    let reader = Grib2Reader::from_bytes(bytes).expect("a committed fixture must parse");

    println!("rap_jpeg2000_lambert.grib2 — JPEG 2000 (5.40) on a 451x337 Lambert grid");
    println!(
        "{:>7}  {:>11}  {:>12}  {:>9}",
        "reduce", "raster", "median ms", "vs full"
    );

    let mut full_ms = f64::NAN;
    for reduction in 0u8..=5 {
        let options = DecodeOptions::new(reduction);
        let Ok(warm) = reader.decode_message_raster_with(0, options) else {
            println!("{reduction:>7}  {:>11}", "refused");
            continue;
        };
        let (ni, nj) = (warm.ni(), warm.nj());
        drop(warm);

        let mut samples = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let start = Instant::now();
            let raster = reader
                .decode_message_raster_with(0, options)
                .expect("it decoded once already");
            samples.push(start.elapsed().as_secs_f64() * 1e3);
            drop(raster);
        }
        let ms = median_ms(&mut samples);
        if reduction == 0 {
            full_ms = ms;
        }
        println!(
            "{reduction:>7}  {:>11}  {ms:>12.2}  {:>8.1}%",
            format!("{ni}x{nj}"),
            100.0 * ms / full_ms
        );
    }
}
