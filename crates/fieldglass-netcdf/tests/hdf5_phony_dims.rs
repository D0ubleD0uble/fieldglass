//! Anonymous ("phony") dimensions for scale-less HDF5 files (issue #533).
//!
//! A `.h5` written straight through HDF5 rather than by the netCDF-4 API has no
//! dimension scales, so nothing declares an axis length. Such a file used to
//! open and list its variables with every axis reporting length 0 — the names
//! were invented but never registered as dimensions, and an axis whose name is
//! absent from the dimension table resolves to 0. Nothing could be drawn.
//!
//! The axes are now sized from each dataset's own dataspace, following the rule
//! netCDF-C uses so that `ncdump -h` and Fieldglass name the same axis
//! `phony_dim_0`. `hdf5_phony_dims.h5` is built by
//! `tools/build_hdf5_fixtures.py` with shapes chosen to rule out every simpler
//! rule; the expectations below were read back from netCDF-C through
//! netCDF4-python.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};

const PHONY: &[u8] = include_bytes!("fixtures/hdf5_phony_dims.h5");
const FLETCHER32: &[u8] = include_bytes!("fixtures/hdf5_fletcher32.h5");
const ZSTD: &[u8] = include_bytes!("fixtures/hdf5_zstd.h5");
const EXTENSIBLE: &[u8] = include_bytes!("fixtures/hdf5_ea_chunk_index.h5");

fn metadata(bytes: &[u8]) -> fieldglass_netcdf::Hdf5Metadata {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    assert!(
        matches!(&reader.backing, NetcdfBacking::Hdf5(_)),
        "fixture must use the HDF5 backing"
    );
    reader.hdf5_metadata().expect("metadata resolves")
}

/// The dimension table and every variable's axes, against netCDF-C's own answer.
#[test]
fn anonymous_dimensions_match_netcdf_c() {
    let meta = metadata(PHONY);

    assert_eq!(
        meta.dimensions
            .iter()
            .map(|d| (d.name.as_str(), d.length))
            .collect::<Vec<_>>(),
        [
            ("phony_dim_0", 8),
            ("phony_dim_1", 8),
            ("phony_dim_2", 4),
            ("phony_dim_3", 6),
            ("phony_dim_4", 7),
        ],
        "dimension table should match `ncdump -h`"
    );

    let axes = |name: &str| -> Vec<String> {
        meta.variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the variable list"))
            .dimensions
            .clone()
    };

    // Two axes of one length still take two dimensions.
    assert_eq!(axes("a_8x8"), ["phony_dim_0", "phony_dim_1"]);
    // An identical shape reuses them rather than allocating more.
    assert_eq!(axes("b_8x8"), ["phony_dim_0", "phony_dim_1"]);
    assert_eq!(axes("c_4x6"), ["phony_dim_2", "phony_dim_3"]);
    // Transposed: the same pair the other way round, which is what rules out
    // matching whole shapes rather than individual extents.
    assert_eq!(axes("d_6x4"), ["phony_dim_3", "phony_dim_2"]);
    assert_eq!(axes("e_1d7"), ["phony_dim_4"]);
}

/// The fixture's datasets are created in an order that is *not* their
/// alphabetical order, because netCDF-C numbers invented dimensions by name. A
/// reader that numbered by on-disk discovery order would still pass every
/// length check above while disagreeing with `ncdump` about which axis is
/// `phony_dim_0`, so this pins the thing that distinguishes them: the file's
/// first-created dataset is the 1-D one, and it must come *last*.
#[test]
fn anonymous_dimensions_are_numbered_by_name_not_by_discovery_order() {
    let meta = metadata(PHONY);
    let one_d = meta
        .variables
        .iter()
        .find(|v| v.name == "e_1d7")
        .expect("e_1d7 resolves");
    assert_eq!(
        one_d.dimensions,
        ["phony_dim_4"],
        "e_1d7 is written first but sorts last, so it must take the last number"
    );
}

/// The two filter fixtures are scale-less too, which is why neither could be
/// rendered before. Their axes now carry the datasets' real 8 x 8 extent.
#[test]
fn the_filter_fixtures_report_their_real_extent() {
    for (label, bytes) in [("fletcher32", FLETCHER32), ("zstd", ZSTD)] {
        let meta = metadata(bytes);
        for variable in &meta.variables {
            // `f32_odd` is the deliberately 1-D odd-chunk dataset.
            if variable.dimensions.len() != 2 {
                continue;
            }
            let lengths: Vec<u64> = variable
                .dimensions
                .iter()
                .map(|name| {
                    meta.dimensions
                        .iter()
                        .find(|d| &d.name == name)
                        .unwrap_or_else(|| {
                            panic!("{label}: {} references unknown {name}", variable.name)
                        })
                        .length
                })
                .collect();
            assert_eq!(
                lengths,
                [8, 8],
                "{label}: {} should be 8 x 8, not {lengths:?}",
                variable.name
            );
        }
    }
}

/// `H5S_UNLIMITED` survives into the invented dimension. netCDF-C reads
/// `hdf5_ea_chunk_index.h5` — two scale-less 1-D datasets with `maxshape=(None,)`
/// — back as `phony_dim_0 = 600 (unlimited)` and `phony_dim_1 = 1120 (unlimited)`,
/// so an extensible axis must not be flattened to a fixed one on the way through.
#[test]
fn an_unlimited_axis_stays_unlimited() {
    let meta = metadata(EXTENSIBLE);
    assert_eq!(
        meta.dimensions
            .iter()
            .map(|d| (d.name.as_str(), d.length, d.is_unlimited))
            .collect::<Vec<_>>(),
        [("phony_dim_0", 600, true), ("phony_dim_1", 1120, true)]
    );
}
