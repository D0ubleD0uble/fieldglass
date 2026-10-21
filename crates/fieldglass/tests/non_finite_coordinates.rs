//! A coordinate that holds NaN or infinity places nothing (#1172).
//!
//! CF requires a coordinate variable to be strictly monotonic with no missing
//! values (CF §1.3, after the NUG), and a NaN or an infinity is neither. Before
//! #1172 an all-NaN latitude came back as a lat/lon placement with NaN corners,
//! so `decode_slice` handed a host a field with a NaN georef to warp, and a NaN
//! in the middle of a latitude was placed from its finite ends as if the axis
//! were whole. The 1-D path already refused a *masked* value for the same
//! reason, so a non-finite one is now refused the same way: `decode`, naming
//! the coordinate, the value and where it is.
//!
//! Through `Session`, over a committed NetCDF file and over Zarr stores built
//! here, so both containers reach the one placement in `core`. The browser and
//! Node hosts are held to the NetCDF refusal by the conformance suite's
//! `non_finite_coordinates/*` cases.

use fieldglass::{DecodeOptions, Error, MemoryObjects, Session, WindowRequest};

const NETCDF: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/non_finite_coordinates.nc");

fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no variable {name}"))
        .index
}

/// Both ways a host asks where a slice is: decoding it and placing it.
fn refusals(session: &Session, name: &str) -> [Error; 2] {
    let v = variable(session, name);
    [
        session
            .decode_slice(v, 0, 1, &[0, 0], &DecodeOptions::default())
            .expect_err("a non-finite coordinate places nothing"),
        session
            .place_slice(v, 0, 1)
            .map(|_| ())
            .expect_err("a non-finite coordinate places nothing"),
    ]
}

fn assert_refused(session: &Session, name: &str, says: &[&str]) {
    for err in refusals(session, name) {
        assert_eq!(err.code(), "decode", "{name}: {err}");
        let message = err.message();
        for s in says {
            assert!(message.contains(s), "{name}: {s:?} is not in {message:?}");
        }
    }
}

#[test]
fn a_netcdf_coordinate_that_is_not_finite_is_refused_naming_it() {
    let session = Session::open(NETCDF.to_vec()).expect("opens");
    assert_refused(&session, "all_nan", &["`lat_nan`", "NaN", "index 0"]);
    // Finite ends, which are all the corners read: the hole in the middle is
    // what has to be found.
    assert_refused(&session, "hole", &["`lat_hole`", "NaN", "index 1"]);
    assert_refused(&session, "infinite_end", &["`lon_inf`", "inf", "index 3"]);
    // A value at the coordinate's own `_FillValue`, masked as it is read: the
    // refusal a masked value always had, now in the same words.
    assert_refused(
        &session,
        "masked",
        &["`lat_fill`", "a fill value", "index 1"],
    );

    // The control: the same field on finite coordinates is placed.
    let placed = variable(&session, "placed");
    let field = session
        .decode_slice(placed, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("finite coordinates place");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
}

// ---- Zarr, built in memory ---------------------------------------------------

const V3_GROUP: &[u8] = b"{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{}}";

/// A float64 v3 array whose fill value is NaN, as xarray writes coordinates.
fn v3_array(shape: &str, dims: &str, attributes: &str) -> Vec<u8> {
    format!(
        "{{\"zarr_format\":3,\"node_type\":\"array\",\"shape\":{shape},\"data_type\":\"float64\",\
         \"chunk_grid\":{{\"name\":\"regular\",\"configuration\":{{\"chunk_shape\":{shape}}}}},\
         \"chunk_key_encoding\":{{\"name\":\"default\",\"configuration\":{{\"separator\":\"/\"}}}},\
         \"fill_value\":\"NaN\",\"codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}}],\
         \"dimension_names\":{dims},\"attributes\":{attributes}}}"
    )
    .into_bytes()
}

fn f8(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// `t(lat, lon)`, 3 × 4, with these latitudes. `None` leaves `lat/c/0` out of
/// the store, so every latitude is the array's NaN fill value: what a store
/// holds when the writer skipped a chunk equal to its fill.
fn store(latitudes: Option<&[f64]>) -> Session {
    let mut objects = vec![
        ("zarr.json".to_string(), V3_GROUP.to_vec()),
        (
            "lat/zarr.json".to_string(),
            v3_array("[3]", "[\"lat\"]", "{\"units\":\"degrees_north\"}"),
        ),
        (
            "lon/zarr.json".to_string(),
            v3_array("[4]", "[\"lon\"]", "{\"units\":\"degrees_east\"}"),
        ),
        ("lon/c/0".to_string(), f8(&[0.0, 90.0, 180.0, 270.0])),
        (
            "t/zarr.json".to_string(),
            v3_array("[3,4]", "[\"lat\",\"lon\"]", "{}"),
        ),
        (
            "t/c/0/0".to_string(),
            f8(&(0..12).map(f64::from).collect::<Vec<_>>()),
        ),
    ];
    if let Some(lat) = latitudes {
        objects.push(("lat/c/0".to_string(), f8(lat)));
    }
    Session::open_store(MemoryObjects::from_iter(objects)).expect("opens")
}

#[test]
fn a_zarr_latitude_the_store_never_wrote_is_refused_when_its_fill_is_nan() {
    assert_refused(&store(None), "t", &["`lat`", "NaN", "index 0"]);
}

#[test]
fn a_nan_or_infinity_anywhere_in_a_zarr_coordinate_is_refused() {
    for (lat, at) in [
        ([f64::NAN, 20.0, 30.0], "index 0"),
        ([10.0, f64::NAN, 30.0], "index 1"),
        ([10.0, 20.0, f64::NEG_INFINITY], "index 2"),
    ] {
        assert_refused(&store(Some(&lat)), "t", &["`lat`", at]);
    }
    // Finite, the same store places.
    let session = store(Some(&[10.0, 20.0, 30.0]));
    let field = session
        .decode_slice(0, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
}

#[test]
fn a_window_over_a_latitude_the_store_never_wrote_names_the_chunk() {
    // #1163's refusal holds now that placement refuses first: `unsupported`,
    // naming the chunk whose fill value is the cause, and why placement failed.
    let session = store(None);
    let err = session
        .plan_window(&WindowRequest::new(
            0,
            0,
            1,
            [0.0, 40.0, 0.0, 180.0],
            vec![0, 0],
        ))
        .expect_err("no latitudes");
    assert_eq!(err.code(), "unsupported", "{err}");
    let message = err.message();
    assert!(
        message.contains("`lat/c/0`") && message.contains("fill value") && message.contains("NaN"),
        "{message}"
    );
}

#[test]
fn a_window_over_finite_ends_whose_centres_are_not_finite_is_refused() {
    // Every latitude is a finite number, so the slice places, but the step
    // between ends of ±1e308 overflows: the centres the planner matches a box
    // against are not numbers, and it says so rather than planning nothing.
    let session = store(Some(&[-1e308, 0.0, 1e308]));
    session
        .place_slice(0, 0, 1)
        .expect("finite coordinates place");
    let err = session
        .plan_window(&WindowRequest::new(
            0,
            0,
            1,
            [0.0, 40.0, 0.0, 180.0],
            vec![0, 0],
        ))
        .expect_err("no centre is a number");
    assert_eq!(err.code(), "unsupported", "{err}");
    assert!(err.message().contains("not all finite"), "{err}");
}

/// A CF `geostationary` slice, `t(y, x)` 3 × 4, on these scan angles.
fn geostationary(x: &[f64]) -> Session {
    let mapping = "{\"grid_mapping_name\":\"geostationary\",\
        \"perspective_point_height\":35786023.0,\"semi_major_axis\":6378137.0,\
        \"semi_minor_axis\":6356752.31414,\"longitude_of_projection_origin\":-75.0,\
        \"sweep_angle_axis\":\"x\"}";
    Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", V3_GROUP.to_vec()),
        ("proj/zarr.json", v3_array("[1]", "[\"one\"]", mapping)),
        ("proj/c/0", f8(&[0.0])),
        (
            "x/zarr.json",
            v3_array("[4]", "[\"x\"]", "{\"units\":\"rad\"}"),
        ),
        ("x/c/0", f8(x)),
        (
            "y/zarr.json",
            v3_array("[3]", "[\"y\"]", "{\"units\":\"rad\"}"),
        ),
        ("y/c/0", f8(&[0.05, 0.0, -0.05])),
        (
            "t/zarr.json",
            v3_array("[3,4]", "[\"y\",\"x\"]", "{\"grid_mapping\":\"proj\"}"),
        ),
        ("t/c/0/0", f8(&(0..12).map(f64::from).collect::<Vec<_>>())),
    ]))
    .expect("opens")
}

#[test]
fn a_geostationary_scan_angle_that_is_not_finite_is_refused() {
    // The scan angles are a 1-D coordinate like any other, read the same way.
    let session = geostationary(&[-0.05, -0.02, 0.02, 0.05]);
    let t = variable(&session, "t");
    let field = session
        .decode_slice(t, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("places");
    assert_eq!(field.georef.kind, "space_view", "{:?}", field.georef);

    assert_refused(
        &geostationary(&[-0.05, f64::NAN, 0.02, 0.05]),
        "t",
        &["`x`", "NaN", "index 1"],
    );
}

/// A WRF Lambert domain, 3 × 4: `t(south_north, west_east)` beside the 2-D
/// `XLAT` / `XLONG` whose `(0, 0)` cell is the grid's origin.
fn wrf(xlat_origin: f64) -> Session {
    let group = b"{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{\
        \"MAP_PROJ\":1,\"TRUELAT1\":30.0,\"TRUELAT2\":60.0,\"STAND_LON\":-98.0,\
        \"DX\":12000.0,\"DY\":12000.0}}"
        .to_vec();
    let dims = "[\"south_north\",\"west_east\"]";
    let mut xlat: Vec<f64> = (0..12).map(|k| 35.0 + f64::from(k / 4) * 0.1).collect();
    xlat[0] = xlat_origin;
    let xlong: Vec<f64> = (0..12).map(|k| -100.0 + f64::from(k % 4) * 0.1).collect();
    Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", group),
        ("XLAT/zarr.json", v3_array("[3,4]", dims, "{}")),
        ("XLAT/c/0/0", f8(&xlat)),
        ("XLONG/zarr.json", v3_array("[3,4]", dims, "{}")),
        ("XLONG/c/0/0", f8(&xlong)),
        ("t/zarr.json", v3_array("[3,4]", dims, "{}")),
        ("t/c/0/0", f8(&(0..12).map(f64::from).collect::<Vec<_>>())),
    ]))
    .expect("opens")
}

#[test]
fn a_wrf_origin_that_is_not_finite_is_refused_like_a_masked_one() {
    // Finite, the domain is placed on Lambert from that corner.
    let session = wrf(35.0);
    let t = variable(&session, "t");
    let field = session
        .decode_slice(t, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("places");
    assert_eq!(field.georef.kind, "lambert", "{:?}", field.georef);

    for origin in [f64::NAN, f64::INFINITY] {
        assert_refused(&wrf(origin), "t", &["XLAT[0]"]);
    }
}
