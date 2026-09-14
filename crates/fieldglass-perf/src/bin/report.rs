//! The report tier: wall time. Printed, never gated.
//!
//!     FIELDGLASS_PERF_CORPUS=<dir> [FIELDGLASS_PERF_MANIFEST=manifests/era5.json \
//!         FIELDGLASS_PERF_CACHE=~/.cache/fieldglass-perf] cargo run --release --bin report > native.json
//!
//! Configured through the environment rather than arguments, like the corpus:
//! reading `std::env::args()` trips semgrep's `rust.lang.security.args.args`,
//! and this repo keeps zero suppressions (the same reason `bench_decode` takes
//! none).
//!
//! Wall time on a shared runner is noise, which is why none of this gates. It
//! is here because a ratio against a reference tool, and a real scrub over real
//! bytes, are the numbers that say whether the gated ones are close to the best
//! anyone does — the instruction counts only say whether they moved.
//!
//! Every scenario in the catalogue is prepared afresh and timed `ITERATIONS`
//! times; the median is reported. Preparation is not timed, for the reason the
//! gated tiers exclude it.

use std::path::PathBuf;
use std::time::Instant;

use fieldglass_perf::{Corpus, Prepared, Via, catalogue, real};
use serde_json::{Map, json};

/// Median of this many runs per scenario, the same count as `bench.mjs`.
const ITERATIONS: usize = 5;

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

/// A path from the environment, or `None` when the variable is unset. Set and
/// empty is a mistake, not a default.
fn path_from_env(name: &str) -> Option<PathBuf> {
    match std::env::var_os(name) {
        Some(value) if value.is_empty() => fail(format!("{name} is set but empty")),
        Some(value) => Some(PathBuf::from(value)),
        None => None,
    }
}

fn main() {
    let corpus = Corpus::from_env().unwrap_or_else(|e| fail(e));
    let mut native = Map::new();
    for scenario in catalogue(&corpus) {
        let mut times = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let mut prepared = Prepared::new(&corpus, &scenario, Via::Memory);
            let start = Instant::now();
            prepared.execute();
            times.push(start.elapsed().as_secs_f64() * 1e3);
            drop(prepared);
        }
        times.sort_by(f64::total_cmp);
        native.insert(scenario.id.clone(), json!(times[ITERATIONS / 2]));
    }

    let real_rows = match (
        path_from_env("FIELDGLASS_PERF_MANIFEST"),
        path_from_env("FIELDGLASS_PERF_CACHE"),
    ) {
        (Some(manifest), Some(cache)) => real::era5(&manifest, &cache)
            .unwrap_or_else(|e| fail(e))
            .into_iter()
            .map(|row| {
                json!({
                    "name": row.name,
                    "frames": row.frames,
                    "cells": row.cells,
                    "bytes": row.bytes,
                    "requests": row.requests,
                    "bound_bytes": row.bound_bytes,
                    "open_ms": row.open_ms,
                    "frame_ms": row.frame_ms,
                })
            })
            .collect(),
        (None, None) => Vec::new(),
        _ => fail("FIELDGLASS_PERF_MANIFEST and FIELDGLASS_PERF_CACHE go together"),
    };

    let report = json!({
        "iterations": ITERATIONS,
        "digest": corpus.digest(),
        "native": native,
        "real": real_rows,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("plain values serialise")
    );
}
