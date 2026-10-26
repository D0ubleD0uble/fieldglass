//! An szip chunk that states more than its bytes can hold is refused before
//! the stated length is allocated (#975).
//!
//! A chunk's decoded length comes from its dimensions, the file's own numbers,
//! and may be up to 256 MiB (#970). Two szip paths used to take it on trust:
//!
//! - **The size prefix.** With only shuffle before szip, the prefix must equal
//!   the chunk's length, and the reader zeroed a buffer of that length before
//!   decoding. A 20-byte chunk stating 256 MiB committed 256 MiB, which on
//!   wasm is real linear memory, then failed. No szip stream codes more than
//!   its length allows (CCSDS 121.0-B-3 Tables 3-2 and 5-1), so the reader
//!   now refuses the prefix first.
//! - **A codec behind szip.** With `[szip, deflate]`, deflate's output is
//!   szip's stream, whose length nothing records. It was bounded by 33 times
//!   the chunk, so a 4 MiB chunk let a 32 KB bomb inflate all 32 MiB. It is
//!   now bounded by the longest stream szip's own parameters allow for the
//!   chunk, here about the chunk itself.
//!
//! The fixture comes from `tools/build_hdf5_szip_hostile_fixture.py`;
//! provenance in `tests/fixtures/NOTICE.md`. A tracking global allocator
//! records the largest single allocation, only while this thread has armed
//! it, so tests on other threads cannot add to it. The file is parsed before
//! arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_netcdf::NetcdfReader;
use serde_json::Value;

// Embedded rather than read at run time: the wasm32-wasip1 run preopens only
// the working directory and its parent.
const FILE: &[u8] = include_bytes!("fixtures/hdf5_szip_hostile.h5");
const ORACLE: &str = include_str!("fixtures/hdf5_szip_hostile.h5.oracle.json");

/// One cell, for the region reads.
const CELL: [Range<u64>; 2] = [0..1, 0..1];

struct Tracking;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; tracking touches only
// const-initialized thread-locals, which never allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// The largest single allocation made on this thread while `f` runs.
fn largest_allocation_in<R>(f: impl FnOnce() -> R) -> (usize, R) {
    LARGEST.with(|l| l.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (LARGEST.with(Cell::get), result)
}

/// One dataset's oracle entry.
fn oracle(name: &str) -> Value {
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    v["datasets"][name].clone()
}

/// The parse error each of a whole read and a one-cell region read of `name`
/// gives, with the largest allocation either made.
fn refusals(name: &str) -> Vec<(usize, String)> {
    let reader = NetcdfReader::from_bytes(FILE.to_vec()).expect("parse");
    let view = reader.view().expect("view");
    let index = view
        .vars
        .iter()
        .find(|v| v.name() == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .decode_index;
    [false, true]
        .into_iter()
        .map(|region| {
            let (largest, result) = largest_allocation_in(|| {
                if region {
                    reader.decode_region_raw(index, &CELL)
                } else {
                    reader.decode_variable_raw(index)
                }
            });
            match result {
                Err(FieldglassError::Parse(m)) => (largest, m),
                other => panic!("{name}: expected a parse error, got {other:?}"),
            }
        })
        .collect()
}

/// A 20-byte chunk under `[shuffle, szip]` whose prefix states 256 MiB, the
/// most a chunk may decode to, ahead of a 16-byte stream. Before #975 the
/// reader zeroed 268,435,456 bytes for it and then failed in the codec. 16
/// bytes of szip at these parameters (32-bit pixels coded as 8-bit byte
/// planes, blocks of 32, scanlines of 4096) code at most `⌊64·128 / 9⌋ = 910`
/// blocks of 32 bytes, 29,120 bytes.
#[test]
fn a_short_stream_stating_256_mib_is_refused_before_it_is_allocated() {
    let o = oracle("szip_short_stream");
    assert_eq!(o["pipeline"], serde_json::json!([2, 4]), "[shuffle, szip]");
    assert_eq!(o["szip_cd_values"], serde_json::json!([169, 32, 32, 4096]));
    assert_eq!(o["chunk_bytes"], 268_435_456);
    assert_eq!(o["chunk_stored_bytes"], 20);
    // libhdf5 reads it as zeros: libsz returns success on a stream that ends
    // early (ADR-0012 decision 4), so the chunk is whatever its buffer held.
    assert!(o["libhdf5_values"].is_array(), "{o}");

    for (largest, m) in refusals("szip_short_stream") {
        assert!(
            m.contains(
                "declares 268435456 bytes, but its 16-byte stream can decode to at most 29120 bytes"
            ),
            "{m}"
        );
        // Nothing near the stated length: about 160 bytes, where master
        // allocated 268,435,456.
        assert!(largest < 4096, "largest allocation {largest} bytes");
    }
}

/// A 32 KB zlib stream of 32 MiB under `[szip, deflate]` with a 4 MiB chunk.
/// deflate's output is szip's stream, bounded by the longest stream szip's
/// parameters (8-bit pixels, blocks of 32, scanlines of 2048) allow for 4 MiB:
/// 131,072 blocks of 3 + 256 bits, 4,243,456 bytes, plus 4 KiB for the size
/// prefix and padding: 4,247,552. Before #975 the bound was 33 times the
/// chunk, 138 MB, so all 32 MiB inflated; until #1016 no codec got less than
/// the chunk plus an eighth plus 4 KiB (4,722,688).
#[test]
fn a_bomb_behind_szip_stops_at_what_szip_could_hold() {
    let o = oracle("szip_deflate_bomb");
    assert_eq!(o["pipeline"], serde_json::json!([4, 1]), "[szip, deflate]");
    assert_eq!(o["szip_cd_values"], serde_json::json!([141, 32, 8, 2048]));
    assert_eq!(o["chunk_bytes"], 4_194_304);
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    let inflated = v["bomb_inflated_bytes"].as_u64().expect("bomb size");
    assert_eq!(inflated, 32 << 20);
    // libhdf5 inflates all of it and then fails in szip.
    assert!(o["libhdf5_error"].is_string(), "{o}");

    for (largest, m) in refusals("szip_deflate_bomb") {
        assert!(
            m.contains("deflate chunk decompresses past the 4247552-byte ceiling"),
            "{m}"
        );
        // Within the bound itself, and far from the bomb: before #975 the
        // reader allocated 66,811,904, and before #981 miniz_oxide's last
        // resize reserved 8,351,488 for a buffer bounded at 4,722,688.
        assert!(largest <= 4_247_552, "largest allocation {largest} bytes");
        assert!(
            (largest as u64) < inflated / 4,
            "largest allocation {largest} bytes"
        );
    }
}
