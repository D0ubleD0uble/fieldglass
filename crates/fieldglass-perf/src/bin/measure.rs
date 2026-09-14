//! The deterministic tier: bytes, requests, allocations and peak heap per
//! scenario.
//!
//!     FIELDGLASS_PERF_CORPUS=<dir> cargo run --release --bin measure > measured.json
//!
//! Each scenario runs twice. Once over a [`Recording`] source for the I/O
//! numbers, and once over the plain in-memory source under dhat's heap
//! profiler, so the recording's own bookkeeping — a `String` per `get` — is
//! never counted as the reader's allocation. The profiler starts after the
//! scenario is prepared and stops before its output is dropped, so a row is the
//! operation and nothing around it.
//!
//! dhat counts requested sizes, not what the system allocator rounds them to,
//! which is why these numbers are the same on every machine for the same code
//! and toolchain. That is what lets them gate a PR exactly.
//!
//! [`Recording`]: fieldglass_core::testing::Recording

use fieldglass_perf::{Corpus, Prepared, Via, bound_bytes, catalogue};
use serde_json::{Map, Value, json};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let corpus = Corpus::from_env().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let scenarios = catalogue(&corpus);
    let mut rows = Map::new();
    for scenario in &scenarios {
        let mut recorded = Prepared::new(&corpus, scenario, Via::Recorded);
        recorded.execute();
        let io = recorded.io();
        drop(recorded);

        let mut prepared = Prepared::new(&corpus, scenario, Via::Memory);
        let (cells, stats) = {
            let _profiler = dhat::Profiler::builder().testing().build();
            let cells = prepared.execute();
            (cells, dhat::HeapStats::get())
        };
        let width = prepared.value_width();
        drop(prepared);

        eprintln!(
            "{:<40} {:>10} blocks {:>12} peak",
            scenario.id, stats.total_blocks, stats.max_bytes
        );
        rows.insert(
            scenario.id.clone(),
            json!({
                "cells": cells,
                "width": width,
                "bytes": io.bytes,
                "requests": io.requests,
                "bound_bytes": bound_bytes(&corpus, scenario),
                "input_bytes": corpus.total_bytes(&scenario.input),
                "blocks": stats.total_blocks,
                "peak": stats.max_bytes,
            }),
        );
    }
    // Catalogue order, which is also the order Gungraun numbers its benchmarks
    // in: `scenario_N` is `order[N]`.
    let order: Vec<&str> = scenarios.iter().map(|s| s.id.as_str()).collect();
    let report =
        json!({ "digest": corpus.digest(), "order": order, "scenarios": Value::Object(rows) });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("plain values serialise")
    );
}
