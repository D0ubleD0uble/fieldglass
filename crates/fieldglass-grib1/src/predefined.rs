//! Predefined GRIB1 grids — WMO ON388 Table B.
//!
//! When a GRIB1 message carries no Grid Description Section (PDS octet 8 bit 1
//! clear), the grid is identified instead by the catalogue number in PDS octet
//! 7 (`grid_number`). This module maps a curated subset of those numbers to the
//! grid geometry the absent GDS would have described, so the rest of the
//! pipeline (dimensions, bounds, reprojection) treats them like any other grid.
//!
//! Source of truth is the NCEP ON388 Table B specification
//! (<https://www.nco.ncep.noaa.gov/pmb/docs/on388/tableb.html>), the same
//! authority as the other lookup tables in this crate — not eccodes, which only
//! ships the legacy pole-staggered hemispheric grids (21–26, 61–64). The subset
//! here is the standard regular global lat/lon grids, which synthesise to exact
//! [`LatLonGrid`]s (`Ni·Nj` points, no staggering).

use crate::gds::{GridDescription, LatLonGrid, ResolutionFlags, ScanningMode};

/// One ON388 Table B regular lat/lon grid. All entries scan north→south,
/// west→east with the increments given.
struct PredefinedLatLon {
    grid_number: u8,
    ni: u32,
    nj: u32,
    /// Latitude / longitude of the first (NW) and last (SE) grid points, degrees.
    la1: f64,
    lo1: f64,
    la2: f64,
    lo2: f64,
    /// Increments, degrees.
    di: f64,
    dj: f64,
}

/// The supported subset: NCEP global lat/lon grids 2 (2.5°), 3 (1.0°), and
/// 4 (0.5°). Each is a full, regular global grid (`Lo2 = 360 − Di`).
const PREDEFINED: &[PredefinedLatLon] = &[
    PredefinedLatLon {
        grid_number: 2,
        ni: 144,
        nj: 73,
        la1: 90.0,
        lo1: 0.0,
        la2: -90.0,
        lo2: 357.5,
        di: 2.5,
        dj: 2.5,
    },
    PredefinedLatLon {
        grid_number: 3,
        ni: 360,
        nj: 181,
        la1: 90.0,
        lo1: 0.0,
        la2: -90.0,
        lo2: 359.0,
        di: 1.0,
        dj: 1.0,
    },
    PredefinedLatLon {
        grid_number: 4,
        ni: 720,
        nj: 361,
        la1: 90.0,
        lo1: 0.0,
        la2: -90.0,
        lo2: 359.5,
        di: 0.5,
        dj: 0.5,
    },
];

/// Resolve a GDS-absent message's `grid_number` to its predefined geometry.
/// Returns `None` for `255` (no predefined grid) and for any number outside the
/// supported subset, leaving the message with no grid (as before).
pub fn predefined_grid(grid_number: u8) -> Option<GridDescription> {
    let g = PREDEFINED.iter().find(|g| g.grid_number == grid_number)?;
    Some(GridDescription::LatLon(LatLonGrid {
        ni: g.ni,
        nj: g.nj,
        lat_first: g.la1,
        lon_first: g.lo1,
        lat_last: g.la2,
        lon_last: g.lo2,
        di: g.di,
        dj: g.dj,
        // Increments are given; the grid is spherical and earth-relative — the
        // defaults an ON388 regular global grid implies.
        resolution_flags: ResolutionFlags {
            increments_given: true,
            earth_oblate: false,
            uv_relative_to_grid: false,
        },
        // North→south, west→east, row-major.
        scanning_mode: ScanningMode {
            i_negative: false,
            j_positive: false,
            j_consecutive: false,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_2_is_the_global_2p5_degree_grid() {
        let GridDescription::LatLon(g) = predefined_grid(2).expect("grid 2 is known") else {
            panic!("expected LatLon");
        };
        assert_eq!((g.ni, g.nj), (144, 73));
        assert_eq!((g.lat_first, g.lon_first), (90.0, 0.0));
        assert_eq!((g.lat_last, g.lon_last), (-90.0, 357.5));
        assert_eq!((g.di, g.dj), (2.5, 2.5));
        // A full regular global grid stores Ni·Nj points.
        assert_eq!(g.ni * g.nj, 10_512);
    }

    #[test]
    fn grids_report_geometry_and_full_point_count() {
        // (number, Ni, Nj, Lo2): the published ON388 Table B values.
        for &(number, ni, nj, lo2) in &[
            (2u8, 144u32, 73u32, 357.5),
            (3, 360, 181, 359.0),
            (4, 720, 361, 359.5),
        ] {
            let gd = predefined_grid(number).expect("known grid");
            assert_eq!(gd.grid_type_name(), "latlon");
            assert_eq!(gd.dimensions(), Some((ni, nj)));
            assert_eq!(gd.num_data_points(), Some((ni * nj) as usize));
            let (la1, lo1, la2, got_lo2) = gd.bounds().expect("has bounds");
            assert_eq!((la1, lo1, la2), (90.0, 0.0, -90.0));
            assert_eq!(got_lo2, lo2);
        }
    }

    #[test]
    fn unknown_and_sentinel_grid_numbers_are_none() {
        assert!(predefined_grid(255).is_none(), "255 = no predefined grid");
        assert!(predefined_grid(99).is_none(), "unsupported number");
    }
}
