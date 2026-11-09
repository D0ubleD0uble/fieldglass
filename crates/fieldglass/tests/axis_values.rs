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
        assert!(err.to_string().contains(r#""salinity""#), "{err}");
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

/// Which way a vertical axis runs, read from its coordinate array by CF §4.3,
/// so a host can draw it the right way up (#1043): ERSST's `lev` is a depth,
/// `positive = "down"` in metres, and its other axes are not vertical.
#[test]
fn a_depth_axis_says_it_runs_down() {
    let session = session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&session, "sst");
    let lev = session.axis_values(sst, 1).expect("lev");
    assert_eq!(lev.dimension, "lev");
    let vertical = lev.vertical.expect("lev is vertical");
    assert_eq!(vertical.positive, Some(fieldglass::Positive::Down));
    assert!(!vertical.log_pressure, "metres are not a pressure");
    for dim in [0, 2, 3] {
        assert_eq!(
            session.axis_values(sst, dim).expect("an axis").vertical,
            None,
            "dim {dim}"
        );
    }
    // The line along it reads the same.
    let line = session
        .decode_line(sst, 1, &[0, 0, 0, 0], &DecodeOptions::default())
        .expect("a line");
    assert_eq!(line.coordinate_vertical, Some(vertical));
    let time = session
        .decode_line(sst, 0, &[0, 0, 0, 0], &DecodeOptions::default())
        .expect("a line");
    assert_eq!(time.coordinate_vertical, None, "time is not vertical");
}

/// A pressure level that states no `positive` runs down all the same, since CF
/// requires the attribute only for units that are not a pressure; on the wire
/// it reads `{"positive": "down", "logPressure": true}`. Built in memory as a Zarr
/// v2 store, the way the NCEP/NCAR reanalysis writes `level`, less `positive`.
#[test]
fn a_pressure_level_runs_down_without_saying_so() {
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let session = Session::open_store(fieldglass::MemoryObjects::from_iter(vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("level/.zarray".to_string(), array("[3]")),
        (
            "level/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["level"],"units":"millibar"}"#.to_vec(),
        ),
        ("level/0".to_string(), f64s(&[1000.0, 500.0, 10.0])),
        ("t/.zarray".to_string(), array("[3,2,2]")),
        (
            "t/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["level","y","x"],"units":"K"}"#.to_vec(),
        ),
        ("t/0.0.0".to_string(), f64s(&[1.0; 12])),
    ]))
    .expect("opens");
    let t = variable(&session, "t");
    let level = session.axis_values(t, 0).expect("level");
    let vertical = level.vertical.expect("a pressure is vertical");
    assert_eq!(vertical.positive, Some(fieldglass::Positive::Down));
    assert!(vertical.log_pressure);
    assert_eq!(
        serde_json::to_value(vertical).expect("serializes"),
        serde_json::json!({ "positive": "down", "logPressure": true })
    );
}

/// The calendar a time axis counts on, read from its coordinate array (#1033),
/// so a host does not write a `360_day` time as a Gregorian date. ERSST v5
/// counts `minutes since 1870-01-01 00:00` on `360_day`; the axis and a line
/// along it both carry that, and an axis that states no calendar carries none.
#[test]
fn a_time_axis_carries_its_calendar() {
    let ersst = session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&ersst, "sst");
    let time = ersst.axis_values(sst, 0).expect("time");
    assert_eq!(
        time.units.as_deref(),
        Some("minutes since 1870-01-01 00:00")
    );
    assert_eq!(time.calendar.as_deref(), Some("360_day"));
    let json = serde_json::to_value(&time).expect("serializes");
    assert_eq!(json["calendar"], "360_day", "{json}");
    for dim in [1, 2, 3] {
        assert_eq!(
            ersst.axis_values(sst, dim).expect("an axis").calendar,
            None,
            "dim {dim} states no calendar"
        );
    }
    let line = ersst
        .decode_line(sst, 0, &[0, 0, 0, 0], &DecodeOptions::default())
        .expect("a line along time");
    assert_eq!(line.coordinate_calendar.as_deref(), Some("360_day"));
    let json = serde_json::to_value(&line).expect("serializes");
    assert_eq!(json["coordinateCalendar"], "360_day", "{json}");

    // A time that states no calendar is `standard` by CF, and is reported as
    // stating none rather than given a name the file did not write.
    let dimscale = session("netcdf4_dimscale.nc");
    let temperature = variable(&dimscale, "temperature");
    let time = dimscale.axis_values(temperature, 0).expect("time");
    assert!(time.units.is_some(), "a time axis");
    assert_eq!(time.calendar, None);
}

/// A calendar stored as `NC_STRING`, a variable-length string, as
/// netCDF4-python's `setncattr_string`, h5netcdf and h5py write it, reaches the
/// axis the same way (#1549). It used to read as absent, so the `noleap` time
/// was dated on the Gregorian calendar.
#[test]
fn an_nc_string_calendar_reaches_the_time_axis() {
    let session = session("netcdf4_string_attributes.nc");
    let temperature = variable(&session, "temperature");
    let time = session.axis_values(temperature, 0).expect("time");
    assert_eq!(time.units.as_deref(), Some("days since 2000-01-01"));
    assert_eq!(time.calendar.as_deref(), Some("noleap"));
    let line = session
        .decode_line(temperature, 0, &[0, 0, 0], &DecodeOptions::default())
        .expect("a line along time");
    assert_eq!(line.coordinate_calendar.as_deref(), Some("noleap"));
}

/// A `calendar` or `units` stored as a list of several strings is not one
/// calendar or one unit, and no element can be picked for it with any
/// certainty (#1570). The axis is handed every element as `ncdump` prints
/// them, each quoted, which names no calendar and no time unit, so a host does
/// not date the time and shows what the file states instead. A one-element
/// list is the string itself, so `tas`'s time keeps its units.
#[test]
fn a_calendar_or_units_of_several_strings_reach_the_axis_as_every_element() {
    let session = session("netcdf4_string_lists.nc");
    let tas = variable(&session, "tas");
    let time = session.axis_values(tas, 0).expect("time");
    assert_eq!(time.units.as_deref(), Some("days since 2000-01-01"));
    assert_eq!(time.calendar.as_deref(), Some(r#""noleap", "360_day""#));
    let line = session
        .decode_line(tas, 0, &[0, 0, 0], &DecodeOptions::default())
        .expect("a line along time");
    assert_eq!(
        line.coordinate_calendar.as_deref(),
        Some(r#""noleap", "360_day""#)
    );

    let pr = variable(&session, "pr");
    let step = session.axis_values(pr, 0).expect("step");
    assert_eq!(
        step.units.as_deref(),
        Some(r#""hours since 2000-01-01", "days since 2000-01-01""#)
    );
    assert_eq!(step.calendar.as_deref(), Some("standard"));
    assert_eq!(step.coordinates, Some(vec![0.0, 6.0]));
}

/// A time left undated because its `calendar` or its `units` is a list says
/// so, beside the number a host shows in place of the date (#1604): a reader
/// who sees `0` where a date was expected is told the file is why. A time
/// whose attributes are each one string is dated, and says nothing.
#[test]
fn a_time_not_dated_for_a_list_says_why() {
    let session = session("netcdf4_string_lists.nc");
    let tas = variable(&session, "tas");
    assert_eq!(
        session
            .axis_values(tas, 0)
            .expect("time")
            .not_dated
            .as_deref(),
        Some("not shown as a date: its calendar holds several values")
    );
    let pr = variable(&session, "pr");
    assert_eq!(
        session
            .axis_values(pr, 0)
            .expect("step")
            .not_dated
            .as_deref(),
        Some("not shown as a date: its units hold several values")
    );
    // The space axes are not times, lists or not.
    assert_eq!(session.axis_values(tas, 1).expect("lat").not_dated, None);

    let dated = self::session("netcdf4_dimscale.nc");
    let temperature = variable(&dated, "temperature");
    assert_eq!(
        dated.axis_values(temperature, 0).expect("time").not_dated,
        None
    );
}

/// A Zarr store's time reads its calendar from `.zattrs` the same way, as the
/// file spells it, and a blank one states none (#1033).
#[test]
fn a_zarr_time_axis_carries_its_calendar() {
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let store = |calendar: &str| {
        fieldglass::MemoryObjects::from_iter(vec![
            (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
            ("time/.zarray".to_string(), array("[2]")),
            (
                "time/.zattrs".to_string(),
                format!(
                    r#"{{"_ARRAY_DIMENSIONS":["time"],"units":"days since 2000-01-01","calendar":"{calendar}"}}"#
                )
                .into_bytes(),
            ),
            ("time/0".to_string(), f64s(&[0.0, 360.0])),
            ("t/.zarray".to_string(), array("[2,2,2]")),
            (
                "t/.zattrs".to_string(),
                br#"{"_ARRAY_DIMENSIONS":["time","y","x"],"units":"K"}"#.to_vec(),
            ),
            ("t/0.0.0".to_string(), f64s(&[1.0; 8])),
        ])
    };
    let session = Session::open_store(store("NoLeap")).expect("opens");
    let t = variable(&session, "t");
    assert_eq!(
        session.axis_values(t, 0).expect("time").calendar.as_deref(),
        Some("NoLeap"),
        "as the file spells it"
    );
    let line = session
        .decode_line(t, 0, &[0, 0, 0], &DecodeOptions::default())
        .expect("a line");
    assert_eq!(line.coordinate_calendar.as_deref(), Some("NoLeap"));

    let blank = Session::open_store(store("  ")).expect("opens");
    let t = variable(&blank, "t");
    assert_eq!(blank.axis_values(t, 0).expect("time").calendar, None);
}

/// A Zarr time whose `calendar` or `units` is not one string is left undated
/// and told why, truthfully (#1630): several values only for a list of two or
/// more, "not text" for `null`, a boolean, an object and a list of one thing
/// that is no string. Before, every JSON value the model keeps opaque read
/// "holds several values". A list of one string is that string, as NetCDF
/// reads it, so it is dated and says nothing. A number is handed to the host
/// as no calendar with no note there, and its note is `calendar_not_dated`
/// (#1645). Each case also holds the note to the calendar the host is handed.
#[test]
fn a_zarr_time_not_dated_says_what_its_attribute_is() {
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let open = |units: &str, calendar: &str| {
        let store = fieldglass::MemoryObjects::from_iter(vec![
            (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
            ("time/.zarray".to_string(), array("[2]")),
            (
                "time/.zattrs".to_string(),
                format!(
                    r#"{{"_ARRAY_DIMENSIONS":["time"],"units":{units},"calendar":{calendar}}}"#
                )
                .into_bytes(),
            ),
            ("time/0".to_string(), f64s(&[0.0, 360.0])),
            ("t/.zarray".to_string(), array("[2,2,2]")),
            (
                "t/.zattrs".to_string(),
                br#"{"_ARRAY_DIMENSIONS":["time","y","x"],"units":"K"}"#.to_vec(),
            ),
            ("t/0.0.0".to_string(), f64s(&[1.0; 8])),
        ]);
        let session = Session::open_store(store).expect("opens");
        let t = variable(&session, "t");
        session.axis_values(t, 0).expect("time")
    };
    let since = r#""days since 2000-01-01""#;
    let note = |what: &str| Some(format!("not shown as a date: {what}"));

    for not_text in [
        "null",
        "true",
        r#"{"a":1}"#,
        "[]",
        "[true]",
        r#"[["a","b"]]"#,
    ] {
        let axis = open(since, not_text);
        assert_eq!(
            axis.not_dated,
            note("its calendar is not text"),
            "calendar {not_text}"
        );
        // The host is handed the file's own text, which names no calendar it
        // counts, so it shows the number the note is beside.
        assert_eq!(
            axis.calendar.as_deref(),
            Some(not_text),
            "calendar {not_text}"
        );
    }
    // A number is handed over as no calendar, with no note on `not_dated`, as
    // in 0.6.1. Its note is `calendar_not_dated` (#1645, tested below).
    for number in ["5", "[1,2]"] {
        let axis = open(since, number);
        assert_eq!(axis.not_dated, None, "calendar {number}");
        assert_eq!(axis.calendar, None, "calendar {number}");
        assert_eq!(
            axis.calendar_not_dated,
            note("its calendar is not text"),
            "calendar {number}"
        );
    }
    // Every other calendar has no such note: `not_dated` says it all.
    for other in ["null", r#"["a","b"]"#, r#""noleap""#] {
        assert_eq!(open(since, other).calendar_not_dated, None, "{other}");
    }
    for several in [r#"["noleap","360_day"]"#, r#"["a",1]"#, r#"["a, b","c"]"#] {
        let axis = open(since, several);
        assert_eq!(
            axis.not_dated,
            note("its calendar holds several values"),
            "calendar {several}"
        );
        assert_eq!(
            axis.calendar.as_deref(),
            Some(several),
            "calendar {several}"
        );
    }
    // The units name a time in every element, and are still not one unit.
    let two_units = r#"["days since 2000-01-01","hours since 2000-01-01"]"#;
    assert_eq!(
        open(two_units, r#""noleap""#).not_dated,
        note("its units hold several values")
    );
    assert_eq!(
        open(two_units, "null").not_dated,
        note("its units hold several values and its calendar is not text")
    );
    assert_eq!(
        open(two_units, r#"["a","b"]"#).not_dated,
        note("its units and calendar hold several values")
    );
    // The axis still hands over what the file states.
    assert_eq!(open(since, "null").calendar.as_deref(), Some("null"));

    // A list of one string is that string, and a comma inside it is its own.
    for one in [r#"["noleap"]"#, r#""noleap""#] {
        let axis = open(since, one);
        assert_eq!(axis.not_dated, None, "calendar {one}");
        assert_eq!(axis.calendar.as_deref(), Some("noleap"), "calendar {one}");
    }
    let comma = open(since, r#"["a, b"]"#);
    assert_eq!(comma.not_dated, None);
    assert_eq!(comma.calendar.as_deref(), Some("a, b"));
    let listed_units = open(r#"["days since 2000-01-01"]"#, r#""noleap""#);
    assert_eq!(listed_units.not_dated, None);
    assert_eq!(listed_units.units.as_deref(), Some("days since 2000-01-01"));

    // Units that are no text name no time, so there is nothing to date.
    assert_eq!(open("[1]", "null").not_dated, None);
}

/// A Zarr v2 store with a `time(time)` of `[0, 360]` and a `t(time, y, x)`
/// over it, as in `netcdf_numeric_calendar.nc`, `time` carrying `units` and
/// `calendar` as the JSON given.
fn zarr_time(units: &str, calendar: &str) -> Session {
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    Session::open_store(fieldglass::MemoryObjects::from_iter(vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("time/.zarray".to_string(), array("[2]")),
        (
            "time/.zattrs".to_string(),
            format!(r#"{{"_ARRAY_DIMENSIONS":["time"],"units":{units},"calendar":{calendar}}}"#)
                .into_bytes(),
        ),
        ("time/0".to_string(), f64s(&[0.0, 360.0])),
        ("t/.zarray".to_string(), array("[2,2,2]")),
        (
            "t/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["time","y","x"],"units":"K"}"#.to_vec(),
        ),
        ("t/0.0.0".to_string(), f64s(&[1.0; 8])),
    ]))
    .expect("opens")
}

/// A `calendar` that is a number or a list of numbers, in a NetCDF file or a
/// Zarr store, is never dated on the standard calendar (#1645, owner decision
/// 2026-11-08). CF §4.4.1 makes `calendar` a string, and `360` may mean
/// `360_day`, so the time shows its number and the axis says why:
/// `calendar_not_dated` is `not shown as a date: its calendar is not text`.
/// `calendar` and `not_dated` read as 0.6.1 read them, `None` both. A line
/// along the axis says the same, and the host's JSON carries it.
///
/// In `netcdf_numeric_calendar.nc`, `t` is over `time` (`int calendar =
/// 360`) and `u` over `time2` (`int calendar = 1, 2`).
#[test]
fn a_numeric_calendar_is_not_dated_and_says_so() {
    let note = Some("not shown as a date: its calendar is not text".to_string());
    let since = r#""days since 2000-01-01""#;
    let cases = [
        (
            "NetCDF, one number",
            session("netcdf_numeric_calendar.nc"),
            "t",
        ),
        (
            "NetCDF, a list of numbers",
            session("netcdf_numeric_calendar.nc"),
            "u",
        ),
        ("Zarr, one number", zarr_time(since, "360"), "t"),
        ("Zarr, a list of numbers", zarr_time(since, "[1,2]"), "t"),
    ];
    for (case, session, field) in &cases {
        let t = variable(session, field);
        let time = session.axis_values(t, 0).expect("time");
        assert_eq!(time.coordinates, Some(vec![0.0, 360.0]), "{case}");
        assert_eq!(
            time.units.as_deref(),
            Some("days since 2000-01-01"),
            "{case}"
        );
        assert_eq!(time.calendar, None, "{case}: as 0.6.1 hands it over");
        assert_eq!(time.not_dated, None, "{case}: as 0.6.1 notes it");
        assert_eq!(time.calendar_not_dated, note, "{case}");
        let json = serde_json::to_value(&time).expect("serializes");
        assert_eq!(
            json["calendarNotDated"],
            note.as_deref().unwrap(),
            "{case}: {json}"
        );

        let line = session
            .decode_line(t, 0, &[0, 0, 0], &DecodeOptions::default())
            .expect("a line along time");
        assert_eq!(line.coordinate_calendar, None, "{case}");
        assert_eq!(line.coordinate_not_dated, None, "{case}");
        assert_eq!(line.coordinate_calendar_not_dated, note, "{case}");
        let json = serde_json::to_value(&line).expect("serializes");
        assert_eq!(
            json["coordinateCalendarNotDated"],
            note.as_deref().unwrap(),
            "{case}: {json}"
        );

        // The space axes are no time, whatever the calendar on time.
        assert_eq!(
            session.axis_values(t, 1).expect("y").calendar_not_dated,
            None
        );
    }

    // With units that are no one unit either, one note says both, as
    // `not_dated` combines two faults; `not_dated` keeps its own half.
    let two_units = r#"["days since 2000-01-01","hours since 2000-01-01"]"#;
    let session = zarr_time(two_units, "360");
    let time = session
        .axis_values(variable(&session, "t"), 0)
        .expect("time");
    assert_eq!(
        time.not_dated.as_deref(),
        Some("not shown as a date: its units hold several values")
    );
    assert_eq!(
        time.calendar_not_dated.as_deref(),
        Some("not shown as a date: its units hold several values and its calendar is not text")
    );
    // Units that are no time leave nothing to date, so no note.
    let session = zarr_time(r#""K""#, "360");
    let time = session
        .axis_values(variable(&session, "t"), 0)
        .expect("time");
    assert_eq!(time.calendar_not_dated, None);
    // A calendar that is text keeps no such note, in either container.
    let session = zarr_time(r#""days since 2000-01-01""#, r#""360_day""#);
    let time = session
        .axis_values(variable(&session, "t"), 0)
        .expect("time");
    assert_eq!(time.calendar_not_dated, None);
    let ersst = self::session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&ersst, "sst");
    assert_eq!(
        ersst.axis_values(sst, 0).expect("time").calendar_not_dated,
        None
    );
}

/// A time whose `units` or `calendar` the reader kept only as its first
/// element is not dated on it, and says so (#1687). In
/// `netcdf4_unread_axes.nc`, `time`'s `units` and `step`'s `calendar` are
/// each two fixed-length strings with data for fewer than declared, so each
/// reads as its first element: `days since 2000-01-01` and `standard`, which
/// a host would date.
#[test]
fn a_time_read_in_part_is_not_dated() {
    let session = session("netcdf4_unread_axes.nc");
    let tas = variable(&session, "tas");
    let time = session.axis_values(tas, 0).expect("time");
    assert_eq!(time.units.as_deref(), Some("days since 2000-01-01"));
    assert_eq!(time.calendar, None);
    assert_eq!(
        time.not_dated.as_deref(),
        Some("not shown as a date: its units were read only in part")
    );
    let pr = variable(&session, "pr");
    let step = session.axis_values(pr, 0).expect("step");
    assert_eq!(step.units.as_deref(), Some("hours since 2000-01-01"));
    assert_eq!(step.calendar.as_deref(), Some("standard"));
    assert_eq!(
        step.not_dated.as_deref(),
        Some("not shown as a date: its calendar was read only in part")
    );
    let line = session
        .decode_line(pr, 0, &[0, 0, 0], &DecodeOptions::default())
        .expect("a line through step");
    assert_eq!(line.coordinate_not_dated, step.not_dated);
    let line = session
        .decode_line(tas, 0, &[0, 0, 0], &DecodeOptions::default())
        .expect("a line through time");
    assert_eq!(line.coordinate_not_dated, time.not_dated);
}

/// A projected `y` and `x` in meters whose `units` and `standard_name` the
/// reader lost are read as latitude and longitude from their names, as 0.6.0
/// read them, and the variable drawn over them says so (#1687), and that
/// their values are no degrees (#1719). Their axes say their units were not
/// read.
#[test]
fn a_projected_axis_read_by_its_name_is_named_on_the_variable() {
    let session = session("netcdf4_unread_axes.nc");
    let note = "\"y\" is read as latitude from its name alone: its attributes \
                \"standard_name\", \"units\" were not read, and its values run from -1000 \
                to 1000, past the poles, so the field is not placed on the map by it; \
                \"x\" is read as longitude from its name alone: its attributes \
                \"standard_name\", \"units\" were not read, and its values run from -2000 \
                to 2000, more than twice around the globe, so the field is not placed on \
                the map by it";
    for name in ["tas", "pr"] {
        let listed = session
            .variables()
            .into_iter()
            .find(|v| v.name == name)
            .expect("listed");
        assert_eq!(
            (listed.detected_y_dim, listed.detected_x_dim),
            (Some(1), Some(2))
        );
        assert_eq!(listed.placed_by_name.as_deref(), Some(note), "{name}");
        let json = serde_json::to_value(&listed).expect("serializes");
        assert_eq!(json["placedByName"], note, "{json}");
    }
    let tas = variable(&session, "tas");
    for dim in [1, 2] {
        let axis = session.axis_values(tas, dim).expect("a space axis");
        assert_eq!(axis.units, None);
        assert_eq!(
            axis.not_dated.as_deref(),
            Some("its units were not read, so it is not shown as a date"),
            "{}",
            axis.dimension
        );
    }
    // A file whose space axes lost nothing names none.
    let whole = self::session("netcdf4_string_lists.nc");
    assert!(whole.variables().iter().all(|v| v.placed_by_name.is_none()));
}

/// A `y` and `x` read as latitude and longitude from their names, whose
/// values run from -1000 to 1000 and -2000 to 2000, are no latitude and no
/// longitude axis, so they do not place the field on the map (#1719). The slice
/// is drawn in its own grid, which offers no coastlines or graticule, and
/// its rows keep the order `y` gives them.
#[test]
fn an_axis_read_by_its_name_whose_values_are_no_degrees_places_nothing() {
    use fieldglass::Placement;
    let session = session("netcdf4_unread_axes.nc");
    for name in ["tas", "pr"] {
        let v = variable(&session, name);
        let placed = session.place_slice(v, 1, 2).expect("places");
        assert_eq!(placed.family(), "source", "{name}");
        // `y` runs from -1000 up to 1000, so row 0 is its southern edge, as it
        // was when the slice was placed.
        assert!(placed.scan().j_positive, "{name}");
        let field = session
            .decode_slice(v, 1, 2, &[0, 0, 0], &DecodeOptions::default())
            .expect("decodes");
        assert_eq!(field.georef.placement, Placement::Unplaceable, "{name}");
        assert_eq!(field.georef.bounds_lonlat, None, "{name}");
    }
}

/// `netcdf4_unread_axes.nc` with its `y` read as `f(value)`, and with
/// further arrays a test adds. The reader's record of what it lost is
/// forwarded.
struct Edited {
    arrays: fieldglass::netcdf::NetcdfArrays,
    group: fieldglass_core::array::Group,
    y: fn(f64) -> f64,
    /// 2-D arrays over `(y, x)` the file does not hold, in C order.
    added: Vec<(String, Vec<f64>)>,
}

impl Edited {
    fn new(y: fn(f64) -> f64) -> Self {
        use fieldglass_core::array::ArraySource;
        let bytes = std::fs::read(format!("{NETCDF}/netcdf4_unread_axes.nc")).expect("fixture");
        let reader = fieldglass::netcdf::NetcdfReader::from_bytes(bytes).expect("NetCDF-4");
        let arrays = fieldglass::netcdf::NetcdfArrays::open(reader).expect("opens");
        Self {
            group: arrays.group().clone(),
            arrays,
            y,
            added: Vec::new(),
        }
    }

    fn length(&self, dimension: &str) -> usize {
        let d = self
            .group
            .dimensions
            .iter()
            .find(|d| d.name == dimension)
            .expect("a dimension");
        usize::try_from(d.length).expect("fits")
    }
}

impl fieldglass_core::array::ArraySource for Edited {
    fn group(&self) -> &fieldglass_core::array::Group {
        &self.group
    }

    fn read_region(
        &self,
        array: &str,
        region: &[std::ops::Range<u64>],
    ) -> Result<Vec<Option<f64>>, fieldglass_core::FieldglassError> {
        if let Some((_, values)) = self.added.iter().find(|(name, _)| name == array) {
            let columns = self.length("x") as u64;
            let (rows, cols) = (region[0].clone(), region[1].clone());
            return Ok(rows
                .flat_map(|r| cols.clone().map(move |c| r * columns + c))
                .map(|at| Some(values[at as usize]))
                .collect());
        }
        let values = self.arrays.read_region(array, region)?;
        if array != "y" {
            return Ok(values);
        }
        Ok(values.into_iter().map(|v| v.map(self.y)).collect())
    }

    fn attributes_not_read(&self, array: &str) -> Option<String> {
        self.arrays.attributes_not_read(array)
    }

    fn unread_attribute_names(
        &self,
        array: &str,
    ) -> Option<fieldglass_core::array::UnreadAttributeNames> {
        self.arrays.unread_attribute_names(array)
    }
}

/// `tas`'s note in [`fieldglass::variables_of`].
fn tas_note(source: &Edited) -> String {
    fieldglass::variables_of(source)
        .into_iter()
        .find(|v| v.name == "tas")
        .and_then(|v| v.placed_by_name)
        .expect("tas is placed by name")
}

/// The values clause is said only when the slice is not placed: `tas` given
/// a 2-D pair of possible degrees is placed by it, so its `y` is named for
/// being read by its name, and nothing is said of its values (#1719).
#[test]
fn an_axis_whose_values_are_no_degrees_is_named_for_them_only_when_nothing_places_the_field() {
    use fieldglass_core::array::{ArraySource, Attribute};
    let unplaced = Edited::new(|v| v);
    assert!(tas_note(&unplaced).contains("from -1000 to 1000, past the poles"));

    let mut placed = Edited::new(|v| v);
    let (rows, columns) = (placed.length("y"), placed.length("x"));
    let lats: Vec<f64> = (0..rows * columns)
        .map(|at| -45.0 + 90.0 * (at / columns) as f64 / rows as f64)
        .collect();
    let lons: Vec<f64> = (0..rows * columns)
        .map(|at| 180.0 * (at % columns) as f64 / columns as f64)
        .collect();
    for (name, units) in [("lat2d", "degrees_north"), ("lon2d", "degrees_east")] {
        let mut array = placed
            .group
            .arrays
            .iter()
            .find(|a| a.name == "tas")
            .expect("tas")
            .clone();
        array.name = name.to_string();
        array.dimensions = vec!["y".to_string(), "x".to_string()];
        array.attributes = vec![Attribute::text("units", units)];
        placed.group.arrays.push(array);
    }
    placed.added = vec![("lat2d".to_string(), lats), ("lon2d".to_string(), lons)];
    placed
        .group
        .arrays
        .iter_mut()
        .find(|a| a.name == "tas")
        .expect("tas")
        .attributes
        .push(Attribute::text("coordinates", "lat2d lon2d"));
    let tas = placed
        .group()
        .arrays
        .iter()
        .find(|a| a.name == "tas")
        .expect("tas");
    let (y, x) = (
        tas.dimensions.iter().position(|d| d == "y").expect("y"),
        tas.dimensions.iter().position(|d| d == "x").expect("x"),
    );
    let geometry = fieldglass_core::cf::slice_placement(&placed, "tas", y, x)
        .expect("resolves")
        .geometry;
    assert!(
        matches!(
            geometry,
            fieldglass_core::projection::GridGeometry::Lookup(_)
        ),
        "the premise: the pair places tas, as {geometry:?}"
    );
    assert_eq!(
        tas_note(&placed),
        "\"y\" is read as latitude from its name alone: its attributes \
         \"standard_name\", \"units\" were not read; \"x\" is read as longitude from \
         its name alone: its attributes \"standard_name\", \"units\" were not read"
    );
}

/// A coordinate stored near the edge of `f64` reads in a few digits, not
/// three hundred (#1719).
#[test]
fn values_no_degree_can_hold_read_in_a_few_digits() {
    let huge = Edited::new(|v| v * 1e305);
    let note = tas_note(&huge);
    assert!(
        note.contains("its values run from -1e308 to 1e308, past the poles,"),
        "{note}"
    );
    let fraction = Edited::new(|v| v / 3.0 + 90.5);
    let note = tas_note(&fraction);
    assert!(
        note.contains("its values run from -242.833 to 423.833, past the poles,"),
        "{note}"
    );
}

/// The names the session decides from are the reader's structured record
/// (#1687): `y` and `x` lost two attributes each, left out, and `time` and
/// `step` one each, read in part.
#[test]
fn the_unread_names_are_the_readers() {
    use fieldglass_core::array::ArraySource;
    let bytes = std::fs::read(format!("{NETCDF}/netcdf4_unread_axes.nc")).expect("fixture");
    let reader = fieldglass::netcdf::NetcdfReader::from_bytes(bytes).expect("NetCDF-4");
    let arrays = fieldglass::netcdf::NetcdfArrays::open(reader).expect("opens");
    for (array, names, part) in [
        ("y", &["standard_name", "units"][..], &[][..]),
        ("x", &["standard_name", "units"], &[]),
        ("time", &["units"], &["units"]),
        ("step", &["calendar"], &["calendar"]),
    ] {
        let lost = arrays.unread_attribute_names(array).expect(array);
        let mut all: Vec<&str> = lost.names().collect();
        all.sort_unstable();
        assert_eq!(all, names, "{array}");
        assert_eq!(lost.read_in_part().collect::<Vec<_>>(), part, "{array}");
        assert_eq!(lost.unnamed(), 0);
        assert!(arrays.attributes_not_read(array).is_some(), "{array}");
    }
    for array in ["tas", "pr"] {
        assert_eq!(arrays.unread_attribute_names(array), None, "{array}");
    }
}

/// `netcdf4_string_attributes.nc` as a container that lost attributes whose
/// names it could not read, as a NetCDF-4 dense attribute past the heap
/// object cap is lost (#997): `time` without its `calendar` and `lat`
/// without its `units`, each with one unnamed loss reported. The container
/// is the real reader's; only the loss is staged, since no committed file
/// loses an unnamed attribute on a coordinate.
struct LostUnnamed {
    arrays: fieldglass::netcdf::NetcdfArrays,
    group: fieldglass_core::array::Group,
    staged: &'static [(&'static str, &'static str)],
}

impl LostUnnamed {
    fn new() -> Self {
        Self::staging(
            "netcdf4_string_attributes.nc",
            &[("time", "calendar"), ("lat", "units")],
        )
    }

    /// `file` with each `(array, attribute)` of `staged` lost, unnamed.
    fn staging(file: &str, staged: &'static [(&'static str, &'static str)]) -> Self {
        let bytes = std::fs::read(format!("{NETCDF}/{file}")).expect("fixture");
        let reader = fieldglass::netcdf::NetcdfReader::from_bytes(bytes).expect("NetCDF-4");
        let arrays = fieldglass::netcdf::NetcdfArrays::open(reader).expect("opens");
        let mut group = fieldglass_core::array::ArraySource::group(&arrays).clone();
        for &(array, lost) in staged {
            let array = group
                .arrays
                .iter_mut()
                .find(|a| a.name == array)
                .expect("the fixture's array");
            assert!(array.attributes.iter().any(|a| a.name == lost), "{lost}");
            array.attributes.retain(|a| a.name != lost);
        }
        Self {
            arrays,
            group,
            staged,
        }
    }
}

impl fieldglass_core::array::ArraySource for LostUnnamed {
    fn group(&self) -> &fieldglass_core::array::Group {
        &self.group
    }

    fn read_region(
        &self,
        array: &str,
        region: &[std::ops::Range<u64>],
    ) -> Result<Vec<Option<f64>>, fieldglass_core::FieldglassError> {
        self.arrays.read_region(array, region)
    }

    fn attributes_not_read(&self, array: &str) -> Option<String> {
        self.staged
            .iter()
            .any(|(a, _)| *a == array)
            .then(|| "an attribute not read".to_string())
    }

    fn unread_attribute_names(
        &self,
        array: &str,
    ) -> Option<fieldglass_core::array::UnreadAttributeNames> {
        self.attributes_not_read(array)
            .map(|_| fieldglass_core::array::UnreadAttributeNames::new(Vec::new(), Vec::new(), 1))
    }
}

/// An attribute a time needs, absent from an array that lost attributes
/// whose names were not read, may be one of them (#1687): `time` is not
/// dated as `standard` on a calendar that may have been lost, and `lat`,
/// read as latitude by its name, says so on the variable drawn over it.
#[test]
fn an_attribute_that_may_be_among_the_unnamed_losses_says_so() {
    let source = LostUnnamed::new();
    let time = fieldglass::axis_values(&source, "temperature", 0).expect("time");
    assert_eq!(time.calendar, None);
    assert_eq!(
        time.not_dated.as_deref(),
        Some("not shown as a date: its calendar may not have been read")
    );
    let lat = fieldglass::axis_values(&source, "temperature", 1).expect("lat");
    assert_eq!(
        lat.not_dated.as_deref(),
        Some("its units may not have been read, so it is not shown as a date")
    );
    let lon = fieldglass::axis_values(&source, "temperature", 2).expect("lon");
    assert_eq!(lon.not_dated, None, "lon lost nothing");
    let temperature = fieldglass::variables_of(&source)
        .into_iter()
        .find(|v| v.name == "temperature")
        .expect("listed");
    assert_eq!(
        temperature.placed_by_name.as_deref(),
        Some(
            "\"lat\" is read as latitude from its name alone: some of its attributes were not read"
        )
    );
}

/// The 2-D pair a variable's `coordinates` attribute names is held to the
/// same rule as a 1-D axis (#1687): MiRS's `Latitude` read by its name,
/// with its `units` among attributes whose names were not read.
#[test]
fn a_curvilinear_coordinate_read_by_its_name_is_named_on_the_variable() {
    let source = LostUnnamed::staging("mirs_swath_n21.nc", &[("Latitude", "units")]);
    let tpw = fieldglass::variables_of(&source)
        .into_iter()
        .find(|v| v.name == "TPW")
        .expect("listed");
    assert_eq!(
        tpw.placed_by_name.as_deref(),
        Some(
            "\"Latitude\" is read as latitude from its name alone: some of its attributes were \
             not read"
        )
    );
}
