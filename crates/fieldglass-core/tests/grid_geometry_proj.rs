//! `GridGeometry` places a projected grid where PROJ places it.
//!
//! [`GridGeometry::proj4`] exists so a browser map library can be handed one
//! string and put the field in the right place. That is a claim about agreeing
//! with PROJ, so PROJ is what checks it — not a golden of our own output, which
//! would only prove we still compute what we used to.
//!
//! `tools/gen_grid_geometry_proj_golden.py` writes the golden by running the
//! whole geolocation in PROJ instead of Rust: forward the stated first point to
//! get the grid origin in metres, step out `i·dx` / `j·dy`, and invert. That is
//! `PlanarGridProjector::grid_point_lonlat` with a different implementation
//! underneath, which is what makes it an oracle rather than a snapshot.
//!
//! The proj4 string is asserted too, so editing `proj4()` fails here until the
//! generator is re-run and PROJ has had its say about the new string.
//!
//! The grids are the real ones from `grid_round_trip.rs`, for the reason
//! recorded there: #488 hid in a synthetic fixture that never left the northern
//! hemisphere.

use fieldglass_core::projection::{GridGeometry, normalise_lon};

const GOLDEN: &str = include_str!("grid_geometry_proj.golden.json");

/// Degrees, and the number is the point: all three grids come back at
/// 4.99e-10, which is half of PROJ's own printing quantum at nine decimals.
/// The two implementations therefore agree as exactly as this file can express
/// it — the residual is the golden being rounded, not arithmetic drifting. Set
/// just above that quantum so a real disagreement of even 1e-9 deg (about
/// 0.1 mm) fails rather than passes.
const TOL_DEG: f64 = 1e-9;

/// Signed difference between two longitudes, taking the short way round, so
/// -179.9999 and 180.0001 compare as the 0.0002 apart that they are.
fn lon_delta(a: f64, b: f64) -> f64 {
    let d = normalise_lon(a) - normalise_lon(b);
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

#[test]
fn projected_grids_geolocate_where_proj_says() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden parses");
    let cases = golden["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "golden has no cases");

    let mut checked = 0usize;
    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let geom: GridGeometry =
            serde_json::from_value(case["geometry"].clone()).expect("geometry deserialises");

        assert_eq!(
            geom.proj4().expect("a projected family has a CRS"),
            case["proj4"].as_str().expect("golden proj4"),
            "{name}: proj4 string drifted from the one PROJ was asked about; \
             re-run tools/gen_grid_geometry_proj_golden.py",
        );

        let mut worst_lat = 0.0f64;
        let mut worst_lon = 0.0f64;
        for pt in case["points"].as_array().expect("points array") {
            let (i, j) = (
                pt["i"].as_u64().expect("i") as u32,
                pt["j"].as_u64().expect("j") as u32,
            );
            let (lat, lon) = geom
                .forward(i, j)
                .unwrap_or_else(|| panic!("{name}: no position for grid point ({i}, {j})"));
            worst_lat = worst_lat.max((lat - pt["lat"].as_f64().expect("lat")).abs());
            worst_lon = worst_lon.max(lon_delta(lon, pt["lon"].as_f64().expect("lon")).abs());
            checked += 1;
        }
        assert!(
            worst_lat < TOL_DEG && worst_lon < TOL_DEG,
            "{name}: worst disagreement with PROJ {worst_lat:e} deg lat, \
             {worst_lon:e} deg lon (tolerance {TOL_DEG:e})",
        );
        println!("{name}: {worst_lat:e} deg lat, {worst_lon:e} deg lon");
    }
    assert!(checked > 500, "expected the full golden, checked {checked}");
}
