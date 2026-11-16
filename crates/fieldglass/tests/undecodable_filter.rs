//! An HDF5 dataset stored with a filter Fieldglass does not decode, through
//! `Session` (#1855).
//!
//! `hdf5_scale_offset_listing.h5` (provenance in the NetCDF fixtures'
//! `NOTICE.md`) holds `packed` and the coordinate `x` under HDF5's
//! scale-offset filter, and `field`, which reads, on `x`. Embedded so the
//! `wasm32-wasip1` run reads it. The reader-level tests are
//! `fieldglass-netcdf`'s `tests/hdf5_unsupported_filter.rs`.

use fieldglass::{DecodeOptions, LeftOutCause, Session};

const LISTING: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_scale_offset_listing.h5");

/// `undecodable` names both with the read's refusal, and moves neither:
/// `packed` is still a variable and `x` still undrawable, as a coordinate.
#[test]
fn undecodable_names_them_beside_the_lists_that_still_hold_them() {
    let session = Session::open(LISTING.to_vec()).expect("opens");
    let undecodable = session.undecodable();
    let names: Vec<&str> = undecodable.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["packed", "x"]);
    for u in &undecodable {
        assert!(u.reason.contains("HDF5 filter id 6"), "{}", u.reason);
        assert_eq!(u.cause, LeftOutCause::Unsupported);
    }
    let variables: Vec<String> = session.variables().into_iter().map(|v| v.name).collect();
    assert!(variables.contains(&"packed".to_string()), "{variables:?}");
    assert!(variables.contains(&"field".to_string()), "{variables:?}");
    assert!(
        session.undrawable().iter().any(|u| u.name == "x"),
        "x is still undrawable, as a coordinate"
    );
    assert!(session.left_out().is_empty());
}

/// `field` on the undecodable `x` is placed source-only and decodes, and its
/// listing says why it is not placed. Before, every decode of it failed with
/// the filter's refusal, read from `x`.
#[test]
fn a_field_on_an_undecodable_axis_decodes_source_only_and_says_why() {
    let session = Session::open(LISTING.to_vec()).expect("opens");
    let variables = session.variables();
    let field = variables
        .iter()
        .find(|v| v.name == "field")
        .expect("field listed");
    let note = field.not_placed.as_deref().unwrap_or_default();
    assert!(
        note.contains("\"x\" cannot be decoded, so the field is not placed on the map by it"),
        "{note:?}"
    );
    let decoded = session
        .decode_slice(
            field.index,
            field.default_y_dim,
            field.default_x_dim,
            &[0, 0],
            &DecodeOptions::default(),
        )
        .expect("decodes");
    assert_eq!(decoded.georef.geometry.label(), "source");
    let skipped = variables
        .iter()
        .find(|v| v.name == "skipped")
        .expect("skipped listed");
    assert_eq!(skipped.not_placed, None, "no undecodable axis, no note");
}
