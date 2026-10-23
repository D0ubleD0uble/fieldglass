//! A CF `geostationary` grid whose `x`/`y` are in metres (#966).
//!
//! CF allows the coordinates of a geostationary mapping as scan angles in
//! radians, as GOES-R writes them, or as PROJ's `+proj=geos` easting and
//! northing in metres, as satpy, pyresample and GDAL write them. Units were
//! never read, so a metre file was placed as if each metre were a radian.
//! The metre fixture is the radian fixture's grid with its axes multiplied by
//! `perspective_point_height`; it has to place where the radian one does.
//! Units that are neither decline the mapping, which `non_finite_geometry.rs`
//! checks beside the other declined mappings.

use fieldglass::{DecodeOptions, Field, GridGeometry, Placement, Session};

const FIXTURES: &str = "../fieldglass-netcdf/tests/fixtures/";

fn rad_slice(fixture: &str) -> Field {
    let session = Session::open(std::fs::read(format!("{FIXTURES}{fixture}")).expect("fixture"))
        .expect("opens");
    let var = session
        .variables()
        .iter()
        .find(|v| v.name == "Rad")
        .expect("Rad")
        .clone();
    let (y, x) = (
        var.detected_y_dim.expect("y"),
        var.detected_x_dim.expect("x"),
    );
    session
        .decode_slice(var.index, y, x, &[0, 0], &DecodeOptions::default())
        .expect("decodes")
}

#[test]
fn a_geostationary_grid_in_metres_places_where_its_radian_twin_does() {
    let radian = rad_slice("goes_geostationary.nc");
    let metres = rad_slice("goes_geostationary_metres.nc");
    let (want, got) = (&radian.georef, &metres.georef);
    assert_eq!(want.kind, "space_view", "the radian fixture is placed");
    assert_eq!(got.kind, want.kind);
    assert_eq!(got.label, want.label);
    assert_eq!(got.placement, Placement::Placed);
    assert_eq!(got.reprojectable, want.reprojectable);
    assert_eq!(got.proj4, want.proj4);
    let (Some(got_bounds), Some(want_bounds)) = (&got.bounds_lonlat, &want.bounds_lonlat) else {
        panic!(
            "both placed: {:?} vs {:?}",
            got.bounds_lonlat, want.bounds_lonlat
        );
    };
    // The radian twin stores its axes as scaled `int16`, so the two agree to
    // that packing, far below a cell (about 0.5° here).
    for (g, w) in got_bounds.iter().zip(want_bounds.iter()) {
        assert!(
            (g - w).abs() < 1e-9,
            "bounds {got_bounds:?} vs {want_bounds:?}"
        );
    }
    assert_eq!(metres.values, radian.values);
}

/// The scan angles picked the other way round are not a disc (#1387).
///
/// A geostationary grid is placed from its `x` axis across the columns and its
/// `y` axis down the rows. Picked transposed, `x` down the rows and `y`
/// across, the same formula placed a real but wrong grid: on the GOES sample,
/// a pixel at 29° N 128° W landed at 39° S 35° W. The extension's axis
/// pickers swap in one pick, so the transposed pair is one pick away. It is
/// drawn in its own source projection, as a cross-section is, not placed.
#[test]
fn a_transposed_geostationary_pick_is_not_placed() {
    for fixture in ["goes_geostationary.nc", "goes_geostationary_metres.nc"] {
        let session =
            Session::open(std::fs::read(format!("{FIXTURES}{fixture}")).expect("fixture"))
                .expect("opens");
        let var = session
            .variables()
            .iter()
            .find(|v| v.name == "Rad")
            .expect("Rad")
            .clone();
        let (y, x) = (
            var.detected_y_dim.expect("y"),
            var.detected_x_dim.expect("x"),
        );
        let upright = session
            .decode_slice(var.index, y, x, &[0, 0], &DecodeOptions::default())
            .expect("decodes");
        assert_eq!(
            upright.georef.placement,
            Placement::Placed,
            "{fixture}: the premise"
        );
        let transposed = session
            .decode_slice(var.index, x, y, &[0, 0], &DecodeOptions::default())
            .expect("decodes");
        assert_eq!(
            transposed.georef.placement,
            Placement::Unplaceable,
            "{fixture}: the transposed scan angles were placed at {:?}",
            transposed.georef.bounds_lonlat
        );
        let placed = session.place_slice(var.index, x, y).expect("places");
        assert!(
            matches!(
                placed.geometry(),
                GridGeometry::Unsupported { label, .. } if label == "source"
            ),
            "{fixture}: place_slice gave {:?}",
            placed.geometry()
        );
    }
}
