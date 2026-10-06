//! An earliest-format HDF5 file whose root holds soft links opens through
//! `Session` (#914): it used to fail on the soft link's undefined header
//! address. The reader-level test is `fieldglass-netcdf`'s
//! `tests/hdf5_soft_links.rs`.

use fieldglass::Session;

#[test]
fn a_file_with_soft_links_opens() {
    let bytes = std::fs::read("../fieldglass-netcdf/tests/fixtures/hdf5_soft_links_earliest.h5")
        .expect("fixture");
    let session = Session::open(bytes).expect("the file opens");
    let dims: Vec<String> = session.dimensions().into_iter().map(|d| d.name).collect();
    assert_eq!(dims.len(), 1, "a's one dimension: {dims:?}");
}
