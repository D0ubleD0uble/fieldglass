//! A grid parameter no grid can be built from (#823, #844).
//!
//! Two GRIB2 §3 parameters are IEEE floats rather than scaled integers: §3.1's
//! angle of rotation and §3.12's scale factor at the reference point. A NaN or
//! an infinity in either used to build a grid anyway: the rotated grid
//! reported itself placed and reprojectable with NaN bounds and a `NaN` in its
//! PROJ string, and both messages carried a non-finite geometry field, which
//! the Node binding writes as `null` and the browser binding as `NaN`. The grid
//! is now declined, the way a §3.90 with no usable camera is (#823).
//!
//! Finite values can describe nothing too. The scale factor is a ratio of two
//! distances, so it is positive, and a negative one reported a placed grid. An
//! Earth whose stated radius or axes are zero, or whose minor axis is the
//! longer, was refused by the projectors but still wrote `+R=0` or `+a=0 +b=0`
//! into the PROJ string. PROJ refuses all of these. They are declined the same
//! way, and so is the CF geostationary mapping that states them (#844).
//!
//! Each case is a committed fixture with the parameter overwritten.

use fieldglass::{DecodeOptions, MessageInfo, Placement, Session};

const G2: &str = "../fieldglass-grib2/tests/fixtures/";

/// The fixture, its IEEE parameter's offset in the §3 template payload (octet
/// 15 onward), the parameter's name, and finite values that still place.
const CASES: [(&str, usize, &str, [f32; 3]); 2] = [
    // §3.1 octets 81-84: angle of rotation of projection.
    (
        "rotated_latlon_surface.grib2",
        66,
        "angle of rotation",
        [0.0, 30.0, -45.0],
    ),
    // §3.12 octets 48-51: scale factor at the reference point.
    (
        "transverse_mercator_ukv.grib2",
        33,
        "scale factor",
        [1.0, 0.9996, 0.5],
    ),
];

/// The committed fixture with bytes of its §3 template payload replaced.
fn with_template_bytes(fixture: &str, payload_offset: usize, value: &[u8]) -> Vec<u8> {
    let mut bytes = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
    let mut at = 16; // past §0
    while at + 5 <= bytes.len() && &bytes[at..at + 4] != b"7777" {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        if bytes[at + 4] == 3 {
            let field = at + 14 + payload_offset;
            bytes[field..field + value.len()].copy_from_slice(value);
            return bytes;
        }
        at += len;
    }
    panic!("{fixture}: no §3");
}

/// The committed fixture with four bytes of its §3 template payload replaced.
fn with_template_float(fixture: &str, payload_offset: usize, value: f32) -> Vec<u8> {
    with_template_bytes(fixture, payload_offset, &value.to_be_bytes())
}

/// Every non-finite IEEE value: a quiet NaN, a NaN with the sign bit and a
/// payload, and both infinities.
fn non_finite() -> [f32; 4] {
    [
        f32::NAN,
        f32::from_bits(0xffc0_0001),
        f32::INFINITY,
        f32::NEG_INFINITY,
    ]
}

/// Every `null` in a JSON value, by path.
fn nulls(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Null => out.push(path.to_string()),
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                nulls(item, &format!("{path}[{i}]"), out);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                nulls(v, &format!("{path}.{k}"), out);
            }
        }
        _ => {}
    }
}

/// `edited` opens as a raster nothing places, with no PROJ string, no bounds
/// and nothing in its geometry that crosses as `null`, and its values are still
/// `original`'s, on the raster the section declares.
fn assert_declined(edited: Vec<u8>, original: &[u8], what: &str) {
    let session = Session::open(edited).expect("opens");
    let info = session.message(0).expect("message 0");
    let grid = info.grid.as_ref().expect("a GRIB2 message declares a grid");

    // A raster nothing places, as a §3.90 camera that sees no Earth is.
    assert_eq!(grid.placement, Placement::Unplaceable, "{what}");
    assert_eq!(info.placement, Placement::Unplaceable, "{what}");
    assert!(!grid.reprojectable && !info.reprojectable, "{what}");
    assert_eq!(grid.kind, "unsupported", "{what}");
    assert_eq!(grid.bounds_lonlat, None, "{what}");
    assert_eq!(grid.proj4, None, "{what}");
    // Still a raster of the size the section declares: a host sizes what
    // it draws from these (#823 review, where they were 0 x 0).
    let declared = Session::open(original.to_vec())
        .expect("opens")
        .message(0)
        .expect("message 0")
        .grid
        .map(|g| (g.ni, g.nj))
        .expect("a grid");
    let declared = (declared.0.expect("columns"), declared.1.expect("rows"));
    assert!(declared.0 > 1 && declared.1 > 1, "{what}");
    assert_eq!(
        (grid.ni, grid.nj),
        (Some(declared.0), Some(declared.1)),
        "{what}"
    );
    let placed = session.place_message(0).expect("places");
    assert_eq!(
        (placed.ni, placed.nj),
        (Some(declared.0), Some(declared.1)),
        "{what}"
    );
    assert_eq!(placed.placement, Placement::Unplaceable, "{what}");
    assert_eq!(placed.proj4, None, "{what}");
    // The values are still the file's, laid out in grid coordinates:
    // what `unplaceable` promises a host that offers a source render.
    let field = session
        .decode(0, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(field.georef.placement, Placement::Unplaceable, "{what}");
    assert!(!field.georef.reprojectable, "{what}");
    assert_eq!(field.georef.bounds_lonlat, None, "{what}");
    assert_eq!(field.georef.proj4, None, "{what}");
    assert_eq!(
        (field.georef.ni, field.georef.nj),
        (Some(declared.0), Some(declared.1)),
        "{what}"
    );
    let want = Session::open(original.to_vec())
        .expect("opens")
        .decode(0, &DecodeOptions::default())
        .expect("the unedited fixture decodes");
    assert_eq!((field.ni, field.nj), (want.ni, want.nj), "{what}");
    assert_eq!(field.values, want.values, "{what}");
    assert_eq!(field.mask, want.mask, "{what}");

    // The geometry the message carries holds no non-finite number, so
    // nothing in it becomes `null` on the way out.
    let wire = serde_json::to_value(&info).expect("serialises");
    let mut found = Vec::new();
    nulls(&wire["grid"]["geometry"], "grid.geometry", &mut found);
    assert!(found.is_empty(), "{what}: null at {found:?}");
    // And what crosses reads back as the message it came from, which a
    // NaN that became `null` cannot: the Node binding's wire form is
    // this `serde_json` value, the browser's is the struct itself.
    let back: MessageInfo = serde_json::from_value(wire).expect("reads back");
    assert_eq!(back, info, "{what}");
}

#[test]
fn a_non_finite_grid_parameter_declines_the_grid() {
    for (fixture, offset, name, _) in CASES {
        let original = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
        for value in non_finite() {
            let what = format!("{fixture}, {name} = {value} ({:#010x})", value.to_bits());
            assert_declined(
                with_template_float(fixture, offset, value),
                &original,
                &what,
            );
        }
    }
}

/// The same parameters, finite, still place: the check is on finiteness, not
/// on the value.
#[test]
fn a_finite_grid_parameter_still_places() {
    for (fixture, offset, name, finite) in CASES {
        for value in finite {
            let session =
                Session::open(with_template_float(fixture, offset, value)).expect("opens");
            let info = session.message(0).expect("message 0");
            assert_eq!(
                info.placement,
                Placement::Placed,
                "{fixture}, {name} = {value}"
            );
        }
    }
}

/// A §3.12 scale factor that is finite but not positive. WMO defines it as
/// the ratio of a distance on the map to the distance on the spheroid, and
/// PROJ refuses `+k_0` at or below zero. On master `-1` and `-f32::MAX`
/// reported `Placed`; zero was unplaceable but still wrote `+k_0=0`.
#[test]
fn a_scale_factor_that_is_not_positive_declines_the_grid() {
    let fixture = "transverse_mercator_ukv.grib2";
    let original = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
    for value in [-1.0, -0.9996, -f32::MAX, -f32::MIN_POSITIVE, 0.0, -0.0] {
        let what = format!(
            "{fixture}, scale factor = {value} ({:#010x})",
            value.to_bits()
        );
        assert_declined(with_template_float(fixture, 33, value), &original, &what);
    }
}

/// A Code Table 3.2 shape-of-earth group (§3 octets 15-30): the shape code,
/// then the spherical radius, the major axis and the minor axis, each a
/// scale factor and a scaled value, or all ones where it is not given.
fn earth(
    shape: u8,
    radius: Option<(u8, u32)>,
    major: Option<(u8, u32)>,
    minor: Option<(u8, u32)>,
) -> [u8; 16] {
    let mut out = [0xFFu8; 16];
    out[0] = shape;
    for (at, pair) in [(1, radius), (6, major), (11, minor)] {
        if let Some((scale, value)) = pair {
            out[at] = scale;
            out[at + 1..at + 5].copy_from_slice(&value.to_be_bytes());
        }
    }
    out
}

/// Shapes whose stated Earth is no body.
fn shapeless_earths() -> [(&'static str, [u8; 16]); 5] {
    [
        ("shape 1, radius 0", earth(1, Some((0, 0)), None, None)),
        (
            "shape 3, axes 0 km",
            earth(3, None, Some((0, 0)), Some((0, 0))),
        ),
        (
            "shape 7, axes 0 m",
            earth(7, None, Some((0, 0)), Some((0, 0))),
        ),
        (
            "shape 7, minor axis 0 m",
            earth(7, None, Some((0, 6_378_137)), Some((0, 0))),
        ),
        (
            "shape 7, minor axis longer than major",
            earth(7, None, Some((0, 6_356_752)), Some((0, 6_378_137))),
        ),
    ]
}

/// Shapes a producer really states: the same codes, describing an Earth.
fn healthy_earths() -> [(&'static str, [u8; 16]); 2] {
    [
        (
            "shape 1, radius 6371229 m",
            earth(1, Some((0, 6_371_229)), None, None),
        ),
        (
            "shape 7, WGS84 axes",
            earth(7, None, Some((0, 6_378_137)), Some((0, 6_356_752))),
        ),
    ]
}

/// Every committed fixture whose template projects on the stated Earth:
/// §3.30, §3.20, §3.12 and §3.140. (§3.90 is below, built from the lat/lon
/// fixture; the geographic templates do not use the radius.)
const PROJECTED: [&str; 4] = [
    "eta_lambert_msg0.grib2",
    "polar_stereographic_surface.grib2",
    "transverse_mercator_ukv.grib2",
    "lambert_azimuthal_efas.grib2",
];

#[test]
fn an_earth_that_is_no_body_declines_the_grid() {
    for fixture in PROJECTED {
        let original = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
        for (name, shape) in shapeless_earths() {
            assert_declined(
                with_template_bytes(fixture, 0, &shape),
                &original,
                &format!("{fixture}, {name}"),
            );
        }
    }
}

/// The same codes stating a real Earth keep the grid the fixture had: the
/// check is on the body, not on the shape code.
#[test]
fn a_stated_earth_that_is_a_body_still_builds_the_grid() {
    for fixture in PROJECTED {
        let want = Session::open(std::fs::read(format!("{G2}{fixture}")).expect("fixture"))
            .expect("opens")
            .message(0)
            .expect("message 0");
        for (name, shape) in healthy_earths() {
            let info = Session::open(with_template_bytes(fixture, 0, &shape))
                .expect("opens")
                .message(0)
                .expect("message 0");
            let grid = info.grid.as_ref().expect("a grid");
            assert_eq!(info.placement, want.placement, "{fixture}, {name}");
            assert_ne!(grid.kind, "unsupported", "{fixture}, {name}");
            assert!(grid.proj4.is_some(), "{fixture}, {name}");
        }
    }
}

/// The committed lat/lon fixture with its §3 rewritten as template 3.90, same
/// raster, on the stated `earth`, with the camera at `nr` Earth radii x 10^6
/// from the centre, and the section and message lengths fixed to match.
fn space_view_from_latlon(nr: u32, earth: [u8; 16]) -> Vec<u8> {
    let bytes = std::fs::read(format!("{G2}regular_latlon_surface.grib2")).expect("fixture");
    let mut at = 16;
    loop {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        if bytes[at + 4] == 3 {
            // §3.0 octets 31-38: Ni and Nj.
            let ni = &bytes[at + 30..at + 34];
            let nj = &bytes[at + 34..at + 38];
            let mut p = vec![0u8; 66];
            p[..16].copy_from_slice(&earth);
            p[16..20].copy_from_slice(ni);
            p[20..24].copy_from_slice(nj);
            p[33..37].copy_from_slice(&1_000u32.to_be_bytes()); // Dx
            p[37..41].copy_from_slice(&1_000u32.to_be_bytes()); // Dy
            p[54..58].copy_from_slice(&nr.to_be_bytes()); // Nr
            let mut section = bytes[at..at + 14].to_vec();
            section[12..14].copy_from_slice(&90u16.to_be_bytes());
            section.extend_from_slice(&p);
            let section_len = section.len() as u32;
            section[0..4].copy_from_slice(&section_len.to_be_bytes());
            let mut out = bytes[..at].to_vec();
            out.extend_from_slice(&section);
            out.extend_from_slice(&bytes[at + len..]);
            let total = out.len() as u64;
            out[8..16].copy_from_slice(&total.to_be_bytes());
            return out;
        }
        at += len;
    }
}

/// Shape code 6: a sphere of radius 6,371,229 m, nothing producer-specified.
fn mean_sphere() -> [u8; 16] {
    earth(6, Some((0, 0)), Some((0, 0)), Some((0, 0)))
}

/// A §3.90 whose camera sees no Earth is the same shape of grid: a raster the
/// section states and no geometry to place it. `message` called it
/// `unplaceable` and `decode` refused it, so a host offered a render that
/// failed; it now decodes in grid coordinates, as the non-finite grids do.
#[test]
fn a_space_view_that_sees_no_earth_decodes_unplaced() {
    let session = Session::open(space_view_from_latlon(1_000_000, mean_sphere())).expect("opens");
    let info = session.message(0).expect("message 0");
    let grid = info.grid.as_ref().expect("a grid");
    assert_eq!(grid.label, "space_view");
    assert_eq!(info.placement, Placement::Unplaceable);
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes in grid coordinates");
    assert_eq!(field.georef.placement, Placement::Unplaceable);
    assert!(!field.georef.reprojectable);
    assert_eq!((grid.ni, grid.nj), (Some(field.ni), Some(field.nj)));
    assert_eq!(
        (field.georef.ni, field.georef.nj),
        (Some(field.ni), Some(field.nj))
    );
    let placed = session.place_message(0).expect("places");
    assert_eq!((placed.ni, placed.nj), (Some(field.ni), Some(field.nj)));
    let want = Session::open(std::fs::read(format!("{G2}regular_latlon_surface.grib2")).unwrap())
        .unwrap()
        .decode(0, &DecodeOptions::default())
        .unwrap();
    assert_eq!((field.ni, field.nj), (want.ni, want.nj));
    assert_eq!(field.values, want.values);
}

/// §3.90 on an Earth that is no body: `r_pol / r_eq` is `NaN` on a zero
/// radius, and the geometry used to carry it. Geostationary orbit, so only the
/// Earth is wrong.
#[test]
fn a_space_view_on_an_earth_that_is_no_body_declines_the_grid() {
    let original =
        std::fs::read(format!("{G2}regular_latlon_surface.grib2")).expect("the lat/lon fixture");
    for (name, shape) in shapeless_earths() {
        assert_declined(
            space_view_from_latlon(6_610_710, shape),
            &original,
            &format!("space view, {name}"),
        );
    }
    for (name, shape) in healthy_earths() {
        let info = Session::open(space_view_from_latlon(6_610_710, shape))
            .expect("opens")
            .message(0)
            .expect("message 0");
        let grid = info.grid.as_ref().expect("a grid");
        assert_eq!(grid.kind, "space_view", "space view, {name}");
        assert!(grid.proj4.is_some(), "space view, {name}");
    }
}

/// The classic twin of the CF geostationary fixture. The NetCDF-4 original
/// cannot be edited in place: its HDF5 object header checksums the attributes.
const GOES: &str = "../fieldglass-netcdf/tests/fixtures/goes_geostationary_classic.nc";

/// The CF geostationary fixture with one `f64` grid-mapping attribute value
/// replaced. A classic header stores it big-endian, and each of the three
/// values occurs exactly once in the file, which is checked.
fn goes_with(attribute_value: f64, replacement: f64) -> Vec<u8> {
    let mut bytes = std::fs::read(GOES).expect("fixture");
    let needle = attribute_value.to_be_bytes();
    let at: Vec<usize> = bytes
        .windows(8)
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "{attribute_value} occurs once");
    bytes[at[0]..at[0] + 8].copy_from_slice(&replacement.to_be_bytes());
    bytes
}

/// The georef and values of every slice of `bytes` that has a map axis pair.
fn slices(bytes: Vec<u8>) -> Vec<(String, fieldglass::Field)> {
    let session = Session::open(bytes).expect("opens");
    let mut out = Vec::new();
    for var in session.variables() {
        let (Some(y), Some(x)) = (var.detected_y_dim, var.detected_x_dim) else {
            continue;
        };
        let fixed = vec![0u32; var.dims.len()];
        let field = session
            .decode_slice(var.index, y, x, &fixed, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e:?}", var.name));
        out.push((var.name.clone(), field));
    }
    out
}

/// The NetCDF instance of the same defect: a CF `geostationary` grid mapping
/// states its own axes and camera height, and a zero, `NaN` or prolate axis,
/// or a camera at or below the surface, built a geometry the projector refused
/// to place but whose PROJ string still said `+a=0` or `+h=0`. It now resolves
/// to no geostationary grid, the way a mapping missing one of them does.
#[test]
fn a_cf_geostationary_mapping_that_describes_no_camera_is_not_placed() {
    const SEMI_MAJOR: f64 = 6_378_137.0;
    const SEMI_MINOR: f64 = 6_356_752.314_14;
    const HEIGHT: f64 = 35_786_023.0;
    let want = slices(std::fs::read(GOES).expect("fixture"));
    let placed: Vec<_> = want
        .iter()
        .filter(|(_, f)| f.georef.kind == "space_view")
        .collect();
    assert!(!placed.is_empty(), "the fixture has a geostationary slice");
    assert!(placed.iter().all(|(_, f)| f.georef.proj4.is_some()));

    for (attribute, value, replacement) in [
        ("semi_major_axis", SEMI_MAJOR, 0.0),
        ("semi_major_axis", SEMI_MAJOR, f64::NAN),
        ("semi_minor_axis", SEMI_MINOR, 0.0),
        ("semi_minor_axis", SEMI_MINOR, -SEMI_MINOR),
        ("semi_minor_axis", SEMI_MINOR, 7_000_000.0),
        ("perspective_point_height", HEIGHT, 0.0),
        ("perspective_point_height", HEIGHT, -1.0),
    ] {
        let what = format!("{attribute} = {replacement}");
        let got = slices(goes_with(value, replacement));
        assert_eq!(got.len(), want.len(), "{what}");
        for ((name, field), (_, before)) in got.iter().zip(&want) {
            if before.georef.kind != "space_view" {
                continue;
            }
            let g = &field.georef;
            assert_eq!(g.kind, "unsupported", "{what}: {name}");
            assert_eq!(g.placement, Placement::Unplaceable, "{what}: {name}");
            assert!(!g.reprojectable, "{what}: {name}");
            assert_eq!(g.proj4, None, "{what}: {name}");
            assert_eq!(g.bounds_lonlat, None, "{what}: {name}");
            assert_eq!(field.values, before.values, "{what}: {name}");
        }
    }
}
