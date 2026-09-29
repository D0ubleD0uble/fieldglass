//! A decode allocates nothing (ADR-0012 decision 2), and neither does an szip
//! decompression, where libsz allocates a padded copy of up to 32 times the
//! output and a second copy to deinterleave byte planes.
//!
//! A counting global allocator counts only while this thread has armed it,
//! so tests running on other threads cannot add to a count. The stream is
//! built before arming, and covers every coding option at 1 Mi samples.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use common::{Bits, FIXTURES, SZ_CASES, manifest, rows, text, uint};
use fieldglass_aec::sz::{self, SzParams};
use fieldglass_aec::{AecError, Flags, Params, Sink, decode, decode_to_bytes};

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; counting touches only
// const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations made on this thread while `f` runs.
fn allocations_in<R>(f: impl FnOnce() -> R) -> (usize, R) {
    COUNT.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (COUNT.with(Cell::get), result)
}

const SAMPLES: usize = 1 << 20;
const BLOCK: u16 = 32;
const RSI: u16 = 128;

/// 16-bit preprocessed samples, 1 Mi of them, cycling through uncompressed,
/// split (every k), second-extension and zero blocks. The values are
/// arbitrary valid codes: the test is about allocation, not about values.
fn stream() -> (Params, Vec<u8>) {
    let params = Params::new(16, BLOCK, RSI, Flags::PREPROCESS | Flags::MSB).unwrap();
    let mut bits = Bits::default();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let blocks = SAMPLES / usize::from(BLOCK);
    for b in 0..blocks {
        let has_ref = b % usize::from(RSI) == 0;
        let coded = usize::from(BLOCK) - usize::from(has_ref);
        match b % 17 {
            0 => {
                bits.put(0b1111, 4);
                for _ in 0..BLOCK {
                    bits.put(next() & 0xFFFF, 16);
                }
            }
            1 => {
                // Second extension: small pairs.
                bits.put(0, 4).put(1, 1);
                if has_ref {
                    bits.put(next() & 0xFFFF, 16);
                    // Beside a reference the first pair is (0, δ): codewords
                    // 0, 2 and 5 are (0, 0), (0, 1) and (0, 2).
                    bits.fs([0, 2, 5][usize::try_from(next() % 3).unwrap()]);
                }
                for _ in 0..coded / 2 {
                    bits.fs(next() % 6);
                }
            }
            2 => {
                // One zero block.
                bits.put(0, 4).put(0, 1);
                if has_ref {
                    bits.put(next() & 0xFFFF, 16);
                }
                bits.fs(0);
            }
            option => {
                // Split, k from 0 to 13.
                let k = u32::try_from(option - 3).unwrap();
                bits.put(u64::from(k) + 1, 4);
                if has_ref {
                    bits.put(next() & 0xFFFF, 16);
                }
                for _ in 0..coded {
                    bits.fs(next() % 3);
                }
                if k > 0 {
                    for _ in 0..coded {
                        bits.put(next() & ((1 << k) - 1), k);
                    }
                }
            }
        }
    }
    (params, bits.done())
}

/// Folds the samples into a checksum, storing nothing.
struct Checksum(u64);

impl Sink for Checksum {
    fn samples(&mut self, block: &[u32]) {
        for &s in block {
            self.0 = self.0.rotate_left(5) ^ u64::from(s);
        }
    }

    fn repeat(&mut self, value: u32, count: usize) {
        for _ in 0..count {
            self.0 = self.0.rotate_left(5) ^ u64::from(value);
        }
    }
}

#[test]
fn the_counter_counts() {
    // Positive control: one deliberate allocation is seen.
    let (count, v) = allocations_in(|| black_box(vec![0u8; 64]));
    assert_eq!(count, 1);
    drop(v);
    let (count, ()) = allocations_in(|| ());
    assert_eq!(count, 0);
}

#[test]
fn decode_into_a_sink_allocates_nothing() {
    let (params, stream) = stream();
    let mut sink = Checksum(0);
    let (count, result) = allocations_in(|| decode(&stream, &params, SAMPLES, &mut sink));
    result.unwrap();
    assert_eq!(count, 0);
    assert_ne!(sink.0, 0);
}

#[test]
fn decode_to_bytes_allocates_nothing() {
    let (params, stream) = stream();
    let mut out = vec![0u8; SAMPLES * params.bytes_per_sample()];
    let (count, result) = allocations_in(|| decode_to_bytes(&stream, &params, &mut out));
    result.unwrap();
    assert_eq!(count, 0);
    assert!(out.iter().any(|&b| b != 0));
}

#[test]
fn an_error_allocates_nothing() {
    let (params, stream) = stream();
    let cut = &stream[..stream.len() / 2];
    let mut out = vec![0u8; SAMPLES * params.bytes_per_sample()];
    let (count, result) = allocations_in(|| decode_to_bytes(cut, &params, &mut out));
    assert!(matches!(result, Err(AecError::Truncated { .. })));
    assert_eq!(count, 0);
}

/// One pixel per scanline in blocks of 32, libsz's 32x worst case: 64 Ki
/// 8-bit pixels, each the reference sample of a zero block that covers its
/// 31 pads. libsz would allocate 2 MiB to decode this into 64 KiB.
#[test]
fn szip_at_one_pixel_per_scanline_allocates_nothing() {
    const PIXELS: u64 = 1 << 16;
    let params = SzParams::new(sz::NN_OPTION_MASK, 8, 32, 1).unwrap();
    let mut bits = Bits::default();
    for i in 0..PIXELS {
        // A zero block (id 0b000, then 0), the reference sample, one block.
        bits.put(0, 3).put(0, 1).put(i & 0xFF, 8).fs(0);
    }
    let stream = bits.done();
    let mut out = vec![0u8; 1 << 16];
    let (count, result) = allocations_in(|| sz::decompress(&stream, &params, &mut out));
    result.unwrap();
    assert_eq!(count, 0);
    assert!(
        out.iter()
            .enumerate()
            .all(|(i, &b)| usize::from(b) == i & 0xFF)
    );
}

/// Every szip case in the corpus, byte planes and padded scanlines included,
/// whole and cut short.
#[test]
fn szip_corpus_decodes_allocate_nothing() {
    let manifest = manifest();
    let mut checked = 0;
    for row in rows(&manifest, "sz_cases", SZ_CASES) {
        let name = text(row, "name");
        let field = |key| u32::try_from(uint(row, key)).unwrap();
        let params = SzParams::new(
            field("options_mask"),
            field("bits_per_pixel"),
            field("pixels_per_block"),
            field("pixels_per_scanline"),
        )
        .unwrap();
        let stream = std::fs::read(format!("{FIXTURES}/{}", text(row, "stream"))).unwrap();
        let mut out = vec![0u8; usize::try_from(uint(row, "dest_len")).unwrap()];
        let (count, result) = allocations_in(|| sz::decompress(&stream, &params, &mut out));
        result.unwrap();
        assert_eq!(count, 0, "{name}");

        let cut = &stream[..stream.len() / 2];
        let (count, result) = allocations_in(|| sz::decompress(cut, &params, &mut out));
        assert!(matches!(result, Err(AecError::Truncated { .. })), "{name}");
        assert_eq!(count, 0, "{name} cut");
        checked += 1;
    }
    assert_eq!(checked, SZ_CASES);
}
