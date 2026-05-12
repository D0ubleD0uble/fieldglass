//! Integration coverage for §3 GDS template parsing across the three
//! fixtures shipped with the crate.

use fieldglass_grib2::{Grib2Reader, GridTemplate, lookup_earth_shape, lookup_grid_template};

const GFS_LATLON: &[u8] = include_bytes!("fixtures/gfs_c255_latlon.grib2");
const ETA_LAMBERT: &[u8] = include_bytes!("fixtures/eta_lambert_msg0.grib2");
const ECMWF_GAUSSIAN: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn gfs_latlon_decodes_template_3_0() {
    let reader = Grib2Reader::from_bytes(GFS_LATLON.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 0);
    assert_eq!(msg.gds.num_data_points, 144 * 73);

    let t = match msg.gds.template {
        GridTemplate::LatLon(t) => t,
        other => panic!("expected LatLon, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!(t.ni, 144);
    assert_eq!(t.nj, 73);
    assert!((t.la1 - 90.0).abs() < 1e-9);
    assert!((t.lo1 - 0.0).abs() < 1e-9);
    assert!((t.la2 - (-90.0)).abs() < 1e-9);
    assert!((t.lo2 - 357.5).abs() < 1e-9);
    assert_eq!(t.di, Some(2.5));
    assert_eq!(t.dj, Some(2.5));

    assert_eq!(msg.gds.dimensions(), Some((144, 73)));
    assert_eq!(msg.gds.template_name(), "latlon");
    assert_eq!(lookup_grid_template(0), "Latitude/longitude");
    assert_eq!(
        lookup_earth_shape(t.shape_of_earth),
        "Spherical (radius 6 371 229.0 m)"
    );
}

#[test]
fn eta_lambert_decodes_template_3_30() {
    let reader = Grib2Reader::from_bytes(ETA_LAMBERT.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 30);
    assert_eq!(msg.gds.num_data_points, 93 * 65);

    let t = match msg.gds.template {
        GridTemplate::Lambert(t) => t,
        other => panic!("expected Lambert, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!(t.nx, 93);
    assert_eq!(t.ny, 65);
    assert!((t.la1 - 12.190).abs() < 1e-3, "la1={}", t.la1);
    assert!((t.lo1 - 226.541).abs() < 1e-3, "lo1={}", t.lo1);
    assert!((t.lad - 25.0).abs() < 1e-9);
    assert!((t.lov - 265.0).abs() < 1e-9);
    // Eta operational 80 km tangent-Lambert grid.
    assert!((t.dx_metres - 81271.0).abs() < 1.0, "dx={}", t.dx_metres);
    assert!((t.dy_metres - 81271.0).abs() < 1.0);
    assert!((t.latin1 - 25.0).abs() < 1e-9);
    assert!((t.latin2 - 25.0).abs() < 1e-9);

    assert_eq!(msg.gds.dimensions(), Some((93, 65)));
    assert_eq!(msg.gds.template_name(), "lambert");
    assert_eq!(lookup_grid_template(30), "Lambert conformal");
}

#[test]
fn ecmwf_gaussian_decodes_template_3_40_reduced() {
    let reader = Grib2Reader::from_bytes(ECMWF_GAUSSIAN.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 40);
    assert_eq!(msg.gds.num_data_points, 6114);
    // Reduced grid: optional list carries one entry per parallel.
    assert_eq!(msg.gds.optional_list_octet_size, 2);
    assert_eq!(msg.gds.optional_list_interp, 1);

    let t = match msg.gds.template {
        GridTemplate::Gaussian(t) => t,
        other => panic!("expected Gaussian, got {other:?}"),
    };
    assert!(t.is_reduced, "fixture is a reduced Gaussian");
    assert_eq!(t.ni, None, "reduced grids have no constant Ni");
    assert_eq!(t.nj, 64);
    assert_eq!(t.di, None, "reduced grids have no constant Di");
    assert_eq!(t.n_parallels, 32);
    // N32 reduced Gaussian — first/last parallel pair is symmetric ~±87.864°.
    assert!((t.la1 - 87.8638).abs() < 1e-3, "la1={}", t.la1);
    assert!((t.la2 - (-87.8638)).abs() < 1e-3);

    // Reduced grids cannot report dimensions but bounds remain meaningful.
    assert_eq!(msg.gds.dimensions(), None);
    assert!(msg.gds.bounds().is_some());
    assert_eq!(msg.gds.template_name(), "gaussian");
    assert_eq!(lookup_grid_template(40), "Gaussian latitude/longitude");
}
