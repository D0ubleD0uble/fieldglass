//! One plane of a variable too large to read whole draws, and costs about the
//! plane and the chunk under it (#939).
//!
//! `netcdf4_large_sparse.nc` (40 KB, provenance in the NetCDF fixtures'
//! `NOTICE.md`) declares `t2m(time = 120, lat = 721, lon = 1440)` float32,
//! 124,588,800 values: about 2.5 GB decoded whole, past the reader's
//! whole-variable budget. It stores two of its 120 one-plane chunks and leaves
//! the rest to the fill value. `Session::decode_slice` used to decode the whole
//! variable to cut one plane out of it; it now reads the plane, so it draws, and
//! the most it holds at once is a few planes' worth of bytes rather than
//! gigabytes.
//!
//! A tracking global allocator records the peak of live bytes allocated on this
//! thread while it is armed, so tests on other threads cannot add to it. The
//! session is opened before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass::{DecodeOptions, Session};

const LARGE_SPARSE: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc");

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

/// The most bytes live at once, counted from zero when `f` starts, while `f`
/// runs on this thread. What `f` returns is dropped before the count stops,
/// so the result is what the operation held, not what it handed back.
fn peak_while<R>(f: impl FnOnce() -> R) -> (isize, R) {
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (PEAK.with(Cell::get), result)
}

/// The plane: 721 × 1440 cells.
const CELLS: isize = 721 * 1440;

#[test]
fn a_plane_of_a_variable_too_large_to_read_whole_draws_at_the_cost_of_a_plane() {
    let session = Session::open(LARGE_SPARSE.to_vec()).expect("the file opens");
    let index = session
        .variables()
        .iter()
        .position(|v| v.name == "t2m")
        .expect("t2m is offered") as u32;
    let options = DecodeOptions::default();
    // Placement is memoised by the session and read from the 1-D coordinates;
    // take it first, so the peak below is the read and nothing the first call
    // alone pays.
    session
        .decode_slice(index, 1, 2, &[0, 0, 0], &options)
        .expect("an unstored plane draws");

    let (peak, field) = peak_while(|| session.decode_slice(index, 1, 2, &[7, 0, 0], &options));
    let field = field.expect("a stored plane draws");
    assert_eq!((field.ni, field.nj), (1440, 721));
    // Row 0 is latitude 90, rounded; row 360 the equator.
    let fieldglass::Values::F32(values) = &field.values else {
        panic!("a float32 variable decodes to single precision");
    };
    assert_eq!(values[0], 90.0);
    assert_eq!(values[360 * 1440 + 17], 0.0);
    assert_eq!(field.stats.max, Some(90.0));
    assert_eq!(field.stats.min, Some(-90.0));

    // What a read of the plane needs: the region's stored bytes (4 a cell),
    // its decoded values (16), the decompressed chunk (4), and the field built
    // from them. Under 48 bytes a cell is generous for all of it, and two
    // orders of magnitude under the whole variable, which is 2.5 GB.
    assert!(peak > 0);
    assert!(
        peak < 48 * CELLS,
        "a plane read held {peak} bytes at once, {} a cell",
        peak / CELLS
    );
}

#[test]
fn an_unstored_plane_reads_as_the_fill_value_masked() {
    let session = Session::open(LARGE_SPARSE.to_vec()).expect("the file opens");
    let index = session
        .variables()
        .iter()
        .position(|v| v.name == "t2m")
        .expect("t2m is offered") as u32;
    let field = session
        .decode_slice(index, 1, 2, &[50, 0, 0], &DecodeOptions::default())
        .expect("an unstored plane draws");
    assert_eq!(field.mask.len(), CELLS as usize);
    assert_eq!(field.stats.valid_count, 0, "every cell is the fill value");
}
