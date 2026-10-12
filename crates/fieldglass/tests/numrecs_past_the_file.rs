//! A NetCDF classic file that states more records than it holds is refused
//! through `Session` before the reader plans a range per stated record
//! (#1000).
//!
//! `numrecs_huge_cdf1.nc` and `numrecs_huge_cdf5.nc` (provenance in the
//! NetCDF fixtures' `NOTICE.md`) are a few hundred bytes holding 2 records,
//! and state 60,000,000. `v(time, x = 1)` is a `byte` variable, so a slice
//! of it over `time`, a line along `time`, and the `time` coordinate that
//! labels that axis are each within the 64 Mi-value field cap, and each used
//! to build 60 M byte ranges, about 1.5 GB with their record offsets, before
//! the read found the file too short. Each is now refused, or for the axis
//! labels left without coordinates, holding almost nothing. The two records
//! that are there still draw.
//!
//! `numrecs_streaming_*.nc` state STREAMING, an indeterminate count, and are
//! read as the whole records their size holds (#1008).
//!
//! A tracking global allocator records the peak of live bytes on this thread
//! while it is armed, so tests on other threads cannot add to it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass::{DecodeOptions, Dtype, Session};

// Embedded rather than read at run time: the wasm32-wasip1 run preopens only
// the working directory and its parent.
const HUGE: [(&str, &[u8]); 2] = [
    (
        "numrecs_huge_cdf1.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_huge_cdf1.nc"),
    ),
    (
        "numrecs_huge_cdf5.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_huge_cdf5.nc"),
    ),
];
const STREAMING: [(&str, &[u8]); 2] = [
    (
        "numrecs_streaming_cdf1.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_streaming_cdf1.nc"),
    ),
    (
        "numrecs_streaming_cdf5.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_streaming_cdf5.nc"),
    ),
];

/// The streaming files cut short part way through their last record.
const PARTIAL: [(&str, &[u8]); 2] = [
    (
        "numrecs_streaming_partial_cdf1.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_streaming_partial_cdf1.nc"),
    ),
    (
        "numrecs_streaming_partial_cdf5.nc",
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/numrecs_streaming_partial_cdf5.nc"),
    ),
];

/// Generous for a refused read: messages, shapes, the variable listing. A
/// range per stated record would be 16 bytes times 60 M.
const SMALL: isize = 256 * 1024;

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

fn opts() -> DecodeOptions {
    DecodeOptions::new(Dtype::Auto)
}

/// `v`'s position in the session's variables, and its axes' lengths.
fn v(session: &Session) -> (u32, Vec<u64>) {
    let vars = session.variables();
    let index = vars
        .iter()
        .position(|var| var.name == "v")
        .expect("v is offered");
    let lengths = vars[index].dims.iter().map(|d| d.length).collect();
    (index as u32, lengths)
}

/// A read refused because its records lie past the file's end, not for some
/// other reason: the reader's decode error, naming the source's size.
fn refused_past_the_file(name: &str, what: &str, err: Option<fieldglass::Error>) {
    let err = err.unwrap_or_else(|| panic!("{name}: {what} is refused"));
    assert_eq!(err.code(), "decode", "{name}: {what}: {err}");
    assert!(
        err.message().contains("source size"),
        "{name}: {what}: {err}"
    );
}

#[test]
fn reads_of_records_the_file_lacks_are_refused_before_their_ranges_are_built() {
    for (name, bytes) in HUGE {
        let (peak, session) = peak_while(|| Session::open(bytes.to_vec()));
        let session = session.unwrap_or_else(|e| panic!("{name}: the header parses: {e}"));
        assert!(peak < SMALL, "{name}: opening held {peak} bytes at once");
        let (index, lengths) = v(&session);
        assert_eq!(lengths, [60_000_000, 1], "{name}");

        let (peak, slice) = peak_while(|| session.decode_slice(index, 0, 1, &[0, 0], &opts()));
        refused_past_the_file(name, "a slice over every stated record", slice.err());
        assert!(peak < SMALL, "{name}: the refused slice held {peak} bytes");

        let (peak, line) = peak_while(|| session.decode_line(index, 0, &[0, 0], &opts()));
        refused_past_the_file(name, "a line along every stated record", line.err());
        assert!(peak < SMALL, "{name}: the refused line held {peak} bytes");

        // A coordinate that cannot be read leaves the axis unlabelled rather
        // than failing it, so this one succeeds, without its coordinates.
        let (peak, axis) = peak_while(|| session.axis_values(index, 0));
        let axis = axis.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            (axis.length, axis.coordinates),
            (60_000_000, None),
            "{name}"
        );
        assert!(peak < SMALL, "{name}: the axis labels held {peak} bytes");
    }
}

#[test]
fn the_records_the_file_holds_still_draw() {
    for (name, bytes) in HUGE {
        let session = Session::open(bytes.to_vec()).expect("the header parses");
        let (index, _) = v(&session);
        for (record, want) in [(0, 7.0), (1, -8.0)] {
            let line = session
                .decode_line(index, 1, &[record, 0], &opts())
                .unwrap_or_else(|e| panic!("{name} record {record}: {e}"));
            assert_eq!(line.mask, [1], "{name} record {record}");
            assert_eq!(line.stats.min, Some(want), "{name} record {record}");
        }
        assert!(
            session.decode_line(index, 1, &[2, 0], &opts()).is_err(),
            "{name}: record 2 is stated and not there"
        );
    }
}

/// `v`'s records through `Session`, for a streaming file holding `records`
/// whole records of the base file's 2 (#1008).
fn assert_draws_records(name: &str, bytes: &[u8], records: u64) {
    let session = Session::open(bytes.to_vec())
        .unwrap_or_else(|e| panic!("{name}: a streaming file opens: {e}"));
    let (index, lengths) = v(&session);
    assert_eq!(lengths, [records, 1], "{name}: counted from the size");
    for (record, want) in [(0, 7.0), (1, -8.0)].into_iter().take(records as usize) {
        let line = session
            .decode_line(index, 1, &[record, 0], &opts())
            .unwrap_or_else(|e| panic!("{name} record {record}: {e}"));
        assert_eq!(line.mask, [1], "{name} record {record}");
        assert_eq!(line.stats.min, Some(want), "{name} record {record}");
    }
    let line = session
        .decode_line(index, 0, &[0, 0], &opts())
        .unwrap_or_else(|e| panic!("{name}: a line along time: {e}"));
    assert_eq!(line.mask.len() as u64, records, "{name}");
    let axis = session.axis_values(index, 0).expect("the axis exists");
    assert_eq!(axis.length, records, "{name}");
    assert_eq!(
        axis.coordinates,
        Some([0.0, 6.0][..records as usize].to_vec()),
        "{name}: the time coordinate labels every counted record"
    );
}

#[test]
fn a_streaming_file_draws_the_records_it_holds() {
    for (name, bytes) in STREAMING {
        assert_draws_records(name, bytes, 2);
    }
}

#[test]
fn a_streaming_file_cut_short_draws_its_whole_records_only() {
    for (name, bytes) in PARTIAL {
        assert_draws_records(name, bytes, 1);
    }
}
