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

/// A real file with the arrays in `names` reported undecodable, and every
/// read of them refused as a filter this build lacks would refuse it.
struct Undecodable {
    inner: fieldglass::netcdf::NetcdfArrays,
    names: Vec<String>,
}

impl Undecodable {
    fn new(bytes: &[u8], names: &[&str]) -> Self {
        let reader = fieldglass::netcdf::NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        Self {
            inner: fieldglass::netcdf::NetcdfArrays::open(reader).expect("a view"),
            names: names.iter().map(|n| n.to_string()).collect(),
        }
    }
}

impl fieldglass_core::array::ArraySource for Undecodable {
    fn group(&self) -> &fieldglass_core::array::Group {
        self.inner.group()
    }

    fn read_region(
        &self,
        array: &str,
        region: &[std::ops::Range<u64>],
    ) -> Result<Vec<Option<f64>>, fieldglass_core::FieldglassError> {
        if self.names.iter().any(|n| n == array) {
            return Err(fieldglass_core::FieldglassError::UnsupportedSection(
                format!("{array} is stored with a filter this build lacks"),
            ));
        }
        self.inner.read_region(array, region)
    }

    fn undecodable(&self) -> Vec<fieldglass_core::array::LeftOut> {
        self.names
            .iter()
            .map(|n| fieldglass_core::array::LeftOut {
                name: n.clone(),
                reason: "a filter this build lacks".into(),
                cause: fieldglass_core::array::LeftOutCause::Unsupported,
            })
            .collect()
    }
}

/// Each kind of coordinate placement reads beside a 1-D axis, a 2-D pair,
/// WRF's corners and a geostationary scan angle, is placed without when the
/// source names it undecodable, and the listing has one clause for each, and
/// for no other (#1855). The coordinates `absent` answers `true` for are
/// exactly the ones named. The entry point without `absent` still gives the
/// read's refusal, as 0.6.1 documents.
#[test]
fn every_coordinate_placed_without_has_its_clause() {
    use fieldglass_core::cf::{slice_placement, slice_placement_with_absent};
    const WRF: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/wrf_lambert.nc");
    const SWATH: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc");
    const GEOS: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/goes_geostationary.nc");
    for (file, bytes, field, names) in [
        ("wrf", WRF, "T2", &["XLAT", "XLONG"][..]),
        ("swath", SWATH, "TPW", &["Latitude", "Longitude"][..]),
        ("geostationary", GEOS, "Rad", &["x", "y"][..]),
    ] {
        let source = Undecodable::new(bytes, names);
        let listed = fieldglass::variables_of(&source);
        let field = listed
            .iter()
            .find(|v| v.name == field)
            .unwrap_or_else(|| panic!("{file}: {field} listed"));
        let (y, x) = (field.default_y_dim as usize, field.default_x_dim as usize);
        let err = slice_placement(&source, &field.name, y, x).expect_err("the 0.6.1 error");
        assert!(
            matches!(err, fieldglass_core::FieldglassError::UnsupportedSection(_)),
            "{file}: {err:?}"
        );

        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let placement = slice_placement_with_absent(&source, &field.name, y, x, |n| {
            let hit = source.names.iter().any(|u| u == n);
            if hit && !asked.borrow().iter().any(|a| a == n) {
                asked.borrow_mut().push(n.to_owned());
            }
            hit
        })
        .unwrap_or_else(|e| panic!("{file}: placed, not refused: {e}"));
        assert_eq!(placement.geometry.label(), "source", "{file}");
        let mut asked = asked.into_inner();
        asked.sort();
        assert_eq!(asked, names, "{file}");

        let note = field.not_placed.as_deref().unwrap_or_default();
        let mut got: Vec<&str> = note
            .split("; ")
            .filter(|c| c.contains("cannot be decoded"))
            .collect();
        got.sort_unstable();
        let want: Vec<String> = names
            .iter()
            .map(|n| {
                format!("{n:?} cannot be decoded, so the field is not placed on the map by it")
            })
            .collect();
        assert_eq!(got, want, "{file}: {note:?}");
    }
}
