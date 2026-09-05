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
use fieldglass_core::spatial_index::SpatialIndex;

/// Great-circle angle between two `(lat, lon)` pairs, in radians — the metric a
/// lookup grid's search minimises, and the one its round trip is measured in.
///
/// Haversine rather than the `acos` of a dot product, because this is used on
/// pairs that should be *identical*. There `cos d` is `1 - ~1e-16` and `acos`
/// returns about `1.5e-8` radians of pure rounding noise — 9 cm on the ground,
/// large enough to swamp any threshold worth setting. Haversine works from the
/// half-chord instead and reports zero for a point against itself.
fn central_angle(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lat2) = (a.0.to_radians(), b.0.to_radians());
    let (d_lat, d_lon) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * h.sqrt().clamp(0.0, 1.0).asin()
}

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
    let LonLatBox { lat_min, .. } = p.lonlat_bbox();
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

/// The raster an octahedral `O32` expands into, which is a grid family in its
/// own right as far as the warp is concerned (#503).
///
/// A reduced grid never reaches the projector as itself: its rows are widened
/// to `max(PL)` columns at the decode boundary, and it is that rectangle the
/// forward and inverse maps have to agree on. The parameters here are the ones
/// the render seam builds — 144 columns, and an east edge derived from that
/// width rather than the 357.1875 the file declares. Using the declared value
/// is the failure this pins: the forward map would place column 143 at
/// 357.1875 while the inverse divides the same span by 143, and the two would
/// still agree with each other, which is exactly why the round trip alone is
/// not the whole check — `decode_reduced_gaussian.rs` holds the east edge
/// against the row widths.
#[test]
fn an_expanded_octahedral_raster_inverts_every_grid_point() {
    let roots = gaussian_latitudes(32);
    let width = reduced_raster_width(&[20, 24, 144, 144, 24, 20]);
    assert_eq!(width, 144, "the widest row is the raster width");
    let p = GaussianParams {
        ni: width,
        nj: 64,
        lat_first: roots[0],
        lon_first: 0.0,
        lat_last: roots[roots.len() - 1],
        lon_last: reduced_raster_lon_last(0.0, width),
        n_parallels: 32,
    };
    assert_eq!(p.lon_last, 357.5, "not the 357.1875 the file declares");
    let g = GaussianProjector::new(p);
    let dropped = round_trip_fns(
        "expanded octahedral O32",
        p.ni,
        p.nj,
        |i, j| g.grid_point_lonlat(i, j),
        |lat, lon| g.inverse(lat, lon),
        1e-9,
    );
    assert_eq!(dropped, 0);
}

/// A lookup grid places its own points too, and this is the family that needs
/// the assertion phrased differently (#445).
///
/// The walk is the same one every formula family gets, but the answer cannot be
/// an index: where two cells share a centre the search may return either, and
/// demanding the original index would be demanding a tie-break the geometry
/// does not define. What must hold is that the cell it hands back is *at the
/// same place*, which is what a warp depends on.
///
/// The grid is a tripolar fold in miniature — two limbs of the same mesh, half
/// a world apart, whose columns are index-adjacent — plus a row of the northern
/// convergence where several cells crowd within a degree of the pole. Both are
/// shapes a k-d tree over lat/lon would get wrong and one over unit vectors
/// does not.
#[test]
fn a_lookup_grid_inverts_every_cell_to_its_own_position() {
    let (ni, nj) = (8u32, 6u32);
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for j in 0..nj {
        for i in 0..ni {
            // Columns alternate between two limbs 180° apart, so `(i, j)` and
            // `(i + 1, j)` are never neighbours on the ground.
            let limb = if i % 2 == 0 { 0.0 } else { 180.0 };
            let along = 20.0 * (i as f64 / 2.0).floor();
            lats.push(60.0 + 6.0 * j as f64);
            lons.push(limb + along);
        }
    }
    // The last row converges: every column within a degree of the pole.
    for i in 0..ni as usize {
        let k = (nj as usize - 1) * ni as usize + i;
        lats[k] = 89.5;
        lons[k] = 45.0 * i as f64;
    }

    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    let mut worst = 0.0f64;
    for j in 0..nj {
        for i in 0..ni {
            let (lat, lon) = ix.centre(i, j).expect("every centre is finite");
            let found = ix
                .nearest(lat, lon)
                .unwrap_or_else(|| panic!("({i},{j}) at ({lat}, {lon}) was not found"));
            let (flat, flon) = ix
                .centre(found.i as u32, found.j as u32)
                .expect("the cell it found has a position");
            let separation = central_angle((lat, lon), (flat, flon)).to_degrees();
            worst = worst.max(separation);
            assert!(
                separation < 1e-12,
                "({i},{j}) resolved to a cell {separation}° away"
            );
        }
    }
    assert!(worst < 1e-12, "worst separation {worst}°");
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

/// Geostationary was the one family that still failed, and it failed on
/// `master` too — a different projector from #488's, the same class of defect:
/// its inverse had no edge snap, so a grid point whose scan angle carried
/// round-off past the window bound was refused (#490, fixed). `scan_angles` is
/// a ray/ellipsoid intersection and does not return a bound exactly.
///
/// The windows below are the real ABI product windows rather than a synthetic
/// sector, because the defect is a property of the window bounds — a grid point
/// landing on one — and not of how many points lie between them. The point
/// count is scaled down and the step derived from it.
///
/// `forward` returning `None` means the grid point is not a place on Earth at
/// all, which is ordinary near the limb of a full-disc window; `round_trip_fns`
/// skips those. A *drop* is a point that geolocates and then fails to come
/// back, which is always a defect.
fn abi(ni: u32, nj: u32, x: (f64, f64), y: (f64, f64)) -> GeostationaryParams {
    GeostationaryParams {
        ni,
        nj,
        h_metres: 42_164_160.0,
        r_eq: 6_378_137.0,
        r_pol: 6_356_752.314_14,
        sub_lon_deg: -75.0,
        sweep_x: true,
        x0: x.0,
        dx_rad: (x.1 - x.0) / (ni as f64 - 1.0),
        // ABI rows run north to south, so the y step is negative.
        y0: y.0,
        dy_rad: (y.1 - y.0) / (nj as f64 - 1.0),
    }
}

fn geos_round_trip(name: &str, p: GeostationaryParams) -> u32 {
    let g = GeostationaryProjector::new(p);
    let dropped = round_trip_fns(
        name,
        p.ni,
        p.nj,
        |i, j| g.grid_point_lonlat(i, j),
        |lat, lon| g.inverse(lat, lon),
        1e-9,
    );
    assert_eq!(
        dropped, 0,
        "{name}: {dropped} points on the Earth were refused"
    );
    dropped
}

/// The ABI CONUS window. Every one of its 37,003 on-Earth points is on the
/// Earth, so nothing masks a dropped edge cell; before the snap, 414 were
/// refused, all of them on the first or last row or column.
#[test]
fn a_conus_sector_inverts_every_grid_point() {
    geos_round_trip(
        "goes conus",
        abi(250, 150, (-0.101_332, 0.038_612), (0.128_212, 0.044_268)),
    );
}

/// A mesoscale window north-east of nadir, well inside the limb. 204 refused
/// before the snap.
#[test]
fn a_mesoscale_sector_inverts_every_grid_point() {
    geos_round_trip("goes meso", abi(100, 100, (-0.028, 0.028), (0.078, 0.022)));
}

/// A full disc, where most of the outer ring legitimately looks past the limb
/// and is skipped rather than dropped. The assertion that matters here is that
/// the skip and the drop stay distinguishable.
#[test]
fn a_full_disc_window_inverts_every_point_that_is_on_the_earth() {
    let half = 0.151_844;
    let p = abi(271, 271, (-half, half), (half, -half));
    let g = GeostationaryProjector::new(p);
    let on_earth = (0..p.nj)
        .flat_map(|j| (0..p.ni).map(move |i| (i, j)))
        .filter(|&(i, j)| g.grid_point_lonlat(i, j).is_some())
        .count();
    assert!(
        on_earth < (p.ni * p.nj) as usize,
        "a full disc must have off-limb corners for this to test the skip path"
    );
    geos_round_trip("goes disc", p);
}

/// Meteosat sweeps about the other axis, which swaps the two scan-angle
/// rotations — a different arithmetic path to the same edge. 166 refused
/// before the snap.
#[test]
fn a_meteosat_sector_inverts_every_grid_point() {
    let p = GeostationaryParams {
        sub_lon_deg: 0.0,
        sweep_x: false,
        ..abi(120, 120, (-0.03, 0.03), (0.03, -0.03))
    };
    geos_round_trip("meteosat sector", p);
}

/// Only the border was ever affected, which is what made #490 an edge-snap gap
/// and nothing deeper: an interior point is nowhere near a bound, so round-off
/// cannot reach one. Kept as a standing assertion, since a future change that
/// started refusing interior points would be a different and worse bug.
#[test]
fn no_interior_point_is_ever_refused() {
    let p = abi(250, 150, (-0.101_332, 0.038_612), (0.128_212, 0.044_268));
    let g = GeostationaryProjector::new(p);
    for j in 1..p.nj - 1 {
        for i in 1..p.ni - 1 {
            let Some((lat, lon)) = g.grid_point_lonlat(i, j) else {
                continue;
            };
            assert!(
                g.inverse(lat, lon).is_some(),
                "interior point ({i}, {j}) at ({lat}, {lon}) was refused"
            );
        }
    }
}
