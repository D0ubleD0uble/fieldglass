//! A Zarr chunk that decompresses past its own length is refused without
//! reserving past that length (#981).
//!
//! Every codec in a chain is bounded by the chunk's length, its element count
//! times its element width. Before #981 the buffer each one stopped in still
//! grew by doubling: `miniz_oxide`'s bounded entry points through
//! `Vec::resize`, zstd's through `read_to_end`. So the step that reached the
//! limit could reserve nearly twice it, and on wasm linear memory never
//! shrinks.
//!
//! Each case is an 8 MiB run of zeros compressed by the codec's own encoder,
//! in a chunk of `(1536, 2048)` bytes, 3 MiB, which is not a power of two
//! times where either buffer starts, so doubling overshoots it. Each is read
//! through `ChunkDecoder`, the path a store's chunk reads take, under a
//! tracking global allocator that records the largest single allocation,
//! only while this thread has armed it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass_core::FieldglassError;
use fieldglass_zarr::ChunkDecoder;

/// The chunk's own length: the most any codec may write, and so the most any
/// one allocation of a read may be.
const CHUNK_BYTES: usize = 1536 * 2048;

/// What each bomb decompresses to.
const BOMB_BYTES: usize = 8 << 20;

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

/// A v2 `uint8` array of one `(1536, 2048)` chunk under `compressor`.
fn decoder(compressor: &str) -> ChunkDecoder {
    let meta = format!(
        r#"{{"zarr_format": 2, "shape": [1536, 2048], "chunks": [1536, 2048],
            "dtype": "|u1", "compressor": {compressor}, "fill_value": 0,
            "order": "C", "filters": null}}"#
    );
    ChunkDecoder::from_v2_metadata(&meta).expect("metadata")
}

/// The parse error reading `stored` gives, and the largest allocation the
/// read made. The decoder is built before arming.
fn refusal(compressor: &str, stored: &[u8]) -> (usize, String) {
    let decoder = decoder(compressor);
    LARGEST.with(|l| l.set(0));
    ARMED.with(|a| a.set(true));
    let result = decoder.decode(stored);
    ARMED.with(|a| a.set(false));
    match result {
        Err(FieldglassError::Parse(m)) => (LARGEST.with(Cell::get), m),
        other => panic!("{compressor}: expected a parse error, got {other:?}"),
    }
}

fn zlib_bomb() -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(&vec![0; BOMB_BYTES], 9)
}

fn zstd_bomb() -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(
        &vec![0; BOMB_BYTES][..],
        ruzstd::encoding::CompressionLevel::Fastest,
    )
}

/// A gzip member around a raw DEFLATE stream of the bomb. The trailer states
/// the true length; the stream is refused before it is reached.
fn gzip_bomb() -> Vec<u8> {
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF];
    out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(
        &vec![0; BOMB_BYTES],
        9,
    ));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(BOMB_BYTES as u32).to_le_bytes());
    out
}

/// A blosc1 buffer of one unsplit block that states the chunk's own length,
/// so the header passes, around `stream` in compressor format `format` (3 is
/// zlib, 4 zstd).
fn blosc(format: u8, stream: &[u8]) -> Vec<u8> {
    const HEADER: usize = 16;
    const DONT_SPLIT: u8 = 0x10;
    let start = HEADER + 4;
    let total = start + 4 + stream.len();
    let mut out = vec![2, 1, (format << 5) | DONT_SPLIT, 1];
    for word in [CHUNK_BYTES, CHUNK_BYTES, total, start] {
        out.extend_from_slice(&(word as u32).to_le_bytes());
    }
    out.extend_from_slice(&(stream.len() as i32).to_le_bytes());
    out.extend_from_slice(stream);
    out
}

fn assert_within_the_chunk(compressor: &str, stored: &[u8], message: &str) {
    let (largest, m) = refusal(compressor, stored);
    assert!(m.contains(message), "{compressor}: {m}");
    assert!(
        largest <= CHUNK_BYTES,
        "{compressor}: largest allocation {largest} bytes"
    );
}

/// `miniz_oxide`'s bounded entry point started at twice the stream and
/// doubled, so its last resize to the 3 MiB limit reserved past it. Before
/// #981 the largest allocation was larger still, 16,252,928 bytes: the error
/// message, which printed `DecompressError`'s `Debug`, every byte inflated.
#[test]
fn a_zlib_bomb_reserves_no_more_than_its_chunk() {
    assert_within_the_chunk(
        r#"{"id": "zlib", "level": 9}"#,
        &zlib_bomb(),
        "zlib stream inflates past the 3145728-byte ceiling",
    );
}

/// The same stream behind gzip's framing; the same 16,252,928 bytes before
/// #981.
#[test]
fn a_gzip_bomb_reserves_no_more_than_its_chunk() {
    assert_within_the_chunk(
        r#"{"id": "gzip", "level": 9}"#,
        &gzip_bomb(),
        "gzip stream inflates past the 3145728-byte ceiling",
    );
}

/// `read_to_end` doubled its buffer from 32 bytes to hold the limit plus the
/// one byte that tells a longer stream apart: 4,194,304 bytes before #981.
#[test]
fn a_zstd_bomb_reserves_no_more_than_its_chunk() {
    assert_within_the_chunk(
        r#"{"id": "zstd", "level": 1}"#,
        &zstd_bomb(),
        "zstd stream decompresses past the 3145728-byte ceiling",
    );
}

/// blosc's own buffers are sized from its header, which states the chunk's
/// length; the inner codec's are the ones that grew. 9,699,328 bytes for the
/// zlib block before #981, most of it the error message again.
#[test]
fn blosc_bombs_reserve_no_more_than_their_chunk() {
    let compressor = r#"{"id": "blosc", "cname": "zlib", "clevel": 9, "shuffle": 0}"#;
    assert_within_the_chunk(
        compressor,
        &blosc(3, &zlib_bomb()),
        "blosc zlib stream inflates past the 3145728-byte ceiling",
    );
    assert_within_the_chunk(
        compressor,
        &blosc(4, &zstd_bomb()),
        "zstd stream decompresses past the 3145728-byte ceiling",
    );
}
