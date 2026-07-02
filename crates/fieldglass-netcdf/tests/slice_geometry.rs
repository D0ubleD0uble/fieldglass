//! Tests for NetCDF 2-D slice geometry (decision 0002): CF axis detection,
//! renderable-variable selection, corner / regularity derivation, and strided
//! plane extraction. The committed ERSST classic fixture anchors the
//! end-to-end view assertions; the unit cases pin the CF-conventions table.

use fieldglass_netcdf::geometry::{
    AxisKind, DatasetView, DimView, VarView, corner_and_regularity, detect_axis, extract_plane,
    synthesize_geometry,
};
use fieldglass_netcdf::{ClassicHeader, NcType, NetcdfBacking, NetcdfReader};

const ERSST_CDF1: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");

fn classic_header(reader: &NetcdfReader) -> &ClassicHeader {
    match &reader.backing {
        NetcdfBacking::Classic(h) => h,
        NetcdfBacking::Hdf5(_) => panic!("expected classic backing"),
    }
}

fn coord_var(name: &str, units: &str) -> VarView {
    VarView {
        decode_index: 0,
        name: name.to_string(),
        nc_type: NcType::Double,
        dim_names: vec![name.to_string()],
        attrs: vec![("units".to_string(), units.to_string())],
    }
}

// ---------------------------------------------------------------------------
// Axis detection — the CF units / standard_name / axis / name-heuristic table.
// ---------------------------------------------------------------------------

#[test]
fn detects_latitude_units_spellings() {
    for units in [
        "degrees_north",
        "degree_north",
        "degrees_N",
        "degree_N",
        "degreesN",
        "degreeN",
        "degrees north",
    ] {
        assert_eq!(
            detect_axis(&coord_var("y", units)),
            Some(AxisKind::Latitude),
            "units {units:?} should read as latitude"
        );
    }
}

#[test]
fn detects_longitude_units_spellings() {
    for units in [
        "degrees_east",
        "degree_east",
        "degrees_E",
        "degree_E",
        "degreesE",
        "degreeE",
    ] {
        assert_eq!(
            detect_axis(&coord_var("x", units)),
            Some(AxisKind::Longitude),
            "units {units:?} should read as longitude"
        );
    }
}

#[test]
fn non_horizontal_units_are_not_an_axis() {
    // A plain "degrees" (temperature, angle) or a vertical/time unit must not
    // be mistaken for a horizontal axis.
    assert_eq!(detect_axis(&coord_var("t2m", "degrees")), None);
    assert_eq!(detect_axis(&coord_var("lev", "hPa")), None);
    assert_eq!(
        detect_axis(&coord_var("time", "hours since 1900-01-01")),
        None
    );
}

#[test]
fn standard_name_and_axis_attrs_classify() {
    let by_std = VarView {
        attrs: vec![("standard_name".to_string(), "latitude".to_string())],
        ..coord_var("rlat", "1")
    };
    assert_eq!(detect_axis(&by_std), Some(AxisKind::Latitude));

    let by_axis = VarView {
        attrs: vec![("axis".to_string(), "X".to_string())],
        ..coord_var("rlon", "1")
    };
    assert_eq!(detect_axis(&by_axis), Some(AxisKind::Longitude));
}

#[test]
fn units_win_over_a_misleading_name() {
    // Priority: a real CF unit beats the name heuristic.
    let v = coord_var("x", "degrees_north");
    assert_eq!(detect_axis(&v), Some(AxisKind::Latitude));
}

#[test]
fn name_heuristic_is_the_last_resort() {
    let no_attrs = VarView {
        attrs: vec![],
        ..coord_var("latitude", "")
    };
    assert_eq!(detect_axis(&no_attrs), Some(AxisKind::Latitude));
    let lon = VarView {
        attrs: vec![],
        ..coord_var("lon", "")
    };
    assert_eq!(detect_axis(&lon), Some(AxisKind::Longitude));
}

// ---------------------------------------------------------------------------
// Corner / regularity.
// ---------------------------------------------------------------------------

#[test]
fn regular_axis_reports_corners_and_uniform_spacing() {
    let lat = [90.0, 89.0, 88.0, 87.0];
    let (first, last, regular) = corner_and_regularity(&lat).unwrap();
    assert_eq!((first, last), (90.0, 87.0));
    assert!(regular, "descending uniform axis is regular");
}

#[test]
fn irregular_axis_is_flagged() {
    // A Gaussian-like latitude row: non-uniform deltas.
    let lat = [88.0, 80.0, 60.0, 30.0, 0.0];
    let (_, _, regular) = corner_and_regularity(&lat).unwrap();
    assert!(!regular, "non-uniform spacing must be flagged irregular");
}

#[test]
fn short_and_empty_axes_are_handled() {
    assert_eq!(corner_and_regularity(&[5.0]), Some((5.0, 5.0, true)));
    assert_eq!(corner_and_regularity(&[1.0, 2.0]), Some((1.0, 2.0, true)));
    assert_eq!(corner_and_regularity(&[]), None);
}

#[test]
fn synthesize_geometry_maps_corners_and_dims() {
    let lat = [90.0, 60.0, 30.0, 0.0]; // descending, regular
    let lon = [0.0, 90.0, 180.0, 270.0];
    let geom = synthesize_geometry(&lat, &lon).unwrap();
    assert_eq!((geom.ni, geom.nj), (4, 4));
    assert_eq!((geom.lat_first, geom.lat_last), (90.0, 0.0));
    assert_eq!((geom.lon_first, geom.lon_last), (0.0, 270.0));
    assert!(!geom.irregular);
    // A descending latitude axis is the common north-to-south layout; only a
    // descending *longitude* axis raises the flag.
    assert!(!geom.lon_descending);
}

#[test]
fn synthesize_geometry_flags_descending_longitude() {
    let lat = [0.0, 30.0, 60.0, 90.0];
    // Monotonically decreasing (east-to-west) — the west-to-east inverse map
    // would misread this as an antimeridian wrap, so it must be flagged.
    let lon = [270.0, 180.0, 90.0, 0.0];
    let geom = synthesize_geometry(&lat, &lon).unwrap();
    assert!(geom.lon_descending);
    // A wrapped-storage axis that jumps back across 0° is ascending storage,
    // not a descending scan — its corner pair (180, 90) is a true wrap.
    let wrapped = [180.0, 270.0, 0.0, 90.0];
    let geom = synthesize_geometry(&lat, &wrapped).unwrap();
    assert!(!geom.lon_descending);
}

// ---------------------------------------------------------------------------
// Plane extraction.
// ---------------------------------------------------------------------------

#[test]
fn extract_plane_pulls_the_right_slice_from_a_4d_buffer() {
    // shape time=2, lev=2, lat=2, lon=3 → 24 values, value = flat C-order index.
    let shape = [2u64, 2, 2, 3];
    let values: Vec<Option<f64>> = (0..24).map(|i| Some(i as f64)).collect();
    // Hold time=1, lev=0; image axes lat(=2), lon(=3).
    let plane = extract_plane(&values, &shape, 2, 3, &[1, 0, 0, 0]).unwrap();
    // Base offset = time*12 + lev*6 = 12. Row-major lat×lon = [12,13,14, 15,16,17].
    assert_eq!(
        plane,
        vec![12, 13, 14, 15, 16, 17]
            .into_iter()
            .map(|v| Some(v as f64))
            .collect::<Vec<_>>()
    );
}

#[test]
fn extract_plane_transposes_when_x_precedes_y() {
    // shape a=2 (lon), b=3 (lat); assign y_dim=1(b), x_dim=0(a) → 3 rows × 2 cols.
    let shape = [2u64, 3];
    let values: Vec<Option<f64>> = (0..6).map(|i| Some(i as f64)).collect();
    // C-order: index = a*3 + b. Row j over b, col i over a → value = i*3 + j.
    let plane = extract_plane(&values, &shape, 1, 0, &[0, 0]).unwrap();
    assert_eq!(
        plane,
        vec![0.0, 3.0, 1.0, 4.0, 2.0, 5.0]
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>()
    );
}

#[test]
fn extract_plane_rejects_bad_assignment_and_oob_index() {
    let shape = [2u64, 3];
    let values: Vec<Option<f64>> = (0..6).map(|i| Some(i as f64)).collect();
    assert!(extract_plane(&values, &shape, 0, 0, &[0, 0]).is_err());
    assert!(extract_plane(&values, &shape, 0, 5, &[0, 0]).is_err());
    assert!(extract_plane(&values, &shape, 0, 1, &[0]).is_err());
    // Holding a non-image dim out of range. shape lev=2,lat=1,lon=1; hold lev=9.
    let shape3 = [2u64, 1, 1];
    let v3: Vec<Option<f64>> = (0..2).map(|i| Some(i as f64)).collect();
    assert!(extract_plane(&v3, &shape3, 1, 2, &[9, 0, 0]).is_err());
}

// ---------------------------------------------------------------------------
// End-to-end view over the committed ERSST classic fixture (time×lev×lat×lon).
// ---------------------------------------------------------------------------

#[test]
fn ersst_view_detects_axes_and_lists_sst_as_renderable() {
    let reader = NetcdfReader::from_bytes(ERSST_CDF1.to_vec()).unwrap();
    let view = DatasetView::from_classic(classic_header(&reader));

    let renderable = view.renderable_variables();
    let sst = renderable
        .iter()
        .find(|v| v.name == "sst")
        .expect("sst should be renderable");
    // sst is time × lev × lat × lon, so lat is axis 2 and lon axis 3.
    assert_eq!(sst.detected_y_dim, Some(2));
    assert_eq!(sst.detected_x_dim, Some(3));
    assert_eq!(sst.dims.len(), 4);

    // Coordinate variables (lat, lon) are 1-D and excluded from the render list.
    assert!(!renderable.iter().any(|v| v.name == "lat"));
    assert!(!renderable.iter().any(|v| v.name == "lon"));

    // Both horizontal coordinate variables resolve to a decode index.
    assert!(view.coordinate_index("lat").is_some());
    assert!(view.coordinate_index("lon").is_some());
}

#[test]
fn ersst_geometry_matches_the_coordinate_arrays() {
    let reader = NetcdfReader::from_bytes(ERSST_CDF1.to_vec()).unwrap();
    let view = DatasetView::from_classic(classic_header(&reader));

    let decode = |dim: &str| -> Vec<f64> {
        let idx = view.coordinate_index(dim).unwrap();
        reader
            .decode_variable_values(idx)
            .unwrap()
            .into_iter()
            .map(|v| v.expect("coordinate values are never masked"))
            .collect()
    };
    let lat = decode("lat");
    let lon = decode("lon");
    let geom = synthesize_geometry(&lat, &lon).unwrap();

    // ERSST is a regular 2°×2° grid: lat 89 rows, lon 180 columns.
    assert_eq!((geom.nj, geom.ni), (89, 180));
    assert_eq!(geom.lat_first, *lat.first().unwrap());
    assert_eq!(geom.lat_last, *lat.last().unwrap());
    assert_eq!(geom.lon_first, *lon.first().unwrap());
    assert_eq!(geom.lon_last, *lon.last().unwrap());
    assert!(!geom.irregular, "ERSST is a regular grid");
}

#[test]
fn ersst_dimview_lengths_resolve() {
    let reader = NetcdfReader::from_bytes(ERSST_CDF1.to_vec()).unwrap();
    let view = DatasetView::from_classic(classic_header(&reader));
    let by_name: std::collections::HashMap<&str, &DimView> =
        view.dims.iter().map(|d| (d.name.as_str(), d)).collect();
    assert_eq!(by_name["lat"].length, 89);
    assert_eq!(by_name["lon"].length, 180);
}
