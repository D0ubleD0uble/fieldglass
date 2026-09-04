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
    GaussianParams, GridGeometry, LambertParams, LatLonParams, PolarStereoParams,
    reduced_raster_width, signed_grid_increments,
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
            other => Self::Unsupported {
                label: template_label(other).to_string(),
            },
        }
    }
}

/// How an unmodelled template names itself. The four families above never
/// reach here, so they are absent by design rather than by omission.
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
                let (la1, lo1) = match &msg.gds.template {
                    GridTemplate::LatLon(t) => (t.la1, t.lo1),
                    GridTemplate::Gaussian(t) => (t.la1, t.lo1),
                    GridTemplate::Lambert(t) => (t.la1, t.lo1),
                    GridTemplate::PolarStereographic(t) => (t.la1, t.lo1),
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
        let expected: std::collections::BTreeSet<String> =
            ["latlon", "gaussian", "lambert", "polar_stereo"]
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

    #[test]
    fn an_unmodelled_template_names_itself() {
        let gds = section_from("tests/fixtures/rotated_latlon_surface.grib2", 0);
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), "rotated_latlon");
        assert!(geom.dims().is_none());
        assert!(geom.inverse(45.0, 10.0).is_none());
        assert_eq!(
            template_label(&GridTemplate::Unsupported(32_768)),
            "unsupported"
        );
    }
}
