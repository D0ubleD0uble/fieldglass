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
    let (largest, _, m) = measured_refusal(&decoder(compressor), compressor, stored);
    (largest, m)
}

/// The parse error `decoder` gives for `stored`, with the largest allocation
/// the read made and the most it held at once.
fn measured_refusal(
    decoder: &ChunkDecoder,
    compressor: &str,
    stored: &[u8],
) -> (usize, usize, String) {
    LARGEST.with(|l| l.set(0));
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = decoder.decode(stored);
    ARMED.with(|a| a.set(false));
    let peak = PEAK.with(Cell::get).max(0) as usize;
    match result {
        Err(FieldglassError::Parse(m)) => (LARGEST.with(Cell::get), peak, m),
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
    blosc_of(format, stream, CHUNK_BYTES)
}

/// [`blosc`] for a buffer and a block that state `bytes`.
fn blosc_of(format: u8, stream: &[u8], bytes: usize) -> Vec<u8> {
    const HEADER: usize = 16;
    const DONT_SPLIT: u8 = 0x10;
    let start = HEADER + 4;
    let total = start + 4 + stream.len();
    let mut out = vec![2, 1, (format << 5) | DONT_SPLIT, 1];
    for word in [bytes, bytes, total, start] {
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

/// A blosc buffer stating one 3 MiB block, around a stream of 64 bytes.
/// blosc reserves the buffer and the block from its header before decoding,
/// two of 3 MiB. #981 then started the inner codec's buffer at the stream's
/// stated length as well, a third 3 MiB, zeroed before the stream ended and
/// the block's length check refused it (#986). That start is now bounded by
/// what the stream's own bytes can decode to: 1,032 per byte of zlib
/// (RFC 1951), 128 KiB per three bytes of zstd (RFC 8878).
#[test]
fn a_short_blosc_stream_is_not_handed_its_stated_length_to_fill() {
    let compressor = r#"{"id": "blosc", "cname": "zlib", "clevel": 9, "shuffle": 0}"#;
    let decoder = decoder(compressor);
    let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&[0; 64], 9);
    let zstd = ruzstd::encoding::compress_to_vec(
        &[0u8; 64][..],
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    for (format, stream) in [(3, zlib), (4, zstd)] {
        let (_, peak, m) = measured_refusal(&decoder, compressor, &blosc(format, &stream));
        assert!(
            m.contains(&format!("decoded to 64 bytes, not the {CHUNK_BYTES}")),
            "format {format}: {m}"
        );
        // The buffer and the block, and the codec's start beside them, which
        // is under a megabyte for either stream: 6,323,677 bytes held at once
        // for zlib and 6,955,755 for zstd, where zlib held 9,447,733 before.
        assert!(
            peak <= 2 * CHUNK_BYTES + (1 << 20),
            "format {format}: held {peak} bytes at once"
        );
    }
}
