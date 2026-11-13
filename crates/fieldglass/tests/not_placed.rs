//! Why a variable is not placed, when its 1-D coordinates are the reason
//! (#1727).
//!
//! #1719 refused a 1-D latitude past the poles and a longitude axis more than
//! twice around the globe, and said so only for a coordinate that was read as
//! one by its name after losing its attributes. A coordinate that never had
//! any (`bare`) and one that states `degrees_north` and holds 2000
//! (`attributed`) were refused in silence. `VariableInfo::not_placed` says why
//! for those, and says nothing where the values are not the reason.

use fieldglass::{Session, VariableInfo, WindowRequest};

const NETCDF: &str = "../fieldglass-netcdf/tests/fixtures";

fn session(name: &str) -> Session {
    Session::open(std::fs::read(format!("{NETCDF}/{name}")).expect(name)).expect("opens")
}

fn listed(session: &Session, name: &str) -> (u32, VariableInfo) {
    session
        .variables()
        .into_iter()
        .enumerate()
        .find(|(_, v)| v.name == name)
        .map(|(i, v)| (i as u32, v))
        .unwrap_or_else(|| panic!("{name} is renderable"))
}

/// A `y` and `x` that never had attributes are read as latitude and longitude
/// by their names; the reader lost nothing, so `placed_by_name` has nothing to
/// say, and `not_placed` names both axes and what is wrong with each.
#[test]
fn bare_axes_holding_no_degrees_are_named_with_their_values() {
    let session = session("netcdf_values_no_degrees.nc");
    let (_, bare) = listed(&session, "bare");
    assert_eq!(bare.placed_by_name, None);
    assert_eq!(
        bare.not_placed.as_deref(),
        Some(
            "\"y\" holds latitudes from -1000 to 1000, past the poles, so the field is not \
             placed on the map by it; \"x\" holds longitudes from -2000 to 2000, more than \
             twice around the globe, so the field is not placed on the map by it"
        )
    );
    // The host reads it under its camelCase key.
    let json = serde_json::to_value(&bare).expect("serializes");
    assert_eq!(json["notPlaced"], serde_json::json!(bare.not_placed));
}

/// A `lat` that says it is in `degrees_north` and holds 2000 is named; its
/// `lon` is a real longitude and is not.
#[test]
fn an_attributed_latitude_past_the_poles_is_named() {
    let session = session("netcdf_values_no_degrees.nc");
    let (_, attributed) = listed(&session, "attributed");
    assert_eq!(attributed.placed_by_name, None);
    assert_eq!(
        attributed.not_placed.as_deref(),
        Some(
            "\"lat\" holds latitudes from 0 to 2000, past the poles, so the field is not \
             placed on the map by it"
        )
    );
    // A latitude that says it is one and holds 2000 is wrong per CF, so
    // decoding the slice is refused, naming the values (#1305). A
    // NetCDF reader names no unwritten chunks, so there is no such note.
    let (index, _) = listed(&session, "attributed");
    let err = session
        .decode_slice(index, 0, 1, &[0, 0], &fieldglass::DecodeOptions::default())
        .expect_err("refused");
    assert_eq!(err.code(), "decode", "{err}");
    assert!(
        err.message()
            .contains("\"lat\" holds latitudes from 0 to 2000, past the poles"),
        "{err}"
    );
    assert_eq!(attributed.coordinates_not_written, None);
    // `bare` says nothing of being a latitude, so it is still shown
    // unplaced, and so is time against the latitude, which places nothing.
    let (bare, _) = listed(&session, "bare");
    let field = session
        .decode_slice(bare, 0, 1, &[0, 0], &fieldglass::DecodeOptions::default())
        .expect("decodes");
    assert_eq!(field.georef.kind, "unsupported", "{:?}", field.georef);
    let (section, _) = listed(&session, "section");
    session
        .decode_slice(
            section,
            0,
            1,
            &[0, 0],
            &fieldglass::DecodeOptions::default(),
        )
        .expect("a section decodes");
}

/// Time against latitude is not placed whatever the latitudes hold, so their
/// values are not why and nothing is said (the gate that the axes must read as
/// latitude down the rows and longitude across). Each half of that gate holds
/// alone: time against the bad longitude `x`, and the bad latitude `lat`
/// against time, say nothing either.
#[test]
fn a_section_against_that_latitude_says_nothing_of_its_values() {
    let session = session("netcdf_values_no_degrees.nc");
    for name in ["section", "hovmoller", "profile"] {
        let (_, section) = listed(&session, name);
        assert_eq!(section.not_placed, None, "{name}");
        assert_eq!(section.placed_by_name, None, "{name}");
    }
}

/// WRF's globals place `T2` before its 1-D axes are read, so the meters on
/// `south_north` and `west_east` do not unplace it and are not named.
#[test]
fn a_wrf_field_placed_by_its_globals_says_nothing_of_its_axes() {
    let session = session("netcdf_values_no_degrees.nc");
    let (_, t2) = listed(&session, "T2");
    assert_eq!(t2.not_placed, None);
    assert_eq!(t2.placed_by_name, None);
}

/// Where `placed_by_name` already gives the values clause, `not_placed` does
/// not repeat it.
#[test]
fn a_reason_placed_by_name_gives_is_not_given_twice() {
    let session = session("netcdf4_unread_axes.nc");
    for v in session.variables() {
        assert_eq!(v.not_placed, None, "{}", v.name);
    }
}

/// The same gate holds for a window: across `attributed` with its axes
/// swapped, longitude runs down the rows and latitude across, which places
/// nothing whatever the values hold, so the refusal does not blame them. The
/// axes the right way round do.
#[test]
fn a_window_blames_the_values_only_on_axes_that_would_place_the_field() {
    let session = session("netcdf_values_no_degrees.nc");
    let (attributed, _) = listed(&session, "attributed");
    let blame = "\"lat\" holds latitudes from 0 to 2000";

    let err = session
        .plan_window(&WindowRequest::new(
            attributed,
            0,
            1,
            [0.0, 10.0, 0.0, 10.0],
            vec![0, 0],
        ))
        .expect_err("not placed");
    assert!(err.to_string().contains(blame), "{err}");

    let err = session
        .plan_window(&WindowRequest::new(
            attributed,
            1,
            0,
            [0.0, 10.0, 0.0, 10.0],
            vec![0, 0],
        ))
        .expect_err("not placed either way");
    assert!(!err.to_string().contains("holds latitudes"), "{err}");
}

/// A WRF slice is placed, as `lambert`, by its globals, so a window over it
/// is refused for not being a regular lat/lon grid, not for the meters on
/// `south_north` and `west_east`, which placement never read as degrees.
#[test]
fn a_window_over_a_placed_wrf_slice_does_not_blame_its_axes() {
    let session = session("netcdf_values_no_degrees.nc");
    for name in ["T2", "XLAT", "XLONG"] {
        let (index, _) = listed(&session, name);
        let result = session.plan_window(&WindowRequest::new(
            index,
            0,
            1,
            [0.0, 10.0, 0.0, 10.0],
            vec![0, 0],
        ));
        if let Err(err) = result {
            let err = err.to_string();
            assert!(err.contains("`lambert`"), "{name}: {err}");
            assert!(!err.contains("holds latitudes"), "{name}: {err}");
            assert!(!err.contains("holds longitudes"), "{name}: {err}");
        }
    }
}
