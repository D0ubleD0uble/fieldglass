//! A read that fails while looking for the HDF5 signature is that read's error,
//! not "this is not NetCDF" (#998).

use fieldglass_core::FieldglassError;
use fieldglass_core::testing::Short;
use fieldglass_netcdf::NetcdfReader;

#[test]
fn a_short_read_at_a_userblock_offset_is_a_short_read() {
    let err = NetcdfReader::from_source(Short::new(vec![0u8; 4096], 512, 1))
        .map(|_| ())
        .expect_err("a truncated signature read is not a verdict");
    assert!(matches!(err, FieldglassError::ShortRead { .. }), "{err}");
}

#[test]
fn zeros_served_whole_are_still_not_netcdf() {
    let err = NetcdfReader::from_source(vec![0u8; 4096])
        .map(|_| ())
        .expect_err("zeros are no container");
    assert!(matches!(err, FieldglassError::InvalidMagic { .. }), "{err}");
}
