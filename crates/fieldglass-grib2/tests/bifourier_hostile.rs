//! A bi-Fourier message whose truncation disagrees with §5, or whose §7 is
//! too short for it, is refused before anything proportional to the
//! truncation is allocated (#849).
//!
//! The seed is the committed fuzz input
//! `fuzz/corpus/decode/constant_field_bifourier_ellipse_wide.grib2` (145
//! bytes; provenance in `fuzz/README.md`): an ellipse truncation N =
//! 16,783,359, M = 0, with §5 declaring 80 coefficients. Its limit array alone
//! is 134 MB, so the reader used to spend that and about 0.3 s before the
//! count refused it.
//!
//! A tracking global allocator records the largest single allocation, only
//! while this thread has armed it, so tests on other threads cannot add to it.
//! The file is read and parsed before arming.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass_grib2::Grib2Reader;
use fieldglass_grib2::drs::BiFourierPackingTemplate;
use fieldglass_grib2::gds::BiFourierTemplate;
use fieldglass_grib2::spectral::decode_bifourier;

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

#[test]
fn wide_ellipse_seed_is_refused_before_its_layout_is_allocated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fuzz/corpus/decode/constant_field_bifourier_ellipse_wide.grib2"
    );
    let bytes = std::fs::read(path).expect("committed fuzz seed");
    let reader = Grib2Reader::from_bytes(bytes).expect("parse");
    let bf = reader.messages[0].gds.bifourier().expect("§3 bi-Fourier");
    assert_eq!(
        (bf.bif_i, bf.bif_j, bf.truncation_type),
        (16_783_359, 0, 88)
    );

    let (largest, result) = largest_allocation_in(|| reader.decode_bifourier_message(0));
    let err = result.expect_err("§5 disagrees with the truncation");
    assert!(
        format!("{err:?}").contains("reconstructs 67133440 coefficients but §5 declares 80"),
        "{err:?}"
    );
    // The limit array would be 8·(N+1) = 134,266,880 bytes. Nothing near the
    // truncation's size may be allocated; the error message is the largest.
    assert!(largest < 4096, "largest allocation {largest} bytes");
}

#[test]
fn matching_count_with_an_empty_section_7_is_refused_before_its_layout() {
    // The seed with its axes swapped and §5 set to match: an ellipse with
    // N = 0 and M = 16,783,359 holds one pair per row, 4·(M+1) coefficients,
    // so the count agrees and only §7's length can refuse it. Its full limit
    // array would again be 8·(M+1) bytes.
    let gds = BiFourierTemplate {
        spectral_type: 2,
        bif_i: 0,
        bif_j: 16_783_359,
        truncation_type: 88,
    };
    let packing = BiFourierPackingTemplate {
        reference_value: 0.0,
        binary_scale_factor: 0,
        decimal_scale_factor: 0,
        bits_per_value: 12,
        sub_truncation_type: 77,
        packing_mode_for_axes: 0,
        laplacian_scaling_factor: 0,
        sub_i: 2,
        sub_j: 2,
        total_values_in_unpacked_subset: 0,
        unpacked_subset_precision: 1,
    };
    let (largest, result) =
        largest_allocation_in(|| decode_bifourier(&[], &packing, &gds, 67_133_440));
    let err = result.expect_err("§7 is empty");
    assert!(format!("{err:?}").contains("§7 holds only 0"), "{err:?}");
    assert!(largest < 4096, "largest allocation {largest} bytes");
}
