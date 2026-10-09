//! An earliest-format HDF5 file whose root holds soft links opens through
//! `Session` (#914): it used to fail on the soft link's undefined header
//! address. The reader-level test is `fieldglass-netcdf`'s
//! `tests/hdf5_soft_links.rs`.
//!
//! The root's datasets `a`, `m` and `z` have lengths 3, 4 and 5, so each
//! brings its own dimension; `m` and `z` sort after a soft link, and a reader
//! that stopped there would offer only `a`'s (#919).

use fieldglass::Session;

#[test]
fn a_file_with_soft_links_opens() {
    let bytes = std::fs::read("../fieldglass-netcdf/tests/fixtures/hdf5_soft_links_earliest.h5")
        .expect("fixture");
    let session = Session::open(bytes).expect("the file opens");
    let mut lengths: Vec<u64> = session.dimensions().into_iter().map(|d| d.length).collect();
    lengths.sort_unstable();
    assert_eq!(lengths, [3, 4, 5], "one dimension each for a, m and z");
}
