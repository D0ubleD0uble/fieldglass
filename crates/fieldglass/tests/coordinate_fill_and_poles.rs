//! A coordinate's own fill value is no position, a Zarr v2 `fill_value` is no
//! statement that one is missing, and a pole stored a hair past 90 is the
//! pole (#1305).
//!
//! A NetCDF reader masks a coordinate's `_FillValue` and `missing_value` as
//! it reads, so placement refused a 1-D axis holding one. A Zarr store reads
//! them as numbers, so a latitude stored at its own `_FillValue` was placed
//! as a position, and a swath cell at -999 was placed at latitude -999.
//! Placement now masks by the fill values a coordinate states, whichever
//! container read it. It does not mask by a v2 `fill_value` the store only
//! presents as `_FillValue`: zarr-python writes one for every array, 0 by
//! default, and 0° is a real longitude, which the reverted attempt in #1172
//! masked.
//!
//! A latitude within 0.001° past a pole is placed at the pole, so the row is
//! inside a box that runs up to it. One further out is still not placed
//! (#1719), and a window over it now names the values.
//!
//! Every case is a Zarr store built here in memory and opened through
//! `Session`, the path both hosts take.

use fieldglass::{DecodeOptions, Error, Field, MemoryObjects, Session, WindowRequest};

const V3_GROUP: &[u8] = b"{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{}}";
const NORTH: &str = "{\"units\":\"degrees_north\"}";
const EAST: &str = "{\"units\":\"degrees_east\"}";

/// A v3 array of `data_type` with its own chunk shape and fill value (JSON).
fn v3(data_type: &str, shape: &str, chunks: &str, fill: &str, dims: &str, attrs: &str) -> Vec<u8> {
    format!(
        "{{\"zarr_format\":3,\"node_type\":\"array\",\"shape\":{shape},\"data_type\":\"{data_type}\",\
         \"chunk_grid\":{{\"name\":\"regular\",\"configuration\":{{\"chunk_shape\":{chunks}}}}},\
         \"chunk_key_encoding\":{{\"name\":\"default\",\"configuration\":{{\"separator\":\"/\"}}}},\
         \"fill_value\":{fill},\"codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}}],\
         \"dimension_names\":{dims},\"attributes\":{attrs}}}"
    )
    .into_bytes()
}

/// A float64 v3 array in one chunk, filled with NaN.
fn f64_array(shape: &str, dims: &str, attrs: &str) -> Vec<u8> {
    v3("float64", shape, shape, "\"NaN\"", dims, attrs)
}

fn f8(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn f4(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn grid() -> Vec<u8> {
    f8(&(0..12).map(f64::from).collect::<Vec<_>>())
}

/// `t(lat, lon)`, 3 × 4, over a `lat` described by `lat_meta` and stored as
/// `lat_chunks`, and the longitudes 0, 90, 180 and 270.
fn store(lat_meta: Vec<u8>, lat_chunks: Vec<(&str, Vec<u8>)>) -> Session {
    let mut objects = vec![
        ("zarr.json".to_string(), V3_GROUP.to_vec()),
        ("lat/zarr.json".to_string(), lat_meta),
        (
            "lon/zarr.json".to_string(),
            f64_array("[4]", "[\"lon\"]", EAST),
        ),
        ("lon/c/0".to_string(), f8(&[0.0, 90.0, 180.0, 270.0])),
        (
            "t/zarr.json".to_string(),
            f64_array("[3,4]", "[\"lat\",\"lon\"]", "{}"),
        ),
        ("t/c/0/0".to_string(), grid()),
    ];
    objects.extend(lat_chunks.into_iter().map(|(k, v)| (k.to_string(), v)));
    Session::open_store(MemoryObjects::from_iter(objects)).expect("opens")
}

/// The same store with its latitudes in one float64 chunk.
fn latitudes(lat: &[f64], attrs: &str) -> Session {
    store(
        f64_array("[3]", "[\"lat\"]", attrs),
        vec![("lat/c/0", f8(lat))],
    )
}

fn decode(session: &Session) -> Result<Field, Error> {
    session.decode_slice(0, 0, 1, &[0, 0], &DecodeOptions::default())
}

/// A window over the whole globe.
fn plan_world(session: &Session) -> Result<fieldglass::WindowPlan, Error> {
    session.plan_window(&WindowRequest::new(
        0,
        0,
        1,
        [-90.0, 90.0, -180.0, 360.0],
        vec![0, 0],
    ))
}

// ---- Fill values ------------------------------------------------------------

#[test]
fn a_latitude_at_the_fill_value_the_coordinate_states_is_refused() {
    // 20 is a latitude, and here it is the one the coordinate calls missing:
    // CF gives a coordinate no missing values, so the axis has a hole, and
    // the slice is refused as a NetCDF file's is. Master placed it from 10
    // to 30.
    for attrs in [
        "{\"units\":\"degrees_north\",\"_FillValue\":20.0}",
        "{\"units\":\"degrees_north\",\"missing_value\":[-1.0,20.0]}",
        // xarray's v3 spelling: the float's little-endian bytes, base64.
        "{\"units\":\"degrees_north\",\"_FillValue\":\"AAAAAAAANEA=\"}",
    ] {
        let session = latitudes(&[10.0, 20.0, 30.0], attrs);
        for err in [
            decode(&session).expect_err("a hole in the axis"),
            plan_world(&session).expect_err("a hole in the axis"),
        ] {
            assert_eq!(err.code(), "decode", "{attrs}: {err}");
            let message = err.message();
            assert!(
                message.contains("\"lat\"")
                    && message.contains("a fill value")
                    && message.contains("index 1"),
                "{attrs}: {message}"
            );
        }
    }
    // The control: without the attribute the same values place.
    let field = decode(&latitudes(&[10.0, 20.0, 30.0], NORTH)).expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
}

#[test]
fn a_latitude_at_a_stated_fill_of_minus_999_is_refused_not_dropped_from_the_map() {
    // -999 is past the poles too, so on master the slice was drawn without a
    // map and nothing said why. It is the coordinate's own fill value, so the
    // refusal names the hole.
    let session = latitudes(
        &[10.0, -999.0, 30.0],
        "{\"units\":\"degrees_north\",\"_FillValue\":-999.0}",
    );
    let err = decode(&session).expect_err("a hole in the axis");
    assert_eq!(err.code(), "decode", "{err}");
    assert!(err.message().contains("a fill value at index 1"), "{err}");
}

/// A Zarr v2 array document, with no `_FillValue` among its attributes.
fn v2_array(shape: &str, fill: &str) -> Vec<u8> {
    format!(
        "{{\"zarr_format\":2,\"shape\":{shape},\"chunks\":{shape},\"dtype\":\"<f8\",\
         \"fill_value\":{fill},\"compressor\":null,\"filters\":null,\"order\":\"C\"}}"
    )
    .into_bytes()
}

fn v2_attrs(dims: &str, rest: &str) -> Vec<u8> {
    format!("{{\"_ARRAY_DIMENSIONS\":{dims}{rest}}}").into_bytes()
}

/// `t(lat, lon)` as a v2 store whose every array has `fill_value: 0`, as
/// zarr-python writes by default, over the longitudes 0, 90, 180 and 270 and
/// the latitudes 0, 10 and 20; `lon_attrs` is added to `lon`'s attributes.
fn v2_store(lon_attrs: &str) -> Session {
    Session::open_store(MemoryObjects::from_iter([
        (".zgroup", b"{\"zarr_format\":2}".to_vec()),
        ("lat/.zarray", v2_array("[3]", "0")),
        (
            "lat/.zattrs",
            v2_attrs("[\"lat\"]", ",\"units\":\"degrees_north\""),
        ),
        ("lat/0", f8(&[0.0, 10.0, 20.0])),
        ("lon/.zarray", v2_array("[4]", "0")),
        (
            "lon/.zattrs",
            v2_attrs(
                "[\"lon\"]",
                &format!(",\"units\":\"degrees_east\"{lon_attrs}"),
            ),
        ),
        ("lon/0", f8(&[0.0, 90.0, 180.0, 270.0])),
        ("t/.zarray", v2_array("[3,4]", "0")),
        ("t/.zattrs", v2_attrs("[\"lat\",\"lon\"]", "")),
        ("t/0.0", grid()),
    ]))
    .expect("opens")
}

#[test]
fn a_v2_fill_value_of_zero_masks_no_coordinate() {
    // The store presents each `fill_value: 0` as a `_FillValue`, which is how
    // xarray reads a data array. On a coordinate it is no statement: the
    // equator and the prime meridian are real, and the slice places.
    let session = v2_store("");
    let field = decode(&session).expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
    assert_eq!(
        field.georef.corners,
        Some([0.0, 0.0, 20.0, 270.0]),
        "{:?}",
        field.georef
    );
    let plan = plan_world(&session).expect("plans");
    assert_eq!(plan.rows, [0, 3]);
    assert_eq!(plan.columns, vec![[0, 4]]);

    // A v2 array that states its own `_FillValue` among its attributes is
    // masked by it like any other.
    let err = decode(&v2_store(",\"_FillValue\":0.0")).expect_err("lon 0 is missing");
    assert_eq!(err.code(), "decode", "{err}");
    assert!(
        err.message().contains("\"lon\"") && err.message().contains("a fill value at index 0"),
        "{err}"
    );
}

#[test]
fn an_unwritten_coordinate_chunk_whose_fill_is_no_latitude_is_not_placed() {
    // `lat` is chunked by one, and the store never wrote `lat/c/0`, whose
    // fill value is -999: no latitude. The slice is not placed, and a window
    // names both the chunk and the values it gave.
    let session = store(
        v3("float64", "[3]", "[1]", "-999.0", "[\"lat\"]", NORTH),
        vec![("lat/c/1", f8(&[20.0])), ("lat/c/2", f8(&[30.0]))],
    );
    let field = decode(&session).expect("decodes");
    assert_eq!(field.georef.kind, "unsupported", "{:?}", field.georef);
    let err = plan_world(&session).expect_err("not placed");
    assert_eq!(err.code(), "unsupported", "{err}");
    let message = err.message();
    assert!(
        message.contains(r#""lat/c/0""#)
            && message.contains("latitudes from -999 to 30, past the poles"),
        "{message}"
    );
}

// ---- 2-D coordinates --------------------------------------------------------

/// `t(y, x)`, 2 × 2, placed by the 2-D `lat2d` and `lon2d` it names, which
/// state `attrs` beside their units.
fn swath(lat: [f64; 4], lon: [f64; 4], attrs: &str) -> Session {
    let dims = "[\"y\",\"x\"]";
    let lat_attrs = format!("{{\"units\":\"degrees_north\"{attrs}}}");
    let lon_attrs = format!("{{\"units\":\"degrees_east\"{attrs}}}");
    Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", V3_GROUP.to_vec()),
        ("lat2d/zarr.json", f64_array("[2,2]", dims, &lat_attrs)),
        ("lat2d/c/0/0", f8(&lat)),
        ("lon2d/zarr.json", f64_array("[2,2]", dims, &lon_attrs)),
        ("lon2d/c/0/0", f8(&lon)),
        (
            "t/zarr.json",
            f64_array("[2,2]", dims, "{\"coordinates\":\"lat2d lon2d\"}"),
        ),
        ("t/c/0/0", f8(&[1.0, 2.0, 3.0, 4.0])),
    ]))
    .expect("opens")
}

/// Where each of the slice's four cells is placed, `None` for a cell with no
/// position.
fn cells(session: &Session) -> Vec<Option<(f64, f64)>> {
    let placed = session.place_slice(0, 0, 1).expect("places");
    assert_eq!(placed.family(), "curvilinear");
    let forward = placed.geometry().forward_at();
    (0..2)
        .flat_map(|j| (0..2).map(move |i| (i, j)))
        .map(|(i, j)| forward(i, j))
        .collect()
}

#[test]
fn a_swath_cell_at_its_own_fill_value_has_no_position() {
    // The last cell is -999 in both arrays, their own `_FillValue`. On
    // master it was placed at latitude -999.
    let session = swath(
        [10.0, 10.0, 11.0, -999.0],
        [20.0, 21.0, 20.0, -999.0],
        ",\"_FillValue\":-999.0",
    );
    let placed = cells(&session);
    assert_eq!(placed[3], None, "{placed:?}");
    assert!(placed[..3].iter().all(Option::is_some), "{placed:?}");
}

/// A v2 swath as xarray writes it, whose coordinates keep their fill only in
/// `.zarray`'s `fill_value` and state no `_FillValue`
/// (`crates/fieldglass-zarr/tests/fixtures/stores/cf_v2/lat/.zarray`).
/// `t(y, x)`, 2 × 2, over `lat2d` and `lon2d`, both `fill_value` -999.
fn v2_swath(lat: [f64; 4], lon: [f64; 4]) -> Session {
    let dims = "[\"y\",\"x\"]";
    Session::open_store(MemoryObjects::from_iter([
        (".zgroup", b"{\"zarr_format\":2}".to_vec()),
        ("lat2d/.zarray", v2_array("[2,2]", "-999.0")),
        (
            "lat2d/.zattrs",
            v2_attrs(dims, ",\"units\":\"degrees_north\""),
        ),
        ("lat2d/0.0", f8(&lat)),
        ("lon2d/.zarray", v2_array("[2,2]", "-999.0")),
        (
            "lon2d/.zattrs",
            v2_attrs(dims, ",\"units\":\"degrees_east\""),
        ),
        ("lon2d/0.0", f8(&lon)),
        ("t/.zarray", v2_array("[2,2]", "\"NaN\"")),
        (
            "t/.zattrs",
            v2_attrs(dims, ",\"coordinates\":\"lat2d lon2d\""),
        ),
        ("t/0.0", f8(&[1.0, 2.0, 3.0, 4.0])),
    ]))
    .expect("opens")
}

/// A presented fill no latitude can hold is no position: the swath is placed
/// as 0.6.1 placed it, and the cell is not placed at latitude -999. A
/// presented fill that is a latitude, such as zarr-python's default 0, is a
/// position, as a v2 `fill_value` masks no real one (#1305).
#[test]
fn a_v2_swath_whose_fill_is_only_in_its_fill_value_is_placed() {
    let session = v2_swath([10.0, 10.0, 11.0, -999.0], [20.0, 21.0, 20.0, -999.0]);
    let placed = cells(&session);
    assert_eq!(placed[3], None, "{placed:?}");
    assert!(placed[..3].iter().all(Option::is_some), "{placed:?}");

    // The fill read as the latitude it is.
    let at_fill = Session::open_store(MemoryObjects::from_iter([
        (".zgroup", b"{\"zarr_format\":2}".to_vec()),
        ("lat2d/.zarray", v2_array("[2,2]", "0")),
        (
            "lat2d/.zattrs",
            v2_attrs("[\"y\",\"x\"]", ",\"units\":\"degrees_north\""),
        ),
        ("lat2d/0.0", f8(&[10.0, 10.0, 11.0, 0.0])),
        ("lon2d/.zarray", v2_array("[2,2]", "0")),
        (
            "lon2d/.zattrs",
            v2_attrs("[\"y\",\"x\"]", ",\"units\":\"degrees_east\""),
        ),
        ("lon2d/0.0", f8(&[20.0, 21.0, 20.0, 0.0])),
        ("t/.zarray", v2_array("[2,2]", "\"NaN\"")),
        (
            "t/.zattrs",
            v2_attrs("[\"y\",\"x\"]", ",\"coordinates\":\"lat2d lon2d\""),
        ),
        ("t/0.0", f8(&[1.0, 2.0, 3.0, 4.0])),
    ]))
    .expect("opens");
    let placed = cells(&at_fill);
    assert!(placed.iter().all(Option::is_some), "{placed:?}");
    let (lat, _) = placed[3].expect("placed");
    assert!(lat.abs() < 1e-9, "the fill is the equator: {placed:?}");

    // A latitude past the poles that is not the fill still refuses the pair.
    let past = v2_swath([10.0, 10.0, 11.0, -500.0], [20.0, 21.0, 20.0, 22.0]);
    let geometry = session_family(&past);
    assert_eq!(geometry, "source");
}

fn session_family(session: &Session) -> String {
    session
        .place_slice(0, 0, 1)
        .expect("resolves")
        .family()
        .to_string()
}

// ---- Latitudes at and past the poles -----------------------------------------

#[test]
fn a_pole_stored_a_hair_past_90_is_placed_at_90() {
    // 90.000003, a pole after a float32 round trip or a packed integer's
    // scale, is the pole. On master it was placed as stored, and a box up to
    // the pole left its row out.
    let session = latitudes(&[30.0, 60.0, 90.000_003], NORTH);
    let field = decode(&session).expect("places");
    assert_eq!(
        field.georef.corners,
        Some([30.0, 0.0, 90.0, 270.0]),
        "{:?}",
        field.georef
    );
    assert_eq!(plan_world(&session).expect("plans").rows, [0, 3]);

    // The south pole too, and a float32 pole one step past 90.
    let session = latitudes(&[-90.0005, -60.0, -30.0], NORTH);
    assert_eq!(
        decode(&session).expect("places").georef.corners,
        Some([-90.0, 0.0, -30.0, 270.0])
    );
    let past = f32::from_bits(90.0_f32.to_bits() + 1);
    let session = store(
        v3("float32", "[3]", "[3]", "\"NaN\"", "[\"lat\"]", NORTH),
        vec![("lat/c/0", f4(&[30.0, 60.0, past]))],
    );
    assert_eq!(
        decode(&session).expect("places").georef.corners,
        Some([30.0, 0.0, 90.0, 270.0])
    );
}

#[test]
fn a_latitude_past_the_poles_is_not_placed_and_a_window_names_it() {
    // Further out than 0.001° the axis is no latitude, and the slice is drawn
    // without a map as #1719 has it. A window cannot be planned over it, and
    // the refusal now says which values are to blame.
    for (lat, says) in [
        (
            [30.0, 60.0, 90.002],
            "latitudes from 30 to 90.002, past the poles",
        ),
        (
            [0.0, 100.0, 200.0],
            "latitudes from 0 to 200, past the poles",
        ),
        (
            [-1e308, 0.0, 1e308],
            "latitudes from -1e308 to 1e308, past the poles",
        ),
    ] {
        let session = latitudes(&lat, NORTH);
        let field = decode(&session).expect("decodes");
        assert_eq!(
            field.georef.kind, "unsupported",
            "{lat:?}: {:?}",
            field.georef
        );
        let err = plan_world(&session).expect_err("not placed");
        assert_eq!(err.code(), "unsupported", "{lat:?}: {err}");
        assert!(
            err.message()
                .contains(&format!("coordinate \"lat\" holds {says}")),
            "{lat:?}: {err}"
        );
    }
}

/// A WRF domain, 3 × 4, with `MAP_PROJ` `map_proj`, whose 2-D `XLAT` has
/// `origin` at its first cell and `far` at its last. Mercator (3) and
/// unrotated lat/lon (6) are both pinned by those two corners.
fn wrf(map_proj: u8, origin: f64, far: f64) -> Session {
    let group = format!(
        "{{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{{\
         \"MAP_PROJ\":{map_proj},\"TRUELAT1\":30.0,\"STAND_LON\":-98.0,\
         \"DX\":12000.0,\"DY\":12000.0}}}}"
    )
    .into_bytes();
    let dims = "[\"south_north\",\"west_east\"]";
    let mut xlat: Vec<f64> = (0..12).map(|k| 35.0 + f64::from(k / 4) * 0.1).collect();
    xlat[0] = origin;
    xlat[11] = far;
    let xlong: Vec<f64> = (0..12).map(|k| -100.0 + f64::from(k % 4) * 0.1).collect();
    Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", group),
        ("XLAT/zarr.json", f64_array("[3,4]", dims, "{}")),
        ("XLAT/c/0/0", f8(&xlat)),
        ("XLONG/zarr.json", f64_array("[3,4]", dims, "{}")),
        ("XLONG/c/0/0", f8(&xlong)),
        ("t/zarr.json", f64_array("[3,4]", dims, "{}")),
        ("t/c/0/0", f8(&(0..12).map(f64::from).collect::<Vec<_>>())),
    ]))
    .expect("opens")
}

#[test]
fn a_wrf_corner_past_the_poles_places_no_domain() {
    // The premise: the domain is placed on Mercator from its corners.
    let field = decode(&wrf(3, 35.0, 35.2)).expect("places");
    assert_eq!(field.georef.kind, "mercator", "{:?}", field.georef);
    // A corner latitude further than 0.001° past a pole is no latitude, at
    // either corner, as a 1-D latitude past the poles is not. Master placed
    // a Mercator grid from latitude 200.
    for (origin, far) in [(200.0, 35.2), (35.0, -200.0)] {
        let field = decode(&wrf(3, origin, far)).expect("decodes");
        assert_eq!(
            field.georef.kind, "unsupported",
            "{origin}, {far}: {:?}",
            field.georef
        );
    }
}

#[test]
fn a_wrf_corner_a_hair_past_the_pole_is_the_pole() {
    // An unrotated lat/lon domain whose far corner is stored at 90.0005.
    let field = decode(&wrf(6, 35.0, 90.0005)).expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
    let corners = field.georef.corners.expect("corners");
    assert_eq!(corners[2], 90.0, "{corners:?}");
}

#[test]
fn a_swath_cell_a_hair_past_the_pole_is_placed_at_it() {
    let session = swath([89.0, 89.0, 90.0005, 90.0005], [0.0, 90.0, 0.0, 90.0], "");
    let placed = cells(&session);
    assert_eq!(placed[2].map(|p| p.0), Some(90.0), "{placed:?}");
}

#[test]
fn a_window_over_a_projected_grid_names_no_degrees() {
    // `x` and `y` are meters on a Lambert grid, `axis` X and Y: refused for
    // the projection, not because they are no longitudes, so the refusal
    // says nothing about their values.
    let mapping = "{\"grid_mapping_name\":\"lambert_conformal_conic\",\
        \"standard_parallel\":[30.0,60.0],\"longitude_of_central_meridian\":-98.0,\
        \"latitude_of_projection_origin\":40.0}";
    let session = Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", V3_GROUP.to_vec()),
        ("crs/zarr.json", f64_array("[1]", "[\"one\"]", mapping)),
        ("crs/c/0", f8(&[0.0])),
        (
            "x/zarr.json",
            f64_array("[4]", "[\"x\"]", "{\"axis\":\"X\",\"units\":\"m\"}"),
        ),
        ("x/c/0", f8(&[-3e6, -1e6, 1e6, 3e6])),
        (
            "y/zarr.json",
            f64_array("[3]", "[\"y\"]", "{\"axis\":\"Y\",\"units\":\"m\"}"),
        ),
        ("y/c/0", f8(&[-2e6, 0.0, 2e6])),
        (
            "t/zarr.json",
            f64_array("[3,4]", "[\"y\",\"x\"]", "{\"grid_mapping\":\"crs\"}"),
        ),
        ("t/c/0/0", grid()),
    ]))
    .expect("opens");
    let t = session
        .variables()
        .iter()
        .find(|v| v.name == "t")
        .expect("t")
        .index;
    let err = session
        .plan_window(&WindowRequest::new(
            t,
            0,
            1,
            [-90.0, 90.0, -180.0, 360.0],
            vec![0, 0],
        ))
        .expect_err("not a lat/lon grid");
    assert_eq!(err.code(), "unsupported", "{err}");
    assert!(
        err.message().contains("is placed as `source`") && !err.message().contains(" holds "),
        "{err}"
    );
}
