//! [`GridDefinitionSection`] → [`GridGeometry`] (#460).
//!
//! The GRIB2 half of the conversion `fieldglass-grib1` makes from its own GDS;
//! read that module's header for the three rules both sides follow (scan-signed
//! increments, reduced grids describing their expanded raster, unmodelled
//! families naming themselves rather than erroring).
//!
//! It converts from the **section**, not from [`GridTemplate`] alone: a reduced
//! Gaussian grid's raster width comes from the `PL` list, which §3 carries
//! outside the template, and [`GridDefinitionSection::points_per_row`] is the
//! one place that decides whether the optional list really is a row count.

use fieldglass_core::{
    GaussianParams, GridGeometry, LambertParams, LatLonParams, MercatorParams, PolarStereoParams,
    RotatedLatLonParams, reduced_raster_width, signed_grid_increments,
};

use crate::gds::{GridDefinitionSection, GridTemplate};

impl From<&GridDefinitionSection> for GridGeometry {
    fn from(gds: &GridDefinitionSection) -> Self {
        match &gds.template {
            GridTemplate::LatLon(t) => Self::LatLon(LatLonParams {
                ni: t.ni,
                nj: t.nj,
                lat_first: t.la1,
                lon_first: t.lo1,
                lat_last: t.la2,
                lon_last: t.lo2,
            }),
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
                        gds.raster_bounds().map_or(t.lo2, |(_, _, _, lo2)| lo2),
                    ),
                    // A regular Gaussian grid with no `Ni` at all is malformed
                    // rather than reduced; `dims()` of `0` is what the section
                    // itself reports and keeps the two answers consistent.
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
            GridTemplate::Lambert(t) => {
                let (dx, dy) = signed_grid_increments(
                    t.dx_metres,
                    t.dy_metres,
                    t.scanning_mode & 0x80 != 0,
                    t.scanning_mode & 0x40 != 0,
                );
                Self::Lambert(LambertParams {
                    earth_radius_m: t.earth_radius_m,
                    ni: t.nx,
                    nj: t.ny,
                    lat_first: t.la1,
                    lon_first: t.lo1,
                    lad: t.lad,
                    lov: t.lov,
                    dx_metres: dx,
                    dy_metres: dy,
                    latin1: t.latin1,
                    latin2: t.latin2,
                })
            }
            GridTemplate::PolarStereographic(t) => {
                let (dx, dy) = signed_grid_increments(
                    t.dx_metres,
                    t.dy_metres,
                    t.scanning_mode & 0x80 != 0,
                    t.scanning_mode & 0x40 != 0,
                );
                Self::PolarStereo(PolarStereoParams {
                    earth_radius_m: t.earth_radius_m,
                    ni: t.nx,
                    nj: t.ny,
                    lat_first: t.la1,
                    lon_first: t.lo1,
                    lov: t.lov,
                    // §3.20 states LaD, unlike GRIB1's fixed ±60°.
                    lad: t.lad,
                    dx_metres: dx,
                    dy_metres: dy,
                    south_pole: t.south_pole,
                })
            }
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
                None => Self::Unsupported {
                    label: template_label(&gds.template).to_string(),
                },
            },
            other => Self::Unsupported {
                label: template_label(other).to_string(),
            },
        }
    }
}

/// How an unmodelled template names itself.
///
/// Every modelled family still has an arm: `Mercator` and the rest are
/// unreachable through the conversion above, but a §3.90 whose camera does not
/// describe a view reaches this table by name, and an exhaustive match is what
/// makes adding a template a compile error rather than a silent
/// `"unsupported"`.
fn template_label(template: &GridTemplate) -> &'static str {
    match template {
        GridTemplate::RotatedLatLon(_) => "rotated_latlon",
        GridTemplate::Mercator(_) => "mercator",
        GridTemplate::TransverseMercator(_) => "transverse_mercator",
        GridTemplate::LambertAzimuthal(_) => "lambert_azimuthal",
        GridTemplate::SpaceView(_) => "space_view",
        GridTemplate::SphericalHarmonic(_) => "spherical_harmonic",
        GridTemplate::BiFourier(_) => "bifourier",
        GridTemplate::Healpix(_) => "healpix",
        GridTemplate::Unsupported(_) => "unsupported",
        GridTemplate::LatLon(_) => "latlon",
        GridTemplate::Gaussian(_) => "gaussian",
        GridTemplate::Lambert(_) => "lambert",
        GridTemplate::PolarStereographic(_) => "polar_stereo",
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
                // Longitudes are compared modulo a turn: `forward` normalises
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
        // or §3.90 fixture, not because they are unmodelled; both are covered
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

    /// §3.1 used to land here. It is a modelled family now, so the case is
    /// carried by a template that really has no raster to place: spherical
    /// harmonics live in wavenumber space.
    #[test]
    fn an_unmodelled_template_names_itself() {
        let gds = section_from("tests/fixtures/spectral_simple_t63.grib2", 0);
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), "spherical_harmonic");
        assert!(geom.dims().is_none());
        assert!(geom.inverse(45.0, 10.0).is_none());
        assert_eq!(
            template_label(&GridTemplate::Unsupported(32_768)),
            "unsupported"
        );
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

    /// An 11x11 space view centred on the sub-satellite point, GOES-East
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
    /// read. The check is that the point the message centres on — the
    /// sub-satellite point — lands back at the grid index it was centred at,
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
        // 6.61071 equatorial radii from the centre.
        assert!((p.h_metres / p.r_eq - 6.610_71).abs() < 1e-4);
        assert!(p.sweep_x, "GRIB2 §3.90 is the GOES-R sweep-x convention");
        // The far side of the Earth is not on this grid.
        assert!(geom.inverse(0.0, 105.0).is_none());

        let north = geom.inverse(20.0, -75.0).expect("north on grid");
        let south = geom.inverse(-20.0, -75.0).expect("south on grid");
        let east = geom.inverse(0.0, -60.0).expect("east on grid");
        assert!(north.j < 5.0, "north row {} should be < centre", north.j);
        assert!(south.j > 5.0, "south row {} should be > centre", south.j);
        assert!(east.i > 5.0, "east col {} should be > centre", east.i);
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
                // exactly one radius from the centre: on the surface.
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
}
