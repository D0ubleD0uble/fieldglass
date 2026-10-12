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

/// The leading read is checked too: a classic or HDF5 file whose first bytes
/// come back short is a short read, not "neither".
#[test]
fn a_short_leading_read_is_a_short_read() {
    let classic = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc").to_vec();
    let hdf5 = include_bytes!("fixtures/oisst_avhrr_v2.nc").to_vec();
    for (name, bytes) in [("classic", classic), ("hdf5", hdf5)] {
        let err = NetcdfReader::from_source(Short::new(bytes, 0, 60))
            .map(|_| ())
            .expect_err("a truncated leading read is not a verdict");
        assert!(
            matches!(err, FieldglassError::ShortRead { .. }),
            "{name}: {err}"
        );
    }
}

#[test]
fn zeros_served_whole_are_still_not_netcdf() {
    let err = NetcdfReader::from_source(vec![0u8; 4096])
        .map(|_| ())
        .expect_err("zeros are no container");
    assert!(matches!(err, FieldglassError::InvalidMagic { .. }), "{err}");
}
