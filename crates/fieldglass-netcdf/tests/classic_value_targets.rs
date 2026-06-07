//! Pins the per-variable type + shape matrix that classic NetCDF value decode
//! (#108) builds on, for the bundled `netcdf_classic_dummy.nc` fixture.
//!
//! The committed `*.values.json` oracles next to the fixtures are the value
//! targets: once #108 reads variable data from each `begin` offset, the
//! decoded array must match the `netCDF4` (libnetcdf) statistics and samples
//! recorded there. This test guards the metadata that decode depends on —
//! every `nc_type` and every layout the fixture exercises — so the decode
//! work starts from a parser that already agrees with netCDF4 on the shape of
//! the problem. Expected values are lifted from the oracle (`ncdump -h`); no
//! synthesis. The ERSST value targets are covered by `ersst_real_world.rs`.

use fieldglass_netcdf::{ClassicHeader, NcType, NetcdfBacking, NetcdfReader};

const CLASSIC: &[u8] = include_bytes!("fixtures/netcdf_classic_dummy.nc");

fn header() -> ClassicHeader {
    match NetcdfReader::from_bytes(CLASSIC.to_vec())
        .expect("classic fixture parses")
        .backing
    {
        NetcdfBacking::Classic(h) => h,
        other => panic!("expected Classic backing, got {:?}", other.label()),
    }
}

/// Resolve a variable's shape from its dimension references, mirroring how a
/// value decoder sizes the array (record dims contribute `numrecs`).
fn shape_of(h: &ClassicHeader, name: &str) -> (NcType, Vec<u64>) {
    let v = h
        .variables
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("variable {name} present"));
    let shape = v
        .dim_ids
        .iter()
        .map(|&d| h.dimensions[d as usize].length)
        .collect();
    (v.nc_type, shape)
}

#[test]
fn dummy_variable_type_and_shape_matrix() {
    let h = header();
    // numrecs = 0: the unlimited `time` dimension contributes 0 to record
    // variables, so they decode to empty arrays.
    assert_eq!(h.numrecs, Some(0));

    // One representative variable per nc_type × layout combination the
    // fixture exercises (values from the netCDF4 oracle / ncdump -h).
    let cases: &[(&str, NcType, &[u64])] = &[
        ("feature_type_instance", NcType::Char, &[64]), // char, 1-D fixed
        ("crs", NcType::Int, &[]),                      // int32, scalar
        ("platform", NcType::Int, &[]),                 // int32, scalar
        ("latitude", NcType::Double, &[]),              // float64, scalar
        ("sensor_depth", NcType::Float, &[]),           // float32, scalar
        ("z", NcType::Double, &[8]),                    // float64, 1-D fixed
        ("bindist", NcType::Float, &[8]),               // float32, 1-D fixed
        ("Hdg_1215", NcType::Double, &[0]),             // float64, 1-D record (0 recs)
        ("AGC_1202", NcType::Double, &[0, 8]),          // float64, 2-D record
    ];
    for (name, want_type, want_shape) in cases {
        let (ty, shape) = shape_of(&h, name);
        assert_eq!(ty, *want_type, "{name} nc_type");
        assert_eq!(shape, *want_shape, "{name} shape");
    }
}

/// `begin` / `vsize` are the on-disk coordinates a value decoder reads from;
/// pin that every variable's data region is declared inside the file. (The
/// exact decoded values are checked against the oracle once #108 lands.)
#[test]
fn dummy_variable_data_regions_are_in_bounds() {
    let h = header();
    for v in &h.variables {
        assert!(v.begin > 0, "{}: begin must point past the header", v.name);
        let end = v.begin + v.vsize;
        assert!(
            end as usize <= CLASSIC.len(),
            "{}: data region [{}, {}) exceeds file size {}",
            v.name,
            v.begin,
            end,
            CLASSIC.len()
        );
    }
}
