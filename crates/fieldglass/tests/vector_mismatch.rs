//! `Session::vector_mismatch`: whether the two fields arrows are drawn from
//! are at one level and one time, through the real readers (#1140).
//!
//! The GRIB cases read `uv_mismatch.grib2`, `uv_mismatch.grib1` and
//! `uv_members.grib2`, a u and several v built from committed messages by
//! `tools/build_conformance_uv_fixtures.py` (see `fixtures/NOTICE.md`). The
//! cases the files do not hold, a GRIB2 layer and a surface that names a
//! place, rewrite a committed message in memory, as `vector_pairing.rs` does.
//! The Zarr stores are written in memory with the coordinates on their
//! variables.

use fieldglass::{Session, VectorMismatch, VectorPairing};

const UV2: &[u8] = include_bytes!("fixtures/uv_mismatch.grib2");
const UV1: &[u8] = include_bytes!("fixtures/uv_mismatch.grib1");
const MEMBERS: &[u8] = include_bytes!("fixtures/uv_members.grib2");
const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");

const TEN_M: &str = "10 m above ground";
const P850: &str = "850 hPa";
const GROUND: &str = "Ground or water surface";

fn pair(a: &str, b: &str) -> Option<[String; 2]> {
    Some([a.to_string(), b.to_string()])
}

fn mismatch(s: &Session, u: u32, v: u32) -> (Option<[String; 2]>, Option<[String; 2]>) {
    let m = s.vector_mismatch(u, v).expect("answers");
    (m.level, m.time)
}

#[test]
fn grib2_levels_and_times() {
    let s = Session::open(UV2.to_vec()).expect("opens");
    assert_eq!(s.count(), 14);
    assert_eq!(mismatch(&s, 0, 1), (None, None), "one level, one time");
    assert_eq!(mismatch(&s, 0, 2), (pair(TEN_M, P850), None), "two levels");
    assert_eq!(
        mismatch(&s, 0, 3),
        (None, pair("+204h", "+210h")),
        "two steps"
    );
    assert_eq!(
        mismatch(&s, 0, 4),
        (pair(TEN_M, P850), pair("+204h", "+210h")),
        "both"
    );
    // u's order first, whichever way round they are asked.
    assert_eq!(mismatch(&s, 2, 0), (pair(P850, TEN_M), None));
    // 10 m stated as 100 with scale factor 1 is the same height.
    assert_eq!(
        mismatch(&s, 0, 5),
        (None, None),
        "one height, two spellings"
    );
    // Valid at one instant, from two runs: two times, and both reference times
    // are said because they differ.
    assert_eq!(
        mismatch(&s, 0, 6),
        (
            None,
            pair("2012-03-02T06:00:00Z +204h", "2012-03-02T12:00:00Z +198h")
        ),
        "two runs"
    );
    let runs = s.vector_mismatch(0, 6).expect("answers").run;
    assert_eq!(runs, pair("2012-03-02T06:00:00Z", "2012-03-02T12:00:00Z"));
    for v in 1..6 {
        assert_eq!(
            s.vector_mismatch(0, v).expect("answers").run,
            None,
            "#{v}: one run"
        );
    }
    // 0.3 m stated as 3 with scale factor 1 and as 30 with scale factor 2 is
    // one height, compared on the codes, not on `0.30000000000000004`.
    assert_eq!(
        mismatch(&s, 7, 8),
        (None, None),
        "one height, two spellings"
    );
    assert_eq!(
        mismatch(&s, 7, 0),
        (pair("0.3 m above ground", TEN_M), None)
    );
    // A surface that names a place against a height: the name, not a value.
    assert_eq!(mismatch(&s, 0, 9), (pair(TEN_M, GROUND), None));
    // Nothing to compare: a first surface coded missing, and a height whose
    // value is coded missing.
    assert_eq!(mismatch(&s, 0, 10), (None, None), "a missing surface");
    assert_eq!(mismatch(&s, 0, 11), (None, None), "a missing value");
    // A layer down to the ground: the ground's value octets mean nothing.
    assert_eq!(mismatch(&s, 12, 13), (None, None), "one layer");
    assert_eq!(
        mismatch(&s, 12, 0),
        (pair(&format!("{TEN_M} to {GROUND}"), TEN_M), None),
        "a layer is not its top alone"
    );
}

#[test]
fn grib1_levels_and_times() {
    let s = Session::open(UV1.to_vec()).expect("opens");
    assert_eq!(s.count(), 7);
    assert_eq!(mismatch(&s, 0, 1), (None, None));
    assert_eq!(mismatch(&s, 0, 2), (pair(TEN_M, P850), None));
    assert_eq!(mismatch(&s, 0, 3), (None, pair("+0h", "+6h")));
    assert_eq!(mismatch(&s, 0, 4), (pair(TEN_M, P850), pair("+0h", "+6h")));
    // Level type 1, the ground, takes no value: its octets are not compared.
    assert_eq!(mismatch(&s, 5, 6), (None, None), "one level, two octets");
    let ground = s.message(5).expect("5").level_type.expect("named");
    assert_eq!(mismatch(&s, 0, 5), (pair(TEN_M, &ground), None));
}

/// Two members of one ensemble, by the perturbation number of template 4.1,
/// at one level and one time.
#[test]
fn grib2_ensemble_members() {
    let s = Session::open(MEMBERS.to_vec()).expect("opens");
    assert_eq!(s.count(), 3);
    let m = s.vector_mismatch(0, 1).expect("answers");
    assert_eq!(
        (m.level, m.time, m.run, m.ensemble),
        (None, None, None, None)
    );
    let m = s.vector_mismatch(0, 2).expect("answers");
    assert_eq!((m.level, m.time, m.run), (None, None, None));
    assert_eq!(m.ensemble, pair("1", "7"));
    // Not a member: nothing to compare.
    let two = Session::open([UV2, MEMBERS].concat()).expect("opens");
    assert_eq!(two.vector_mismatch(0, 16).expect("answers").ensemble, None);
}

/// The parameters' answer does not move: every v here is the u's partner,
/// whatever its level or time (#1140, 2026-11-06 triage).
#[test]
fn the_pairing_answer_is_unchanged() {
    // Each file's u messages; every other message is a v.
    for (bytes, count, us) in [
        (UV2, 14, &[0, 7, 12][..]),
        (UV1, 7, &[0][..]),
        (MEMBERS, 3, &[0][..]),
    ] {
        let s = Session::open(bytes.to_vec()).expect("opens");
        for &u in us {
            for v in (1..count).filter(|v| !us.contains(v)) {
                assert_eq!(
                    s.vector_pairing(u, v).expect("answers"),
                    VectorPairing::Recognized,
                    "#{u} with #{v}"
                );
            }
        }
    }
}

#[test]
fn an_index_past_the_end_is_refused() {
    let s = Session::open(UV2.to_vec()).expect("opens");
    for (u, v) in [(0, 14), (14, 0)] {
        assert!(
            matches!(
                s.vector_mismatch(u, v),
                Err(fieldglass::Error::NoSuchMessage {
                    index: 14,
                    count: 14
                })
            ),
            "{u} with {v}"
        );
    }
    let z = Session::open_store(store(&[("u", &["lat", "lon"], None)], &[])).expect("opens");
    assert!(matches!(
        z.vector_mismatch(0, 3),
        Err(fieldglass::Error::NoSuchMessage { index: 3, .. })
    ));
}

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

/// The GFS message with its two fixed surfaces (§4 octets 23-28 and 29-34)
/// rewritten, each as type, scale factor and scaled value.
fn gfs_at(first: (u8, u8, u32), second: (u8, u8, u32)) -> Vec<u8> {
    let mut msg = GFS.to_vec();
    let s4 = grib2_section(&msg, 4);
    for (at, (kind, scale, value)) in [(s4 + 22, first), (s4 + 28, second)] {
        msg[at] = kind;
        msg[at + 1] = scale;
        msg[at + 2..at + 6].copy_from_slice(&value.to_be_bytes());
    }
    msg
}

/// A GRIB2 layer is both its surfaces, so two layers with one top and two
/// bottoms are two levels, and the note says the bottoms.
#[test]
fn a_grib2_layer_is_compared_by_both_surfaces() {
    let s = Session::open(
        [
            gfs_at((100, 0, 100_000), (100, 0, 85_000)),
            gfs_at((100, 0, 100_000), (100, 0, 50_000)),
            gfs_at((100, 0, 100_000), (100, 0, 85_000)),
        ]
        .concat(),
    )
    .expect("opens");
    assert_eq!(
        mismatch(&s, 0, 1),
        (pair("1000 hPa to 850 hPa", "1000 hPa to 500 hPa",), None)
    );
    assert_eq!(mismatch(&s, 0, 2), (None, None));
}

/// A surface that names a place (Code Table 4.5's 1, the ground) is the same
/// level whatever value octets it carries: both read "Ground or water
/// surface", so there is nothing to say.
#[test]
fn a_named_surface_is_one_level_whatever_its_value() {
    let missing = (255, 255, u32::MAX);
    let s = Session::open([gfs_at((1, 0, 0), missing), gfs_at((1, 0, 7), missing)].concat())
        .expect("opens");
    assert_eq!(s.message(1).expect("1").level.as_deref(), Some(GROUND));
    assert_eq!(mismatch(&s, 0, 1), (None, None));
}

/// A Zarr v2 store of 2 × 2 fields over `lat` and `lon`. Each variable is
/// `(name, dimensions, coordinates attribute)`; each coordinate is
/// `(name, dimensions, values, attributes)`, a 0-D array when its dimension
/// list is empty and an axis of that many values otherwise.
type Coordinate<'a> = (&'a str, &'a [&'a str], &'a [f64], serde_json::Value);

fn store(
    variables: &[(&str, &[&str], Option<&str>)],
    coordinates: &[Coordinate<'_>],
) -> fieldglass::MemoryObjects {
    let array = |shape: &str, chunks: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{chunks},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut objects = vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("lat/.zarray".to_string(), array("[2]", "[2]")),
        (
            "lat/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["lat"],"units":"degrees_north"}"#.to_vec(),
        ),
        ("lat/0".to_string(), f64s(&[10.0, 20.0])),
        ("lon/.zarray".to_string(), array("[2]", "[2]")),
        (
            "lon/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["lon"],"units":"degrees_east"}"#.to_vec(),
        ),
        ("lon/0".to_string(), f64s(&[30.0, 40.0])),
    ];
    for (name, dims, values, attrs) in coordinates {
        let mut attrs = attrs.clone();
        attrs["_ARRAY_DIMENSIONS"] = serde_json::json!(dims);
        let shape = if dims.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", values.len())
        };
        objects.push((format!("{name}/.zarray"), array(&shape, &shape)));
        objects.push((format!("{name}/.zattrs"), attrs.to_string().into_bytes()));
        objects.push((format!("{name}/0"), f64s(values)));
    }
    let length = |dim: &str| {
        coordinates
            .iter()
            .find(|(name, dims, ..)| *name == dim && dims == &[dim])
            .map_or(1, |(_, _, values, _)| values.len())
    };
    for (name, dims, coordinates) in variables {
        let mut attrs = serde_json::json!({ "_ARRAY_DIMENSIONS": dims });
        if let Some(c) = coordinates {
            attrs["coordinates"] = (*c).into();
        }
        let shape: Vec<usize> = dims
            .iter()
            .map(|d| {
                if matches!(*d, "lat" | "lon") {
                    2
                } else {
                    length(d)
                }
            })
            .collect();
        let cells = shape.iter().product();
        let shape = serde_json::to_string(&shape).expect("a shape");
        let chunk = vec!["0"; dims.len()].join(".");
        objects.push((format!("{name}/.zarray"), array(&shape, &shape)));
        objects.push((format!("{name}/.zattrs"), attrs.to_string().into_bytes()));
        objects.push((format!("{name}/{chunk}"), f64s(&vec![1.0; cells])));
    }
    fieldglass::MemoryObjects::from_iter(objects)
}

/// The held coordinates of a variable: a CF scalar coordinate its
/// `coordinates` attribute names, and an axis of length 1.
#[test]
fn a_store_compares_the_coordinates_each_variable_is_held_at() {
    use serde_json::json;
    let height = json!({ "units": "m", "positive": "up", "standard_name": "height" });
    let feet = json!({ "units": "ft", "positive": "up", "standard_name": "height" });
    let member = json!({ "standard_name": "realization", "units": "1" });
    let pressure = json!({ "units": "Pa", "standard_name": "air_pressure" });
    let time = json!({ "units": "hours since 2000-01-01 00:00:00", "standard_name": "time" });
    let s = Session::open_store(store(
        &[
            ("u10", &["lat", "lon"], Some("height time")),
            ("v10", &["lat", "lon"], Some("height time")),
            ("v850", &["lat", "lon"], Some("plev time")),
            ("v10_later", &["lat", "lon"], Some("height later")),
            // 10 again, in feet: one number, two heights.
            ("v10ft", &["lat", "lon"], Some("height_ft time")),
            ("u_m1", &["lat", "lon"], Some("height time member1")),
            ("v_m1", &["lat", "lon"], Some("height time member1")),
            ("v_m7", &["lat", "lon"], Some("height time member7")),
            // The same as `v850`, by an axis of length 1 rather than a scalar.
            ("v_axis", &["level", "lat", "lon"], Some("time")),
            // On two levels: the host's slice holds one, so the variable holds
            // none, and its first is not compared with `v850`'s 85000.
            ("u_levels", &["levels", "lat", "lon"], Some("time")),
            // Nothing held: there is nothing to compare.
            ("bare", &["lat", "lon"], None),
        ],
        &[
            ("height", &[], &[10.0], height),
            ("height_ft", &[], &[10.0], feet),
            ("member1", &[], &[1.0], member.clone()),
            ("member7", &[], &[7.0], member),
            ("plev", &[], &[85_000.0], pressure.clone()),
            ("level", &["level"], &[85_000.0], pressure.clone()),
            ("levels", &["levels"], &[10.0, 85_000.0], pressure),
            ("time", &[], &[6.0], time.clone()),
            ("later", &[], &[12.0], time),
        ],
    ))
    .expect("opens");
    let index = |name: &str| {
        s.variables()
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} is offered"))
            .index
    };
    let ask = |u: &str, v: &str| mismatch(&s, index(u), index(v));
    assert_eq!(ask("u10", "v10"), (None, None));
    assert_eq!(ask("u10", "v850"), (pair("10 m", "85000 Pa"), None));
    assert_eq!(
        ask("u10", "v10_later"),
        (
            None,
            pair(
                "6 hours since 2000-01-01 00:00:00",
                "12 hours since 2000-01-01 00:00:00"
            )
        )
    );
    assert_eq!(ask("u10", "v_axis"), (pair("10 m", "85000 Pa"), None));
    assert_eq!(
        ask("v850", "v_axis"),
        (None, None),
        "a scalar and an axis alike"
    );
    assert_eq!(ask("u10", "bare"), (None, None), "nothing to compare");
    assert_eq!(
        ask("u_levels", "v850"),
        (None, None),
        "an axis of two values is the slice's"
    );
    assert_eq!(ask("u_levels", "u10"), (None, None));
    assert_eq!(
        ask("u10", "v10ft"),
        (pair("10 m", "10 ft"), None),
        "units differ"
    );
    let members = |u: &str, v: &str| {
        let m = s.vector_mismatch(index(u), index(v)).expect("answers");
        (m.level, m.time, m.run, m.ensemble)
    };
    assert_eq!(members("u_m1", "v_m1"), (None, None, None, None));
    // A member number is worded bare, whatever units the coordinate states.
    assert_eq!(members("u_m1", "v_m7"), (None, None, None, pair("1", "7")));
    assert_eq!(
        members("u10", "v_m7"),
        (None, None, None, None),
        "u is no member"
    );
}

#[test]
fn the_wire_form() {
    let s = Session::open(UV2.to_vec()).expect("opens");
    let m: VectorMismatch = s.vector_mismatch(0, 4).expect("answers");
    assert_eq!(
        serde_json::to_value(&m).expect("serializes"),
        serde_json::json!({
            "level": [TEN_M, P850],
            "time": ["+204h", "+210h"],
            "run": null,
            "ensemble": null,
        })
    );
    assert_eq!(
        serde_json::to_value(s.vector_mismatch(0, 1).expect("answers")).expect("serializes"),
        serde_json::json!({ "level": null, "time": null, "run": null, "ensemble": null })
    );
}
