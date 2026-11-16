//! A variable whose HDF5 filter pipeline a read cannot undo is listed as not
//! decoded, with the refusal the read gives (#1855).
//!
//! Before, the listing never looked at the pipeline: HDF5's scale-offset
//! filter (id 6) was listed as an ordinary variable, offered for drawing, and
//! every read of it failed. Two committed fixtures:
//!
//! - `hdf5_szip2_cross_le.h5`, the HDF5 repository's `le_data.h5`, with 12
//!   `Scale_offset_*` datasets beside the two szip ones it was committed for
//!   (#820).
//! - `hdf5_scale_offset_listing.h5`, built by
//!   `tools/build_hdf5_scale_offset_listing_fixture.py`: a coordinate
//!   variable under scale-offset, and two scale-offset datasets a read never
//!   runs the filter on, which still read.
//!
//! See `tests/fixtures/NOTICE.md` for both.

use fieldglass_core::array::ArraySource;
use fieldglass_core::array::LeftOutCause;
use fieldglass_netcdf::array_source::NetcdfArrays;
use fieldglass_netcdf::{ChildKind, NetcdfReader, list_root_children};

const CROSS_LE: &[u8] = include_bytes!("fixtures/hdf5_szip2_cross_le.h5");
const LISTING: &[u8] = include_bytes!("fixtures/hdf5_scale_offset_listing.h5");

fn reader(bytes: &[u8]) -> NetcdfReader {
    NetcdfReader::from_bytes(bytes.to_vec()).expect("recognized HDF5")
}

/// Each root dataset's decode index, the position `decode_variable_raw` reads
/// it by, whether or not the listing keeps it.
fn decode_index(r: &NetcdfReader, bytes: &[u8], name: &str) -> usize {
    let fieldglass_netcdf::NetcdfBacking::Hdf5(probe) = &r.backing else {
        panic!("expected the HDF5 backing");
    };
    list_root_children(bytes, probe)
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .position(|c| c.name == name)
        .unwrap_or_else(|| panic!("no dataset {name}"))
}

/// The issue's acceptance: the 12 scale-offset datasets are under "Variables
/// not decoded", each with the reason a read of it gives, which names filter
/// 6, and none is offered as a variable. The two n-bit datasets are still
/// declined for their datatype, as before.
#[test]
fn scale_offset_datasets_are_listed_as_not_decoded_with_the_reads_reason() {
    let r = reader(CROSS_LE);
    let meta = r.hdf5_metadata().expect("resolves");
    let scale_offset: Vec<&str> = meta
        .unsupported
        .iter()
        .filter(|u| u.name.starts_with("Scale_offset_"))
        .map(|u| u.name.as_str())
        .collect();
    let mut want: Vec<String> = ["char", "double", "float", "int", "long_long", "short"]
        .iter()
        .flat_map(|t| ["be", "le"].map(|e| format!("Scale_offset_{t}_data_{e}")))
        .collect();
    want.sort();
    let mut got = scale_offset.clone();
    got.sort_unstable();
    assert_eq!(got, want);

    for u in meta
        .unsupported
        .iter()
        .filter(|u| u.name.starts_with("Scale_offset_"))
    {
        let read = r
            .decode_variable_raw(decode_index(&r, CROSS_LE, &u.name))
            .expect_err("a read still refuses it");
        assert_eq!(u.reason, read.to_string(), "{}", u.name);
        assert!(
            u.reason.contains("HDF5 filter id 6"),
            "{}: {}",
            u.name,
            u.reason
        );
        assert_eq!(u.cause, LeftOutCause::Unsupported, "{}", u.name);
    }
    let nbit: Vec<&str> = meta
        .unsupported
        .iter()
        .filter(|u| u.name.starts_with("Nbit_"))
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(nbit, ["Nbit_float_data_be", "Nbit_float_data_le"]);
    assert_eq!(meta.unsupported.len(), 14, "{:?}", meta.unsupported);

    let listed: Vec<&str> = meta.variables.iter().map(|v| v.name.as_str()).collect();
    assert!(
        !listed
            .iter()
            .any(|n| n.starts_with("Scale_offset_") || n.starts_with("Nbit_")),
        "{listed:?}"
    );
    assert_eq!(listed.len(), 10, "{listed:?}");
}

/// The view and the array seam a host lists from say the same, so the
/// umbrella, the Node addon and the npm package need nothing of their own.
#[test]
fn the_view_and_the_array_seam_name_them_too() {
    let r = reader(CROSS_LE);
    let view = r.view().expect("view");
    assert_eq!(
        view.unsupported,
        r.hdf5_metadata().expect("resolves").unsupported
    );
    let arrays = NetcdfArrays::open(r).expect("view resolves");
    let left: Vec<String> = arrays
        .left_out()
        .into_iter()
        .filter(|l| l.name.starts_with("Scale_offset_"))
        .map(|l| l.name)
        .collect();
    assert_eq!(left.len(), 12, "{left:?}");
    let offered: Vec<&str> = arrays
        .group()
        .arrays
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert!(
        !offered.iter().any(|n| n.starts_with("Scale_offset_")),
        "{offered:?}"
    );
}

/// The two szip datasets in the same file still list and read: 0.33333,
/// 0.66667 and 1.0 at grid (0, 0), (1, 0) and (2, 0).
#[test]
fn the_szip_datasets_beside_them_still_read() {
    let r = reader(CROSS_LE);
    let meta = r.hdf5_metadata().expect("resolves");
    for name in ["Szip_float_data_be", "Szip_float_data_le"] {
        let v = meta
            .variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} listed"));
        let values = r.decode_variable_raw(v.decode_index).expect("reads");
        let first: Vec<f64> = values[..3].iter().map(|v| v.expect("a value")).collect();
        for (got, want) in first.iter().zip([1.0 / 3.0, 2.0 / 3.0, 1.0]) {
            assert!((got - want).abs() < 1e-5, "{name}: {first:?}");
        }
    }
}

/// A float that is not IEEE (the n-bit datasets' 20-bit precision) is named
/// in words, not in the reader's debug form, which the page cut short.
#[test]
fn a_non_ieee_float_is_declined_in_words() {
    let meta = reader(CROSS_LE).hdf5_metadata().expect("resolves");
    let nbit = meta
        .unsupported
        .iter()
        .find(|u| u.name == "Nbit_float_data_le")
        .expect("declined");
    assert_eq!(
        nbit.reason,
        "unsupported section: HDF5 floating-point datatype with a non-IEEE layout: \
         a float of 20 bits in a 32-bit element, with an exponent of 6 bits, a \
         mantissa of 13 bits and exponent bias 31"
    );
}

/// A coordinate variable stored under scale-offset is declined as a variable
/// but is still its dimension, so the variables on that axis keep its name.
#[test]
fn a_declined_coordinate_variable_is_still_its_dimension() {
    let r = reader(LISTING);
    let meta = r.hdf5_metadata().expect("resolves");
    let declined: Vec<&str> = meta.unsupported.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(declined, ["packed", "x"]);
    let x = meta
        .dimensions
        .iter()
        .find(|d| d.name == "x")
        .expect("x is still a dimension");
    assert_eq!(x.length, 6);
    let field = meta
        .variables
        .iter()
        .find(|v| v.name == "field")
        .expect("field listed");
    assert_eq!(field.dimensions, ["y", "x"]);
    let values = r.decode_variable_raw(field.decode_index).expect("reads");
    assert_eq!(values[..3], [Some(0.0), Some(0.25), Some(0.5)]);
    assert!(
        r.decode_variable_raw(decode_index(&r, LISTING, "x"))
            .is_err(),
        "x's values are still not read"
    );
}

/// A scale-offset dataset a read never runs the filter on still lists and
/// reads: one nothing was written to reads as its fill value, and one whose
/// single chunk skipped the filter (its mask bit set) reads as stored.
#[test]
fn a_dataset_whose_read_runs_no_refused_filter_still_lists() {
    let r = reader(LISTING);
    let meta = r.hdf5_metadata().expect("resolves");
    let read = |name: &str| {
        let v = meta
            .variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} listed"));
        r.decode_variable_raw(v.decode_index).expect("reads")
    };
    assert_eq!(read("unwritten"), vec![Some(-1.0); 24]);
    let want: Vec<Option<f64>> = (0..24).map(|k| Some(200.0 + 0.25 * f64::from(k))).collect();
    assert_eq!(read("skipped"), want);
}
