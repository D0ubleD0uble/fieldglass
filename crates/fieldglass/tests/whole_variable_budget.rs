//! A slice of a variable too large to read whole is refused through
//! `Session`, not allocated (#847).
//!
//! `Session::decode_slice` still reads the whole variable and picks the plane
//! out of it, so it meets the reader's whole-variable byte budget. The seed is
//! a 13 KB HDF5 file whose fill-only dataset reads whole as about 2.9 GB. On a
//! `wasm32` host that read was worse than slow: its output alone is past
//! `isize::MAX`, so `Vec::with_capacity` panicked. The reader-level test is
//! `fieldglass-netcdf`'s `tests/whole_variable_budget.rs`.

use fieldglass::{DecodeOptions, Session};

const SEED: &[u8] =
    include_bytes!("../../fieldglass-netcdf/fuzz/corpus/parse/oom_large_fill_dataset.h5");

#[test]
fn a_slice_of_a_variable_past_the_budget_is_refused() {
    let session = Session::open(SEED.to_vec()).expect("the seed opens");
    let (index, var) = session
        .variables()
        .into_iter()
        .enumerate()
        .find(|(_, v)| v.dims.iter().map(|d| d.length).product::<u64>() > 1 << 22)
        .expect("the seed offers its large dataset");
    let rank = var.dims.len();
    assert_eq!(rank, 2, "{var:?}");

    let refused = session
        .decode_slice(
            u32::try_from(index).expect("small index"),
            0,
            1,
            &vec![0; rank],
            &DecodeOptions::default(),
        )
        .expect_err("a slice of a 2.9 GB variable is refused");
    let message = refused.to_string();
    assert!(
        message.contains("2936014080") && message.contains("2147483648"),
        "{message}"
    );
}
