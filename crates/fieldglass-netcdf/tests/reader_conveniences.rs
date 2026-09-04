//! The one-call reader conveniences over the decode → plane → unpack chain
//! (#548): [`NetcdfReader::view`], [`NetcdfReader::decode_variable_physical`],
//! and [`NetcdfReader::decode_plane`].
//!
//! These wrap steps that already have their own tests, so what is asserted here
//! is that the wrapper reaches the *same* answer as the chain it replaces —
//! including on the NetCDF-4 backing, where the view's `decode_index` is not a
//! position in `vars` and the wrong one silently unpacks with another
//! variable's attributes.

use fieldglass_netcdf::{
    DatasetView, NcType, NetcdfBacking, NetcdfReader, extract_plane, unpack_cf_data,
};

const CLASSIC: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
const NC4: &[u8] = include_bytes!("fixtures/netcdf4_dimscale.nc");
const PHONY: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");

#[test]
fn view_matches_the_hand_written_backing_match_on_both_layouts() {
    let classic = NetcdfReader::from_bytes(CLASSIC.to_vec()).expect("classic parses");
    let NetcdfBacking::Classic(header) = &classic.backing else {
        panic!("the CDF-1 fixture is a classic backing");
    };
    assert_eq!(
        classic.view().expect("classic view"),
        DatasetView::from_classic(header)
    );

    let nc4 = NetcdfReader::from_bytes(NC4.to_vec()).expect("nc4 parses");
    assert!(matches!(nc4.backing, NetcdfBacking::Hdf5(_)));
    assert_eq!(
        nc4.view().expect("nc4 view"),
        DatasetView::from_hdf5(&nc4.hdf5_metadata().expect("hdf5 metadata"))
    );
}

/// The empty view is what a host falls back to when an HDF5 layout outside the
/// decoded subset makes `view()` fail; it has to be constructible without
/// naming the three fields.
#[test]
fn the_default_view_is_empty() {
    let empty = DatasetView::default();
    assert!(empty.dims.is_empty());
    assert!(empty.vars.is_empty());
    assert!(empty.global_attrs.is_empty());
    assert!(empty.var(0).is_none());
    assert!(empty.renderable_variables().is_empty());
}

/// Every numeric variable of both backings, decoded both ways.
/// `decode_variable_physical` has to equal
/// decode-then-unpack-with-*that*-variable's-attributes for all of them, which
/// is the mistake the convenience exists to prevent. `char` variables are
/// skipped: value decode refuses them by design, so a fixture that gained a
/// text variable would otherwise fail this test rather than the code.
#[test]
fn physical_decode_equals_decode_then_unpack_for_every_variable() {
    for (label, bytes) in [("classic", CLASSIC), ("nc4", NC4)] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("parses");
        let view = reader.view().expect("view");
        let numeric: Vec<_> = view
            .vars
            .iter()
            .filter(|v| v.nc_type != NcType::Char)
            .collect();
        assert!(
            !numeric.is_empty(),
            "{label}: fixture has numeric variables"
        );
        for var in numeric {
            let raw = reader
                .decode_variable_values(var.decode_index)
                .unwrap_or_else(|e| panic!("{label}: {} decodes: {e}", var.name));
            assert_eq!(
                reader
                    .decode_variable_physical(var.decode_index)
                    .unwrap_or_else(|e| panic!("{label}: {} unpacks: {e}", var.name)),
                unpack_cf_data(&raw, &var.attrs),
                "{label}: {}",
                var.name
            );
        }
    }
}

/// `decode_plane` is decode → `extract_plane` → unpack, in that order, and the
/// order is not observable: unpacking is per point, so extracting first must
/// give what unpacking the whole variable and then extracting would.
#[test]
fn decode_plane_equals_the_three_steps_run_by_hand() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).expect("parses");
    let view = reader.view().expect("view");
    let sst = view
        .vars
        .iter()
        .find(|v| v.name == "sst")
        .expect("ERSST has an sst variable");
    // sst(time, lev, lat, lon) — hold the leading axes, image axes lat × lon.
    assert_eq!(sst.dim_names.len(), 4, "sst is 4-D");
    let shape = reader.variable_shape(sst.decode_index).expect("shape");
    let raw = reader
        .decode_variable_values(sst.decode_index)
        .expect("decode");

    let by_hand = unpack_cf_data(
        &extract_plane(&raw, &shape, 2, 3, &[0, 0, 0, 0]).expect("plane"),
        &sst.attrs,
    );
    assert_eq!(
        reader
            .decode_plane(sst, 2, 3, &[0, 0, 0, 0])
            .expect("decode_plane"),
        by_hand
    );

    // Unpacking before extracting reaches the same values, so the documented
    // order is a cost choice, not a correctness one.
    assert_eq!(
        by_hand,
        extract_plane(
            &unpack_cf_data(&raw, &sst.attrs),
            &shape,
            2,
            3,
            &[0, 0, 0, 0]
        )
        .expect("plane")
    );

    // The extraction's own guards still fire through the wrapper.
    assert!(reader.decode_plane(sst, 2, 2, &[0, 0, 0, 0]).is_err());
    assert!(reader.decode_plane(sst, 2, 3, &[0, 0]).is_err());
    assert!(reader.decode_plane(sst, 2, 9, &[0, 0, 0, 0]).is_err());
}

/// A NetCDF-4 pure-dimension placeholder is a dataset with a decode index but
/// no variable in the view, so there are no attributes to unpack it with. The
/// physical decode refuses rather than passing raw codes off as physical units.
#[test]
fn physical_decode_refuses_an_index_with_no_variable() {
    let reader = NetcdfReader::from_bytes(PHONY.to_vec()).expect("parses");
    let view = reader.view().expect("view");
    let mut orphans = (0..)
        .take_while(|&i| reader.decode_variable_values(i).is_ok())
        .filter(|&i| view.var(i).is_none())
        .peekable();
    assert!(
        orphans.peek().is_some(),
        "fixture must have a decodable dataset that is not a variable"
    );
    for index in orphans {
        assert!(
            reader.decode_variable_physical(index).is_err(),
            "index {index} has no variable, so no attributes"
        );
    }
}
