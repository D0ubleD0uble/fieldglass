//! A second-order BDS too short for what its header states is refused before
//! anything of the stated size is allocated (#1001), through the real reader.
//!
//! Each case takes a committed hand-built fixture of the layout, restates its
//! grid as 2048 × 2048 in the GDS, and replaces the BDS with one of a few
//! bytes. The decoders used to allocate from the header's group count before
//! the bytes behind it were read: `row_by_row` laid out one row length per
//! stated group before reading the groups' widths, the general-extended
//! decoder stored one table entry per group, which a zero-bit table field
//! makes free to state, and both reserved the grid before its residuals were
//! found missing.
//!
//! A tracking global allocator records the largest single allocation made on
//! this thread while armed, so tests on other threads cannot add to it. Each
//! message is parsed before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass_grib1::{FieldglassError, Grib1Reader};

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

/// The most a refusal may allocate: its error message and the decoder's
/// fixed bookkeeping, nothing proportional to the stated field. Every stated
/// table or field below is 16 MiB or more.
const REFUSAL_BUDGET: usize = 64 << 10;

const ROW_BY_ROW: &[u8] = include_bytes!("fixtures/hand_second_order_row_by_row.grib1");
const EXTENDED: &[u8] = include_bytes!("fixtures/hand_second_order_no_SPD.grib1");

/// The grid every case states, and its point count.
const SIDE: u16 = 2048;
const POINTS: usize = SIDE as usize * SIDE as usize;

fn u24(bytes: &[u8]) -> usize {
    usize::from(bytes[0]) << 16 | usize::from(bytes[1]) << 8 | usize::from(bytes[2])
}

fn put_u24(out: &mut [u8], value: usize) {
    out.copy_from_slice(&(value as u32).to_be_bytes()[1..]);
}

/// `message` (one PDS, one GDS, no BMS) with the GDS restating a
/// `SIDE × SIDE` latitude/longitude grid and `bds` as its whole BDS, its
/// first three octets set to its length.
fn restate(message: &[u8], mut bds: Vec<u8>) -> Vec<u8> {
    let pds_end = 8 + u24(&message[8..]);
    assert_eq!(message[8 + 7] & 0xC0, 0x80, "a GDS and no BMS");
    let gds_end = pds_end + u24(&message[pds_end..]);
    let mut out = message[..gds_end].to_vec();
    out[pds_end + 6..pds_end + 8].copy_from_slice(&SIDE.to_be_bytes()); // Ni
    out[pds_end + 8..pds_end + 10].copy_from_slice(&SIDE.to_be_bytes()); // Nj
    let len = bds.len();
    put_u24(&mut bds[..3], len);
    out.extend_from_slice(&bds);
    out.extend_from_slice(b"7777");
    let total = out.len();
    put_u24(&mut out[4..7], total);
    out
}

/// The fixture's own BDS, for its flag octets.
fn fixture_bds(message: &[u8]) -> &[u8] {
    let pds_end = 8 + u24(&message[8..]);
    let gds_end = pds_end + u24(&message[pds_end..]);
    &message[gds_end..gds_end + u24(&message[gds_end..])]
}

/// Decode message 0 of `bytes` with the allocator armed, and assert it is a
/// refusal naming `needle`, made within the budget.
fn assert_refused(bytes: Vec<u8>, needle: &str) {
    let reader = Grib1Reader::from_bytes(bytes).expect("the restated message parses");
    let (largest, result): (usize, Result<_, FieldglassError>) =
        largest_allocation_in(|| reader.decode_message_values(0));
    let err = result.expect_err("a BDS this short cannot fill the stated field");
    assert!(
        largest < REFUSAL_BUDGET,
        "largest allocation {largest} bytes before refusing: {err}"
    );
    assert!(err.to_string().contains(needle), "{err}");
}

/// `row_by_row`'s fixed header with `groups` groups stated:
/// `codedNumberOfGroups + 65536 · extraValues`.
fn row_by_row_header(groups: usize) -> Vec<u8> {
    let mut bds = fixture_bds(ROW_BY_ROW)[..21].to_vec();
    bds[16..18].copy_from_slice(&((groups % 65536) as u16).to_be_bytes());
    bds[20] = (groups / 65536) as u8;
    bds
}

/// `row_by_row` stating one group per point, 4 Mi of them, in a 22-byte BDS.
/// Its 32 MiB of row lengths were laid out before the 4 Mi width octets were
/// found missing.
#[test]
fn row_by_row_groups_past_the_section_are_refused_before_their_row_lengths() {
    let mut bds = row_by_row_header(POINTS);
    bds.push(0); // one width octet of the 4 Mi stated
    assert_refused(
        restate(ROW_BY_ROW, bds),
        "too short for row_by_row groupWidths",
    );
}

/// `row_by_row` with its 2048 rows' widths (8 bits each) and first-order
/// values present and none of their residuals: the 32 MiB output was reserved
/// before the 4 MiB of residuals were found missing.
#[test]
fn row_by_row_residuals_missing_from_the_section_are_refused_before_the_output() {
    let rows = usize::from(SIDE);
    let mut bds = row_by_row_header(rows);
    let width_of_first = usize::from(bds[10]);
    bds.extend(std::iter::repeat_n(8, rows));
    bds.extend(std::iter::repeat_n(0, (rows * width_of_first).div_ceil(8)));
    assert_refused(
        restate(ROW_BY_ROW, bds),
        "row_by_row second-order values need 33554432 bits but the section holds 0",
    );
}

/// The general-extended header (no SPD) with `groups` groups, its width and
/// length fields `width_of_widths` and `width_of_lengths` bits wide,
/// first-order values of `width_of_first` bits, and `tables` (the widths and
/// lengths blocks, then the first-order values) after it; `N1` and `N2` point
/// where those blocks end.
fn extended(
    groups: usize,
    width_of_widths: u8,
    width_of_lengths: u8,
    width_of_first: u8,
    descriptors: &[u8],
    first_order: &[u8],
) -> Vec<u8> {
    let mut bds = fixture_bds(EXTENDED)[..25].to_vec();
    assert_eq!(bds[13] & 0x07, 0, "the fixture has no SPD");
    bds[10] = width_of_first;
    bds[16..18].copy_from_slice(&((groups % 65536) as u16).to_be_bytes());
    bds[20] = (groups / 65536) as u8;
    bds[21] = width_of_widths;
    bds[22] = width_of_lengths;
    bds.extend_from_slice(descriptors);
    let n1 = bds.len() + 1; // N1 and N2 count octets from 1
    bds.extend_from_slice(first_order);
    let n2 = bds.len() + 1;
    bds[11..13].copy_from_slice(&(n1 as u16).to_be_bytes());
    bds[14..16].copy_from_slice(&(n2 as u16).to_be_bytes());
    bds
}

/// General-extended with every table field zero bits wide: 4 Mi groups for
/// nothing in a 25-byte BDS, three tables of them (the largest 16 MiB) stored
/// before the lengths were found to sum to 0.
#[test]
fn extended_groups_stated_by_zero_width_tables_are_refused_before_the_tables() {
    let bds = extended(POINTS, 0, 0, 0, &[], &[]);
    assert_refused(
        restate(EXTENDED, bds),
        "make 0 values but 4194304 points are present",
    );
}

/// General-extended, one 8-bit group of every point, its width, length and
/// first-order value present and none of its residuals: the 32 MiB output
/// was reserved before the 4 MiB of residuals were found missing.
#[test]
fn extended_residuals_missing_from_the_section_are_refused_before_the_output() {
    let mut descriptors = vec![8]; // the group's width, at 8 bits
    descriptors.extend_from_slice(&(POINTS as u32).to_be_bytes()); // its length
    let bds = extended(1, 8, 32, 8, &descriptors, &[0]);
    assert_refused(
        restate(EXTENDED, bds),
        "second-order values need 33554432 bits but the section holds 0 from N2",
    );
}
