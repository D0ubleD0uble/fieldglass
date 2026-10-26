//! A Zarr chunk that decompresses past its own length is refused without
//! reserving past that length (#981).
//!
//! Every codec in a chain is bounded by the most its own output can be,
//! which for a compressor with nothing growing the bytes ahead of it, as here,
//! is the chunk's length, its element count times its element width (#1012).
//! Before #981 the buffer each one stopped in still grew by doubling:
//! `miniz_oxide`'s bounded entry points through `Vec::resize`, zstd's through
//! `read_to_end`. So the step that reached the limit could reserve nearly
//! twice it, and on wasm linear memory never shrinks.
//!
//! Each case is an 8 MiB run of zeros compressed by the codec's own encoder,
//! in a chunk of `(1536, 2048)` bytes, 3 MiB, which is not a power of two
//! times where either buffer starts, so doubling overshoots it. Each is read
//! through `ChunkDecoder`, the path a store's chunk reads take, under a
//! tracking global allocator that records the largest single allocation,
//! only while this thread has armed it.
#![cfg(feature = "codecs")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass_core::FieldglassError;
use fieldglass_zarr::ChunkDecoder;

/// The chunk's own length: the most a compressor with nothing growing the
/// bytes ahead of it may write, and so the most any one allocation of such a
/// read may be.
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
// const-initialized thread-locals, which never allocate.
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

/// A v2 `uint8` array of one chunk of `bytes` under `compressor`.
fn decoder_of(compressor: &str, bytes: usize) -> ChunkDecoder {
    let meta = format!(
        r#"{{"zarr_format": 2, "shape": [{bytes}], "chunks": [{bytes}],
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

/// A v3 `uint8` array of one `(1536, 2048)` chunk under `codecs`, a JSON list
/// after `bytes`.
fn v3_decoder(codecs: &str) -> ChunkDecoder {
    let meta = format!(
        r#"{{"zarr_format": 3, "node_type": "array", "shape": [1536, 2048],
            "data_type": "uint8",
            "chunk_grid": {{"name": "regular", "configuration": {{"chunk_shape": [1536, 2048]}}}},
            "chunk_key_encoding": {{"name": "default"}}, "fill_value": 0,
            "codecs": [{{"name": "bytes"}}, {codecs}]}}"#
    );
    ChunkDecoder::from_v3_metadata(&meta).expect("metadata")
}

/// Behind a codec that grows the bytes, a compressor is held to what that
/// codec could have handed it, not to the chunk, and its bomb stops there
/// (#1012). A chain of any length stops at twice the chunk and 4 KiB.
#[test]
fn a_bomb_behind_a_growing_codec_stops_at_its_own_ceiling() {
    let gzip = r#"{"name": "gzip", "configuration": {"level": 5}}"#;
    let behind_crc = v3_decoder(&format!(r#"{{"name": "crc32c"}}, {gzip}"#));
    let (largest, _, m) = measured_refusal(&behind_crc, "crc32c, gzip", &gzip_bomb());
    let ceiling = CHUNK_BYTES + 4;
    assert!(
        m.contains(&format!("past the {ceiling}-byte ceiling")),
        "{m}"
    );
    assert!(largest <= ceiling, "largest allocation {largest} bytes");

    let long = v3_decoder(&[gzip; 12].join(", "));
    let (largest, _, m) = measured_refusal(&long, "gzip x12", &gzip_bomb());
    let cap = 2 * CHUNK_BYTES + 4096;
    assert!(m.contains(&format!("past the {cap}-byte ceiling")), "{m}");
    assert!(largest <= cap, "largest allocation {largest} bytes");
}

/// What every short blosc buffer's header states: one block of 256 MiB.
const STATED: usize = 256 << 20;

/// An LZ4 block of 64 zero bytes, hand-built: one literal zero, a match at
/// offset 1 of 4 + 15 + 40 bytes, then four literal zeros to end the block.
const LZ4_64: [u8; 10] = [0x1F, 0, 1, 0, 40, 0x40, 0, 0, 0, 0];

/// A BloscLZ stream of 64 zero bytes, hand-built: a one-byte literal run of a
/// zero, then a match at distance 1 of 7 + 54 + 2 bytes.
const BLOSCLZ_64: [u8; 5] = [0x00, 0x00, 0xE0, 54, 0x00];

/// A blosc buffer of a few dozen bytes whose header states one 256 MiB block,
/// around a stream of 64 bytes, for each compressor blosc decodes. blosc
/// reserved the buffer and the block from its header before any stream
/// proved them, and the inner codec its output too: three 256 MiB
/// reservations, 805,306,527 bytes held at once for the 29-byte blosclz
/// buffer (#986). Each is now bounded by what the bytes behind it could
/// decode to: 255 per byte of blosclz or LZ4, 1,032 of zlib (RFC 1951), and
/// 128 KiB per three bytes of zstd (RFC 8878), the loosest. The read holds
/// 11,124 bytes at once for blosclz, 14,944 for LZ4, 104,461 for zlib and
/// 3,154,667 for zstd.
#[test]
fn a_tiny_blosc_buffer_stating_256_mib_reserves_what_its_bytes_could_fill() {
    let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&[0; 64], 9);
    let zstd = ruzstd::encoding::compress_to_vec(
        &[0u8; 64][..],
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    let cases: [(&str, u8, &[u8], usize); 4] = [
        ("blosclz", 0, &BLOSCLZ_64, 64 << 10),
        ("lz4", 1, &LZ4_64, 64 << 10),
        ("zlib", 3, &zlib, 256 << 10),
        ("zstd", 4, &zstd, 4 << 20),
    ];
    for (cname, format, stream, bound) in cases {
        let compressor =
            format!(r#"{{"id": "blosc", "cname": "{cname}", "clevel": 9, "shuffle": 0}}"#);
        let decoder = decoder_of(&compressor, STATED);
        let stored = blosc_of(format, stream, STATED);
        let (_, peak, m) = measured_refusal(&decoder, &compressor, &stored);
        assert!(
            m.contains("decoded to 64 bytes, not the 268435456"),
            "{cname}: {m}"
        );
        assert!(peak <= bound, "{cname}: held {peak} bytes at once");
    }
}

/// A Zarr v3 chunk of `(16384, 16384)` bytes, 256 MiB, under `transpose`,
/// stored as 64 bytes. The transpose zeroed a buffer of the chunk's length
/// before its first copy found the data short (#986); it now refuses a chunk
/// whose length is not the chunk's before allocating anything for it.
#[test]
fn a_short_transposed_chunk_is_refused_before_its_buffer_is_zeroed() {
    let meta = r#"{"zarr_format": 3, "node_type": "array", "shape": [16384, 16384],
        "data_type": "uint8",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [16384, 16384]}},
        "chunk_key_encoding": {"name": "default"}, "fill_value": 0,
        "codecs": [{"name": "transpose", "configuration": {"order": [1, 0]}},
                   {"name": "bytes"}]}"#;
    let decoder = ChunkDecoder::from_v3_metadata(meta).expect("metadata");
    let (largest, _, m) = measured_refusal(&decoder, "transpose", &[0u8; 64]);
    assert!(
        m.contains("a transposed chunk of 64 bytes cannot hold 268435456 elements"),
        "{m}"
    );
    // The stored bytes' copy and nothing near the chunk: master zeroed
    // 268,435,456 bytes here.
    assert!(largest < 4096, "largest allocation {largest} bytes");
}

/// A zstd frame stating a 64 MiB window as the one 4 KiB chunk of a v2
/// store, from `tools/build_zstd_wide_window_fixtures.py` (#989), read
/// through `ZarrStore` the way a host reads a region.
///
/// `ruzstd` hands back nothing until it holds more than the frame's window,
/// which RFC 8878 section 3.1.1.1.2 makes a decoder keep, so the chunk's
/// 4,096-byte limit saw no output until the decoder held 64 MiB. The window
/// a decoder may keep is now `max(chunk, 8 MiB)`, 8 MiB being the RFC's
/// recommended minimum, so the frame is refused at its header and the decoder
/// holds none of it. A frame may legally state a window larger than its
/// content, so the refusal says the file is fine.
#[test]
fn a_zstd_window_far_past_its_chunk_is_refused_before_the_decoder_holds_it() {
    use fieldglass_core::array::{ArrayError, ArraySource};
    use fieldglass_core::bytes::MemoryObjects;
    use fieldglass_zarr::ZarrStore;

    // Embedded, not read at run time, so the wasm32-wasip1 run needs no path.
    let frame: &[u8] = include_bytes!("fixtures/zstd_wide_window/0.0");
    assert_eq!(frame[..4], [0x28, 0xB5, 0x2F, 0xFD], "zstd magic");
    assert_eq!(frame[4] & 0x20, 0, "windowed, not single-segment");
    assert_eq!(frame[5], 0x80, "Window_Size 2^26, 64 MiB");
    let objects = MemoryObjects::from_iter([
        (
            ".zarray",
            include_bytes!("fixtures/zstd_wide_window/.zarray").to_vec(),
        ),
        ("0.0", frame.to_vec()),
    ]);
    let store = ZarrStore::open(&objects).expect("the store opens");
    let source: &dyn ArraySource = &store;
    let (name, _) = source
        .group()
        .arrays_qualified()
        .into_iter()
        .next()
        .expect("one array");
    for region in [[0..64, 0..64], [0..1, 0..1]] {
        LARGEST.with(|l| l.set(0));
        LIVE.with(|l| l.set(0));
        PEAK.with(|p| p.set(0));
        ARMED.with(|a| a.set(true));
        let result = source.read_region(&name, &region);
        ARMED.with(|a| a.set(false));
        let (largest, peak) = (LARGEST.with(Cell::get), PEAK.with(Cell::get));
        // The whole region's output, 4,096 values of 16 bytes, is the largest
        // allocation, beside the stored frame's copy and the read's
        // bookkeeping: about 70 KB at once. Before #989 the read made one
        // 134,217,729-byte allocation and held 201,409,484 bytes at once.
        assert!(
            largest <= 64 << 10,
            "largest allocation {largest} bytes, {peak} held at once"
        );
        assert!(peak < 128 << 10, "held {peak} bytes at once");
        match result {
            Err(FieldglassError::Array(ArrayError::WindowTooLarge {
                codec: "zstd",
                window,
                limit,
            })) => {
                assert_eq!(window, 64 << 20);
                assert_eq!(limit, 8 << 20);
            }
            other => panic!("{region:?}: expected the window refused, got {other:?}"),
        }
    }
}
