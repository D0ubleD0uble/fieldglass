//! `Session::vector_pairing`: whether the two fields arrows are drawn from are
//! a u/v pair, through the real readers (#1049).
//!
//! Every committed GRIB fixture holds one message and none of them is a wind
//! component, so the pairs are built the way the release plan builds its
//! samples: a committed message with its parameter octets rewritten, the
//! messages concatenated into one file. The Zarr store is written in memory
//! with the CF standard names on its variables.

use fieldglass::{Session, VectorPairing};

const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
const CMC: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib");
const ECMWF: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");

/// The offset of a GRIB2 message's section `number`.
fn grib2_section(msg: &[u8], number: u8) -> usize {
    let mut at = 16;
    while &msg[at..at + 4] != b"7777" {
        if msg[at + 4] == number {
            return at;
        }
        at += u32::from_be_bytes(msg[at..at + 4].try_into().expect("four bytes")) as usize;
    }
    panic!("no section {number}");
}

/// The GFS message as discipline-0 parameter `category`-`number` (§4 octets
/// 10 and 11).
fn grib2_as(category: u8, number: u8) -> Vec<u8> {
    let mut msg = GFS.to_vec();
    let s4 = grib2_section(&msg, 4);
    assert_eq!(msg[6], 0, "discipline 0");
    msg[s4 + 9] = category;
    msg[s4 + 10] = number;
    msg
}

/// A GRIB1 message with its PDS parameter (octet 9) rewritten.
fn grib1_as(msg: &[u8], parameter: u8) -> Vec<u8> {
    let mut msg = msg.to_vec();
    msg[8 + 8] = parameter;
    msg
}

fn session(messages: &[Vec<u8>]) -> Session {
    Session::open(messages.concat()).expect("opens")
}

fn assert_pairings(s: &Session, cases: &[(u32, u32, VectorPairing)], what: &str) {
    for &(u, v, expected) in cases {
        let names = (
            s.message(u).expect("u").parameter,
            s.message(v).expect("v").parameter,
        );
        assert_eq!(
            s.vector_pairing(u, v).expect("answers"),
            expected,
            "{what}: #{u} with #{v}, {names:?}"
        );
    }
}

#[test]
fn grib2_wind_current_and_storm_motion() {
    use VectorPairing::{NotAPair, Recognized, Swapped};
    let s = session(&[
        grib2_as(2, 2),  // 0 UGRD
        grib2_as(2, 3),  // 1 VGRD
        grib2_as(2, 27), // 2 u storm motion
        grib2_as(2, 28), // 3 v storm motion
        grib2_as(2, 1),  // 4 wind speed
        grib2_as(2, 0),  // 5 wind direction
        GFS.to_vec(),    // 6 as committed: a pressure
    ]);
    assert_pairings(
        &s,
        &[
            (0, 1, Recognized),
            (1, 0, Swapped),
            (2, 3, Recognized),
            (3, 2, Swapped),
            (0, 3, NotAPair),
            (4, 5, NotAPair),
            (4, 4, NotAPair),
            (0, 0, NotAPair),
            (6, 1, NotAPair),
        ],
        "GRIB2",
    );
}

/// Two GFS messages rewritten to NCEP local codes 0-2-250 and 0-2-251, which
/// no table names (NCEP's category 2 ends at 237), are shown as `Parameter
/// 0/2/250` and `Parameter 0/2/251`. That is a placeholder, not a name, so
/// whether they pair can't be told (#1142); against a named non-component it
/// can.
#[test]
fn grib2_unnamed_local_parameters_are_unknown() {
    use VectorPairing::{NotAPair, Recognized, Unknown};
    let s = session(&[
        grib2_as(2, 250), // 0 unnamed
        grib2_as(2, 251), // 1 unnamed
        grib2_as(2, 2),   // 2 UGRD
        grib2_as(2, 3),   // 3 VGRD
        grib2_as(2, 1),   // 4 wind speed
    ]);
    assert_eq!(
        (
            s.message(0).expect("0").parameter.as_deref(),
            s.message(1).expect("1").parameter.as_deref(),
        ),
        (Some("Parameter 0/2/250"), Some("Parameter 0/2/251")),
        "the codes resolve to no table entry"
    );
    assert_pairings(
        &s,
        &[
            (0, 1, Unknown),
            (1, 0, Unknown),
            (0, 0, Unknown),
            (2, 1, Unknown),
            (0, 3, Unknown),
            (4, 1, NotAPair),
            (0, 4, NotAPair),
            (2, 3, Recognized),
        ],
        "GRIB2 unnamed",
    );
}

/// ECMWF table 128 names ids 80 to 120 only `Experimental product`: a
/// placeholder, so a pair of them can't be told either (#1142).
#[test]
fn grib1_placeholder_names_are_unknown() {
    use VectorPairing::{NotAPair, Unknown};
    let s = session(&[
        grib1_as(ECMWF, 80),  // 0 Experimental product
        grib1_as(ECMWF, 81),  // 1 Experimental product
        ECMWF.to_vec(),       // 2 2t
        grib1_as(ECMWF, 166), // 3 10v
    ]);
    assert_eq!(
        s.message(0).expect("0").parameter.as_deref(),
        Some("Experimental product"),
    );
    assert_pairings(
        &s,
        &[
            (0, 1, Unknown),
            (1, 0, Unknown),
            (0, 3, Unknown),
            (2, 1, NotAPair),
        ],
        "ECMWF 128 placeholders",
    );
}

/// GRIB1 the same way: ON388 Table 2 names no parameter 0 or 255.
#[test]
fn grib1_unnamed_parameters_are_unknown() {
    use VectorPairing::{NotAPair, Unknown};
    let s = session(&[
        grib1_as(CMC, 0),   // 0 unnamed
        grib1_as(CMC, 255), // 1 unnamed
        CMC.to_vec(),       // 2 WIND
    ]);
    assert_eq!(
        s.message(0).expect("0").parameter.as_deref(),
        Some("Parameter 54/2/0"),
        "the id resolves to no table entry"
    );
    assert_pairings(&s, &[(0, 1, Unknown), (2, 1, NotAPair)], "GRIB1 unnamed");
}

#[test]
fn grib2_ocean_currents() {
    use VectorPairing::{NotAPair, Recognized, Swapped};
    let mut u = GFS.to_vec();
    let mut v = GFS.to_vec();
    let s4 = grib2_section(&u, 4);
    for (msg, number) in [(&mut u, 2), (&mut v, 3)] {
        msg[6] = 10;
        msg[s4 + 9] = 1;
        msg[s4 + 10] = number;
    }
    let s = session(&[u, v, grib2_as(2, 3)]);
    assert_pairings(
        &s,
        &[(0, 1, Recognized), (1, 0, Swapped), (0, 2, NotAPair)],
        "GRIB2 discipline 10",
    );
}

#[test]
fn grib1_on388_table_2() {
    use VectorPairing::{NotAPair, Recognized, Swapped};
    let s = session(&[
        grib1_as(CMC, 33), // 0 UGRD
        grib1_as(CMC, 34), // 1 VGRD
        CMC.to_vec(),      // 2 WIND, as committed
        grib1_as(CMC, 31), // 3 WDIR
    ]);
    assert_pairings(
        &s,
        &[
            (0, 1, Recognized),
            (1, 0, Swapped),
            (2, 2, NotAPair),
            (2, 3, NotAPair),
            (0, 2, NotAPair),
        ],
        "GRIB1 NCEP",
    );
}

#[test]
fn grib1_ecmwf_table_128() {
    use VectorPairing::{NotAPair, Recognized, Swapped};
    let s = session(&[
        grib1_as(ECMWF, 165), // 0 10u
        grib1_as(ECMWF, 166), // 1 10v
        grib1_as(ECMWF, 131), // 2 u
        grib1_as(ECMWF, 132), // 3 v
        ECMWF.to_vec(),       // 4 2t, as committed
    ]);
    assert_pairings(
        &s,
        &[
            (0, 1, Recognized),
            (1, 0, Swapped),
            (2, 3, Recognized),
            (3, 2, Swapped),
            (0, 3, NotAPair),
            (4, 1, NotAPair),
        ],
        "GRIB1 ECMWF",
    );
}

/// A Zarr v2 store of 2 × 2 variables, one per `(name, standard_name,
/// long_name)`.
fn store(variables: &[(&str, Option<&str>, Option<&str>)]) -> fieldglass::MemoryObjects {
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut objects = vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("lat/.zarray".to_string(), array("[2]")),
        (
            "lat/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["lat"],"units":"degrees_north"}"#.to_vec(),
        ),
        ("lat/0".to_string(), f64s(&[10.0, 20.0])),
        ("lon/.zarray".to_string(), array("[2]")),
        (
            "lon/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["lon"],"units":"degrees_east"}"#.to_vec(),
        ),
        ("lon/0".to_string(), f64s(&[30.0, 40.0])),
    ];
    for (name, standard, long) in variables {
        let mut attrs = serde_json::json!({ "_ARRAY_DIMENSIONS": ["lat", "lon"] });
        if let Some(s) = standard {
            attrs["standard_name"] = (*s).into();
        }
        if let Some(l) = long {
            attrs["long_name"] = (*l).into();
        }
        let attrs = attrs.to_string();
        objects.push((format!("{name}/.zarray"), array("[2,2]")));
        objects.push((format!("{name}/.zattrs"), attrs.into_bytes()));
        objects.push((format!("{name}/0.0"), f64s(&[1.0, 2.0, 3.0, 4.0])));
    }
    fieldglass::MemoryObjects::from_iter(objects)
}

#[test]
fn cf_standard_names_on_a_store() {
    use VectorPairing::{NotAPair, Recognized, Swapped, Unknown};
    let s = Session::open_store(store(&[
        ("u10", Some("eastward_wind"), None),
        ("v10", Some("northward_wind"), None),
        ("uc", Some("sea_water_x_velocity"), None),
        ("vc", Some("sea_water_y_velocity"), None),
        ("t", Some("air_temperature"), None),
        ("plain", None, None),
        ("plain2", None, None),
        ("ue", Some("eastward_wind standard_error"), None),
        ("ve", Some("northward_wind standard_error"), None),
        ("u2", Some("square_of_eastward_wind"), None),
        ("v2", Some("square_of_northward_wind"), None),
        // No standard_name: the long_name is not read.
        ("ul", None, Some("U component of wind")),
        ("vl", None, Some("V component of wind")),
        // Not a standard name and at most one CF modifier.
        ("us", Some("eastward wind"), None),
        ("vs", Some("northward wind"), None),
    ]))
    .expect("opens");
    let index = |name: &str| {
        s.variables()
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} is offered"))
            .index
    };
    for (u, v, expected) in [
        ("u10", "v10", Recognized),
        ("v10", "u10", Swapped),
        ("uc", "vc", Recognized),
        ("vc", "uc", Swapped),
        ("u10", "vc", NotAPair),
        ("t", "v10", NotAPair),
        ("u10", "u10", NotAPair),
        // Per-axis statistics (#1142).
        ("ue", "ve", NotAPair),
        ("u2", "v2", NotAPair),
        // No standard_name to read (#1142).
        ("plain", "plain2", Unknown),
        ("plain", "plain", Unknown),
        ("u10", "plain", Unknown),
        ("t", "plain", NotAPair),
        ("ul", "vl", Unknown),
        ("vl", "ul", Unknown),
        ("u10", "vl", Unknown),
        ("us", "vs", Unknown),
        ("us", "v10", Unknown),
        ("us", "t", NotAPair),
    ] {
        assert_eq!(
            s.vector_pairing(index(u), index(v)).expect("answers"),
            expected,
            "{u} with {v}"
        );
    }
}

#[test]
fn an_index_past_the_end_is_refused() {
    let s = session(&[grib2_as(2, 2)]);
    assert!(matches!(
        s.vector_pairing(0, 1),
        Err(fieldglass::Error::NoSuchMessage { index: 1, count: 1 })
    ));
    let z = Session::open_store(store(&[("u10", Some("eastward_wind"), None)])).expect("opens");
    assert!(matches!(
        z.vector_pairing(5, 0),
        Err(fieldglass::Error::NoSuchMessage { index: 5, .. })
    ));
}

#[test]
fn the_wire_form() {
    for (p, wire) in [
        (VectorPairing::Recognized, "\"recognized\""),
        (VectorPairing::Swapped, "\"swapped\""),
        (VectorPairing::NotAPair, "\"not_a_pair\""),
        (VectorPairing::Unknown, "\"unknown\""),
    ] {
        assert_eq!(serde_json::to_string(&p).expect("serializes"), wire);
    }
}
