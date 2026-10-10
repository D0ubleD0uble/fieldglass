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
//!
//! A declined grid keeps its family's name as its label, so the refusals it
//! reaches say its geometry could not be built rather than calling a family
//! this build supports unsupported (#843).

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
    let field = template_payload_at(&bytes) + payload_offset;
    bytes[field..field + value.len()].copy_from_slice(value);
    bytes
}

/// Where the first §3's template payload (octet 15 onward) starts in `bytes`.
fn template_payload_at(bytes: &[u8]) -> usize {
    let mut at = 16; // past §0
    while at + 5 <= bytes.len() && &bytes[at..at + 4] != b"7777" {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        if bytes[at + 4] == 3 {
            return at + 14;
        }
        at += len;
    }
    panic!("no §3");
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
/// to no geostationary grid.
///
/// It is declined as `space_view`, the family the file states, as the GRIB2
/// §3.90 twin is, so its refusals say its geometry could not be built. It used
/// to fall back to the `source` label of a raster nothing places, and every
/// refusal called that grid type unsupported (#961). A mapping missing one of
/// its required attributes is the same case and is declined the same way.
#[test]
fn a_cf_geostationary_mapping_that_describes_no_camera_is_not_placed() {
    let want = slices(std::fs::read(GOES).expect("fixture"));
    let placed: Vec<_> = want
        .iter()
        .filter(|(_, f)| f.georef.kind == "space_view")
        .collect();
    assert!(!placed.is_empty(), "the fixture has a geostationary slice");
    assert!(placed.iter().all(|(_, f)| f.georef.proj4.is_some()));

    for (what, bytes) in declined_goes() {
        let got = slices(bytes.clone());
        assert_eq!(got.len(), want.len(), "{what}");
        let session = Session::open(bytes).expect("opens");
        for ((name, field), (_, before)) in got.iter().zip(&want) {
            if before.georef.kind != "space_view" {
                continue;
            }
            let g = &field.georef;
            assert_eq!(g.kind, "unsupported", "{what}: {name}");
            assert_eq!(g.label, "space_view", "{what}: {name}");
            assert_eq!(g.placement, Placement::Unplaceable, "{what}: {name}");
            assert!(!g.reprojectable, "{what}: {name}");
            assert_eq!(g.proj4, None, "{what}: {name}");
            assert_eq!(g.bounds_lonlat, None, "{what}: {name}");
            assert_eq!(field.values, before.values, "{what}: {name}");
            // Warp, contours and the long CSV among them, and the source
            // view still draws.
            assert_refused_as_declined(&session, field, "space_view", &format!("{what}: {name}"));
        }
    }
}

/// The CF geostationary fixture edited so its mapping describes no camera, as
/// `(what, bytes)`: each axis and the camera height out of range, the camera
/// height missing, and an `x` or `y` axis whose units are no angle or length,
/// so its values are no scan angle (#966).
fn declined_goes() -> Vec<(String, Vec<u8>)> {
    const SEMI_MAJOR: f64 = 6_378_137.0;
    const SEMI_MINOR: f64 = 6_356_752.314_14;
    const HEIGHT: f64 = 35_786_023.0;
    let mut out: Vec<(String, Vec<u8>)> = [
        ("semi_major_axis", SEMI_MAJOR, 0.0),
        ("semi_major_axis", SEMI_MAJOR, f64::NAN),
        ("semi_minor_axis", SEMI_MINOR, 0.0),
        ("semi_minor_axis", SEMI_MINOR, -SEMI_MINOR),
        ("semi_minor_axis", SEMI_MINOR, 7_000_000.0),
        ("perspective_point_height", HEIGHT, 0.0),
        ("perspective_point_height", HEIGHT, -1.0),
    ]
    .into_iter()
    .map(|(attribute, value, replacement)| {
        (
            format!("{attribute} = {replacement}"),
            goes_with(value, replacement),
        )
    })
    .collect();
    // Renamed in place, one letter, so the header keeps its length and the
    // mapping simply has no `perspective_point_height`.
    let mut bytes = std::fs::read(GOES).expect("fixture");
    let name = b"perspective_point_height";
    let at: Vec<usize> = bytes
        .windows(name.len())
        .enumerate()
        .filter(|(_, w)| w == name)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "the attribute name occurs once");
    bytes[at[0] + name.len() - 1] = b'X';
    out.push(("no perspective_point_height".to_string(), bytes));
    // Each axis's `units = "rad"`, a classic `NC_CHAR` attribute of three
    // characters padded to four, overwritten in place with `deg`.
    let rad = b"\0\0\0\x03rad\0";
    let at: Vec<usize> = std::fs::read(GOES)
        .expect("fixture")
        .windows(rad.len())
        .enumerate()
        .filter(|(_, w)| w == rad)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 2, "x and y each state rad once");
    for (axis, at) in ["x", "y"].into_iter().zip(at) {
        let mut bytes = std::fs::read(GOES).expect("fixture");
        bytes[at + 4..at + 7].copy_from_slice(b"deg");
        out.push((format!("{axis} in deg"), bytes));
    }
    out
}

/// Each way a GRIB2 grid of a supported family is declined, as
/// `(what, bytes, family)`: a §3.1 and a §3.12 with a non-finite parameter,
/// a §3.12 scale factor that is not positive, every projected family on an
/// Earth that is no body, and a §3.90 whose camera sits on the surface
/// (#823, #843, #844).
fn declined_grids() -> Vec<(String, Vec<u8>, &'static str)> {
    let mut out = Vec::new();
    for ((fixture, offset, name, _), family) in CASES
        .into_iter()
        .zip(["rotated_latlon", "transverse_mercator"])
    {
        for value in non_finite() {
            out.push((
                format!("{fixture}, {name} = {value}"),
                with_template_float(fixture, offset, value),
                family,
            ));
        }
    }
    for value in [-1.0, 0.0] {
        out.push((
            format!("transverse_mercator_ukv.grib2, scale factor = {value}"),
            with_template_float("transverse_mercator_ukv.grib2", 33, value),
            "transverse_mercator",
        ));
    }
    for (fixture, family) in PROJECTED.into_iter().zip([
        "lambert",
        "polar_stereo",
        "transverse_mercator",
        "lambert_azimuthal",
    ]) {
        for (name, shape) in shapeless_earths() {
            out.push((
                format!("{fixture}, {name}"),
                with_template_bytes(fixture, 0, &shape),
                family,
            ));
        }
    }
    for (name, shape) in shapeless_earths() {
        out.push((
            format!("space view, {name}"),
            space_view_from_latlon(6_610_710, shape),
            "space_view",
        ));
    }
    out.push((
        "a §3.90 camera on the surface".to_string(),
        space_view_from_latlon(1_000_000, mean_sphere()),
        "space_view",
    ));
    out
}

/// Every refusal a declined grid reaches says its geometry could not be
/// built, and none calls a family this build supports unsupported (#843).
///
/// Through the real `Session` over the edited fixtures, and through the calls
/// both bindings make: `Field::source` into the display methods (the Node
/// binding's render, probe, overlay, contour and CSV) and `Session::warp` (the
/// browser binding's warp). The source projection and the matrix CSV still
/// answer, since neither needs a position.
#[test]
fn a_declined_grids_refusals_say_its_geometry_could_not_be_built() {
    for (what, bytes, family) in declined_grids() {
        let session = Session::open(bytes).expect("opens");
        let field = session
            .decode(0, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_refused_as_declined(&session, &field, family, &what);
    }
}

/// Every refusal a host reaches for `field`, a grid declined as `family`, says
/// its geometry could not be built, and the two views that need no position
/// still answer.
fn assert_refused_as_declined(
    session: &Session,
    field: &fieldglass::Field,
    family: &str,
    what: &str,
) {
    use fieldglass::render::{VectorOptions, vector_polylines};
    use fieldglass::{RenderOptions, WarpOptions};

    assert_eq!(field.georef.label, family, "{what}");
    let cells: Vec<Option<f64>> = (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let source = field.source();
    let map = RenderOptions::new("equirectangular", "nearest");
    let latlon = [10.0, 20.0, 30.0, 40.0];

    let refusals = [
        (
            "render",
            session.project(&source, &cells, &map).map(|_| ()),
            "it cannot be reprojected",
        ),
        (
            "probe",
            session.probe_pixel(&source, &cells, &map, 1, 1).map(|_| ()),
            "it cannot be reprojected",
        ),
        (
            "overlay",
            session
                .overlay_polylines(&source, &map, &latlon, &[2])
                .map(|_| ()),
            "it cannot be reprojected",
        ),
        (
            "contours",
            session
                .contour_polylines(&source, &cells, &map, None)
                .map(|_| ()),
            "its contours have no position on a map",
        ),
        (
            "vector arrows",
            vector_polylines(
                &source,
                &cells,
                &source,
                &cells,
                &map,
                &VectorOptions::new(),
            )
            .map(|_| ()),
            "its vectors have no position on a map",
        ),
        (
            "long CSV",
            session.field_csv(&source, &cells, "long").map(|_| ()),
            "its points have no coordinates; export as the Matrix format instead",
        ),
        (
            "warp",
            session.warp(field, &WarpOptions::default()).map(|_| ()),
            "it cannot be reprojected",
        ),
        (
            "warp onto a window",
            session.warp(field, &windowed()).map(|_| ()),
            "it cannot be reprojected",
        ),
    ];
    for (operation, result, consequence) in refusals {
        let message = result
            .err()
            .unwrap_or_else(|| panic!("{what}: {operation} should refuse"))
            .message();
        assert_eq!(
            message,
            format!(
                "the {family:?} grid's geometry could not be built from the parameters \
                 its file declares, so {consequence}"
            ),
            "{what}: {operation}"
        );
    }

    // What needs no position still answers.
    let source_view = RenderOptions::new("source", "nearest");
    let painted = session
        .project(&source, &cells, &source_view)
        .unwrap_or_else(|e| panic!("{what}: the source projection: {e}"));
    assert_eq!((painted.width, painted.height), (field.ni, field.nj));
    session
        .field_csv(&source, &cells, "matrix")
        .unwrap_or_else(|e| panic!("{what}: the matrix CSV: {e}"));
}

/// The other half of #843: a template this build does not model keeps its
/// "not yet supported" refusal. It has no raster to decode, so the source is
/// the one a host builds from `place_message`, as the Node binding does.
#[test]
fn an_unmodelled_templates_refusal_still_says_it_is_not_supported() {
    use fieldglass::{RenderOptions, Source};

    // §3.0's template number (octets 13-14) rewritten to 3.4, which this build
    // does not model.
    let mut bytes = std::fs::read(format!("{G2}regular_latlon_surface.grib2")).expect("fixture");
    let mut at = 16;
    while bytes[at + 4] != 3 {
        at += u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    }
    bytes[at + 12..at + 14].copy_from_slice(&4u16.to_be_bytes());

    let session = Session::open(bytes).expect("opens");
    let georef = session.place_message(0).expect("places");
    assert_eq!(georef.placement, Placement::Unsupported);
    assert_eq!(georef.label, "unsupported(3.4)");
    let source = Source {
        geometry: Ok(&georef.geometry),
        ni: 4,
        nj: 3,
        scan: georef.scan,
        family: &georef.label,
        points_per_row: None,
    };
    let cells = vec![Some(1.0); 12];
    let message = session
        .project(
            &source,
            &cells,
            &RenderOptions::new("equirectangular", "nearest"),
        )
        .expect_err("refused")
        .message();
    assert_eq!(
        message,
        "reprojection not yet supported for grid type \"unsupported(3.4)\""
    );
    let message = session
        .contour_polylines(
            &source,
            &cells,
            &RenderOptions::new("equirectangular", "nearest"),
            None,
        )
        .expect_err("refused")
        .message();
    assert!(
        message.starts_with("contours not yet supported for grid type \"unsupported(3.4)\""),
        "{message}"
    );
}

/// A warp onto a window the caller names, which gives the warp a box whether
/// or not the grid states one.
fn windowed() -> fieldglass::WarpOptions {
    let mut options = fieldglass::WarpOptions::default();
    options.bounds = Some([-20.0, 40.0, -30.0, 60.0]);
    options
}

/// `Session::warp` refuses a grid nothing places whether or not the caller
/// names a window, with the message the render path gives the same grid.
/// With a window it used to warp through an inverse map that answers nothing
/// and return a raster with every cell masked (#843). Here for the slice no
/// coordinates place: a committed HDF5 dataset with only phony dimensions.
/// The declined GRIB2 grids are in the test above.
#[test]
fn a_warp_of_a_slice_nothing_places_is_refused_with_or_without_a_window() {
    use fieldglass::{RenderOptions, WarpOptions};

    let bytes =
        std::fs::read("../fieldglass-netcdf/tests/fixtures/hdf5_phony_dims.h5").expect("fixture");
    let session = Session::open(bytes).expect("opens");
    let variable = session
        .variables()
        .iter()
        .position(|v| v.name == "a_8x8")
        .expect("the 8 x 8 dataset") as u32;
    let field = session
        .decode_slice(variable, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("decodes in grid coordinates");
    assert_eq!(field.georef.placement, Placement::Unplaceable);
    let cells: Vec<Option<f64>> = (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let rendered = session
        .project(
            &field.source(),
            &cells,
            &RenderOptions::new("equirectangular", "nearest"),
        )
        .expect_err("nothing places it")
        .message();
    assert_eq!(
        rendered,
        "reprojection not yet supported for grid type \"source\""
    );
    for options in [WarpOptions::default(), windowed()] {
        let warped = session
            .warp(&field, &options)
            .expect_err("nothing places it")
            .message();
        assert_eq!(warped, rendered, "bounds {:?}", options.bounds);
    }
}

/// `bytes` with the 4-octet sign-magnitude angle at `payload_offset` in its §3
/// template payload moved 5 degrees further from the equator. Every template
/// states angles in micro-degrees.
fn moved_five_degrees(mut bytes: Vec<u8>, payload_offset: usize) -> Vec<u8> {
    let at = template_payload_at(&bytes) + payload_offset;
    let raw = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    bytes[at..at + 4].copy_from_slice(&(raw + 5_000_000).to_be_bytes());
    bytes
}

/// Pairs of declined GRIB2 grids of one family whose declared parameters
/// differ, as `(what, a, b)`: the first point moved by 5 degrees, or for the
/// §3.90, which states no first point, the camera's distance.
fn declined_pairs() -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let lambert = with_template_bytes(
        "eta_lambert_msg0.grib2",
        0,
        &earth(1, Some((0, 0)), None, None),
    );
    let mut out = vec![(
        // The pair from the issue: §3.30 octets 39-42 are La1.
        "eta_lambert_msg0.grib2, radius 0, La1 moved".to_string(),
        lambert.clone(),
        moved_five_degrees(lambert, 24),
    )];
    // The #823 grids: §3.1 octets 47-50 are La1, §3.12 octets 39-42 LaR.
    for value in [f32::NAN, f32::INFINITY] {
        let rotated = with_template_float("rotated_latlon_surface.grib2", 66, value);
        out.push((
            format!("rotated_latlon_surface.grib2, rotation {value}, La1 moved"),
            rotated.clone(),
            moved_five_degrees(rotated, 32),
        ));
    }
    for value in [f32::NAN, -1.0, 0.0] {
        let tm = with_template_float("transverse_mercator_ukv.grib2", 33, value);
        out.push((
            format!("transverse_mercator_ukv.grib2, k {value}, LaR moved"),
            tm.clone(),
            moved_five_degrees(tm, 24),
        ));
    }
    let no_body = earth(1, Some((0, 0)), None, None);
    out.push((
        "space view, radius 0, camera moved".to_string(),
        space_view_from_latlon(6_610_710, no_body),
        space_view_from_latlon(6_620_000, no_body),
    ));
    out
}

/// The refusal two declined grids of one family get when their files
/// declare different grids: neither has a shape or a corner to quote.
fn declare_different(family: &str) -> String {
    format!(
        "the two fields declare different {family} grids, and neither could be placed, \
         so they cannot be combined"
    )
}

/// Message 0 of `bytes`, decoded by a session of its own, as a host that
/// opened the file twice would hold it.
fn decoded(bytes: &[u8]) -> fieldglass::Field {
    Session::open(bytes.to_vec())
        .expect("opens")
        .decode(0, &DecodeOptions::default())
        .expect("decodes in grid coordinates")
}

/// Two declined grids are one grid only when they declare the same one
/// (#962). The label names only the family, so two §3.30 grids on a
/// zero-radius Earth whose first points differ by 5 degrees combined cell for
/// cell, where the same pair on a real Earth is refused. The same message
/// twice still combines, as the extension's Compare row allows.
///
/// Through `Session::combine` (the browser binding) and through
/// `combine_values` over `place_message` (the Node binding's GRIB path).
#[test]
fn two_declined_grids_combine_only_when_they_declare_the_same_grid() {
    use fieldglass::{CombineOp, Source, combine_values};

    let op = CombineOp::Difference;
    for (what, a, b) in declined_pairs() {
        let (fa, fb) = (decoded(&a), decoded(&b));
        assert_eq!(fa.georef.kind, "unsupported", "{what}");
        assert_eq!(fa.georef.label, fb.georef.label, "{what}: one family");
        assert_eq!((fa.ni, fa.nj), (fb.ni, fb.nj), "{what}: one raster shape");

        let refused = Session::open(a.clone())
            .expect("opens")
            .combine(&fa, &fb, op)
            .expect_err("two declarations are two grids")
            .message();
        assert_eq!(refused, declare_different(&fa.georef.label), "{what}");
        Session::open(a.clone())
            .expect("opens")
            .combine(&fa, &decoded(&a), op)
            .unwrap_or_else(|e| panic!("{what}: the same message twice: {e}"));

        // The Node binding pairs `place_message` placements over raw values.
        let placed = |bytes: &[u8]| {
            Session::open(bytes.to_vec())
                .expect("opens")
                .place_message(0)
                .expect("places")
        };
        let (pa, pa2, pb) = (placed(&a), placed(&a), placed(&b));
        fn source(g: &fieldglass::Georef) -> Source<'_> {
            Source {
                geometry: Ok(&g.geometry),
                ni: g.ni.expect("declared columns"),
                nj: g.nj.expect("declared rows"),
                scan: g.scan,
                family: &g.label,
                points_per_row: None,
            }
        }
        let values = vec![Some(1.0); (fa.ni * fa.nj) as usize];
        assert_eq!(
            combine_values(&source(&pa), &values, &source(&pb), &values, op)
                .expect_err("two declarations are two grids")
                .message(),
            declare_different(&fa.georef.label),
            "{what}: placements"
        );
        combine_values(&source(&pa), &values, &source(&pa2), &values, op)
            .unwrap_or_else(|e| panic!("{what}: the same placement twice: {e}"));
    }

    // The move is one the gate sees on a grid that builds: the issue's pair
    // on a real Earth is refused by its corner.
    let real = with_template_bytes(
        "eta_lambert_msg0.grib2",
        0,
        &earth(1, Some((0, 6_371_229)), None, None),
    );
    let refused = Session::open(real.clone())
        .expect("opens")
        .combine(&decoded(&real), &decoded(&moved_five_degrees(real, 24)), op)
        .expect_err("a built grid moved by 5 degrees")
        .message();
    assert!(refused.contains("their grid differs"), "{refused}");
}

/// The NetCDF half: two CF geostationary mappings declined for different
/// numbers are two grids, and one declined slice still combines with itself
/// decoded again (#961, #962). A slice with no coordinates and no mapping keeps
/// comparing by its `source` label, which `a_warp_of_a_slice_nothing_places…`
/// and the umbrella's unit tests hold.
#[test]
fn two_declined_geostationary_slices_combine_only_when_they_declare_the_same_grid() {
    use fieldglass::CombineOp;

    let goes = declined_goes();
    let (what_a, a) = &goes[0];
    let a_slices = slices(a.clone());
    let session = Session::open(a.clone()).expect("opens");
    let mut checked = 0;
    for (what_b, b) in &goes[1..] {
        for (((name, fa), (_, fa2)), (_, fb)) in a_slices
            .iter()
            .zip(slices(a.clone()))
            .zip(slices(b.clone()))
        {
            if fa.georef.label != "space_view" {
                continue;
            }
            assert_eq!(fb.georef.label, "space_view", "{what_b}: {name}");
            let refused = session
                .combine(fa, &fb, CombineOp::Difference)
                .expect_err("two declarations are two grids")
                .message();
            assert_eq!(
                refused,
                declare_different("space_view"),
                "{what_a} against {what_b}: {name}"
            );
            session
                .combine(fa, &fa2, CombineOp::Difference)
                .unwrap_or_else(|e| panic!("{what_a}: {name} against itself: {e}"));
            checked += 1;
        }
    }
    assert!(checked > 0, "the fixture has a geostationary slice");
}

/// The declined `semi_major_axis = 0` GOES slice, rewritten as an in-memory
/// Zarr v2 store: the same axes and mapping numbers, but the mapping's
/// attributes in another order, with a `long_name` the classic file does not
/// have, and the axes stored as plain `f64` rather than scaled `int16`.
/// `semi_major_axis` is `major`.
fn declined_goes_as_zarr(major: f64) -> fieldglass::MemoryObjects {
    let classic = Session::open(goes_with(6_378_137.0, 0.0)).expect("opens");
    let rad = classic
        .variables()
        .iter()
        .find(|v| v.name == "Rad")
        .expect("the Rad variable")
        .index;
    let axis = |dim| {
        classic
            .axis_values(rad, dim)
            .expect("an axis")
            .coordinates
            .expect("coordinates")
    };
    let (y, x) = (axis(0), axis(1));
    let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let array = |shape: &str| {
        format!(
            r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
        )
        .into_bytes()
    };
    let (nx, ny) = (x.len(), y.len());
    let mapping = format!(
        r#"{{"_ARRAY_DIMENSIONS":["one"],"long_name":"GOES-R ABI fixed grid projection","sweep_angle_axis":"x","longitude_of_projection_origin":-75.0,"semi_minor_axis":6356752.31414,"semi_major_axis":{major},"perspective_point_height":35786023.0,"grid_mapping_name":"geostationary"}}"#
    );
    fieldglass::MemoryObjects::from_iter(vec![
        (".zgroup".to_string(), br#"{"zarr_format":2}"#.to_vec()),
        ("goes_imager_projection/.zarray".to_string(), array("[1]")),
        ("goes_imager_projection/.zattrs".to_string(), mapping.into_bytes()),
        ("goes_imager_projection/0".to_string(), f64s(&[0.0])),
        ("x/.zarray".to_string(), array(&format!("[{nx}]"))),
        (
            "x/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["x"],"units":"rad","axis":"X","standard_name":"projection_x_coordinate"}"#.to_vec(),
        ),
        ("x/0".to_string(), f64s(&x)),
        ("y/.zarray".to_string(), array(&format!("[{ny}]"))),
        (
            "y/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["y"],"units":"rad","axis":"Y","standard_name":"projection_y_coordinate"}"#.to_vec(),
        ),
        ("y/0".to_string(), f64s(&y)),
        ("Rad/.zarray".to_string(), array(&format!("[{ny},{nx}]"))),
        (
            "Rad/.zattrs".to_string(),
            br#"{"_ARRAY_DIMENSIONS":["y","x"],"grid_mapping":"goes_imager_projection"}"#.to_vec(),
        ),
        ("Rad/0.0".to_string(), f64s(&vec![1.0; nx * ny])),
    ])
}

/// A declined mapping is fingerprinted by what the resolver reads, not by how
/// a container lists it: the same declined GOES grid as classic NetCDF and as
/// a Zarr store whose mapping lists its attributes in another order, carries a
/// `long_name`, and stores its axes unscaled still combines, and the store
/// stating a different `semi_major_axis` does not (#962).
#[test]
fn a_declined_mapping_combines_across_containers_when_it_declares_the_same_grid() {
    use fieldglass::CombineOp;

    let classic = slices(goes_with(6_378_137.0, 0.0))
        .into_iter()
        .find(|(name, _)| name == "Rad")
        .expect("Rad")
        .1;
    assert_eq!(classic.georef.label, "space_view");
    let zarr = |major: f64| {
        let session = Session::open_store(declined_goes_as_zarr(major)).expect("opens");
        let rad = session
            .variables()
            .iter()
            .find(|v| v.name == "Rad")
            .expect("Rad")
            .index;
        session
            .decode_slice(rad, 0, 1, &[0, 0], &DecodeOptions::default())
            .expect("decodes")
    };
    let same = zarr(0.0);
    assert_eq!(same.georef.label, "space_view");
    assert_eq!(same.georef.placement, Placement::Unplaceable);

    let session = Session::open(goes_with(6_378_137.0, 0.0)).expect("opens");
    session
        .combine(&classic, &same, CombineOp::Difference)
        .unwrap_or_else(|e| panic!("one declared grid in two containers: {e}"));
    assert_eq!(
        session
            .combine(&classic, &zarr(-1.0), CombineOp::Difference)
            .expect_err("a different semi-major axis")
            .message(),
        declare_different("space_view")
    );
}
