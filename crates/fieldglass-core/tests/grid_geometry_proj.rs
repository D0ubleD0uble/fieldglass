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
//! The proj4 string and the affine are asserted too, so editing `proj4()` or
//! `plane_affine()` fails here until the generator is re-run and PROJ has had
//! its say about the new numbers. Together they are the whole claim a map
//! library consumes: which plane, and where in it the corner pixel goes.
//!
//! The grids are the real ones from `grid_round_trip.rs`, for the reason
//! recorded there: #488 hid in a synthetic fixture that never left the northern
//! hemisphere.

use fieldglass_core::projection::{GridGeometry, PlaneUnits, normalise_lon};

const GOLDEN: &str = include_str!("grid_geometry_proj.golden.json");

/// Degrees, and the number is the point: all three grids come back at
/// 4.99e-10, which is half of PROJ's own printing quantum at nine decimals.
/// The two implementations therefore agree as exactly as this file can express
/// it — the residual is the golden being rounded, not arithmetic drifting. Set
/// just above that quantum so a real disagreement of even 1e-9 deg (about
/// 0.1 mm) fails rather than passes.
const TOL_DEG: f64 = 1e-9;

/// Metres. The affine is quoted at 1e-9 m, so this is the tightest bound the
/// golden can express; a real disagreement is a whole cell, not a nanometre.
const TOL_M: f64 = 1e-6;

/// Every family [`GridGeometry::proj4`] names a CRS for. Named rather than
/// counted, because the failure this guards is a family quietly dropping out
/// of the golden: the generator would still write a file, the test would still
/// check hundreds of points, and nothing would be checking the projection that
/// was removed.
const CRS_FAMILIES: [&str; 6] = [
    "lambert",
    "lambert_azimuthal",
    "mercator",
    "polar_stereo",
    "space_view",
    "transverse_mercator",
];

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
    let mut families: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let geom: GridGeometry =
            serde_json::from_value(case["geometry"].clone()).expect("geometry deserialises");
        families.insert(geom.kind().to_string());

        assert_eq!(
            geom.proj4().expect("a projected family has a CRS"),
            case["proj4"].as_str().expect("golden proj4"),
            "{name}: proj4 string drifted from the one PROJ was asked about; \
             re-run tools/gen_grid_geometry_proj_golden.py",
        );

        // The affine says where in that CRS the raster sits. A correct CRS with
        // a wrong origin puts the field somewhere plausible and wrong, which is
        // exactly the failure a picture does not show.
        let want = &case["affine"];
        let got = geom
            .plane_affine()
            .expect("a projected family has an affine");
        assert_eq!(
            serde_json::from_value::<PlaneUnits>(want["units"].clone()).expect("golden units"),
            got.units,
            "{name}: affine units"
        );
        for (label, want, got) in [
            ("x0", want["x0"].as_f64(), Some(got.x0)),
            ("y0", want["y0"].as_f64(), Some(got.y0)),
            ("dx", want["dx"].as_f64(), got.dx),
            ("dy", want["dy"].as_f64(), got.dy),
        ] {
            let (want, got) = (want.expect("golden affine"), got.expect("affine component"));
            assert!(
                (want - got).abs() < TOL_M,
                "{name}: affine {label} is {got} m, PROJ says {want} m"
            );
        }

        let mut worst_lat = 0.0f64;
        let mut worst_lon = 0.0f64;
        let mut off_disc = 0usize;
        for pt in case["points"].as_array().expect("points array") {
            let (i, j) = (
                pt["i"].as_u64().expect("i") as u32,
                pt["j"].as_u64().expect("j") as u32,
            );
            let placed = geom.forward(i, j);
            // A null in the golden is PROJ saying the pixel looks past the
            // limb. Agreeing about which pixels are not places on Earth is part
            // of the claim: a geometry that invented a coordinate there would
            // paint space.
            let Some(want_lat) = pt["lat"].as_f64() else {
                assert!(
                    placed.is_none(),
                    "{name}: grid point ({i}, {j}) is off the Earth for PROJ but \
                     the geometry placed it at {placed:?}"
                );
                off_disc += 1;
                checked += 1;
                continue;
            };
            let (lat, lon) =
                placed.unwrap_or_else(|| panic!("{name}: no position for grid point ({i}, {j})"));
            worst_lat = worst_lat.max((lat - want_lat).abs());
            worst_lon = worst_lon.max(lon_delta(lon, pt["lon"].as_f64().expect("lon")).abs());
            checked += 1;
        }
        assert!(
            worst_lat < TOL_DEG && worst_lon < TOL_DEG,
            "{name}: worst disagreement with PROJ {worst_lat:e} deg lat, \
             {worst_lon:e} deg lon (tolerance {TOL_DEG:e})",
        );
        println!("{name}: {worst_lat:e} deg lat, {worst_lon:e} deg lon, {off_disc} off-disc");
    }
    assert_eq!(
        families,
        CRS_FAMILIES.iter().map(|s| s.to_string()).collect(),
        "the golden must cover every family that names a CRS"
    );
    assert!(checked > 500, "expected the full golden, checked {checked}");
}
