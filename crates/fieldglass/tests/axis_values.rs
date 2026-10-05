//! The coordinate values a cross-section labels its axes from (#171).
//!
//! Three properties: the numbers are the coordinate array's own, in index
//! order, with its units beside them; an axis with no coordinate array is
//! reported as an axis all the same, so a host draws indices rather than
//! nothing; and reading them decodes no field, which is why a host can ask on
//! every axis change.

use fieldglass::{DecodeOptions, Session};

const NETCDF: &str = "../fieldglass-netcdf/tests/fixtures";

fn session(name: &str) -> Session {
    Session::open(std::fs::read(format!("{NETCDF}/{name}")).expect(name)).expect("opens")
}

/// The position of a variable in `Session::variables`.
fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .position(|v| v.name == name)
        .map(|i| i as u32)
        .unwrap_or_else(|| panic!("{name} is renderable"))
}

#[test]
fn an_axis_reports_its_coordinates_and_their_units() {
    let session = session("netcdf4_dimscale.nc");
    let temperature = variable(&session, "temperature");

    let time = session.axis_values(temperature, 0).expect("time axis");
    assert_eq!(time.dimension, "time");
    assert_eq!(time.length, 2);
    assert_eq!(
        time.units.as_deref(),
        Some("hours since 2020-01-01 00:00:00")
    );
    let coordinates = time.coordinates.expect("time has a coordinate array");
    assert_eq!(coordinates.len(), 2, "one value per index");

    // The horizontal axes answer the same way, in degrees.
    let lat = session.axis_values(temperature, 1).expect("lat axis");
    assert_eq!(lat.dimension, "lat");
    assert_eq!(lat.units.as_deref(), Some("degrees_north"));
    assert_eq!(
        lat.coordinates.as_ref().map(Vec::len),
        Some(lat.length as usize)
    );
}

#[test]
fn an_axis_with_no_coordinate_array_is_still_an_axis() {
    // WRF's `Time` has no coordinate variable, and its horizontal axes are
    // projected with none either: all three report a length and no numbers.
    let session = session("wrf_lambert.nc");
    let t2 = variable(&session, "T2");
    for dim in 0..3 {
        let axis = session.axis_values(t2, dim).expect("an axis");
        assert_eq!(axis.coordinates, None, "dim {dim}");
        assert_eq!(axis.units, None, "dim {dim}");
        assert!(axis.length >= 1, "dim {dim}");
    }
}

#[test]
fn an_axis_outside_the_variable_is_refused() {
    let session = session("netcdf4_dimscale.nc");
    let temperature = variable(&session, "temperature");
    let err = session
        .axis_values(temperature, 9)
        .expect_err("dim 9 of a 3-D variable");
    assert_eq!(err.code(), "invalid_option");
    assert!(err.to_string().contains("outside them"), "{err}");

    let err = session
        .axis_values(9_999, 0)
        .expect_err("a variable that does not exist");
    assert_eq!(err.code(), "no_such_message");
}

/// The functions keyed by name refuse a name the source does not list, and say
/// which name (#872). They used to answer `no_such_message` for message 0,
/// which tells a NetCDF caller about a position its call never named.
#[test]
fn an_array_name_the_source_does_not_list_is_refused_by_name() {
    use fieldglass::netcdf::{NetcdfArrays, NetcdfReader};
    use fieldglass::{DecodeOptions, Dtype, axis_values, line_through};

    let bytes = std::fs::read(format!("{NETCDF}/netcdf4_dimscale.nc")).expect("fixture");
    let arrays =
        NetcdfArrays::open(NetcdfReader::from_bytes(bytes).expect("reader")).expect("arrays");
    // The same source answers for a name it does list, so the refusals below
    // are about the name and not the source.
    axis_values(&arrays, "temperature", 0).expect("a listed name answers");

    let opts = DecodeOptions::new(Dtype::Auto);
    for err in [
        axis_values(&arrays, "salinity", 0).expect_err("axis of an unlisted name"),
        line_through(&arrays, "salinity", 0, &[0, 0, 0], &opts).expect_err("line of one"),
    ] {
        assert_eq!(err.code(), "invalid_option", "{err:?}");
        assert!(err.to_string().contains("`salinity`"), "{err}");
    }
}

/// A GRIB file is addressed by messages, so it has no variable axes to read.
#[test]
fn a_message_addressed_file_says_so() {
    let session = Session::open(
        std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
            .expect("fixture"),
    )
    .expect("opens");
    let err = session.axis_values(0, 0).expect_err("GRIB has no axes");
    assert_eq!(err.code(), "wrong_addressing");
}

/// Every axis of a 4-D variable answers before anything is decoded, and the
/// field still decodes afterwards — so a panel may ask on every axis change.
#[test]
fn every_axis_answers_without_decoding_first() {
    let session = session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&session, "sst");
    // Every axis, before anything has been decoded.
    for dim in 0..4 {
        session.axis_values(sst, dim).expect("an axis");
    }
    // And the field still decodes afterwards, so nothing was consumed.
    let field = session
        .decode_slice(sst, 2, 3, &[0, 0, 0, 0], &DecodeOptions::default())
        .expect("decodes");
    assert!(field.values.len() > 1000, "the field is the big read");
}

/// A coordinate value that is not a finite number is a hole, the same as a
/// missing one, so the axis reports no coordinates at all (#574).
///
/// Two reasons, and the second is why this is decided here rather than left to
/// each host. A `NaN` or infinite position has nowhere to be plotted, which is
/// the rule an absent value already follows. And the two hosts cannot carry it
/// alike: the Node addon hands a DTO over through `serde_json`, which writes a
/// non-finite float as `null`, while the browser binding passes `NaN` through,
/// so one file would read `[0, null]` in one host and `[0, NaN]` in the other,
/// under a declaration saying `number[]` in both. Refusing the array here means
/// no non-finite coordinate reaches either wire.
///
/// Built in memory: a Zarr v2 store whose time coordinate holds a non-finite
/// value with no `fill_value` to mark it missing, which is what a writer that
/// did not declare one produces.
#[test]
fn a_non_finite_coordinate_is_a_hole() {
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let store = |time: &[f64]| {
        fieldglass::MemoryObjects::from_iter(vec![
            (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
            ("time/.zarray".to_string(), array("[2]")),
            (
                "time/.zattrs".to_string(),
                br#"{"_ARRAY_DIMENSIONS":["time"],"units":"hours since 2020-01-01"}"#.to_vec(),
            ),
            ("time/0".to_string(), f64s(time)),
            ("t/.zarray".to_string(), array("[2,2,2]")),
            (
                "t/.zattrs".to_string(),
                br#"{"_ARRAY_DIMENSIONS":["time","y","x"],"units":"K"}"#.to_vec(),
            ),
            ("t/0.0.0".to_string(), f64s(&[1.0; 8])),
        ])
    };

    // The control: the same store with a finite time axis reports it, so the
    // store is readable and the refusal below is about the value alone.
    let finite = Session::open_store(store(&[0.0, 6.0])).expect("opens");
    let t = variable(&finite, "t");
    assert_eq!(
        finite.axis_values(t, 0).expect("time").coordinates,
        Some(vec![0.0, 6.0])
    );

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let session = Session::open_store(store(&[0.0, bad])).expect("opens");
        let t = variable(&session, "t");
        let axis = session.axis_values(t, 0).expect("time");
        assert_eq!(
            axis.coordinates, None,
            "{bad}: a non-finite value is a hole"
        );
        assert_eq!(
            axis.units, None,
            "{bad}: no coordinates, so no units either"
        );
        // A line along the same axis reads the same coordinates, so it falls
        // back to indices too.
        let line = session
            .decode_line(t, 0, &[0, 0, 0], &DecodeOptions::default())
            .expect("a line");
        assert_eq!(line.coordinates, None, "{bad}: the line's axis");
        assert_eq!(line.coordinate_units, None, "{bad}");
    }
}
