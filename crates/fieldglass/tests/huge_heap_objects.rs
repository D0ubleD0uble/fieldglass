//! A NetCDF-4 file whose dense attributes hold a huge fractal-heap object
//! opens through `Session` (#899). It used to fail with "only managed
//! fractal-heap objects are supported" because its root group has ten global
//! attributes and a 5,600-byte `history`. The reader-level tests are
//! `fieldglass-netcdf`'s `tests/hdf5_huge_objects.rs`.

use fieldglass::Session;

const FIXTURE: &str = "../fieldglass-netcdf/tests/fixtures/netcdf4_huge_attributes.nc";

#[test]
fn a_file_with_a_huge_dense_attribute_opens() {
    let bytes = std::fs::read(FIXTURE).expect("fixture");
    let session = Session::open(bytes).expect("the file opens");
    // `t(x)` is one-dimensional, so it is no renderable variable; its
    // dimension is what shows the metadata was built.
    let dims: Vec<String> = session.dimensions().into_iter().map(|d| d.name).collect();
    assert_eq!(dims, ["x"]);
}
