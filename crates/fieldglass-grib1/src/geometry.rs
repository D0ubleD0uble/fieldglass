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
//!   scanning-mode flags, so the planar families multiply them through
//!   [`signed_grid_increments`] — the same call the napi warp setup makes, for
//!   the same reason: a grid scanning north-to-south walks `-Dy`.
//! * **A reduced grid reports the raster its rows expand into**, not the row
//!   list. [`crate::Grib1Reader::decode_message_values`] widens every row to
//!   `max(PL)` at the decode boundary, so the geometry that places those values
//!   has to describe the widened raster — including its last-column longitude,
//!   which is derived ([`reduced_raster_lon_last`]) and is *not* the `Lo2` the
//!   message declares. See that function for why.
//! * **An unmodelled family is `Unsupported`, not an error.** A rotated lat/lon
//!   or spectral message still has metadata worth listing; the geometry simply
//!   cannot place its points, and it says which family it declined.
//!
//! GRIB1 has no `LaD` field for polar stereographic: ON388 fixes the latitude
//! of true scale at ±60°, so the conversion supplies it.

use fieldglass_core::{
    GaussianParams, GridGeometry, LambertParams, LatLonParams, PolarStereoParams,
    reduced_raster_lon_last, reduced_raster_width, signed_grid_increments,
};

use crate::gds::GridDescription;

/// The latitude of true scale ON388 fixes for a GRIB1 polar stereographic
/// grid. GRIB2 §3.20 carries `LaD` explicitly; GRIB1 does not state it at all.
const GRIB1_POLAR_STEREO_LAD_DEG: f64 = 60.0;

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
                let ni = reduced_raster_width(&g.points_per_row);
                Self::LatLon(LatLonParams {
                    ni,
                    nj: g.nj,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lat_last: g.lat_last,
                    lon_last: reduced_raster_lon_last(g.lon_first, ni),
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
                let ni = reduced_raster_width(&g.points_per_row);
                Self::Gaussian(GaussianParams {
                    ni,
                    nj: g.nj,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lat_last: g.lat_last,
                    lon_last: reduced_raster_lon_last(g.lon_first, ni),
                    n_parallels: u32::from(g.n_gaussians),
                })
            }
            GridDescription::LambertConformal(g) => {
                let (dx, dy) = signed_grid_increments(
                    f64::from(g.dx_m),
                    f64::from(g.dy_m),
                    g.scanning_mode.i_negative,
                    g.scanning_mode.j_positive,
                );
                Self::Lambert(LambertParams {
                    earth_radius_m: g.resolution_flags.earth_radius_m(),
                    ni: g.nx,
                    nj: g.ny,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    // ON388 states no separate LaD; the first standard
                    // parallel is the latitude of true scale, matching how the
                    // GDS itself recovers the opposite corner.
                    lad: g.latin1,
                    lov: g.lov,
                    dx_metres: dx,
                    dy_metres: dy,
                    latin1: g.latin1,
                    latin2: g.latin2,
                })
            }
            GridDescription::PolarStereographic(g) => {
                let (dx, dy) = signed_grid_increments(
                    f64::from(g.dx_m),
                    f64::from(g.dy_m),
                    g.scanning_mode.i_negative,
                    g.scanning_mode.j_positive,
                );
                Self::PolarStereo(PolarStereoParams {
                    earth_radius_m: g.resolution_flags.earth_radius_m(),
                    ni: g.nx,
                    nj: g.ny,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lov: g.lov,
                    lad: GRIB1_POLAR_STEREO_LAD_DEG,
                    dx_metres: dx,
                    dy_metres: dy,
                    south_pole: g.south_pole,
                })
            }
            // Modelled by `core`'s projectors but not yet by `GridGeometry`
            // (rotated lat/lon), or not a raster at all (spherical harmonics,
            // which `synthesize_spectral_message` turns into a lat/lon grid
            // with its own geometry). Both name themselves rather than erroring.
            other => Self::Unsupported {
                label: other.grid_type_name().to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gds::{
        GaussianGrid, LambertGrid, LatLonGrid, PolarStereoGrid, ReducedGaussianGrid,
        ResolutionFlags, ScanningMode,
    };

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

    #[test]
    fn an_unmodelled_family_names_itself() {
        let gds = GridDescription::Unsupported { grid_type: 90 };
        let geom = GridGeometry::from(&gds);
        assert_eq!(geom.kind(), "unsupported");
        assert_eq!(geom.label(), gds.grid_type_name());
        assert!(geom.lonlat_bbox().is_none());
        assert!(geom.proj4().is_none());
    }
}
