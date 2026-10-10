//! A slice too large to draw is refused through `Session`, not allocated, and
//! the largest slice allowed draws (#847, #942).
//!
//! `Session::decode_slice` reads one plane as a region (#939), and a region is
//! held to `MAX_FIELD_POINTS`, the most one field holds, before anything is
//! decoded. A 2-D variable's plane is the whole variable, so that cap, not the
//! whole-variable budget, is what a slice of one meets. Both refusals are a
//! valid file asked for too much at once, and reach a host as `unsupported`.
//!
//! Three small fill-only files, embedded so the `wasm32-wasip1` run reads
//! them, which is where a read past the cap used to abort the module rather
//! than fail (#942):
//!
//! - the 13 KB fuzz seed: 146,800,704 four-byte values, 2.9 GB whole;
//! - `fill_only_past_field_cap.nc`: 10000 × 10000, inside the whole-variable
//!   budget and past the field cap;
//! - `fill_only_at_field_cap.nc`: 8192 × 8192, exactly the field cap, which
//!   must draw.
//!
//! The reader-level test is `fieldglass-netcdf`'s `tests/whole_variable_budget.rs`.

use fieldglass::{DecodeOptions, Session};

const SEED: &[u8] =
    include_bytes!("../../fieldglass-netcdf/fuzz/corpus/parse/oom_large_fill_dataset.h5");
const PAST_FIELD_CAP: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/fill_only_past_field_cap.nc");
const HUGE_FIXED_ARRAY: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_fixed_array_huge_count.h5");
const AT_FIELD_CAP: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/fill_only_at_field_cap.nc");

/// The session's largest variable, by index, and its rank.
fn largest(session: &Session) -> (u32, usize) {
    let (index, var) = session
        .variables()
        .into_iter()
        .enumerate()
        .max_by_key(|(_, v)| v.dims.iter().map(|d| d.length).product::<u64>())
        .expect("a variable");
    (u32::try_from(index).expect("small index"), var.dims.len())
}

/// The refusal a slice of `bytes`' largest variable meets.
fn refusal(bytes: &[u8]) -> fieldglass::Error {
    let session = Session::open(bytes.to_vec()).expect("the file opens");
    let (index, rank) = largest(&session);
    assert_eq!(rank, 2);
    session
        .decode_slice(index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect_err("a slice past the field cap is refused")
}

#[test]
fn a_slice_of_a_2_9_gb_variable_is_refused() {
    let refused = refusal(SEED);
    // A valid file asked for too much at once, not a corrupt one: `decode`
    // would tell a host the file is broken.
    assert_eq!(refused.code(), "unsupported", "{refused:?}");
    let message = refused.to_string();
    assert!(
        message.contains("asks for 146800704 values at once")
            && message.contains("more than the 67108864 one field may hold")
            && message.contains("The file itself is fine"),
        "{message}"
    );
}

/// Inside the whole-variable budget, so the reader would read it whole, and
/// past the field cap, which is what the slice meets (#942).
#[test]
fn a_slice_past_the_field_cap_is_refused_inside_the_whole_variable_budget() {
    let refused = refusal(PAST_FIELD_CAP);
    assert_eq!(refused.code(), "unsupported", "{refused:?}");
    assert!(
        refused
            .to_string()
            .contains("asks for 100000000 values at once"),
        "{refused}"
    );
}

/// The worst case the reader accepts: a plane of exactly `MAX_FIELD_POINTS`.
/// It draws, natively and on the 32-bit browser target.
#[test]
fn a_slice_at_the_field_cap_draws() {
    let session = Session::open(AT_FIELD_CAP.to_vec()).expect("the file opens");
    let (index, _) = largest(&session);
    let field = session
        .decode_slice(index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("a plane at the field cap draws");
    assert_eq!((field.ni, field.nj), (8192, 8192));
    // Every value is the `_FillValue`, so every cell is masked.
    assert_eq!(field.mask.len(), 8192 * 8192);
    assert_eq!(field.stats.valid_count, 0);
}

/// A Zarr slice past the field cap reaches a host as the same `unsupported`
/// a NetCDF one does: both are core's one refusal (#942).
#[cfg(feature = "zarr")]
#[test]
fn a_zarr_slice_past_the_field_cap_is_unsupported_too() {
    use fieldglass::MemoryObjects;

    let objects = MemoryObjects::from_iter([
        (".zgroup", br#"{"zarr_format": 2}"#.to_vec()),
        (
            "t/.zarray",
            br#"{"zarr_format": 2, "shape": [10000, 10000], "chunks": [1000, 1000],
                "dtype": "<f4", "fill_value": 1.5, "compressor": null, "filters": null,
                "order": "C"}"#
                .to_vec(),
        ),
    ]);
    let session = Session::open_store(objects).expect("the store opens");
    let (index, rank) = largest(&session);
    assert_eq!(rank, 2);
    let refused = session
        .decode_slice(index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect_err("a slice past the field cap is refused");
    assert_eq!(refused.code(), "unsupported", "{refused:?}");
    assert!(
        refused
            .to_string()
            .contains("asks for 100000000 values at once"),
        "{refused}"
    );
}

/// A line through a dataset whose Fixed Array index counts 2^34 chunks is
/// inside the field cap, so it reaches the index; the index is refused rather
/// than sized from that count, which used to abort the process, and on
/// `wasm32` trap (#939 review).
#[test]
fn a_line_through_a_hostile_fixed_array_is_refused_not_allocated() {
    let session = Session::open(HUGE_FIXED_ARRAY.to_vec()).expect("the file opens");
    let (index, rank) = largest(&session);
    assert_eq!(rank, 2);
    let refused = session
        .decode_line(index, 1, &[3, 0], &DecodeOptions::default())
        .expect_err("the index is refused");
    assert!(refused_hostile_count(&refused.to_string()), "{refused}");
}

/// The refusal a 2^34-entry Fixed Array meets: the chunk-grid cap, or on a
/// 32-bit target, earlier, the entry count itself, which does not fit its
/// address space. Either is an error rather than an allocation.
fn refused_hostile_count(message: &str) -> bool {
    message.contains("chunk grid has 17179869184 chunks")
        || (cfg!(target_pointer_width = "32")
            && message.contains("17179869184 does not fit this target's address space"))
}
