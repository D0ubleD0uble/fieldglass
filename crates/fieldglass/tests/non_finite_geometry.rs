//! A GRIB2 grid parameter that is not a finite number (#823).
//!
//! Two §3 parameters are IEEE floats rather than scaled integers: §3.1's
//! angle of rotation and §3.12's scale factor at the reference point. A NaN or
//! an infinity in either used to build a grid anyway: the rotated grid
//! reported itself placed and reprojectable with NaN bounds and a `NaN` in its
//! PROJ string, and both messages carried a non-finite geometry field, which
//! the Node binding writes as `null` and the browser binding as `NaN`. The grid
//! is now declined, the way a §3.90 with no usable camera is.
//!
//! Each case is a committed fixture with the one parameter overwritten.

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

/// The committed fixture with four bytes of its §3 template payload replaced.
fn with_template_float(fixture: &str, payload_offset: usize, value: f32) -> Vec<u8> {
    let mut bytes = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
    let mut at = 16; // past §0
    while at + 5 <= bytes.len() && &bytes[at..at + 4] != b"7777" {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        if bytes[at + 4] == 3 {
            let field = at + 14 + payload_offset;
            bytes[field..field + 4].copy_from_slice(&value.to_be_bytes());
            return bytes;
        }
        at += len;
    }
    panic!("{fixture}: no §3");
}

/// Every non-finite IEEE value: a quiet NaN, a NaN with a payload and the sign
/// bit, and both infinities.
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

#[test]
fn a_non_finite_grid_parameter_declines_the_grid() {
    for (fixture, offset, name, _) in CASES {
        for value in non_finite() {
            let what = format!("{fixture}, {name} = {value} ({:#010x})", value.to_bits());
            let session =
                Session::open(with_template_float(fixture, offset, value)).expect("opens");
            let info = session.message(0).expect("message 0");
            let grid = info.grid.as_ref().expect("a GRIB2 message declares a grid");

            // A raster nothing places, as a §3.90 camera that sees no Earth is.
            assert_eq!(grid.placement, Placement::Unplaceable, "{what}");
            assert_eq!(info.placement, Placement::Unplaceable, "{what}");
            assert!(!grid.reprojectable && !info.reprojectable, "{what}");
            assert_eq!(grid.kind, "unsupported", "{what}");
            assert_eq!(grid.bounds_lonlat, None, "{what}");
            // Still a raster of the size the section declares: a host sizes what
            // it draws from these (#823 review, where they were 0 x 0).
            let declared = Session::open(std::fs::read(format!("{G2}{fixture}")).unwrap())
                .unwrap()
                .message(0)
                .unwrap()
                .grid
                .map(|g| (g.ni, g.nj))
                .unwrap();
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
            assert_eq!(grid.proj4, None, "{what}");
            assert_eq!(
                session.place_message(0).expect("places").placement,
                Placement::Unplaceable,
                "{what}"
            );
            // The values are still the file's, laid out in grid coordinates:
            // what `unplaceable` promises a host that offers a source render.
            let field = session
                .decode(0, &DecodeOptions::default())
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(field.georef.placement, Placement::Unplaceable, "{what}");
            assert!(!field.georef.reprojectable, "{what}");
            assert_eq!(field.georef.bounds_lonlat, None, "{what}");
            assert_eq!(
                (field.georef.ni, field.georef.nj),
                (Some(declared.0), Some(declared.1)),
                "{what}"
            );
            let original = std::fs::read(format!("{G2}{fixture}")).expect("fixture");
            let want = Session::open(original)
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

/// The committed lat/lon fixture with its §3 rewritten as template 3.90, same
/// raster, with the camera at `nr` Earth radii x 10^6 from the centre, and the
/// section and message lengths fixed to match.
fn space_view_from_latlon(nr: u32) -> Vec<u8> {
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
            p[0] = 6; // a sphere of radius 6,371,229 m
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

/// A §3.90 whose camera sees no Earth is the same shape of grid: a raster the
/// section states and no geometry to place it. `message` called it
/// `unplaceable` and `decode` refused it, so a host offered a render that
/// failed; it now decodes in grid coordinates, as the non-finite grids do.
#[test]
fn a_space_view_that_sees_no_earth_decodes_unplaced() {
    let session = Session::open(space_view_from_latlon(1_000_000)).expect("opens");
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
