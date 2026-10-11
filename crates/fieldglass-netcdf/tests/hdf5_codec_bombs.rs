//! A deflate or zstd chunk that decompresses past its limit is stopped
//! without reserving past it (#981).
//!
//! Each codec is bounded: it may write at most what the chunk can hold, the
//! chunk's own length when every filter before it keeps the length (#813),
//! and a ceiling derived from the chunk when one changes it (#975). Before
//! #981 the buffer it wrote into still grew by doubling, `miniz_oxide`'s
//! through `Vec::resize` and zstd's through `read_to_end`, so the step that
//! reached the limit could reserve nearly twice it. Near the 256 MiB chunk
//! ceiling that is about 512 MiB committed for a chunk that is then refused,
//! and on wasm linear memory never shrinks.
//!
//! The fixture comes from `tools/build_hdf5_codec_bomb_fixture.py`;
//! provenance in `tests/fixtures/NOTICE.md`. Its chunks are 3 MiB, which is
//! not a power of two times where either buffer starts, so doubling
//! overshoots it. A tracking global allocator records the largest single
//! allocation, only while this thread has armed it, so tests on other threads
//! cannot add to it. The file is parsed before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_netcdf::NetcdfReader;
use serde_json::Value;

// Embedded rather than read at run time: the wasm32-wasip1 run preopens only
// the working directory and its parent.
const FILE: &[u8] = include_bytes!("fixtures/hdf5_codec_bombs.h5");
const ORACLE: &str = include_str!("fixtures/hdf5_codec_bombs.h5.oracle.json");

/// One cell, for the region reads.
const CELL: [Range<u64>; 2] = [0..1, 0..1];

/// The chunk's own length, `(1536, 2048)` bytes: the most either codec may
/// write, and so the most any one allocation of a read may be. Everything
/// else a read allocates is smaller: `miniz_oxide`'s state is about 11 KB,
/// and `ruzstd`'s window buffer about a megabyte at most for this frame's
/// 512 KiB window.
const CHUNK_BYTES: usize = 1536 * 2048;

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
// const-initialised thread-locals, which never allocate.
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

/// The oracle's account of one bomb: its pipeline, a chunk of
/// [`CHUNK_BYTES`], and 32 MiB inside it.
fn check_oracle(name: &str, filter: u64) {
    let o = oracle(name);
    assert_eq!(o["pipeline"], serde_json::json!([filter]), "{o}");
    assert_eq!(o["chunk_bytes"], CHUNK_BYTES as u64);
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    assert_eq!(v["bomb_inflated_bytes"], 32 << 20);
    // libhdf5 reads zeros, with no error for the 29 MiB past the chunk; a
    // stream that runs past its chunk is refused here (#813).
    assert!(o["libhdf5_values"].is_array(), "{o}");
}

/// A 32 KB zlib stream of 32 MiB in a 3 MiB `[deflate]` chunk. `miniz_oxide`
/// started at twice the stream and doubled to 2,087,872 bytes, so its last
/// resize, to the 3,145,728-byte limit, reserved 4,175,744.
#[test]
fn a_deflate_bomb_reserves_no_more_than_its_chunk() {
    check_oracle("deflate_bomb", 1);
    for (largest, m) in refusals("deflate_bomb") {
        assert!(
            m.contains("deflate chunk decompresses past the chunk's 3145728-byte length"),
            "{m}"
        );
        assert!(largest <= CHUNK_BYTES, "largest allocation {largest} bytes");
    }
}

/// A 1 KB zstd frame of 32 MiB in a 3 MiB `[zstd]` chunk. `read_to_end`
/// doubled its buffer from 32 bytes, so it reserved 4,194,304 bytes to hold
/// the limit plus the one byte that tells a longer stream apart.
#[test]
fn a_zstd_bomb_reserves_no_more_than_its_chunk() {
    check_oracle("zstd_bomb", 32015);
    // `ruzstd` sizes its own buffer from the window, which no output bound
    // sees (#413). Small here, so the largest allocation is the output's.
    assert_eq!(oracle("zstd_bomb")["zstd_window_bytes"], 512 << 10);
    for (largest, m) in refusals("zstd_bomb") {
        assert!(
            m.contains("zstd chunk decompresses past the chunk's 3145728-byte length"),
            "{m}"
        );
        assert!(largest <= CHUNK_BYTES, "largest allocation {largest} bytes");
    }
}
