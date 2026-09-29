//! `Georef::placement` and `MessageInfo::placement` over real messages (#776).
//!
//! A `null` corner pair used to be the whole answer, and it meant two things: a
//! message with nothing to place, and a raster whose projection cannot place
//! it. These tests open the committed fixtures that are each of those, through
//! `Session`, and read the reason back.
//!
//! One case is hand-built rather than committed: a GRIB1 message with no grid
//! description and a predefined grid number this build has no catalogue entry
//! for. No producer in the corpus writes one, and the reader's own test builds
//! the same shape (`fieldglass-grib1/tests/predefined_grid.rs`).

use fieldglass::{DecodeOptions, Placement, Session};

const G1: &str = "../fieldglass-grib1/tests/fixtures/";
const G2: &str = "../fieldglass-grib2/tests/fixtures/";

fn open(path: &str) -> Session {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Session::open(bytes).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A regular lat/lon grid: placed, as declared and as decoded.
#[test]
fn a_regular_grid_is_placed() {
    let session = open(&format!("{G2}regular_latlon_surface.grib2"));
    let info = session.message(0).expect("message 0");
    let grid = info.grid.as_ref().expect("a GRIB2 message declares a grid");
    assert_eq!(grid.placement, Placement::Placed);
    assert!(grid.corners.is_some() && grid.bounds_lonlat.is_some());
    assert!(grid.reprojectable);
    assert_eq!(info.placement, Placement::Placed);
    assert!(info.reprojectable);

    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    assert_eq!(field.georef.placement, Placement::Placed);
    assert!(field.georef.reprojectable);
}

/// A spectral message declares a grid with no points, and its values are
/// synthesised onto a global lat/lon raster. The declared grid is `no_raster`;
/// the message, which answers for its values, is `placed` and reprojects. Both
/// editions, because each is its own arm of `Session::message`.
#[test]
fn a_spectral_message_declares_no_raster_and_its_values_are_placed() {
    for path in [
        format!("{G1}spectral_simple_t63.grib1"),
        format!("{G2}spectral_simple_t63.grib2"),
    ] {
        let session = open(&path);
        let info = session.message(0).expect("message 0");
        let grid = info.grid.as_ref().expect("the grid it declares");
        assert_eq!(grid.label, "spherical_harmonic", "{path}");
        assert_eq!(grid.placement, Placement::NoRaster, "{path}");
        assert_eq!(grid.corners, None, "{path}");
        assert!(!grid.reprojectable, "{path}");

        assert_eq!(info.placement, Placement::Placed, "{path}");
        assert!(info.reprojectable, "{path}");
        let placed = session.place_message(0).expect("places");
        assert_eq!(placed.placement, Placement::Placed, "{path}");
        assert!(placed.reprojectable, "{path}");
    }
}

/// HEALPix is the other synthesised family, and answers the same way.
#[test]
fn a_healpix_message_declares_no_raster_and_its_values_are_placed() {
    let session = open(&format!("{G2}healpix_n4_ring.grib2"));
    let info = session.message(0).expect("message 0");
    assert_eq!(
        info.grid.as_ref().map(|g| g.placement),
        Some(Placement::NoRaster)
    );
    assert_eq!(info.placement, Placement::Placed);
    assert!(info.reprojectable);
}

/// Bi-Fourier coefficients have no grid points and are not synthesised onto
/// any: `no_raster` both as declared and for the values. This is what tells the
/// family apart from spectral without a list of family names.
#[test]
fn a_bifourier_message_has_no_raster_to_place() {
    let session = open(&format!("{G2}bifourier_ellipse_keepaxes.grib2"));
    let info = session.message(0).expect("message 0");
    assert_eq!(
        info.grid.as_ref().map(|g| g.placement),
        Some(Placement::NoRaster)
    );
    assert_eq!(info.placement, Placement::NoRaster);
    assert!(!info.reprojectable);
}

/// A §3.20 polar stereographic grid stating a zero grid step: a raster the
/// projection cannot place. The file states its corners, so the declared grid
/// still carries them — which is exactly why `corners` alone could not say
/// this. The decoded field's own corners are computed from the geometry and
/// are absent.
#[test]
fn a_zero_step_polar_grid_is_unplaceable() {
    let session = open(&format!("{G2}polar_stereographic_surface.grib2"));
    let info = session.message(0).expect("message 0");
    let grid = info.grid.as_ref().expect("a GRIB2 message declares a grid");
    assert_eq!(grid.placement, Placement::Unplaceable);
    assert!(
        grid.corners.is_some(),
        "the corners the file states are reported as stated"
    );
    assert_eq!(grid.bounds_lonlat, None);
    assert!(!grid.reprojectable);
    assert_eq!(info.placement, Placement::Unplaceable);
    assert!(!info.reprojectable);

    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("an unplaceable grid still decodes");
    assert_eq!(field.georef.placement, Placement::Unplaceable);
    assert_eq!(field.georef.corners, None);
}

/// A single-message GRIB1 stream with no GDS and the given grid number: IS, a
/// 28-byte PDS with the GDS-present flag clear, a stub BDS nothing here
/// decodes, and the end marker. The shape `predefined_grid.rs` builds.
fn grib1_without_gds(grid_number: u8) -> Vec<u8> {
    const BDS_LEN: usize = 12;
    let total_len = 8 + 28 + BDS_LEN + 4;
    let mut msg = Vec::with_capacity(total_len);
    msg.extend_from_slice(b"GRIB");
    msg.extend_from_slice(&[
        (total_len >> 16) as u8,
        (total_len >> 8) as u8,
        total_len as u8,
    ]);
    msg.push(1);
    let mut pds = [0u8; 28];
    pds[0..3].copy_from_slice(&[0, 0, 28]);
    pds[6] = grid_number; // octet 7: grid number
    pds[7] = 0x00; // octet 8: no GDS, no BMS
    msg.extend_from_slice(&pds);
    msg.extend_from_slice(&[0u8; BDS_LEN]);
    msg.extend_from_slice(b"7777");
    msg
}

/// No grid description: the answer depends on what the grid number says.
///
/// - `2` is in the catalogue, so the reader fills the grid in and it is placed.
/// - `7` is a real ON388 Table B number this build has no entry for: the
///   message names a grid, and this build cannot say which.
/// - `255` is Table B's "no predefined grid": there is nothing to place.
#[test]
fn a_grib1_message_without_a_grid_description() {
    let cases = [
        (2, Placement::Placed, true),
        (7, Placement::PredefinedUnresolved, false),
        (255, Placement::NoRaster, false),
    ];
    for (number, placement, has_grid) in cases {
        let session = Session::open(grib1_without_gds(number)).expect("opens");
        let info = session.message(0).expect("message 0");
        assert_eq!(info.grid.is_some(), has_grid, "grid {number}");
        assert_eq!(info.placement, placement, "grid {number}");
        assert_eq!(info.reprojectable, has_grid, "grid {number}");
    }
}

/// A NetCDF-4 / HDF5 slice with no coordinate arrays: `core::cf` resolves it to
/// a source-only geometry with no grid points, and the field still has 10×10
/// cells that render in grid coordinates. A raster nothing places, so
/// `unplaceable` — not `no_raster`, which would tell a host there is nothing
/// to draw.
#[test]
fn a_coordinate_less_array_slice_is_unplaceable() {
    let session = open("../fieldglass-netcdf/tests/fixtures/hdf5_v2_linkinfo.h5");
    let variables = session.variables();
    let var = variables
        .iter()
        .find(|v| v.name.trim_start_matches('/') == "chunked")
        .expect("the fixture holds `chunked`");
    let rank = var.dims.len() as u32;
    assert!(rank >= 2, "a plane to slice");
    let at = vec![0; var.dims.len()];
    let field = session
        .decode_slice(
            var.index,
            rank - 2,
            rank - 1,
            &at,
            &DecodeOptions::default(),
        )
        .expect("decodes");
    assert_eq!((field.ni, field.nj), (10, 10));
    assert_eq!(field.stats.valid_count, 100);
    assert_eq!(field.georef.placement, Placement::Unplaceable);
    assert!(!field.georef.reprojectable);
}

/// Walk a GRIB2 message's sections and patch the §3 template number (octets
/// 13–14) to `template`, leaving everything else — including the point count
/// in octets 7–10 — as the file wrote it.
fn grib2_with_grid_template(bytes: &[u8], template: u16) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut at = 16; // past §0
    while at + 5 <= out.len() && &out[at..at + 4] != b"7777" {
        let len = u32::from_be_bytes([out[at], out[at + 1], out[at + 2], out[at + 3]]) as usize;
        if out[at + 4] == 3 {
            out[at + 12..at + 14].copy_from_slice(&template.to_be_bytes());
            return out;
        }
        at += len;
    }
    panic!("no §3 in the message");
}

/// A grid template this build does not model still declares grid points, so
/// it is `unsupported` rather than `no_raster` — both as declared and for the
/// values — and nothing decodes it. Hand-built from the committed lat/lon
/// fixture with its §3 template renumbered to 3.4 (variable-resolution
/// lat/lon), which no committed producer writes and this build does not parse.
#[test]
fn an_unmodelled_grib2_template_is_unsupported() {
    let bytes = std::fs::read(format!("{G2}regular_latlon_surface.grib2")).expect("fixture");
    let session = Session::open(grib2_with_grid_template(&bytes, 4)).expect("opens");
    let info = session.message(0).expect("message 0");
    let grid = info.grid.as_ref().expect("a GRIB2 message declares a grid");
    assert_eq!(grid.label, "unsupported(3.4)");
    assert_eq!(grid.placement, Placement::Unsupported);
    assert_eq!(info.placement, Placement::Unsupported);
    assert!(!info.reprojectable);
    assert_eq!(
        session.place_message(0).expect("places").placement,
        Placement::Unsupported
    );
    assert!(session.decode(0, &DecodeOptions::default()).is_err());
}

/// The GRIB1 half: a data representation type this parser does not model
/// (90, space view), patched into GDS octet 6 of the committed CMC fixture.
#[test]
fn an_unmodelled_grib1_grid_type_is_unsupported() {
    let mut bytes =
        std::fs::read(format!("{G1}cmc_wind_300_2010052400_p012.grib")).expect("fixture");
    let pds_len = u32::from_be_bytes([0, bytes[8], bytes[9], bytes[10]]) as usize;
    assert_ne!(bytes[8 + 7] & 0x80, 0, "the fixture carries a GDS");
    bytes[8 + pds_len + 5] = 90; // GDS octet 6: data representation type
    let session = Session::open(bytes).expect("opens");
    let info = session.message(0).expect("message 0");
    assert_eq!(
        info.grid.as_ref().map(|g| g.placement),
        Some(Placement::Unsupported)
    );
    assert_eq!(info.placement, Placement::Unsupported);
}
