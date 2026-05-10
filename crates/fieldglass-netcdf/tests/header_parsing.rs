//! Integration tests for the NetCDF header parser, exercising both the
//! pure-Rust classic path and the HDF5 superblock probe against the Unidata
//! fixtures shipped with the crate.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};

const CLASSIC: &[u8] = include_bytes!("fixtures/netcdf_classic_dummy.nc");
const HDF5: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");

#[test]
fn classic_fixture_parses_header() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).expect("classic header parses");
    match reader.backing {
        NetcdfBacking::Classic(h) => {
            // The Unidata sample uses the unlimited time dimension plus a
            // fixed maxStrlen64. It has many variables and global attrs.
            assert!(
                !h.dimensions.is_empty(),
                "expected at least one dimension, got {:?}",
                h.dimensions
            );
            assert!(
                h.dimensions.iter().any(|d| d.is_record),
                "expected an unlimited (record) dimension among {:?}",
                h.dimensions.iter().map(|d| &d.name).collect::<Vec<_>>()
            );
            assert!(
                !h.global_attributes.is_empty(),
                "expected at least one global attribute"
            );
            assert!(!h.variables.is_empty(), "expected at least one variable");
            // Spot-check: the fixture's "project_summary" global attribute
            // exists and has a Char-typed value.
            let proj = h
                .global_attributes
                .iter()
                .find(|a| a.name == "project_summary")
                .expect("project_summary attribute present");
            assert!(
                !proj.value.is_empty(),
                "project_summary value should be a non-empty string"
            );
        }
        other => panic!("expected classic backing, got {:?}", other.label()),
    }
}

#[test]
fn classic_fixture_lists_named_dimensions() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).unwrap();
    if let NetcdfBacking::Classic(h) = reader.backing {
        let names: Vec<&str> = h.dimensions.iter().map(|d| d.name.as_str()).collect();
        // Both names appear in the raw fixture; assert via the parser.
        assert!(
            names.contains(&"time"),
            "expected 'time' dimension in {names:?}"
        );
        assert!(
            names.contains(&"maxStrlen64"),
            "expected 'maxStrlen64' dimension in {names:?}"
        );
    } else {
        panic!("expected classic backing");
    }
}

#[test]
fn classic_fixture_variable_dim_refs_resolve() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).unwrap();
    if let NetcdfBacking::Classic(h) = reader.backing {
        let num_dims = h.dimensions.len();
        for v in &h.variables {
            for &did in &v.dim_ids {
                assert!(
                    (did as usize) < num_dims,
                    "variable {} references out-of-range dim id {did}",
                    v.name
                );
            }
        }
    } else {
        panic!("expected classic backing");
    }
}

#[test]
fn hdf5_fixture_probes_to_a_versioned_superblock() {
    let reader = NetcdfReader::from_bytes(HDF5.to_vec()).expect("HDF5 file is recognized");
    match reader.backing {
        NetcdfBacking::Hdf5(probe) => {
            // Versions 0..=3 are the only documented superblock layouts.
            assert!(
                probe.superblock_version <= 3,
                "got superblock version {}",
                probe.superblock_version
            );
            // Practical files use 8-byte offsets and lengths.
            assert!(
                probe.offset_size == 4 || probe.offset_size == 8,
                "got offset_size {}",
                probe.offset_size
            );
            assert!(
                probe.length_size == 4 || probe.length_size == 8,
                "got length_size {}",
                probe.length_size
            );
        }
        _ => panic!("expected HDF5 backing for the NetCDF-4 fixture"),
    }
}

#[test]
fn hdf5_backing_reports_partial_parse() {
    let reader = NetcdfReader::from_bytes(HDF5.to_vec()).unwrap();
    assert!(
        !reader.backing.is_fully_parsed(),
        "HDF5 fixture should be flagged as not-fully-parsed for the provider's notice"
    );
}

#[test]
fn classic_backing_reports_full_parse() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).unwrap();
    assert!(reader.backing.is_fully_parsed());
}

#[test]
fn truncated_header_yields_parse_error() {
    let truncated = &CLASSIC[..16];
    let err = NetcdfReader::from_bytes(truncated.to_vec()).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("Parse") || msg.contains("InvalidMagic"),
        "expected a parse / magic error, got {msg}"
    );
}

#[test]
fn bogus_magic_errors_clearly() {
    let err = NetcdfReader::from_bytes(b"NOTACDF\x00".to_vec()).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("invalid magic") || msg.contains("magic"),
        "expected a magic-bytes error, got {msg}"
    );
}
