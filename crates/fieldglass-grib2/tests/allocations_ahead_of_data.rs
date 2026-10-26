//! A §7 too short for what §5 and §3 state is refused before anything of the
//! stated size is allocated (#1001), through the real reader.
//!
//! Each case takes a committed fixture of the packing, states a large grid in
//! §3 and §5, and gives it a §7 of a few bytes. The decoders used to allocate
//! from those counts before the bytes behind them were read: PNG zeroed its
//! output buffer from IHDR before IDAT, run-length reserved the field before
//! its runs were counted, and the complex and second-order decoders stored
//! one table entry per stated group, which a zero-bit table field makes free
//! to state. A constant field still expands from a few bytes, as it must; the
//! last cases pin that.
//!
//! A tracking global allocator records the largest single allocation made on
//! this thread while armed, so tests on other threads cannot add to it. Each
//! message is parsed before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_grib2::Grib2Reader;

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

/// The most a refusal may allocate: its error message and the decoder's
/// fixed bookkeeping, nothing proportional to the stated field. The largest
/// fixed piece is the `png` reader's 128 KiB read buffer; every stated field
/// below is 16 MiB or more.
const REFUSAL_BUDGET: usize = 256 << 10;

const PNG: &[u8] = include_bytes!("fixtures/png_eta_lambert.grib2");
const RUN_LENGTH: &[u8] = include_bytes!("fixtures/runlength_regular_latlon.grib2");
const COMPLEX: &[u8] = include_bytes!("fixtures/complex_regular_latlon.grib2");
const COMPLEX_SPD: &[u8] = include_bytes!("fixtures/complex_spd2_regular_latlon.grib2");
const SECOND_ORDER: &[u8] = include_bytes!("fixtures/second_order_regular_latlon.grib2");

/// Each section of `message` after §0, as its number and byte range (header
/// included), up to the `7777` trailer.
fn sections(message: &[u8]) -> Vec<(u8, Range<usize>)> {
    let mut out = Vec::new();
    let mut at = 16;
    while &message[at..at + 4] != b"7777" {
        let len = u32::from_be_bytes(message[at..at + 4].try_into().unwrap()) as usize;
        out.push((message[at + 4], at..at + len));
        at += len;
    }
    out
}

/// `message` with section `number`'s body (after its five-byte header)
/// passed through `edit`, and every length field rewritten to match.
fn edit_section(message: &[u8], number: u8, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = message[..16].to_vec();
    let mut edit = Some(edit);
    for (n, range) in sections(message) {
        let mut body = message[range.start + 5..range.end].to_vec();
        if n == number {
            (edit.take().expect("one section of that number"))(&mut body);
        }
        out.extend_from_slice(&(body.len() as u32 + 5).to_be_bytes());
        out.push(n);
        out.extend_from_slice(&body);
    }
    out.extend_from_slice(b"7777");
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

/// `message` restated on an `ni × nj` grid: §3's point count and extents
/// (template 3.0 and 3.30 keep `Ni`/`Nx` and `Nj`/`Ny` at the same octets) and
/// §5's value count, with `ds` as the whole of §7 and `drs` applied to §5's
/// template payload (octet 12 on, so the template's own offsets).
fn restate(message: &[u8], ni: u32, nj: u32, drs: impl FnOnce(&mut [u8]), ds: &[u8]) -> Vec<u8> {
    let points = ni * nj;
    let message = edit_section(message, 3, |body| {
        body[1..5].copy_from_slice(&points.to_be_bytes());
        body[25..29].copy_from_slice(&ni.to_be_bytes());
        body[29..33].copy_from_slice(&nj.to_be_bytes());
    });
    let message = edit_section(&message, 5, |body| {
        body[0..4].copy_from_slice(&points.to_be_bytes());
        drs(&mut body[6..]);
    });
    edit_section(&message, 7, |body| *body = ds.to_vec())
}

/// Decode message 0 of `bytes` with the allocator armed.
fn decode(bytes: Vec<u8>) -> (usize, Result<Vec<Option<f64>>, FieldglassError>) {
    let reader = Grib2Reader::from_bytes(bytes).expect("the restated message parses");
    largest_allocation_in(|| reader.decode_message_values(0))
}

/// Assert `result` is a refusal naming `needle`, made within the budget.
fn assert_refused(largest: usize, result: Result<Vec<Option<f64>>, FieldglassError>, needle: &str) {
    let err = result.expect_err("a §7 this short cannot fill the stated field");
    assert!(
        largest < REFUSAL_BUDGET,
        "largest allocation {largest} bytes before refusing: {err}"
    );
    assert!(err.to_string().contains(needle), "{err}");
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A well-formed PNG whose IHDR states a `width × height` 16-bit grayscale
/// image and whose one IDAT is a zlib stream of 16 stored bytes.
fn png_stating(width: u32, height: u32) -> Vec<u8> {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[16, 0, 0, 0, 0]);
    // zlib header, one final stored block of 16 zero bytes, Adler-32 of them.
    let mut idat = vec![0x78, 0x01, 0x01, 16, 0, !16, 0xFF];
    idat.extend_from_slice(&[0; 16]);
    idat.extend_from_slice(&0x0010_0001u32.to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &idat);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// PNG (5.41): an IHDR stating 8192 × 8192 16-bit pixels, the 64 Mi-point
/// field cap, behind 16 bytes of IDAT. The 128 MiB output buffer was zeroed
/// before IDAT was read; no 65-byte §7 inflates to it.
#[test]
fn png_ihdr_past_what_its_idat_inflates_to_is_refused_before_its_buffer() {
    let png = png_stating(8192, 8192);
    let (largest, result) = decode(restate(PNG, 8192, 8192, |_| {}, &png));
    assert_refused(largest, result, "inflate to at most");
}

/// Run-length (5.200): one code, a run of one point, for a 64 Mi-point grid.
/// The whole field, 1 GiB of values, was reserved before the runs were
/// counted.
#[test]
fn run_length_codes_short_of_the_grid_are_refused_before_the_field() {
    let (largest, result) = decode(restate(RUN_LENGTH, 8192, 8192, |_| {}, &[1]));
    assert_refused(
        largest,
        result,
        "decoded 1 values but 67108864 were expected",
    );
}

/// Complex (5.2) with every group-table field zero bits wide: 4 Mi groups
/// cost §7 nothing, and three tables of them (the largest 32 MiB) were stored
/// before the lengths were found to sum to 1.
#[test]
fn complex_groups_stated_by_zero_width_tables_are_refused_before_the_tables() {
    let (largest, result) = decode(restate(
        COMPLEX,
        2048,
        2048,
        |t| zero_width_tables(t, 2048 * 2048),
        &[],
    ));
    assert_refused(
        largest,
        result,
        "group lengths sum to 1 but 4194304 values are required",
    );
}

/// 5.3 shares the check, after its spatial-differencing descriptors: the
/// fixture's two-octet descriptors take §7's first six bytes.
#[test]
fn complex_spd_groups_stated_by_zero_width_tables_are_refused_before_the_tables() {
    let (largest, result) = decode(restate(
        COMPLEX_SPD,
        2048,
        2048,
        |t| zero_width_tables(t, 2048 * 2048),
        &[0; 6],
    ));
    assert_refused(
        largest,
        result,
        "group lengths sum to 1 but 4194304 values are required",
    );
}

/// Complex packing's template fields: `NG` groups, every table field zero
/// bits wide, the last group's length 1 and the others' 0.
fn zero_width_tables(t: &mut [u8], groups: u32) {
    t[8] = 0; // bits per group reference
    t[20..24].copy_from_slice(&groups.to_be_bytes());
    t[24] = 0; // width reference
    t[25] = 0; // width bits
    t[26..30].copy_from_slice(&0u32.to_be_bytes()); // length reference
    t[30] = 1; // length increment
    t[31..35].copy_from_slice(&1u32.to_be_bytes()); // last group's length
    t[35] = 0; // length bits
}

/// Complex packing's template fields for one group of `points` points at
/// `width` bits, whose 16-bit reference is §7's first two bytes.
fn one_group(t: &mut [u8], points: u32, width: u8) {
    t[8] = 16;
    t[20..24].copy_from_slice(&1u32.to_be_bytes());
    t[24] = width;
    t[25] = 0;
    t[26..30].copy_from_slice(&0u32.to_be_bytes());
    t[30] = 1;
    t[31..35].copy_from_slice(&points.to_be_bytes());
    t[35] = 0;
}

/// Complex (5.2), one 8-bit group over a 4 Mi-point grid with no offsets
/// behind its reference: the 64 MiB output was reserved before the group's
/// 4 MiB of offsets were found missing.
#[test]
fn complex_group_offsets_missing_from_section_7_are_refused_before_the_output() {
    let (largest, result) = decode(restate(
        COMPLEX,
        2048,
        2048,
        |t| one_group(t, 2048 * 2048, 8),
        &[0, 5],
    ));
    assert_refused(largest, result, "offsets need 33554432 bits but §7 holds 0");
}

/// Second-order (5.50002) with every group-table field zero bits wide: 4 Mi
/// groups for nothing, three tables of them (the largest 16 MiB) stored
/// before the lengths were summed.
#[test]
fn second_order_groups_stated_by_zero_width_tables_are_refused_before_the_tables() {
    let groups = 2048u32 * 2048;
    let (largest, result) = decode(restate(
        SECOND_ORDER,
        2048,
        2048,
        |t| {
            t[8] = 0; // first-order bits per value
            t[9] = 0; // width of first-order values
            t[10..14].copy_from_slice(&groups.to_be_bytes());
            t[18] = 0; // width of widths
            t[19] = 0; // width of lengths
        },
        &[],
    ));
    assert_refused(
        largest,
        result,
        "group lengths + SPD reconstruct 2 values but 4194304 are required",
    );
}

/// Second-order (5.50002), one 8-bit group over the 4 Mi points the fixture's
/// two SPD seeds leave, with its width, length and first-order value in §7
/// and none of its offsets: the 32 MiB output was reserved before they were
/// found missing.
#[test]
fn second_order_group_offsets_missing_from_section_7_are_refused_before_the_output() {
    let length = 2048u32 * 2048 - 2;
    let mut ds = vec![8]; // the group's width, at 8 bits
    ds.extend_from_slice(&length.to_be_bytes()); // its length, at 32 bits
    ds.push(0); // its first-order value, at 8 bits
    let (largest, result) = decode(restate(
        SECOND_ORDER,
        2048,
        2048,
        |t| {
            t[9] = 8; // width of first-order values
            t[10..14].copy_from_slice(&1u32.to_be_bytes());
            t[18] = 8; // width of widths
            t[19] = 32; // width of lengths
        },
        &ds,
    ));
    assert_refused(largest, result, "offsets need 33554416 bits but §7 holds 0");
}

/// The other side: a constant group, one 16-bit reference in §7 and width 0,
/// legitimately fills a 1 Mi-point grid from two bytes, and still does.
#[test]
fn a_zero_width_complex_group_still_fills_the_grid_from_two_bytes() {
    let points = 1024 * 1024;
    let (_, result) = decode(restate(
        COMPLEX,
        1024,
        1024,
        |t| one_group(t, points, 0),
        &[0, 5],
    ));
    let values = result.expect("a constant group decodes");
    assert_eq!(values.len(), points as usize);
    let first = values[0].expect("present");
    assert!(values.iter().all(|&v| v == Some(first)));
}

/// And run-length: one code of level 2 and its base-250 run digits fill a
/// 1 Mi-point grid with level 2's value, `20 × 10^-1`.
#[test]
fn one_run_still_fills_the_grid_from_four_codes() {
    let points = 1024u64 * 1024;
    // The fixture's §5: 8-bit codes, max level 5, so digits are codes 6..=255
    // in base 250, least significant first, and extend the run by their value.
    let mut codes = vec![2u8];
    let mut extra = points - 1;
    while extra > 0 {
        codes.push((extra % 250) as u8 + 6);
        extra /= 250;
    }
    let (_, result) = decode(restate(RUN_LENGTH, 1024, 1024, |_| {}, &codes));
    let values = result.expect("one run covering the grid decodes");
    assert_eq!(values.len() as u64, points);
    assert!(values.iter().all(|&v| v == Some(2.0)), "{:?}", &values[..4]);
}
