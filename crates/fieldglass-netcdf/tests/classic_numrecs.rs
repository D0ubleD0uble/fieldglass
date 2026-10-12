//! A classic header's `numrecs` is a claim the file has to back before the
//! reader plans a range per record (#1000).
//!
//! `numrecs_huge_cdf1.nc` and `numrecs_huge_cdf5.nc` (248 and 364 bytes,
//! provenance in the fixtures' `NOTICE.md`) hold 2 records and state
//! 60,000,000. Their record variables, `time(time)` `double` and `v(time, x =
//! 1)` `byte`, fit the whole-variable budget and the field cap at that count,
//! so nothing refused them before the reader built one byte range per stated
//! record, 960 MB of ranges, and only then found the file too short. Now the
//! slabs are checked against the file first, so the read is refused holding
//! almost nothing, while records 0 and 1, which are there, still read.
//!
//! `numrecs_streaming_*.nc` are the same file with `numrecs` set to the
//! specification's STREAMING value, every bit set. That is an indeterminate
//! count, not a huge one, so the file opens and its record variables have no
//! records.
//!
//! A tracking global allocator records the peak of live bytes on this thread
//! while it is armed, so tests on other threads cannot add to it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;

use fieldglass_netcdf::NetcdfReader;

// Embedded rather than read at run time: the wasm32-wasip1 run preopens only
// the working directory and its parent.
const HUGE: [(&str, &[u8]); 2] = [
    (
        "numrecs_huge_cdf1.nc",
        include_bytes!("fixtures/numrecs_huge_cdf1.nc"),
    ),
    (
        "numrecs_huge_cdf5.nc",
        include_bytes!("fixtures/numrecs_huge_cdf5.nc"),
    ),
];
const STREAMING: [(&str, &[u8]); 2] = [
    (
        "numrecs_streaming_cdf1.nc",
        include_bytes!("fixtures/numrecs_streaming_cdf1.nc"),
    ),
    (
        "numrecs_streaming_cdf5.nc",
        include_bytes!("fixtures/numrecs_streaming_cdf5.nc"),
    ),
];

/// The record count the huge files state.
const STATED: u64 = 60_000_000;

/// Generous for a refused read: the error message, the variable's shape and
/// the few small vectors the layout needs. A range per stated record would be
/// 16 bytes times 60 M.
const SMALL: isize = 64 * 1024;

struct Tracking;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn note(delta: isize) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LIVE.try_with(|live| {
                live.set(live.get() + delta);
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
            });
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; tracking touches only
// const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size as isize - layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(-(layout.size() as isize));
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// The most bytes live at once while `f` runs on this thread, counted from
/// zero when it starts, and what it returned.
fn peak_while<R>(f: impl FnOnce() -> R) -> (isize, R) {
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (PEAK.with(Cell::get), result)
}

fn open(bytes: &[u8]) -> NetcdfReader {
    NetcdfReader::from_bytes(bytes.to_vec()).expect("the header parses")
}

fn index(reader: &NetcdfReader, name: &str) -> usize {
    let fieldglass_netcdf::NetcdfBacking::Classic(header) = &reader.backing else {
        panic!("a classic file");
    };
    header
        .variables
        .iter()
        .position(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} is declared"))
}

#[test]
fn a_whole_read_of_records_the_file_lacks_is_refused_before_its_ranges_are_built() {
    for (name, bytes) in HUGE {
        for var in ["time", "v"] {
            let (peak, result) = peak_while(|| {
                let reader = open(bytes);
                let i = index(&reader, var);
                assert_eq!(reader.variable_shape(i).unwrap()[0], STATED);
                reader.decode_variable_raw(i)
            });
            let err = result.expect_err("the file holds 2 of the 60 M records");
            assert!(
                err.to_string().contains("source size"),
                "{name} {var}: {err}"
            );
            assert!(
                peak < SMALL,
                "{name} {var}: the refused read held {peak} bytes at once"
            );
        }
    }
}

// A one-axis region is one range, not a mistyped `Vec` of its values.
#[allow(clippy::single_range_in_vec_init)]
#[test]
fn a_region_past_the_records_the_file_holds_is_refused_before_its_ranges_are_built() {
    for (name, bytes) in HUGE {
        let reader = open(bytes);
        let v = index(&reader, "v");
        let time = index(&reader, "time");
        let cases: [(usize, Vec<Range<u64>>); 4] = [
            // Every record: 60 M runs of one byte each.
            (v, vec![0..STATED, 0..1]),
            (time, vec![0..STATED]),
            // The first record the file does not hold.
            (v, vec![2..3, 0..1]),
            // Two that are there and one that is not.
            (time, vec![0..3]),
        ];
        for (i, region) in cases {
            let (peak, result) = peak_while(|| reader.decode_region_raw(i, &region));
            let err = result.expect_err("the region reaches past the file");
            assert!(
                err.to_string().contains("source size"),
                "{name} {region:?}: {err}"
            );
            assert!(
                peak < SMALL,
                "{name} {region:?}: the refused read held {peak} bytes at once"
            );
        }
    }
}

// A one-axis region is one range, not a mistyped `Vec` of its values.
#[allow(clippy::single_range_in_vec_init)]
#[test]
fn the_records_the_file_holds_still_read() {
    for (name, bytes) in HUGE {
        let reader = open(bytes);
        let v = index(&reader, "v");
        let time = index(&reader, "time");
        let f = index(&reader, "f");
        assert_eq!(
            reader.decode_region_raw(v, &[0..2, 0..1]).unwrap(),
            [Some(7.0), Some(-8.0)],
            "{name}"
        );
        assert_eq!(
            reader.decode_region_raw(time, &[1..2]).unwrap(),
            [Some(6.0)],
            "{name}"
        );
        // The fixed variable, stored ahead of the records, reads whole
        // whatever the record count claims.
        assert_eq!(
            reader.decode_variable_raw(f).unwrap(),
            [Some(0.5)],
            "{name}"
        );
    }
}

#[test]
fn a_streaming_file_opens_with_no_records() {
    for (name, bytes) in STREAMING {
        let reader = open(bytes);
        let fieldglass_netcdf::NetcdfBacking::Classic(header) = &reader.backing else {
            panic!("{name}: a classic file");
        };
        assert_eq!(header.numrecs, None, "{name}: STREAMING is not a count");
        let v = index(&reader, "v");
        let time = index(&reader, "time");
        let f = index(&reader, "f");
        assert_eq!(reader.variable_shape(v).unwrap(), [0, 1], "{name}");
        assert_eq!(reader.decode_variable_raw(v).unwrap(), [], "{name}");
        assert_eq!(reader.decode_variable_raw(time).unwrap(), [], "{name}");
        assert_eq!(
            reader.decode_variable_raw(f).unwrap(),
            [Some(0.5)],
            "{name}"
        );
    }
}
