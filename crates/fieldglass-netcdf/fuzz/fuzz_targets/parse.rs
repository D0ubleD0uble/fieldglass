//! libFuzzer target for the NetCDF parse path.
//!
//! `fieldglass-netcdf` parses attacker-controllable bytes. The classic
//! (CDF-1/2/5) path does offset- and length-driven header walking
//! (`classic::parse_header` — dim_list / gatt_list / var_list), the same bug
//! class fuzzing surfaced in GRIB1. The NetCDF-4 / HDF5 path adds a second deep
//! surface: from the superblock probe, the on-demand walk reads object headers,
//! group and link tables, dense-attribute fractal heaps + B-tree v2 indexes, and
//! the filter pipeline. This target drives both — `from_bytes` for the eager
//! parse, then `hdf5_metadata` for the deep HDF5 walk — asserting the parser
//! never panics, over-reads, or hangs.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_netcdf::NetcdfReader;

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    let Ok(reader) = NetcdfReader::from_bytes(data.to_vec()) else {
        return;
    };
    // For an HDF5 backing `from_bytes` reads only the superblock probe; the deep
    // object-model walk (the bounded, fail-safe traversal hardened under #33)
    // runs on demand, so drive it too. Errors are expected on crafted input; the
    // contract is no panic / over-read / hang. (Returns a clean error for the
    // classic backing.)
    let _ = reader.hdf5_metadata();
});
