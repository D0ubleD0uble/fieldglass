//! A file whose listing holds more than the reader keeps is refused through
//! `Session` as `unsupported`, saying the file is fine (#921).
//!
//! `hdf5_deep_group_chain_past_name_cap.h5` (117 KB, provenance in the NetCDF
//! fixtures' `NOTICE.md`) is a valid file: 100 nested groups named with 360
//! characters, and a dataset with 2,001 names at the bottom. Each name carries
//! the 36 KB path above it, 72 MB in all, past the 64 MiB of path-qualified
//! names one listing holds. h5py visits all of it. Embedded so the
//! `wasm32-wasip1` run reads it. The reader-level test is `fieldglass-netcdf`'s
//! `tests/hdf5_shared_blocks.rs`.

use fieldglass::Session;

const PAST_NAME_CAP: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_deep_group_chain_past_name_cap.h5");

#[test]
fn a_listing_past_the_name_cap_is_unsupported_not_a_decode_error() {
    let refused = Session::open(PAST_NAME_CAP.to_vec()).expect_err("past the name cap");
    assert_eq!(refused.code(), "unsupported", "{refused:?}");
    let message = refused.to_string();
    assert!(
        message.contains("bytes of path-qualified names")
            && message.contains("The file itself is fine"),
        "{message}"
    );
}
