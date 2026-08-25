//! Every grid point of a projected grid must invert back to itself.
//!
//! The oracle the projected inverses were missing. `.eccodes.ref.json` pins the
//! *forward* direction — where grid point `(i, j)` sits — and the golden in
//! `planar_inverse_golden.rs` pins whatever `inverse` currently does. Neither
//! asks the question that actually matters to a warp: given the lat/lon of a
//! point the grid contains, does `inverse` hand back the point? It needs no
//! external oracle, because the grid's own forward map supplies the answer.
//!
//! It is the test that would have caught #488, where a north polar
//! stereographic grid reaching across the equator refused its own southern
//! 2.6%.

use fieldglass_core::projection::*;

fn lambert() -> LambertParams {
    LambertParams {
        earth_radius_m: DEFAULT_EARTH_RADIUS_M,
        ni: 93,
        nj: 65,
        lat_first: 12.19,
        lon_first: -133.459,
        lad: 25.0,
        lov: -95.0,
        dx_metres: 81_271.0,
        dy_metres: 81_271.0,
        latin1: 25.0,
        latin2: 25.0,
    }
}
/// The CMC regional grid, matching `cmc_wind_300_2010052400_p012.grib`. Its far
/// corner reaches -4.718 degN, which is what #488 was about.
fn cmc_polar() -> PolarStereoParams {
    PolarStereoParams {
        earth_radius_m: DEFAULT_EARTH_RADIUS_M,
        ni: 135,
        nj: 95,
        lat_first: 11.43,
        lon_first: -110.27,
        lov: 247.0,
        lad: 60.0,
        dx_metres: 60_000.0,
        dy_metres: 60_000.0,
        south_pole: false,
    }
}
fn efas() -> LambertAzimuthalParams {
    LambertAzimuthalParams {
        semi_major_m: 6_378_137.0,
        semi_minor_m: 6_356_752.314,
        ni: 20,
        nj: 16,
        lat_first: 35.0,
        lon_first: -10.0,
        standard_parallel: 52.0,
        central_longitude: 10.0,
        dx_metres: 200_000.0,
        dy_metres: 200_000.0,
    }
}
fn ukv() -> TransverseMercatorParams {
    TransverseMercatorParams {
        semi_major_m: 6_377_563.0,
        semi_minor_m: 6_356_257.0,
        ni: 24,
        nj: 30,
        lat_ref: 49.0,
        lon_ref: -2.0,
        scale_factor: 0.999_601_244_926_452_6,
        false_easting_m: 400_000.0,
        false_northing_m: -100_000.0,
        x1_metres: -238_000.0,
        y1_metres: 1_222_000.0,
        dx_metres: 48_000.0,
        dy_metres: -48_000.0,
    }
}

/// Walk every grid point, geolocate it, and invert it back. `tol` is in
/// fractions of a cell; a warp samples at the returned index, so anything much
/// below half a cell reads the intended value.
fn round_trip(name: &str, p: &dyn PlanarGridProjector, tol: f64) {
    let (ni, nj) = p.grid_dims();
    let mut worst = 0.0f64;
    let mut dropped = 0u32;
    let mut first_drop = None;
    for j in 0..nj {
        for i in 0..ni {
            let (lat, lon) = p.grid_point_lonlat(i, j);
            match p.inverse(lat, lon) {
                Some(g) => {
                    worst = worst
                        .max((g.i - i as f64).abs())
                        .max((g.j - j as f64).abs());
                }
                None => {
                    dropped += 1;
                    first_drop.get_or_insert((i, j, lat, lon));
                }
            }
        }
    }
    assert_eq!(
        dropped,
        0,
        "{name}: {dropped} of {} grid points did not invert back; first was \
         ({}, {}) at {:?}",
        ni * nj,
        first_drop.unwrap().0,
        first_drop.unwrap().1,
        (first_drop.unwrap().2, first_drop.unwrap().3),
    );
    assert!(worst < tol, "{name}: worst index error {worst} cells");
}

#[test]
fn lambert_conformal_inverts_every_grid_point() {
    round_trip("lambert", &LambertProjector::new(lambert()), 1e-6);
}

#[test]
fn polar_stereographic_inverts_every_grid_point() {
    round_trip("cmc polar", &PolarStereoProjector::new(cmc_polar()), 1e-6);
}

/// The southern mirror of the CMC case: a south-pole grid whose domain reaches
/// north of the equator. Same defect by symmetry, so same test.
#[test]
fn a_south_polar_grid_reaching_north_inverts_every_grid_point() {
    let p = PolarStereoParams {
        south_pole: true,
        lat_first: -11.43,
        ..cmc_polar()
    };
    round_trip("south polar", &PolarStereoProjector::new(p), 1e-6);
}

#[test]
fn lambert_azimuthal_inverts_every_grid_point() {
    // Looser than the others by three orders of magnitude, and deliberately so:
    // `authalic_to_geodetic` is a truncated series, not an exact inverse, and
    // PROJ and eccodes carry the same asymmetry. This is the same error that
    // `SnapEps::Metres` exists for.
    round_trip("efas laea", &LambertAzimuthalProjector::new(efas()), 1e-3);
}

#[test]
fn transverse_mercator_inverts_every_grid_point() {
    round_trip("ukv tmerc", &TransverseMercatorProjector::new(ukv()), 1e-6);
}

/// What the hemisphere reject was actually defending against, checked directly
/// now that it is gone. The antipodal pole is the one point the projection
/// cannot place, and it does *not* forward-project to infinity in `f64` — so
/// the finiteness check never sees it, and the extent check is what refuses it.
#[test]
fn the_antipodal_pole_is_still_refused() {
    let north = PolarStereoProjector::new(cmc_polar());
    for lon in [-180.0, -110.27, 0.0, 247.0, 180.0] {
        assert!(
            north.inverse(-90.0, lon).is_none(),
            "north grid placed the south pole at lon {lon}"
        );
    }
    let south = PolarStereoProjector::new(PolarStereoParams {
        south_pole: true,
        lat_first: -11.43,
        ..cmc_polar()
    });
    for lon in [-180.0, 0.0, 180.0] {
        assert!(
            south.inverse(90.0, lon).is_none(),
            "south grid placed the north pole at lon {lon}"
        );
    }
}

/// The reject also has to stay a reject for points that are simply not on the
/// grid. `rho` grows without bound towards the antipodal pole, so a far-southern
/// point on a north grid is *further* out than any grid point — it must not
/// alias back into range now that latitude alone no longer refuses it.
#[test]
fn a_point_outside_the_grid_is_still_refused() {
    let p = PolarStereoProjector::new(cmc_polar());
    // Well south of the grid's own -4.718 floor, along its own meridians.
    for lat in [-10.0, -30.0, -60.0, -85.0] {
        for lon in [247.0, 282.3, -110.27] {
            assert!(
                p.inverse(lat, lon).is_none(),
                "a point at {lat},{lon} landed on a grid that stops at -4.718"
            );
        }
    }
}

/// The one geometry where the fix and `pole_inside_grid` meet: a grid centred
/// on the projection pole and wide enough to reach past the equator. Before
/// #488 its whole southern half was unplaceable while `pole_inside_grid`
/// simultaneously reported that it covered every meridian — the bbox said the
/// grid was there and the inverse said it was not.
#[test]
fn a_pole_centred_grid_reaching_past_the_equator_inverts_every_point() {
    // rho at the equator for lad = 60 is about 1.19e7 m, so 220 cells of 60 km
    // from the pole clears it.
    let n = 441;
    let half = (n as f64 - 1.0) / 2.0 * 60_000.0;
    let pole_centred = PolarStereoProjector::new(cmc_polar());
    let (lat0, lon0) = pole_centred.inverse_lonlat(-half, -half);
    let p = PolarStereoProjector::new(PolarStereoParams {
        ni: n,
        nj: n,
        lat_first: lat0,
        lon_first: lon0,
        ..cmc_polar()
    });
    assert!(
        p.pole_inside_grid(),
        "the grid was meant to contain the pole"
    );
    let (lat_min, _, _, _) = p.lonlat_bbox();
    assert!(lat_min < 0.0, "the grid was meant to cross the equator");
    round_trip("pole-centred polar", &p, 1e-6);
}
