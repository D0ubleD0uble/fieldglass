//! A whole-variable read is bounded in bytes, not elements (#847).
//!
//! `fuzz/corpus/parse/oom_large_fill_dataset.h5` is 13 KB. Its second dataset
//! declares a chunked 9,175,044 × 16 shape of four-byte elements and stores no
//! chunks, so it reads whole as the fill value: 146,800,704 elements, inside
//! the old 200 M element cap, and about 2.9 GB held at once (16 bytes of
//! output and 4 stored per element). Every host reads NetCDF through
//! `NetcdfReader::decode_variable_raw`, so the refusal here is the one the Node
//! host, the umbrella `Session` and the browser all get.

use fieldglass_core::array::ArrayError;
use fieldglass_core::{FieldglassError, MAX_VARIABLE_BYTES};
use fieldglass_netcdf::NetcdfReader;

const SEED: &[u8] = include_bytes!("../fuzz/corpus/parse/oom_large_fill_dataset.h5");

/// The seed's one large dataset, by decode index, and its declared shape.
fn large_dataset(reader: &NetcdfReader) -> (usize, Vec<u64>) {
    (0..)
        .map_while(|index| {
            reader
                .variable_shape(index)
                .ok()
                .map(|shape| (index, shape))
        })
        .find(|(_, shape)| shape.iter().product::<u64>() > 1 << 22)
        .expect("the seed declares one large dataset")
}

#[test]
fn the_13_kb_fill_only_dataset_is_refused_before_it_is_allocated() {
    let reader = NetcdfReader::from_bytes(SEED.to_vec()).expect("the seed opens");
    let (index, shape) = large_dataset(&reader);
    assert_eq!(
        shape,
        vec![9_175_044, 16],
        "the seed's large dataset changed"
    );

    let refused = reader
        .decode_variable_raw(index)
        .expect_err("a 2.9 GB read is refused");
    assert!(
        matches!(
            &refused,
            FieldglassError::Array(ArrayError::VariableTooLarge {
                elements: 146_800_704,
                element_bytes: 4,
                bytes: 2_936_014_080,
                limit,
            }) if *limit == MAX_VARIABLE_BYTES
        ),
        "{refused}"
    );
    // What a host shows: the bytes it would have needed and the budget.
    let message = refused.to_string();
    assert!(
        message.contains("2936014080") && message.contains(&MAX_VARIABLE_BYTES.to_string()),
        "{message}"
    );

    // The physical-units entry point reads through the same call.
    let physical = reader
        .decode_variable_physical(index)
        .expect_err("the physical read is the same read");
    assert!(
        matches!(
            physical,
            FieldglassError::Array(ArrayError::VariableTooLarge { .. })
        ),
        "{physical}"
    );
}
