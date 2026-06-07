//! libFuzzer target for the NetCDF header parse path.
//!
//! `fieldglass-netcdf` parses attacker-controllable bytes. The classic
//! (CDF-1/2/5) path does offset- and length-driven header walking
//! (`classic::parse_header` — dim_list / gatt_list / var_list), the same bug
//! class fuzzing surfaced in GRIB1. This target drives `NetcdfReader::from_bytes`
//! against arbitrary input, asserting the parser never panics, over-reads, or
//! hangs. HDF5 input only reaches the lightweight superblock probe today (no
//! deep traversal), so the classic header parser is the substance of what this
//! exercises.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_netcdf::NetcdfReader;

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic. The
    // returned reader (when parsing succeeds) is intentionally discarded — the
    // header walk is the whole attack surface today.
    let _ = NetcdfReader::from_bytes(data.to_vec());
});
