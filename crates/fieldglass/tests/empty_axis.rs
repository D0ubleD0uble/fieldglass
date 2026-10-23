//! A slice across an axis of length 0 says there is nothing to draw (#1425).
//!
//! An unlimited `time` with no records is an ordinary NetCDF file, and every
//! variable on it has no cells. Its slice used to decode to an `8 × 0` field,
//! and painting that gave a raster with no pixels, which a browser's
//! `new ImageData(rgba, 8, 0)` throws on: the panel stayed on "Rendering…" for
//! good. The slice check every host runs before reading a plane now refuses it
//! and names the empty axis, so the Node addon, the browser binding and a Zarr
//! window all say the same thing, whether the empty axis is drawn or held.
//!
//! Paths are relative for the `wasm32-wasip1` run's sandbox, which preopens the
//! package directory and its parent.

#![cfg(all(
    feature = "netcdf",
    feature = "zarr",
    feature = "grib2",
    feature = "render"
))]

use fieldglass::{DecodeOptions, Error, MemoryObjects, PaletteOptions, Session, Values};

const NETCDF: &str = "../fieldglass-netcdf/tests/fixtures";

fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .position(|v| v.name == name)
        .map(|i| i as u32)
        .unwrap_or_else(|| panic!("{name} is renderable"))
}

/// The refusal every empty axis gets: `unsupported`, naming the axis and the
/// array, and not the "past the end" a stale index gets.
fn assert_nothing_to_draw(err: &Error, axis: &str, array: &str, what: &str) {
    assert_eq!(err.code(), "unsupported", "{what}: {err}");
    let text = err.to_string();
    assert!(
        text.starts_with("nothing to draw: ")
            && text.contains(&format!("`{axis}`"))
            && text.contains(&format!("`{array}`"))
            && text.contains("length 0"),
        "{what}: {text}"
    );
    assert!(!text.contains("past the end"), "{what}: {text}");
}

/// `AGC_1202(time, z)` in a file whose unlimited `time` holds no records: the
/// plane is 8 depths by no times, drawn either way round.
#[test]
fn a_slice_drawing_an_empty_axis_is_refused_by_name() {
    let session =
        Session::open(std::fs::read(format!("{NETCDF}/netcdf_classic_dummy.nc")).expect("fixture"))
            .expect("opens");
    let agc = variable(&session, "AGC_1202");
    for (y, x, what) in [(0, 1, "time down the rows"), (1, 0, "time across")] {
        // Placing the slice reads no values, so it still answers: the panel
        // offers the controls the slice takes before drawing it.
        session
            .place_slice(agc, y, x)
            .unwrap_or_else(|e| panic!("{what}: the slice places: {e}"));
        let err = session
            .decode_slice(agc, y, x, &[0, 0], &DecodeOptions::default())
            .expect_err(what);
        assert_nothing_to_draw(&err, "time", "AGC_1202", what);
    }
}

/// A Zarr v2 store with `t(time, y, x)` and no time steps, built in memory.
fn empty_time_store() -> MemoryObjects {
    let array = |shape: &str, chunks: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{chunks},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    MemoryObjects::from_iter(vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("t/.zarray".to_string(), array("[0,2,3]", "[1,2,3]")),
        (
            "t/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["time","y","x"],"units":"K"}"#.to_vec(),
        ),
    ])
}

/// The common shape of an empty record axis: it is held, not drawn. No index
/// on it is right, so the refusal names the axis rather than the index.
#[test]
fn a_slice_or_line_holding_an_empty_axis_is_refused_by_name() {
    let session = Session::open_store(empty_time_store()).expect("opens");
    let t = variable(&session, "t");
    let err = session
        .decode_slice(t, 1, 2, &[0, 0, 0], &DecodeOptions::default())
        .expect_err("a y-by-x plane at a time step that does not exist");
    assert_nothing_to_draw(&err, "time", "t", "the slice");
    // A slice drawing the empty axis is refused the same way.
    let err = session
        .decode_slice(t, 0, 2, &[0, 0, 0], &DecodeOptions::default())
        .expect_err("a time-by-x plane");
    assert_nothing_to_draw(&err, "time", "t", "the cross-section");
    // A line through a cell holds the empty axis too.
    let err = session
        .decode_line(t, 2, &[0, 0, 0], &DecodeOptions::default())
        .expect_err("a line along x at a time step that does not exist");
    assert_nothing_to_draw(&err, "time", "t", "the line");
}

/// A field with no cells is refused at the paint as well, for a field that did
/// not come through a slice (a caller can build a `Field`, #902): a raster with
/// no pixels is never handed back.
#[test]
fn painting_a_field_with_no_cells_is_refused() {
    let bytes = std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
        .expect("fixture");
    let session = Session::open(bytes).expect("opens");
    let mut field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    let ni = field.ni;
    // Every row gone, as a plane across an empty axis would arrive.
    field.nj = 0;
    field.values = Values::F64(Vec::new());
    field.mask.clear();
    let err = session
        .render(&field, &PaletteOptions::default(), false)
        .expect_err("no cells");
    assert_eq!(err.code(), "unsupported", "{err}");
    assert_eq!(
        err.to_string(),
        format!("nothing to draw: the field is {ni} columns by 0 rows")
    );
}
