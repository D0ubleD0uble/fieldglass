//! [`GridDescription`] → [`GridGeometry`] (#460).
//!
//! One direction only: the decoder states the grid in GRIB1's own octets, and
//! `core` owns the typed value everything downstream places points with. The
//! conversion is here rather than in `core` because `core` knows nothing about
//! GRIB1, and it is `From` rather than an inherent method so a host can write
//! `msg.gds.as_ref().map(GridGeometry::from)` without naming this module.
//!
//! Three rules the mapping follows, each one a place a fresh implementation
//! gets it wrong:
//!
//! * **`Dx`/`Dy` carry the scan sign.** GRIB1 stores them as unsigned
//!   magnitudes (ON388 GDS octets 21–23 / 24–26) and puts the direction in the
//!   scanning-mode flags, so the planar families read them through the grid's
//!   own `signed_increments()` — the same accessor the far-corner recovery and
//!   the napi warp setup use, for the same reason: a grid scanning
//!   north-to-south walks `-Dy`.
//! * **A reduced grid reports the raster its rows expand into**, not the row
//!   list. [`crate::Grib1Reader::decode_message_raster`] widens every row to
//!   `max(PL)` at the decode boundary, so the geometry that places those values
//!   has to describe the widened raster — including its last-column longitude,
//!   which is derived and is *not* the `Lo2` the message declares. That
//!   derivation is [`GridDescription::raster_bounds`], read here rather than
//!   repeated, so this conversion and the hosts cannot drift apart about where
//!   an octahedral grid's east edge is. That widening lays every row out as a
//!   full circle, so a reduced grid whose corners cover only part of one is
//!   declined rather than drawn as if it were global (#1760).
//! * **An unmodeled family is `Unsupported`, not an error.** A spectral
//!   message still has metadata worth listing; the geometry simply cannot
//!   place its points, and it says which family it declined.
//!
//! GRIB1 has no `LaD` field for polar stereographic: ON388 fixes the latitude
//! of true scale at ±60°, which [`crate::gds::PolarStereoGrid::lad`] supplies.

use fieldglass_core::{
    GaussianParams, GridFingerprint, GridGeometry, LatLonParams, RotatedLatLonParams,
    reduced_grid_is_global, reduced_raster_width,
};

use crate::gds::GridDescription;

/// The east edge of the raster a reduced grid's rows expand into, read off the
/// GDS ([`GridDescription::raster_bounds`]) so this conversion shares the one
/// rule rather than restating it.
///
/// `declared` is the fallback, and a reduced grid never takes it: every reduced
/// variant reports `bounds()`, so `raster_bounds()` is always `Some` here.
fn raster_lon_last(gds: &GridDescription, declared: f64) -> f64 {
    gds.raster_bounds().map_or(declared, |c| c.lon_last)
}

/// A reduced grid whose declared longitudes cover only part of a circle, or
/// `None` when its rows go all the way around.
///
/// The expansion that places a reduced grid lays every row out as a full
/// circle (`360° / PL[j]` apart), so a sub-area would be drawn spread over the
/// whole globe. Until a sub-area expansion exists such a grid is declined for
/// placement, under a label that says why in plain words; decode still returns
/// its values. eccodes' test decides "global", through
/// [`reduced_grid_is_global`], the same one the GRIB2 reduced arms use.
///
/// GRIB1 keeps no fingerprint of its section, so this one folds in what the
/// GDS declares about the grid: its corners, its rows and its `PL` list. Two
/// different sub-areas then never compare equal and combine.
fn partial_circle(
    family: &str,
    corners: [f64; 4],
    nj: u32,
    points_per_row: &[u32],
) -> Option<GridGeometry> {
    let [_, lon_first, _, lon_last] = corners;
    if reduced_grid_is_global(lon_first, lon_last, points_per_row) {
        return None;
    }
    let declared = corners
        .iter()
        .fold(GridFingerprint::of(family.as_bytes()), |f, c| {
            f.with_u64(c.to_bits())
        })
        .with_u64(u64::from(nj));
    let declared = points_per_row
        .iter()
        .fold(declared, |f, &n| f.with_u64(u64::from(n)));
    Some(GridGeometry::Unsupported {
        label: format!("{family} over part of a circle"),
        declared: Some(declared),
    })
}

impl From<&GridDescription> for GridGeometry {
    fn from(gds: &GridDescription) -> Self {
        match gds {
            GridDescription::LatLon(g) => Self::LatLon(LatLonParams {
                ni: g.ni,
                nj: g.nj,
                lat_first: g.lat_first,
                lon_first: g.lon_first,
                lat_last: g.lat_last,
                lon_last: g.lon_last,
            }),
            GridDescription::ReducedLatLon(g) => {
                let corners = [g.lat_first, g.lon_first, g.lat_last, g.lon_last];
                if let Some(declined) =
                    partial_circle("reduced lat/lon grid", corners, g.nj, &g.points_per_row)
                {
                    return declined;
                }
                let ni = reduced_raster_width(&g.points_per_row);
                Self::LatLon(LatLonParams {
                    ni,
                    nj: g.nj,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lat_last: g.lat_last,
                    lon_last: raster_lon_last(gds, g.lon_last),
                })
            }
            GridDescription::Gaussian(g) => Self::Gaussian(GaussianParams {
                ni: g.ni,
                nj: g.nj,
                lat_first: g.lat_first,
                lon_first: g.lon_first,
                lat_last: g.lat_last,
                lon_last: g.lon_last,
                n_parallels: u32::from(g.n_gaussians),
            }),
            GridDescription::ReducedGaussian(g) => {
                let corners = [g.lat_first, g.lon_first, g.lat_last, g.lon_last];
                if let Some(declined) =
                    partial_circle("reduced Gaussian grid", corners, g.nj, &g.points_per_row)
                {
                    return declined;
                }
                let ni = reduced_raster_width(&g.points_per_row);
                Self::Gaussian(GaussianParams {
                    ni,
                    nj: g.nj,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lat_last: g.lat_last,
                    lon_last: raster_lon_last(gds, g.lon_last),
                    n_parallels: u32::from(g.n_gaussians),
                })
            }
            // ON388 states no separate LaD; the first standard parallel is
            // the latitude of true scale, the same parameters the GDS
            // recovers its opposite corner from.
            GridDescription::LambertConformal(g) => Self::Lambert(g.projection_params()),
            // Grid type 10. Its corners are **rotated-frame** degrees, so
            // they are handed over as stated — unrotating them here would
            // place the grid twice. Same shape as the GRIB2 §3.1 conversion,
            // because it is the same grid written in different octets.
            GridDescription::RotatedLatLon(g) => Self::RotatedLatLon(RotatedLatLonParams {
                ni: g.ni,
                nj: g.nj,
                lat_first: g.lat_first,
                lon_first: g.lon_first,
                lat_last: g.lat_last,
                lon_last: g.lon_last,
                south_pole_lat: g.south_pole_lat,
                south_pole_lon: g.south_pole_lon,
                angle_of_rotation: g.angle_of_rotation,
            }),
            GridDescription::PolarStereographic(g) => Self::PolarStereo(g.projection_params()),
            // Not a raster at all: spherical harmonics live in wavenumber
            // space, and `synthesize_spectral_message` is what turns one into a
            // lat/lon grid with its own geometry. It names itself rather than
            // erroring, and so does a grid type this parser never read past.
            other => Self::Unsupported {
                label: other.grid_type_name().to_string(),
                declared: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gds::{
        GaussianGrid, LambertGrid, LatLonGrid, PolarStereoGrid, ReducedGaussianGrid,
        ResolutionFlags, RotatedLatLonGrid, ScanningMode,
    };
    use fieldglass_core::LonLatBox;

    fn flags() -> ResolutionFlags {
        ResolutionFlags {
            increments_given: true,
            earth_oblate: false,
            uv_relative_to_grid: false,
        }
    }

    /// North-to-south, west-to-east: the operational default, and the scan
    /// under which `Dy` is negative.
    fn scan_north_down() -> ScanningMode {
        ScanningMode {
            i_negative: false,
            j_positive: false,
            j_consecutive: false,
        }
    }

    #[test]
    fn latlon_carries_both_corners() {
        let gds = GridDescription::LatLon(LatLonGrid {
            ni: 360,
            nj: 181,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
            di: 1.0,
            dj: 1.0,
            resolution_flags: flags(),
            scanning_mode: scan_north_down(),
        });
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "latlon");
        assert_eq!(geom.dims(), Some((360, 181)));
        // The declared first point is grid point (0, 0) by definition.
        let (lat, lon) = geom.forward(0, 0).expect("first point");
        assert!((lat - 90.0).abs() < 1e-9, "lat {lat}");
        assert!((lon - 0.0).abs() < 1e-9, "lon {lon}");
    }

    #[test]
    fn gaussian_carries_the_parallel_count() {
        let gds = GridDescription::Gaussian(GaussianGrid {
            ni: 128,
            nj: 64,
            lat_first: 87.863_799,
            lon_first: 0.0,
            lat_last: -87.863_799,
            lon_last: 357.1875,
            di: 2.8125,
            n_gaussians: 32,
            resolution_flags: flags(),
            scanning_mode: scan_north_down(),
        });
        let GridGeometry::Gaussian(p) = GridGeometry::from(&gds) else {
            panic!("expected a Gaussian geometry");
        };
        assert_eq!(p.n_parallels, 32);
        assert_eq!((p.ni, p.nj), (128, 64));
    }

    /// The raster a reduced grid expands into is `max(PL)` wide, and its last
    /// column sits where the expansion puts it — not at the `Lo2` the message
    /// declares, which describes the reference regular grid.
    #[test]
    fn reduced_gaussian_describes_the_expanded_raster() {
        // An octahedral-shaped row list whose widest row (20) is wider than
        // the declared last longitude implies.
        let pl = vec![4, 8, 12, 16, 20, 20, 16, 12, 8, 4];
        let gds = GridDescription::ReducedGaussian(ReducedGaussianGrid {
            nj: 10,
            lat_first: 80.0,
            lon_first: 0.0,
            lat_last: -80.0,
            lon_last: 337.5, // as if 16 columns
            n_gaussians: 5,
            points_per_row: pl,
            resolution_flags: flags(),
            scanning_mode: scan_north_down(),
        });
        let GridGeometry::Gaussian(p) = GridGeometry::from(&gds) else {
            panic!("expected a Gaussian geometry");
        };
        assert_eq!(p.ni, 20, "the raster is as wide as the widest row");
        assert_eq!(p.nj, 10);
        assert!(
            (p.lon_last - 342.0).abs() < 1e-9,
            "lon_last {} should be lon_first + 19·360/20",
            p.lon_last
        );
    }

    /// GRIB1 stores `Dx`/`Dy` as magnitudes. A north-to-south scan walks the
    /// projection plane downward, so `dy_metres` has to come back negative or
    /// every row lands on the wrong side of the origin.
    #[test]
    fn lambert_bakes_the_scan_sign_into_the_increments() {
        let gds = GridDescription::LambertConformal(LambertGrid {
            nx: 100,
            ny: 80,
            lat_first: 20.0,
            lon_first: -120.0,
            lov: -95.0,
            dx_m: 12_000,
            dy_m: 12_000,
            south_pole: false,
            latin1: 25.0,
            latin2: 25.0,
            lat_south_pole: -90.0,
            lon_south_pole: 0.0,
            resolution_flags: flags(),
            scanning_mode: scan_north_down(),
        });
        let GridGeometry::Lambert(p) = GridGeometry::from(&gds) else {
            panic!("expected a Lambert geometry");
        };
        assert_eq!(p.dx_metres, 12_000.0);
        assert_eq!(p.dy_metres, -12_000.0, "north-to-south scan walks -Dy");
        assert_eq!(p.lad, 25.0, "LaD falls back to the first standard parallel");
        assert_eq!(p.earth_radius_m, crate::gds::GRIB1_SPHERICAL_RADIUS_M);
    }

    #[test]
    fn polar_stereo_supplies_the_fixed_true_scale() {
        let gds = GridDescription::PolarStereographic(PolarStereoGrid {
            nx: 51,
            ny: 55,
            lat_first: 33.0,
            lon_first: -155.0,
            lov: -105.0,
            dx_m: 190_500,
            dy_m: 190_500,
            south_pole: false,
            resolution_flags: flags(),
            scanning_mode: ScanningMode {
                i_negative: false,
                j_positive: true,
                j_consecutive: false,
            },
        });
        let GridGeometry::PolarStereo(p) = GridGeometry::from(&gds) else {
            panic!("expected a polar stereographic geometry");
        };
        assert_eq!(p.lad, 60.0, "ON388 fixes GRIB1's true scale at 60°");
        assert!(!p.south_pole);
        assert_eq!(p.dy_metres, 190_500.0, "south-to-north scan walks +Dy");
    }

    /// Grid type 10's corners are rotated-frame degrees. The conversion hands
    /// them over untouched, and the proof that this is right is that the
    /// geometry then places the grid somewhere else: a COSMO-EU-shaped domain
    /// whose corners read as -18..21E lands over Europe, not the Sahara.
    #[test]
    fn rotated_latlon_keeps_its_corners_in_the_rotated_frame() {
        let gds = GridDescription::RotatedLatLon(RotatedLatLonGrid {
            ni: 40,
            nj: 42,
            lat_first: -20.0,
            lon_first: -18.0,
            lat_last: 21.0,
            lon_last: 21.0,
            di: 1.0,
            dj: 1.0,
            south_pole_lat: -40.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
            resolution_flags: flags(),
            scanning_mode: ScanningMode {
                i_negative: false,
                j_positive: true,
                j_consecutive: false,
            },
        });
        let GridGeometry::RotatedLatLon(p) = GridGeometry::from(&gds) else {
            panic!("grid type 10 should convert to a rotated geometry");
        };
        assert_eq!((p.lat_first, p.lon_first), (-20.0, -18.0));
        assert_eq!((p.south_pole_lat, p.south_pole_lon), (-40.0, 10.0));

        let geom = GridGeometry::from(&gds);
        let (lat, lon) = geom.forward(0, 0).expect("a rotated grid is placed");
        assert!(
            (lat - p.lat_first).abs() > 1e-6 || (lon - p.lon_first).abs() > 1e-6,
            "the first point came back at its rotated-frame corner ({lat}, {lon}), \
             so the rotation was never applied",
        );
        let LonLatBox {
            lat_min, lat_max, ..
        } = geom.lonlat_bbox().expect("and framed");
        assert!(
            lat_min > 25.0 && lat_max < 75.0,
            "the box ({lat_min}..{lat_max}) is not over Europe, so the perimeter \
             was read as geographic instead of unrotated",
        );
    }

    #[test]
    fn an_unmodeled_family_names_itself() {
        let gds = GridDescription::Unsupported { grid_type: 90 };
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), gds.grid_type_name());
        assert!(geom.lonlat_bbox().is_none());
        assert!(geom.proj4().is_none());
    }

    /// The committed classic N32 reduced Gaussian fixture, with the byte
    /// offset of its GDS. The GDS follows the 8-octet indicator and the PDS,
    /// whose length is its first three octets.
    fn reduced_fixture() -> (Vec<u8>, usize) {
        let bytes = std::fs::read("tests/fixtures/reduced_gg_n32.grib1").expect("fixture");
        let pds_len =
            usize::from(bytes[8]) << 16 | usize::from(bytes[9]) << 8 | usize::from(bytes[10]);
        (bytes, 8 + pds_len)
    }

    /// GDS surgery: write `Lo2` (octets 21-23, millidegrees, sign bit high)
    /// and, when given, the data representation type (octet 6).
    fn surgery(lon_last_milli: u32, grid_type: Option<u8>) -> Vec<u8> {
        let (mut bytes, gds) = reduced_fixture();
        bytes[gds + 20..gds + 23].copy_from_slice(&lon_last_milli.to_be_bytes()[1..]);
        if let Some(t) = grid_type {
            bytes[gds + 5] = t;
        }
        bytes
    }

    fn geometry_of(bytes: &[u8]) -> (GridDescription, GridGeometry) {
        let reader = crate::Grib1Reader::from_bytes(bytes.to_vec()).expect("parses");
        let gds = reader.messages[0].gds.clone().expect("a GDS");
        let geom = GridGeometry::from(&gds);
        (gds, geom)
    }

    /// Every reduced grid in the committed corpus is global, and still places.
    #[test]
    fn every_reduced_fixture_still_places() {
        let mut seen = 0;
        for entry in std::fs::read_dir("tests/fixtures").expect("fixtures dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("grib1") {
                continue;
            }
            let Ok(reader) = crate::Grib1Reader::from_bytes(std::fs::read(&path).expect("bytes"))
            else {
                continue;
            };
            for msg in &reader.messages {
                let Some(gds) = &msg.gds else { continue };
                if gds.points_per_row().is_none() {
                    continue;
                }
                seen += 1;
                let geom = GridGeometry::from(gds);
                assert!(
                    !matches!(geom, GridGeometry::Unsupported { .. }),
                    "{}: a global reduced grid was declined as {:?}",
                    path.display(),
                    geom.label()
                );
            }
        }
        assert!(seen >= 5, "only {seen} reduced messages found");
    }

    /// A reduced Gaussian grid whose rows cover 0..100° only is declined for
    /// placement in plain words, where it used to be drawn as full circles.
    /// Decode still returns its values.
    #[test]
    fn a_reduced_gaussian_sub_area_is_declined_not_drawn_global() {
        let global = surgery(357_188, None);
        let (_, geom) = geometry_of(&global);
        assert!(matches!(geom, GridGeometry::Gaussian(_)), "{geom:?}");

        let sub_area = surgery(100_000, None);
        let (gds, geom) = geometry_of(&sub_area);
        assert!(matches!(gds, GridDescription::ReducedGaussian(_)));
        let GridGeometry::Unsupported { label, declared } = &geom else {
            panic!("a sub-area placed as {geom:?}");
        };
        assert_eq!(label, "reduced Gaussian grid over part of a circle");
        assert!(declared.is_some(), "two sub-areas must not compare equal");
        let reader = crate::Grib1Reader::from_bytes(sub_area).expect("parses");
        assert!(
            reader.decode_message_raster(0).is_ok(),
            "decode still reads it"
        );

        // Two different sub-areas are different grids.
        let (_, other) = geometry_of(&surgery(90_000, None));
        assert_ne!(geom, other);
    }

    /// The same for a reduced lat/lon grid: the fixture's GDS retyped to
    /// data representation type 0, which keeps `Ni` missing and the list.
    #[test]
    fn a_reduced_latlon_sub_area_is_declined_not_drawn_global() {
        let global = surgery(357_188, Some(0));
        let (gds, geom) = geometry_of(&global);
        assert!(matches!(gds, GridDescription::ReducedLatLon(_)));
        assert!(matches!(geom, GridGeometry::LatLon(_)), "{geom:?}");

        let sub_area = surgery(100_000, Some(0));
        let (_, geom) = geometry_of(&sub_area);
        let GridGeometry::Unsupported { label, declared } = &geom else {
            panic!("a sub-area placed as {geom:?}");
        };
        assert_eq!(label, "reduced lat/lon grid over part of a circle");
        assert!(declared.is_some());
        let reader = crate::Grib1Reader::from_bytes(sub_area).expect("parses");
        assert!(
            reader.decode_message_raster(0).is_ok(),
            "decode still reads it"
        );
    }
}
