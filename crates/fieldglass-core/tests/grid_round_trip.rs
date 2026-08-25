//! Every grid point of every grid family must invert back to itself.
//!
//! `projection.rs` already had this test — `assert_round_trips`, at eight call
//! sites — and it did not catch #488. The shape was right; the *grids* were the
//! gap. Each call site builds a small synthetic grid chosen to be convenient,
//! and the polar stereographic one starts at 27.2 degN, entirely in the northern
//! hemisphere, so it could never have found a projector that refuses the
//! southern half of the sphere. A test's coverage is a property of its fixture,
//! and these fixtures had drifted from the grids the decoders are actually
//! validated against.
//!
//! So this file is not a new oracle. It is the existing one, run on the real
//! grids: the CMC regional 135x95 polar stereographic behind
//! `cmc_wind_300_2010052400_p012.grib`, the 93x65 Lambert, EFAS 20x16 Lambert
//! azimuthal, UKV 24x30 transverse Mercator, and global lat/lon, Gaussian,
//! Mercator, rotated and geostationary layouts.
//!
//! It needs no external oracle. `.eccodes.ref.json` pins where grid point
//! `(i, j)` sits, and the golden in `planar_inverse_golden.rs` pins whatever
//! `inverse` currently does; neither asks the question a warp actually asks —
//! given the lat/lon of a point the grid contains, does `inverse` hand back the
//! point? The grid's own forward map supplies the answer.

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
fn round_trip(name: &str, p: &dyn PlanarGridProjector, tol: f64) -> f64 {
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
    assert!(
        worst < tol,
        "{name}: worst index error {worst:e} cells, tolerance {tol:e}"
    );
    worst
}

#[test]
fn lambert_conformal_inverts_every_grid_point() {
    // Measured worst: 7.1e-14 cells.
    let _ = round_trip("lambert", &LambertProjector::new(lambert()), 1e-12);
}

#[test]
fn polar_stereographic_inverts_every_grid_point() {
    // Measured worst: 1.3e-13 cells.
    let _ = round_trip("cmc polar", &PolarStereoProjector::new(cmc_polar()), 1e-11);
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
    let _ = round_trip("south polar", &PolarStereoProjector::new(p), 1e-11);
}

#[test]
fn lambert_azimuthal_inverts_every_grid_point() {
    // Four orders looser than the others, and the number is the point.
    // `authalic_to_geodetic` is a truncated series, not an exact inverse, and
    // PROJ and eccodes carry the same asymmetry. Measured worst is 4.63e-9
    // cells, which on this grid's 200 km cell is 0.93 mm — the millimetre that
    // `SnapEps::Metres` is documented to exist for, arrived at independently.
    let _ = round_trip("efas laea", &LambertAzimuthalProjector::new(efas()), 1e-7);
}

#[test]
fn transverse_mercator_inverts_every_grid_point() {
    // Measured worst: 3.2e-13 cells.
    let _ = round_trip("ukv tmerc", &TransverseMercatorProjector::new(ukv()), 1e-11);
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
    let _ = round_trip("pole-centred polar", &p, 1e-11);
}

/// The same walk for the families that are not `PlanarGridProjector`
/// implementors: they expose a forward and an inverse but not the trait.
fn round_trip_fns(
    name: &str,
    ni: u32,
    nj: u32,
    forward: impl Fn(u32, u32) -> Option<(f64, f64)>,
    inverse: impl Fn(f64, f64) -> Option<GridIndex>,
    tol: f64,
) -> u32 {
    let mut dropped = 0u32;
    let mut worst = 0.0f64;
    let mut first_drop = None;
    for j in 0..nj {
        for i in 0..ni {
            // A forward that returns `None` is off-grid by construction (a
            // geostationary corner looking past the limb) and is not a drop.
            let Some((lat, lon)) = forward(i, j) else {
                continue;
            };
            match inverse(lat, lon) {
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
    if dropped == 0 {
        assert!(
            worst < tol,
            "{name}: worst index error {worst:e} cells, tolerance {tol:e}"
        );
    } else {
        println!("{name}: {dropped} dropped, first {first_drop:?}");
    }
    dropped
}

/// A global lat/lon grid, both hemispheres and the full seam.
#[test]
fn latlon_inverts_every_grid_point() {
    let p = LatLonParams {
        ni: 360,
        nj: 181,
        lat_first: 90.0,
        lon_first: 0.0,
        lat_last: -90.0,
        lon_last: 359.0,
    };
    let dropped = round_trip_fns(
        "latlon global",
        p.ni,
        p.nj,
        |i, j| latlon_point(&p, i, j),
        |lat, lon| latlon_inverse(&p, lat, lon),
        1e-12,
    );
    assert_eq!(dropped, 0);
}

/// A reduced-free N64 Gaussian, pole to pole. The declared first and last
/// latitudes have to be the true roots: the forward map returns the root, and a
/// declared bound that disagrees with it would put the grid's own first row
/// outside its own extent — a property of the parameters, not of the projector.
#[test]
fn gaussian_inverts_every_grid_point() {
    let roots = gaussian_latitudes(64);
    let p = GaussianParams {
        ni: 128,
        nj: 128,
        lat_first: roots[0],
        lon_first: 0.0,
        lat_last: roots[roots.len() - 1],
        lon_last: 357.1875,
        n_parallels: 64,
    };
    let g = GaussianProjector::new(p);
    let dropped = round_trip_fns(
        "gaussian N64",
        p.ni,
        p.nj,
        |i, j| g.grid_point_lonlat(i, j),
        |lat, lon| g.inverse(lat, lon),
        1e-9,
    );
    assert_eq!(dropped, 0);
}

#[test]
fn mercator_inverts_every_grid_point() {
    let p = MercatorParams {
        ni: 40,
        nj: 40,
        lat_first: -40.0,
        lon_first: 100.0,
        lat_last: 40.0,
        lon_last: 140.0,
    };
    let dropped = round_trip_fns(
        "mercator equator",
        p.ni,
        p.nj,
        |i, j| mercator_point(&p, i, j),
        |lat, lon| mercator_inverse(&p, lat, lon),
        1e-12,
    );
    assert_eq!(dropped, 0);
}

#[test]
fn rotated_latlon_inverts_every_grid_point() {
    let p = RotatedLatLonParams {
        ni: 16,
        nj: 31,
        lat_first: 60.0,
        lon_first: 0.0,
        lat_last: 0.0,
        lon_last: 30.0,
        south_pole_lat: -35.0,
        south_pole_lon: 15.0,
        angle_of_rotation: 0.0,
    };
    let r = RotatedLatLonProjector::new(p);
    let dropped = round_trip_fns(
        "rotated latlon",
        p.ni,
        p.nj,
        |i, j| rotated_latlon_point(&p, i, j),
        |lat, lon| r.inverse(lat, lon),
        1e-11,
    );
    assert_eq!(dropped, 0);
}

/// Geostationary is the one family that still fails, and it fails on `master`
/// too — a different projector from #488's, but the same class of defect: its
/// inverse has no edge snap, so a grid point whose scan angle carries round-off
/// past the window bound is refused. Every drop is on the first or last row or
/// column. Tracked as #490, and asserted as it currently behaves so that fixing
/// it fails here and forces this test to become an ordinary one.
#[test]
fn geostationary_drops_border_points_until_490() {
    let half = 0.10;
    let p = GeostationaryParams {
        ni: 11,
        nj: 11,
        h_metres: 42_164_160.0,
        r_eq: 6_378_137.0,
        r_pol: 6_356_752.314_14,
        sub_lon_deg: -75.0,
        sweep_x: true,
        x0: -half,
        dx_rad: 2.0 * half / 10.0,
        y0: -half,
        dy_rad: 2.0 * half / 10.0,
    };
    let g = GeostationaryProjector::new(p);
    let mut interior_dropped = 0;
    for j in 0..p.nj {
        for i in 0..p.ni {
            let Some((lat, lon)) = g.grid_point_lonlat(i, j) else {
                continue;
            };
            if g.inverse(lat, lon).is_none() && i != 0 && i != p.ni - 1 && j != 0 && j != p.nj - 1 {
                interior_dropped += 1;
            }
        }
    }
    // The part that must hold either way: only the border is ever affected, so
    // #490 is an edge-snap gap and nothing deeper.
    assert_eq!(interior_dropped, 0, "an interior point was refused");
    let dropped = round_trip_fns(
        "geostationary",
        p.ni,
        p.nj,
        |i, j| g.grid_point_lonlat(i, j),
        |lat, lon| g.inverse(lat, lon),
        1e-12,
    );
    assert!(
        dropped > 0,
        "#490 is fixed — turn this into an ordinary round-trip test"
    );
}
