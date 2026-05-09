//! Detection tests for the NetCDF fixtures. The NetCDF reader itself is still
//! a stub (Phase 5 of the roadmap); this file only verifies that
//! `fieldglass-core`'s magic-byte detector recognises both backing formats —
//! classic NetCDF (`CDF\x01`) and NetCDF-4 / HDF5 (`\x89HDF\r\n\x1a\n`).
//!
//! Fixtures sourced from the public Unidata `netcdf4-python` test corpus
//! (`https://github.com/Unidata/netcdf4-python/tree/master/test`).

use fieldglass_core::{Format, detect_from_bytes};

const CLASSIC: &[u8] = include_bytes!("fixtures/netcdf_classic_dummy.nc");
const HDF5: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");

#[test]
fn classic_starts_with_cdf_magic() {
    assert_eq!(&CLASSIC[0..3], b"CDF");
    assert_eq!(
        CLASSIC[3], 1,
        "expected NetCDF classic format (version byte 1)"
    );
}

#[test]
fn hdf5_starts_with_hdf_magic() {
    assert_eq!(&HDF5[0..8], b"\x89HDF\r\n\x1a\n");
}

#[test]
fn classic_detects_as_netcdf() {
    assert!(matches!(detect_from_bytes(CLASSIC), Format::NetCdf));
}

#[test]
fn hdf5_detects_as_netcdf() {
    assert!(matches!(detect_from_bytes(HDF5), Format::NetCdf));
}
