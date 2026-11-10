//! [`GridDefinitionSection`] → [`GridGeometry`] (#460).
//!
//! The GRIB2 half of the conversion `fieldglass-grib1` makes from its own GDS;
//! read that module's header for the three rules both sides follow (scan-signed
//! increments, reduced grids describing their expanded raster, unmodeled
//! families naming themselves rather than erroring).
//!
//! It converts from the **section**, not from [`GridTemplate`] alone: a reduced
//! grid's raster width comes from the `PL` list, which §3 carries
//! outside the template, and [`GridDefinitionSection::points_per_row`] is the
//! one place that decides whether the optional list really is a row count.
//!
//! The name an unmodeled family reports is
//! [`GridDefinitionSection::template_name`], not a second list written out
//! here. This module used to carry its own copy, and the two had already
//! drifted: `template_name` answers `"unsupported(3.99)"` where the copy
//! answered a bare `"unsupported"`, so the same message read one way in the
//! extension's grid-type column and another in the umbrella's `Georef::label`
//! (#645).

use fieldglass_core::{
    GaussianParams, GridGeometry, LatLonParams, MercatorParams, RotatedLatLonParams,
    is_oblate_spheroid, reduced_grid_is_global, reduced_raster_width,
};

use crate::gds::{GridDefinitionSection, GridTemplate};

/// The Earth a projected template's grid is laid on, as `(semi-major,
/// semi-minor)` metres, or `None` for a template whose points are geographic
/// angles and so do not depend on the radius.
///
/// Lambert conformal and polar stereographic project on a sphere, the mean of
/// the stated axes, which reads as zero when the axes describe no body (see
/// `earth_radius_from_shape`). §3.90 asks the same question in
/// [`SpaceViewTemplate::scan_grid`](crate::gds::SpaceViewTemplate::scan_grid).
fn projected_earth(template: &GridTemplate) -> Option<(f64, f64)> {
    match template {
        GridTemplate::Lambert(t) => Some((t.earth_radius_m, t.earth_radius_m)),
        GridTemplate::PolarStereographic(t) => Some((t.earth_radius_m, t.earth_radius_m)),
        GridTemplate::TransverseMercator(t) => Some((t.earth_major_m, t.earth_minor_m)),
        GridTemplate::LambertAzimuthal(t) => Some((t.earth_major_m, t.earth_minor_m)),
        _ => None,
    }
}

/// A grid of a family this build models whose own numbers built no grid,
/// declined under the family's name and fingerprinted by the section that
/// declares it.
///
/// The fingerprint is what tells two such grids apart: the label names only
/// the family, so two §3.30 grids on a zero-radius Earth would otherwise align
/// whatever their first points (#962). The same message decoded twice has the
/// same section, so it still combines with itself.
fn declined(gds: &GridDefinitionSection) -> GridGeometry {
    GridGeometry::Unsupported {
        label: gds.template_name(),
        declared: Some(gds.fingerprint),
    }
}

/// A reduced grid (§3.0 with `Ni` missing, or §3.40 with a list) whose
/// declared longitudes cover only part of a circle, declined for placement
/// the way any grid of a modeled family is ([`declined`]: its template name
/// and the section's fingerprint); `None` for any other grid.
///
/// The expansion that places a reduced grid lays every row out as a full
/// circle (`360° / PL[j]` apart), so a sub-area would be drawn spread over the
/// whole globe. Until a sub-area expansion exists it is declined; its values
/// still decode in grid coordinates. [`reduced_grid_is_global`] decides
/// "global", the same test GRIB1's reduced arms use.
fn partial_circle(gds: &GridDefinitionSection) -> Option<GridGeometry> {
    let pl = gds.points_per_row()?;
    let (lo1, lo2) = match &gds.template {
        GridTemplate::LatLon(t) => (t.lo1, t.lo2),
        GridTemplate::Gaussian(t) => (t.lo1, t.lo2),
        _ => return None,
    };
    (!reduced_grid_is_global(lo1, lo2, pl)).then(|| declined(gds))
}

impl From<&GridDefinitionSection> for GridGeometry {
    fn from(gds: &GridDefinitionSection) -> Self {
        // A reduced grid over part of a circle, which the full-circle
        // expansion would draw as global (#1760). Only a reduced lat/lon or
        // Gaussian grid with a usable list answers, and neither is a projected
        // template or one without a shape, so taking it before the arms below
        // changes no other grid.
        if let Some(declined) = partial_circle(gds) {
            return declined;
        }
        match &gds.template {
            // A projected grid on an Earth that is not a body: a stated radius
            // of zero (Code Table 3.2 shape 1), axes of zero (shapes 3 and 7),
            // or a minor axis longer than the major one. The projectors already
            // refused to place such a grid, but the geometry still carried the
            // axes into a PROJ string (`+R=0`, `+a=0 +b=0`) that PROJ itself
            // rejects. Declined the way a non-finite parameter is (#844).
            t if projected_earth(t).is_some_and(|(a, b)| !is_oblate_spheroid(a, b)) => {
                declined(gds)
            }
            // A grid with a count of points coded missing and no list of points
            // per row to stand in for it has no size, so it places nothing.
            // It used to report a placed grid `4294967295` points wide, which
            // decode then refused (#1760): the two answers now agree.
            _ if gds.unshaped_reason().is_some() => declined(gds),
            // A reduced lat/lon grid (`Ni` missing, a `PL` list) is the regular
            // lat/lon grid its rows expand into, with the east edge derived the
            // way GRIB1's `ReducedLatLon` and the reduced Gaussian arm below
            // derive it.
            GridTemplate::LatLon(t) => {
                let (ni, lon_last) = match gds.points_per_row() {
                    Some(pl) => (
                        reduced_raster_width(pl),
                        gds.raster_bounds().map_or(t.lo2, |c| c.lon_last),
                    ),
                    None => (t.ni, t.lo2),
                };
                Self::LatLon(LatLonParams {
                    ni,
                    nj: t.nj,
                    lat_first: t.la1,
                    lon_first: t.lo1,
                    lat_last: t.la2,
                    lon_last,
                })
            }
            GridTemplate::Gaussian(t) => {
                // A reduced grid states no `Ni`; the raster its rows expand
                // into does, and `decode_message_raster` produces exactly that
                // raster. Its east edge is derived rather than declared, and
                // the section is what derives it — read here rather than
                // repeated, so this conversion and the hosts cannot drift apart
                // about where an octahedral grid's east edge is (#543).
                // `points_per_row` is `None` for a regular grid, which then
                // reports its declared corners unchanged.
                let (ni, lon_last) = match gds.points_per_row() {
                    Some(pl) => (
                        reduced_raster_width(pl),
                        gds.raster_bounds().map_or(t.lo2, |c| c.lon_last),
                    ),
                    // A regular grid. One with no `Ni` at all has no size and
                    // was declined above, so the fallback is never taken.
                    None => (t.ni.unwrap_or(0), t.lo2),
                };
                Self::Gaussian(GaussianParams {
                    ni,
                    nj: t.nj,
                    lat_first: t.la1,
                    lon_first: t.lo1,
                    lat_last: t.la2,
                    lon_last,
                    n_parallels: t.n_parallels,
                })
            }
            // Read off the template, as the three below are, so the corner
            // the section derives and the grid core places are one grid.
            GridTemplate::Lambert(t) => Self::Lambert(t.projection_params()),
            GridTemplate::PolarStereographic(t) => Self::PolarStereo(t.projection_params()),
            // §3.10's corners are geographic, like §3.0's — the rows are
            // simply spaced in the Mercator ordinate rather than in latitude,
            // which the params pin from the corners alone. `Di`/`Dj` and `LaD`
            // are not needed to place a point and are not carried.
            GridTemplate::Mercator(t) => Self::Mercator(MercatorParams {
                ni: t.ni,
                nj: t.nj,
                lat_first: t.la1,
                lon_first: t.lo1,
                lat_last: t.la2,
                lon_last: t.lo2,
            }),
            // §3.1's corners are **rotated-frame** degrees, so they are handed
            // over as stated: unrotating them here would place the grid twice.
            // Two §3 parameters are IEEE floats rather than scaled integers,
            // §3.1's angle of rotation and §3.12's scale factor, so a file can
            // state NaN or an infinity in them. No grid can be built from one,
            // so it declines the way a §3.90 with no usable camera does, rather
            // than reporting a placed grid with NaN bounds (#823).
            //
            // The scale factor is also a ratio of two distances (WMO §3.12:
            // "distance on map to distance on spheroid"), so zero or a negative
            // value is no scale factor either. A negative one used to report a
            // placed grid with `+k_0=-1` in its PROJ string, which PROJ refuses
            // ("it should be > 0") (#844).
            GridTemplate::RotatedLatLon(t) if !t.angle_of_rotation.is_finite() => declined(gds),
            GridTemplate::TransverseMercator(t)
                if !(t.scale_factor.is_finite() && t.scale_factor > 0.0) =>
            {
                declined(gds)
            }
            GridTemplate::RotatedLatLon(t) => Self::RotatedLatLon(RotatedLatLonParams {
                ni: t.ni,
                nj: t.nj,
                lat_first: t.la1,
                lon_first: t.lo1,
                lat_last: t.la2,
                lon_last: t.lo2,
                south_pole_lat: t.south_pole_lat,
                south_pole_lon: t.south_pole_lon,
                angle_of_rotation: t.angle_of_rotation,
            }),
            // The three below read their parameters off the template rather
            // than restating them, so a standalone consumer of this crate gets
            // the same geometry the umbrella does — scan signs, the ellipsoid,
            // and §3.90's derived scan angles included.
            GridTemplate::TransverseMercator(t) => Self::TransverseMercator(t.projection_params()),
            GridTemplate::LambertAzimuthal(t) => Self::LambertAzimuthal(t.projection_params()),
            // A §3.90 that describes no usable camera — a camera at or below
            // the surface, or an Earth of zero apparent diameter — has no scan
            // grid to place points on, so it names itself instead of inventing
            // one.
            GridTemplate::SpaceView(t) => match t.scan_grid() {
                Some(params) => Self::Geostationary(params),
                None => declined(gds),
            },
            _ => Self::Unsupported {
                label: gds.template_name(),
                declared: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse §3 out of a real fixture so the conversion is checked against
    /// octets an encoder wrote, not against a struct this test filled in.
    fn section_from(path: &str, message: usize) -> GridDefinitionSection {
        let bytes = std::fs::read(path).expect("fixture");
        let reader = crate::Grib2Reader::from_bytes(bytes).expect("parse");
        reader.messages[message].gds.clone()
    }

    /// Every fixture the crate ships: the geometry's own `dims()` must agree
    /// with the section's `dimensions()`, and where the geometry can place a
    /// point at all, grid point (0, 0) must be the declared first point.
    ///
    /// This is the check that would catch a template wired to the wrong field:
    /// a `la1`/`lo1` swap, or a reduced grid handed its declared `Ni`.
    #[test]
    fn every_fixture_places_its_own_first_point() {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir("tests/fixtures").expect("fixtures dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("grib2") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("fixture bytes");
            let Ok(reader) = crate::Grib2Reader::from_bytes(bytes) else {
                continue;
            };
            for msg in &reader.messages {
                let geom = GridGeometry::from(&msg.gds);
                if matches!(geom, GridGeometry::Unsupported { .. }) {
                    continue;
                }
                seen.insert(geom.kind().to_string());
                assert_eq!(
                    geom.dims(),
                    msg.gds.dimensions(),
                    "{}: geometry and section disagree about the raster shape",
                    path.display()
                );
                let Some((lat, lon)) = geom.forward(0, 0) else {
                    continue;
                };
                // Only the templates whose `La1`/`Lo1` is a *geographic*
                // first point. §3.1 states its corner in the rotated frame,
                // §3.12 states an origin in projection metres instead, and
                // §3.90 states neither — for those three the declared fields
                // are not the answer `forward(0, 0)` is meant to give.
                let (la1, lo1) = match &msg.gds.template {
                    GridTemplate::LatLon(t) => (t.la1, t.lo1),
                    GridTemplate::Gaussian(t) => (t.la1, t.lo1),
                    GridTemplate::Mercator(t) => (t.la1, t.lo1),
                    GridTemplate::Lambert(t) => (t.la1, t.lo1),
                    GridTemplate::PolarStereographic(t) => (t.la1, t.lo1),
                    GridTemplate::LambertAzimuthal(t) => (t.la1, t.lo1),
                    _ => continue,
                };
                assert!(
                    (lat - la1).abs() < 1e-6,
                    "{}: first point latitude {lat} != declared {la1}",
                    path.display()
                );
                // Longitudes are compared modulo a turn: `forward` normalizes
                // a projected family's answer into [-180, 180], while §3 states
                // it in [0, 360).
                let dlon = ((lon - lo1 + 540.0).rem_euclid(360.0)) - 180.0;
                assert!(
                    dlon.abs() < 1e-6,
                    "{}: first point longitude {lon} != declared {lo1}",
                    path.display()
                );
            }
        }
        // Named rather than counted: a corpus that silently stopped covering
        // polar stereographic would still pass a count, and this conversion's
        // whole risk is a per-family wiring mistake.
        // Mercator and space view are absent because the corpus has no §3.10
        // or §3.90 fixture, not because they are unmodeled; both are covered
        // against PROJ in `fieldglass-core`'s `grid_geometry_proj.rs`.
        let expected: std::collections::BTreeSet<String> = [
            "latlon",
            "gaussian",
            "rotated_latlon",
            "lambert",
            "polar_stereo",
            "transverse_mercator",
            "lambert_azimuthal",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            seen, expected,
            "the fixture corpus must exercise every family this conversion models"
        );
    }

    /// HRRR is the Lambert case, and its scan is south-to-north (`+j`), so
    /// `dy_metres` stays positive while a north-down grid's would not.
    #[test]
    fn hrrr_is_a_lambert_grid_on_its_declared_sphere() {
        let gds = section_from("tests/fixtures/hrrr_complex_spd_lambert.grib2", 0);
        let GridGeometry::Lambert(p) = GridGeometry::from(&gds) else {
            panic!("HRRR should convert to a Lambert geometry");
        };
        assert_eq!((p.ni, p.nj), gds.dimensions().expect("dims"));
        assert!(p.earth_radius_m > 6.0e6, "radius {}", p.earth_radius_m);
        assert!(p.dy_metres > 0.0, "HRRR scans south-to-north");
        let proj4 = GridGeometry::from(&gds).proj4().expect("proj4");
        assert!(proj4.starts_with("+proj=lcc"), "{proj4}");
    }

    /// §3.1 used to land here. It is a modeled family now, so the case is
    /// carried by a template that really has no raster to place: spherical
    /// harmonics live in wavenumber space.
    #[test]
    fn an_unmodeled_template_names_itself() {
        let gds = section_from("tests/fixtures/spectral_simple_t63.grib2", 0);
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), "spherical_harmonic");
        assert!(geom.dims().is_none());
        assert!(geom.inverse(45.0, 10.0).is_none());
    }

    /// A template number this build never read past still names *which* number
    /// it was, because the conversion reads
    /// [`GridDefinitionSection::template_name`] rather than a second list of
    /// its own.
    ///
    /// The second list is what #645 deleted: it answered a bare
    /// `"unsupported"` here while `template_name` — which is what
    /// `fieldglass-napi` puts in the extension's grid-type column — answered
    /// `"unsupported(3.32768)"`, so one message read two ways depending on the
    /// host.
    #[test]
    fn an_unread_template_number_survives_into_the_label() {
        let mut gds = section_from("tests/fixtures/regular_latlon_surface.grib2", 0);
        gds.template = GridTemplate::Unsupported(32_768);
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), gds.template_name());
        assert_eq!(geom.label(), "unsupported(3.32768)");
    }

    /// §3.1's corners are rotated-frame degrees. Handing them to the geometry
    /// as stated is the whole conversion, and the check is that the geometry
    /// then places the grid somewhere *else* — over Europe for this fixture,
    /// not at the numbers in the message.
    #[test]
    fn a_rotated_grid_keeps_its_corners_in_the_rotated_frame() {
        let gds = section_from("tests/fixtures/rotated_latlon_surface.grib2", 0);
        let GridTemplate::RotatedLatLon(t) = gds.template else {
            panic!("the fixture should be a §3.1 message");
        };
        let GridGeometry::RotatedLatLon(p) = GridGeometry::from(&gds) else {
            panic!("§3.1 should convert to a rotated geometry");
        };
        assert_eq!((p.lat_first, p.lon_first), (t.la1, t.lo1));
        assert_eq!(
            (p.south_pole_lat, p.south_pole_lon),
            (t.south_pole_lat, t.south_pole_lon)
        );
        let geom = GridGeometry::from(&gds);
        let (lat, lon) = geom.forward(0, 0).expect("a rotated grid is placed");
        assert!(
            (lat - t.la1).abs() > 1e-6 || (lon - t.lo1).abs() > 1e-6,
            "the first point came back at its rotated-frame corner ({lat}, {lon}), \
             so the rotation was never applied",
        );
    }

    /// An 11x11 space view centered on the sub-satellite point, GOES-East
    /// longitude, default `(i+, j-)` scan.
    fn space_view_template() -> crate::gds::SpaceViewTemplate {
        crate::gds::SpaceViewTemplate {
            shape_of_earth: 5,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314,
            nx: 11,
            ny: 11,
            lap: 0.0,
            lop: -75.0,
            dx: 15,
            dy: 15,
            xp: 5.0,
            yp: 5.0,
            orientation: 0.0,
            nr: Some(6_610_710),
            xo: 0,
            yo: 0,
            resolution_flags: 0,
            scanning_mode: 0,
        }
    }

    /// §3.90 states an apparent Earth diameter and a camera altitude rather
    /// than scan angles, so the geometry it converts to is derived rather than
    /// read. The check is that the point the message centers on — the
    /// sub-satellite point — lands back at the grid index it was centered at,
    /// and that the rows run the way the stored raster does.
    ///
    /// Row orientation is the subtle half, and it is not visible in a
    /// round trip: eccodes reverses the row loop when it writes, so stored row
    /// 0 is the northernmost under this scan mode. Getting it backwards
    /// reprojects a flipped image, which every symmetric assertion below would
    /// still pass — hence the north/south/east comparisons.
    #[test]
    fn a_space_view_places_its_sub_satellite_point_at_its_own_index() {
        let geom =
            GridGeometry::Geostationary(space_view_template().scan_grid().expect("scan grid"));
        let idx = geom
            .inverse(0.0, -75.0)
            .expect("the sub-satellite point is on the grid");
        assert!((idx.i - 5.0).abs() < 1e-6, "i = {}", idx.i);
        assert!((idx.j - 5.0).abs() < 1e-6, "j = {}", idx.j);

        let GridGeometry::Geostationary(p) = geom else {
            unreachable!("built as a geostationary geometry")
        };
        // `Nr` is in units of the Earth's radius x 10^-6, so the camera is
        // 6.61071 equatorial radii from the center.
        assert!((p.h_metres / p.r_eq - 6.610_71).abs() < 1e-4);
        assert!(p.sweep_x, "GRIB2 §3.90 is the GOES-R sweep-x convention");
        // The far side of the Earth is not on this grid.
        assert!(geom.inverse(0.0, 105.0).is_none());

        let north = geom.inverse(20.0, -75.0).expect("north on grid");
        let south = geom.inverse(-20.0, -75.0).expect("south on grid");
        let east = geom.inverse(0.0, -60.0).expect("east on grid");
        assert!(north.j < 5.0, "north row {} should be < center", north.j);
        assert!(south.j > 5.0, "south row {} should be > center", south.j);
        assert!(east.i > 5.0, "east col {} should be > center", east.i);
        assert!(
            ((north.j - 5.0) + (south.j - 5.0)).abs() < 1e-6,
            "north/south not symmetric: {} / {}",
            north.j,
            south.j
        );
    }

    /// A message that describes no view has no scan grid, and the geometry says
    /// so rather than taking the `asin` of a number outside its domain or
    /// dividing by a zero diameter.
    #[test]
    fn a_space_view_with_no_usable_camera_declines() {
        let base = space_view_template();
        for (why, template) in [
            (
                "orthographic: `Nr` is the missing sentinel",
                crate::gds::SpaceViewTemplate { nr: None, ..base },
            ),
            (
                // In units of the Earth's radius x 10^6, so this is a camera
                // exactly one radius from the center: on the surface.
                "a camera at or below the surface",
                crate::gds::SpaceViewTemplate {
                    nr: Some(1_000_000),
                    ..base
                },
            ),
            (
                "an Earth of zero apparent diameter",
                crate::gds::SpaceViewTemplate { dx: 0, ..base },
            ),
        ] {
            assert!(template.scan_grid().is_none(), "{why}");
            let mut gds = section_from("tests/fixtures/regular_latlon_surface.grib2", 0);
            gds.template = GridTemplate::SpaceView(template);
            let geom = GridGeometry::from(&gds);
            assert_eq!(geom.kind(), "unsupported", "{why}");
            assert_eq!(geom.label(), "space_view", "{why}");
        }
    }

    /// A single-message fixture's bytes with §3's `Lo2` (octets 60-63 of
    /// templates 3.0 and 3.40, micro-degrees) rewritten, and when given, its
    /// Code Table 3.11 interpretation of the list (octet 12).
    fn with_lo2(path: &str, lo2_micro: u32, interp: Option<u8>) -> Vec<u8> {
        let mut bytes = std::fs::read(path).expect("fixture");
        let mut at = 16;
        while bytes[at + 4] != 3 {
            at += u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 octets")) as usize;
        }
        bytes[at + 59..at + 63].copy_from_slice(&lo2_micro.to_be_bytes());
        if let Some(code) = interp {
            bytes[at + 11] = code;
        }
        bytes
    }

    fn reduced_case(
        path: &str,
        lo2_micro: u32,
        interp: Option<u8>,
    ) -> (crate::Grib2Reader, GridGeometry) {
        let reader =
            crate::Grib2Reader::from_bytes(with_lo2(path, lo2_micro, interp)).expect("parse");
        assert!(
            reader.messages[0].gds.points_per_row().is_some(),
            "{path}: still reduced"
        );
        let geom = GridGeometry::from(&reader.messages[0].gds);
        (reader, geom)
    }

    /// Every reduced grid the crate ships is global, and still places.
    #[test]
    fn every_reduced_fixture_still_places() {
        let mut seen = 0;
        for entry in std::fs::read_dir("tests/fixtures").expect("fixtures dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("grib2") {
                continue;
            }
            let Ok(reader) = crate::Grib2Reader::from_bytes(std::fs::read(&path).expect("bytes"))
            else {
                continue;
            };
            for msg in &reader.messages {
                if msg.gds.points_per_row().is_none() {
                    continue;
                }
                seen += 1;
                let geom = GridGeometry::from(&msg.gds);
                assert!(
                    !matches!(geom, GridGeometry::Unsupported { .. }),
                    "{}: a global reduced grid was declined as {:?}",
                    path.display(),
                    geom.label()
                );
            }
        }
        assert!(seen >= 4, "only {seen} reduced messages found");
    }

    /// The review's case: a reduced lat/lon grid whose rows are delimited by
    /// its corners (Code Table 3.11 code 2) and run 0..100°. It used to be
    /// placed as a 0..350° grid of full circles. Now its placement is declined
    /// and decode still reads it.
    #[test]
    fn a_reduced_latlon_sub_area_is_declined_not_drawn_global() {
        let path = "tests/fixtures/reduced_latlon_10deg.grib2";
        let (_, global) = reduced_case(path, 350_000_000, Some(2));
        assert_eq!(global.kind(), "latlon", "the fixture's own 0..350 places");

        let (reader, geom) = reduced_case(path, 100_000_000, Some(2));
        let GridGeometry::Unsupported { label, declared } = &geom else {
            panic!("a sub-area placed as {geom:?}");
        };
        assert_eq!(
            label, "reduced_latlon",
            "named as the message table names it"
        );
        assert_eq!(*declared, Some(reader.messages[0].gds.fingerprint));
        assert_eq!(geom.declined_family(), Some("reduced_latlon"));
        assert_eq!(
            reader.decode_message_raster(0).expect("decodes").len(),
            36 * 19
        );
        // The same under code 1: eccodes' test reads the corners alone.
        let (_, geom) = reduced_case(path, 100_000_000, Some(1));
        assert!(matches!(geom, GridGeometry::Unsupported { .. }), "{geom:?}");
    }

    /// The same for a reduced Gaussian grid: the ECMWF N32 fixture with its
    /// rows cut to 0..100°.
    #[test]
    fn a_reduced_gaussian_sub_area_is_declined_not_drawn_global() {
        let path = "tests/fixtures/reduced_gaussian_pressure_level.grib2";
        let (_, global) = reduced_case(path, 357_187_500, None);
        assert_eq!(
            global.kind(),
            "gaussian",
            "the fixture's own 0..357.1875 places"
        );

        let (reader, geom) = reduced_case(path, 100_000_000, None);
        let GridGeometry::Unsupported { label, declared } = &geom else {
            panic!("a sub-area placed as {geom:?}");
        };
        assert_eq!(
            label, "reduced_gaussian",
            "named as the message table names it"
        );
        assert_eq!(*declared, Some(reader.messages[0].gds.fingerprint));
        assert!(
            reader.decode_message_raster(0).is_ok(),
            "decode still reads it"
        );
    }
}
