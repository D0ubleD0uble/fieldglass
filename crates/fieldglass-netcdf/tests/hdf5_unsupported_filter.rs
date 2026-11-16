//! A variable whose HDF5 filter pipeline a read cannot undo is named, with
//! the refusal the read gives, in `AttributesNotRead::undecodable` (#1855).
//!
//! Before, the listing never looked at the pipeline: HDF5's scale-offset
//! filter (id 6) was listed as an ordinary variable, offered for drawing, and
//! every read of it failed. It is still listed, as 0.6.1 promised
//! (`Hdf5Metadata::variables` is every variable in the file), and is now
//! named beside the listing so a host can decline it. Two committed fixtures:
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
/// it by.
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

/// The names `undecodable` gives, in its order.
fn undecodable_names(r: &NetcdfReader) -> Vec<String> {
    let (_, unread) = r.hdf5_metadata_with_unread().expect("resolves");
    unread
        .undecodable()
        .iter()
        .map(|u| u.name.clone())
        .collect()
}

/// The issue's acceptance: the 12 scale-offset datasets are named as
/// undecodable, each with the reason a read of it gives, which names filter
/// 6. They are still listed as variables, at the decode index they read by,
/// and `unsupported` is as in 0.6.1: the two n-bit datasets only.
#[test]
fn scale_offset_datasets_are_named_undecodable_with_the_reads_reason() {
    let r = reader(CROSS_LE);
    let (meta, unread) = r.hdf5_metadata_with_unread().expect("resolves");
    let want: Vec<String> = ["char", "double", "float", "int", "long_long", "short"]
        .iter()
        .flat_map(|t| ["be", "le"].map(|e| format!("Scale_offset_{t}_data_{e}")))
        .collect();
    let got: Vec<&str> = unread
        .undecodable()
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    assert_eq!(got, want, "in name order");

    for u in unread.undecodable() {
        let v = meta
            .variables
            .iter()
            .find(|v| v.name == u.name)
            .unwrap_or_else(|| panic!("{} is still a variable", u.name));
        assert_eq!(v.decode_index, decode_index(&r, CROSS_LE, &u.name));
        let read = r
            .decode_variable_raw(v.decode_index)
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
    let declined: Vec<&str> = meta.unsupported.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(declined, ["Nbit_float_data_be", "Nbit_float_data_le"]);
    assert_eq!(meta.variables.len(), 22);
    assert!(unread.is_empty(), "no attribute was lost");
}

/// The view and the array seam say the same. The view's `unsupported` and
/// the seam's `left_out` are as in 0.6.1, every array is still in the group,
/// and `NetcdfArrays::undecodable` names the 12.
#[test]
fn the_view_and_the_array_seam_name_them_too() {
    let r = reader(CROSS_LE);
    let (view, unread) = r.view_with_unread().expect("view");
    assert_eq!(unread.undecodable().len(), 12);
    assert_eq!(view.unsupported.len(), 2);
    assert!(
        view.vars
            .iter()
            .any(|v| v.name() == "Scale_offset_int_data_le")
    );
    let arrays = NetcdfArrays::open(r).expect("view resolves");
    let undecodable: Vec<String> = arrays.undecodable().into_iter().map(|l| l.name).collect();
    assert_eq!(undecodable, undecodable_names(arrays.reader()));
    assert!(
        arrays
            .left_out()
            .iter()
            .all(|l| !l.name.starts_with("Scale_offset_")),
        "left_out is the arrays not in the group"
    );
    for name in &undecodable {
        assert!(
            arrays.group().arrays.iter().any(|a| &a.name == name),
            "{name} is still in the group"
        );
    }
}

/// The two szip datasets in the same file still list and read: 0.33333,
/// 0.66667 and 1.0 at grid (0, 0), (1, 0) and (2, 0).
#[test]
fn the_szip_datasets_beside_them_still_read() {
    let r = reader(CROSS_LE);
    let undecodable = undecodable_names(&r);
    let meta = r.hdf5_metadata().expect("resolves");
    for name in ["Szip_float_data_be", "Szip_float_data_le"] {
        assert!(!undecodable.iter().any(|n| n == name), "{name}");
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

/// A coordinate variable stored under scale-offset is named undecodable,
/// and is still its dimension and still listed, so the variables on that
/// axis keep its name.
#[test]
fn a_coordinate_variable_under_scale_offset_is_still_its_dimension() {
    let r = reader(LISTING);
    assert_eq!(undecodable_names(&r), ["packed", "x"]);
    let meta = r.hdf5_metadata().expect("resolves");
    assert!(meta.unsupported.is_empty(), "{:?}", meta.unsupported);
    let x = meta
        .dimensions
        .iter()
        .find(|d| d.name == "x")
        .expect("x is still a dimension");
    assert_eq!(x.length, 6);
    let listed: Vec<&str> = meta.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        listed,
        ["field", "packed", "skipped", "unwritten", "x", "y"]
    );
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

/// A scale-offset dataset a read never runs the filter on is not named and
/// still reads: one nothing was written to reads as its fill value, and one
/// whose single chunk skipped the filter (its mask bit set) reads as stored.
#[test]
fn a_dataset_whose_read_runs_no_refused_filter_is_not_named() {
    let r = reader(LISTING);
    let undecodable = undecodable_names(&r);
    assert!(
        !undecodable
            .iter()
            .any(|n| n == "unwritten" || n == "skipped"),
        "{undecodable:?}"
    );
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
