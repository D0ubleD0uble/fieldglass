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
//! The other way round, a stream far shorter than its chunk is not handed
//! the chunk's length to fill (#986). #981 started an exact-length buffer at
//! the length, which the chunk's dimensions state and the stream need not
//! back, so a few bytes stating 256 MiB had 256 MiB zeroed before the length
//! check refused them. The start is now bounded by what the stream's bytes
//! can decode to.
//!
//! The fixture comes from `tools/build_hdf5_codec_bomb_fixture.py`;
//! provenance in `tests/fixtures/NOTICE.md`. Its chunks are 3 MiB, which is
//! not a power of two times where either buffer starts, so doubling
//! overshoots it. A tracking global allocator records the largest single
//! allocation and the peak of what the read holds at once, only while this
//! thread has armed it, so tests on other threads cannot add to it. The file
//! is parsed before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::ArrayError;
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
    /// Bytes allocated minus bytes freed since arming. Signed: the read may
    /// free blocks allocated before it.
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

/// Record a block of `size` bytes coming into being while `freed` bytes go
/// away. A `realloc` that moves holds both at once, so the peak counts the
/// new block before the old one is subtracted.
fn note(size: usize, freed: usize) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
            let _ = LIVE.try_with(|live| {
                let held = live.get() + size as isize;
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(held)));
                live.set(held - freed as isize);
            });
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; tracking touches only
// const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size(), 0);
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size(), 0);
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size, layout.size());
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(0, layout.size());
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// What one read allocated on this thread.
struct Measured {
    /// The largest single allocation.
    largest: usize,
    /// The most it held at once, counting both blocks of a moving `realloc`.
    peak: usize,
    /// The parse error it ended in.
    message: String,
}

/// The allocations made on this thread while `f` runs, and its result.
fn measured<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    LARGEST.with(|l| l.set(0));
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    let peak = PEAK.with(Cell::get).max(0) as usize;
    (LARGEST.with(Cell::get), peak, result)
}

/// One dataset's oracle entry.
fn oracle(name: &str) -> Value {
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    v["datasets"][name].clone()
}

/// The parse error each of a whole read and a one-cell region read of `name`
/// gives, with what each allocated.
fn refusals(name: &str) -> Vec<Measured> {
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
            let (largest, peak, result) = measured(|| {
                if region {
                    reader.decode_region_raw(index, &CELL)
                } else {
                    reader.decode_variable_raw(index)
                }
            });
            match result {
                Err(FieldglassError::Parse(message)) => Measured {
                    largest,
                    peak,
                    message,
                },
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

/// What a read may hold besides its output buffer: `miniz_oxide`'s state,
/// about 11 KB, and the reader's bookkeeping, about 33 KB more here.
const OVERHEAD: usize = 64 << 10;

/// A 32 KB zlib stream of 32 MiB in a 3 MiB `[deflate]` chunk. Before #981
/// `miniz_oxide` started at twice the stream and doubled to 2,087,872 bytes,
/// so its last resize, to the 3,145,728-byte limit, reserved 4,175,744, and
/// the read held 6,307,460 at once. The chunk's length is now where the
/// buffer starts, so a read holds one buffer of the chunk and nothing grows:
/// growing toward it instead held 5,277,444 at once, old and new buffers
/// side by side while one was copied into the other.
#[test]
fn a_deflate_bomb_holds_no_more_than_its_chunk() {
    check_oracle("deflate_bomb", 1);
    for r in refusals("deflate_bomb") {
        assert!(
            r.message
                .contains("deflate chunk decompresses past the chunk's 3145728-byte length"),
            "{}",
            r.message
        );
        assert!(
            r.largest <= CHUNK_BYTES,
            "largest allocation {} bytes",
            r.largest
        );
        assert!(
            r.peak <= CHUNK_BYTES + OVERHEAD,
            "held {} bytes at once",
            r.peak
        );
    }
}

/// A 1 KB zstd frame of 32 MiB in a 3 MiB `[zstd]` chunk. Before #981
/// `read_to_end` doubled its buffer from 32 bytes, so it reserved 4,194,304
/// bytes to hold the limit plus the one byte that tells a longer stream
/// apart, and the read held 8,397,951 at once.
///
/// `ruzstd` keeps its own buffers beside the output: a ring of up to about
/// twice the window, and up to a window more while it drains it. Those are
/// bounded by the window, which no output bound sees (#413), so the peak is
/// allowed three windows on top of the chunk. Reading in steps of a zstd
/// block (`capped::READ_STEP`) is what keeps the ring that small: handed the
/// whole 3 MiB at once, `ruzstd` buffered all of it a second time.
#[test]
fn a_zstd_bomb_holds_no_more_than_its_chunk_and_window() {
    check_oracle("zstd_bomb", 32015);
    let window = 512 << 10;
    assert_eq!(oracle("zstd_bomb")["zstd_window_bytes"], window);
    for r in refusals("zstd_bomb") {
        assert!(
            r.message
                .contains("zstd chunk decompresses past the chunk's 3145728-byte length"),
            "{}",
            r.message
        );
        assert!(
            r.largest <= CHUNK_BYTES,
            "largest allocation {} bytes",
            r.largest
        );
        assert!(
            r.peak <= CHUNK_BYTES + 3 * window as usize + OVERHEAD,
            "held {} bytes at once",
            r.peak
        );
    }
}

/// The length every short stream's chunk states: `(8192, 8192)` `float32`,
/// the most a chunk may decode to (#970).
const SHORT_CHUNK_BYTES: usize = 256 << 20;

/// The oracle's account of one short stream: `[shuffle, filter]`, a chunk
/// stating 256 MiB, and a stream of `stored` bytes that decodes to 64.
fn check_short_oracle(name: &str, filter: u64, stored: u64) {
    let o = oracle(name);
    assert_eq!(o["pipeline"], serde_json::json!([2, filter]), "{o}");
    assert_eq!(o["chunk_bytes"], SHORT_CHUNK_BYTES as u64);
    assert_eq!(o["chunk_stored_bytes"], stored);
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    assert_eq!(v["short_inflated_bytes"], 64);
    // libhdf5 reads it without error, and what it returns is mostly memory
    // its filters never wrote.
    assert!(o["libhdf5_read"].is_string(), "{o}");
}

/// A 12-byte zlib stream of 64 bytes in a `[shuffle, deflate]` chunk stating
/// 256 MiB. The chunk's length is exact, and #981 started the buffer there,
/// so the read zeroed 268,435,456 bytes before the stream ended and the
/// length check refused it (#986). The start is now bounded by the most 12
/// bytes of DEFLATE can produce, 1,032 per byte (RFC 1951).
#[test]
fn a_short_deflate_stream_in_a_256_mib_chunk_zeroes_no_more_than_it_can_fill() {
    check_short_oracle("deflate_short_stream", 1, 12);
    for r in refusals("deflate_short_stream") {
        assert_eq!(r.message, "chunk decoded to 64 bytes, expected 268435456");
        // 12 × 1,032 = 12,384 bytes of buffer, beside `miniz_oxide`'s state:
        // about 24 KB held at once, where master held 268,435,456.
        assert!(
            r.largest < OVERHEAD,
            "largest allocation {} bytes",
            r.largest
        );
        assert!(r.peak < OVERHEAD, "held {} bytes at once", r.peak);
    }
}

/// A 17-byte zstd frame of 64 bytes in a `[shuffle, zstd]` chunk stating
/// 256 MiB: 268,435,456 bytes zeroed before #986, as for deflate. The start
/// is now bounded by the most 17 bytes of zstd can produce: every block has
/// a 3-byte header and decodes to at most 128 KiB (RFC 8878 section
/// 3.1.1.2), so six blocks, 786,432 bytes. That buffer is the largest
/// allocation the read makes; it holds about 796 KB at once.
#[test]
fn a_short_zstd_frame_in_a_256_mib_chunk_zeroes_no_more_than_it_can_fill() {
    check_short_oracle("zstd_short_stream", 32015, 17);
    for r in refusals("zstd_short_stream") {
        assert_eq!(r.message, "chunk decoded to 64 bytes, expected 268435456");
        assert!(
            r.largest <= 6 * (128 << 10),
            "largest allocation {} bytes",
            r.largest
        );
        assert!(
            r.peak <= 6 * (128 << 10) + OVERHEAD,
            "held {} bytes at once",
            r.peak
        );
    }
}

const WIDE_WINDOW: &[u8] = include_bytes!("fixtures/hdf5_zstd_wide_window.h5");
const WIDE_WINDOW_ORACLE: &str = include_str!("fixtures/hdf5_zstd_wide_window.h5.oracle.json");

/// A zstd frame stating a 64 MiB window in a 4 KiB `[zstd]` chunk, from
/// `tools/build_zstd_wide_window_fixtures.py` (#989).
///
/// `ruzstd` hands back nothing until it holds more than the frame's window,
/// which RFC 8878 section 3.1.1.1.2 makes a decoder keep, so the chunk's
/// 4,096-byte limit saw no output until the decoder held 64 MiB. Before #989
/// a read made one 134,217,729-byte allocation and held 201,348,425 bytes at
/// once, then refused the chunk as decompressing past its length. The window
/// a decoder may keep is now `max(chunk, 8 MiB)`, 8 MiB being the RFC's
/// recommended minimum, so the frame is refused at its header and the
/// decoder holds none of it. A frame may legally state a window larger than
/// its content, so the refusal says the file is fine.
#[test]
fn a_zstd_window_far_past_its_chunk_is_refused_before_the_decoder_holds_it() {
    let o: Value = serde_json::from_str(WIDE_WINDOW_ORACLE).expect("oracle JSON");
    assert_eq!(o["pipeline"], serde_json::json!([32015]), "{o}");
    assert_eq!(o["chunk_bytes"], 4096);
    assert_eq!(o["zstd_single_segment"], false);
    assert_eq!(o["zstd_window_bytes"], 64 << 20);
    // libhdf5's zstd filter fails the read too, as a filter failure.
    assert!(o["libhdf5_error"].is_string(), "{o}");

    let reader = NetcdfReader::from_bytes(WIDE_WINDOW.to_vec()).expect("parse");
    let view = reader.view().expect("view");
    let index = view
        .vars
        .iter()
        .find(|v| v.name() == "zstd_wide_window")
        .expect("zstd_wide_window is listed")
        .decode_index;
    for region in [false, true] {
        let (largest, peak, result) = measured(|| {
            if region {
                reader.decode_region_raw(index, &CELL)
            } else {
                reader.decode_variable_raw(index)
            }
        });
        assert!(
            largest < OVERHEAD,
            "largest allocation {largest} bytes, {peak} held at once"
        );
        assert!(peak < OVERHEAD, "held {peak} bytes at once");
        match result {
            Err(FieldglassError::Array(ArrayError::WindowTooLarge {
                codec: "zstd",
                window,
                limit,
            })) => {
                assert_eq!(window, 64 << 20);
                assert_eq!(limit, 8 << 20);
            }
            other => panic!("region {region}: expected the window refused, got {other:?}"),
        }
    }
}
