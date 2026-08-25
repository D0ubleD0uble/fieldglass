//! GRIB2 §3.150 (HEALPix) metadata, against the fixtures eccodes wrote.
//!
//! The template is small, so the risk is not the arithmetic but the offsets:
//! `Nside` sits after a 16-octet shape-of-the-earth block and a resolution
//! flag, and reading it one octet out still yields a plausible power of two.
//! Both orderings and two resolutions are decoded, and the pixel count is
//! checked against §3's own `numberOfDataPoints`, which is an independent
//! statement of the same fact.

use fieldglass_grib2::{Grib2Reader, GridTemplate, gds::GridDefinitionSection};

fn gds_of(bytes: &[u8]) -> GridDefinitionSection {
    let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
    reader.messages[0].gds.clone()
}

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("fixtures/", $name)).as_slice()
    };
}

#[test]
fn a_ring_ordered_healpix_grid_decodes() {
    for (bytes, nside) in [
        (fixture!("healpix_n2_ring.grib2"), 2u32),
        (fixture!("healpix_n4_ring.grib2"), 4),
    ] {
        let gds = gds_of(bytes);
        assert_eq!(gds.template_name(), "healpix");
        let GridTemplate::Healpix(t) = gds.template else {
            panic!("expected a HEALPix template, got {}", gds.template_name());
        };
        assert_eq!(t.nside, nside);
        assert!(!t.nested, "these fixtures are ring-ordered");
        assert_eq!(t.npix(), 12 * nside as u64 * nside as u64);
        // §3 states the point count independently of the template body, so the
        // two agreeing means the offsets are right rather than self-consistent.
        assert_eq!(
            gds.num_data_points as u64,
            t.npix(),
            "12*Nside^2 must equal what section 3 declares"
        );
        // HEALPix fixes this at 45 degrees; a message saying otherwise is a
        // grid we have no oracle for, so it is worth pinning that we read it.
        assert!(
            (t.lon_first - 45.0).abs() < 1e-6,
            "lon_first was {}",
            t.lon_first
        );
        // Code table 3.8 value 4 is "grid points at the centre of the cell",
        // which is what HEALPix means.
        assert_eq!(t.grid_point_position, 4);
    }
}

#[test]
fn a_nested_ordered_healpix_grid_decodes() {
    for (bytes, nside) in [
        (fixture!("healpix_n2_nested.grib2"), 2u32),
        (fixture!("healpix_n4_nested.grib2"), 4),
    ] {
        let gds = gds_of(bytes);
        let GridTemplate::Healpix(t) = gds.template else {
            panic!("expected a HEALPix template");
        };
        assert_eq!(t.nside, nside);
        assert!(t.nested, "ordering 1 is nested");
        assert_eq!(gds.num_data_points as u64, t.npix());
    }
}

/// HEALPix is a list of pixels, not a raster. Reporting a shape here would make
/// it look like a one-row grid to every consumer that keys on `(ni, nj)` — the
/// failure the render cost model warns about, where equirectangular becomes a
/// one-row strip and orthographic allocates `npix²`.
#[test]
fn a_healpix_grid_reports_no_raster_shape_or_corners() {
    let gds = gds_of(fixture!("healpix_n4_ring.grib2"));
    assert_eq!(gds.dimensions(), None, "no (ni, nj)");
    assert_eq!(gds.bounds(), None, "no corner box");
    assert_eq!(gds.first_point(), None, "no stated first point");
    // The scanning mode is still there — it is in the template.
    assert!(gds.scanning_mode().is_some());
}

/// The values must come back in the order the pixels are numbered, because
/// #443 will index them by pixel. Each fixture holds a ramp, so a reordering
/// shows as a value in the wrong place.
#[test]
fn the_values_arrive_in_pixel_order() {
    for (name, bytes) in [
        ("ring", fixture!("healpix_n2_ring.grib2")),
        ("nested", fixture!("healpix_n2_nested.grib2")),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parses");
        let values = reader.decode_message_values(0).expect("decodes");
        assert_eq!(values.len(), 48, "{name}");
        for (k, v) in values.iter().enumerate() {
            let got = v.expect("no pixel is masked in these fixtures");
            assert!(
                (got - k as f64).abs() < 1e-6,
                "{name}: pixel {k} holds {got}, expected the ramp value {k}"
            );
        }
    }
}

/// §3.150 is 28 octets of untrusted input, and two of its fields can describe a
/// grid that does not exist. Both are refused at parse, where the bytes enter,
/// rather than left to fail later as an empty render.
#[test]
fn a_malformed_healpix_section_is_refused() {
    let good = fixture!("healpix_n2_nested.grib2").to_vec();

    // Locate the template body: §3 starts after §0 (16) and §1, and Nside sits
    // 17 octets into the template payload. Rather than compute that, find the
    // known-good Nside bytes and edit them in place.
    let reader = Grib2Reader::from_bytes(good.clone()).expect("baseline parses");
    let GridTemplate::Healpix(t) = reader.messages[0].gds.template else {
        panic!("baseline is HEALPix");
    };
    assert_eq!(t.nside, 2);
    // Find Nside's offset by *verifying* it rather than counting octets: the
    // byte pattern 00 00 00 02 occurs more than once in a GRIB2 message, so a
    // plain search finds the wrong field and the test then proves nothing.
    let patch_at = |at: usize, value: u32| {
        let mut bytes = good.clone();
        bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
        Grib2Reader::from_bytes(bytes)
    };
    let at = (0..good.len() - 4)
        .find(|&at| {
            // Most offsets corrupt the message into something with no messages
            // at all, so this asks only whether Nside actually became 4.
            let Ok(reader) = patch_at(at, 4) else {
                return false;
            };
            matches!(
                reader.messages.first().map(|m| m.gds.template),
                Some(GridTemplate::Healpix(t)) if t.nside == 4
            )
        })
        .expect("some offset is Nside");

    let patch = |value: u32| patch_at(at, value);

    // Nside 0 is not a grid.
    assert!(patch(0).is_err(), "Nside 0 must be refused");
    // NESTED is a quadtree per face, so a non-power-of-two Nside describes no
    // ordering. Left unchecked, `nest2ring` refuses each pixel in turn and the
    // field renders empty instead of erroring.
    assert!(patch(3).is_err(), "nested with Nside 3 must be refused");
    // 12*Nside^2 passes u64::MAX above 2^31; these are four attacker-controlled
    // octets reachable from the parse fuzz target.
    assert!(patch(u32::MAX).is_err(), "an absurd Nside must be refused");
    assert!(patch(1 << 30).is_err(), "and one merely far too large");
    // A power of two in range still works, so the guards are not over-broad.
    assert!(patch(4).is_ok(), "Nside 4 nested is legitimate");
}
