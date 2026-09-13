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
    assert_eq!(time.units, "hours since 2020-01-01 00:00:00");
    let coordinates = time.coordinates.expect("time has a coordinate array");
    assert_eq!(coordinates.len(), 2, "one value per index");

    // The horizontal axes answer the same way, in degrees.
    let lat = session.axis_values(temperature, 1).expect("lat axis");
    assert_eq!(lat.dimension, "lat");
    assert_eq!(lat.units, "degrees_north");
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
        assert_eq!(axis.units, "", "dim {dim}");
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
