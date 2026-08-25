#![cfg(feature = "render")]
//! Characterisation golden for the four planar inverses.
//!
//! #486 pulled `inverse` out of `LambertProjector`, `PolarStereoProjector`,
//! `LambertAzimuthalProjector` and `TransverseMercatorProjector` and made it a
//! provided `PlanarGridProjector` method, with what differed between them
//! moved into three hooks. Two of those differences were real and had drifted:
//! polar stereographic rejects the wrong hemisphere, and Lambert azimuthal
//! snaps grid edges in metres where the others snap in cell fractions.
//!
//! These hashes were captured from the four inherent bodies *before* the
//! extraction, so they pin it against a recording of the old code rather than
//! against the new code's own output. Each folds `inverse` over a 121x121
//! global lat/lon lattice, the grid's own four corners, a ring around each
//! corner at the scale of the edge snap, and the non-finite inputs — hashing
//! the raw `f64` bits of `i` and `j`, so "bit-identical" is literal.
//!
//! A failure is a behaviour change, not a test to re-baseline.
use fieldglass_core::projection::*;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}
fn mix(h: &mut u64, r: Option<GridIndex>) {
    match r {
        None => fnv(h, &[0xff]),
        Some(g) => {
            fnv(h, &[0x01]);
            fnv(h, &g.i.to_bits().to_le_bytes());
            fnv(h, &g.j.to_bits().to_le_bytes());
        }
    }
}

/// A dense lattice plus the pathological inputs, in a fixed order.
fn probes(corners: &[(f64, f64); 4]) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    // 121 x 121 over the whole globe: inside, outside, and across every edge.
    for a in 0..=120 {
        for b in 0..=120 {
            v.push((-90.0 + a as f64 * 1.5, -180.0 + b as f64 * 3.0));
        }
    }
    // The corners themselves — the edge-snap cases this refactor must not move.
    v.extend_from_slice(corners);
    // And a ring immediately around each corner, at the scale of the snap.
    for &(lat, lon) in corners {
        for d in [-1e-9, -1e-12, 0.0, 1e-12, 1e-9, -1e-6, 1e-6] {
            v.push((lat + d, lon));
            v.push((lat, lon + d));
        }
    }
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        v.push((bad, 0.0));
        v.push((0.0, bad));
        v.push((bad, bad));
    }
    v
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

/// The hash a projector that rejects every input produces. Named because half
/// the table is degenerate parameter sets that should reach it — and because a
/// live projector silently collapsing to "no point is on this grid" would
/// otherwise be an ordinary-looking hash change.
const ALL_REJECTED: u64 = 0xf69bbbe95fd1b93f;

fn check(name: &str, p: &dyn PlanarGridProjector, want: u64) {
    let mut h: u64 = 0xcbf29ce484222325;
    let corners = p.grid_corners_lonlat();
    for (lat, lon) in probes(&corners) {
        mix(&mut h, p.inverse(lat, lon));
    }
    assert_eq!(
        h, want,
        "{name} inverts differently than the pre-#486 recording"
    );
}

#[test]
fn the_planar_inverses_match_their_pre_extraction_recording() {
    check(
        "lambert",
        &LambertProjector::new(lambert()),
        0x87ce7e8b13c2aa2a,
    );
    check(
        "lambert_degenerate_parallels",
        &LambertProjector::new(LambertParams {
            latin1: 25.0,
            latin2: -25.0,
            ..lambert()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "lambert_ni1",
        &LambertProjector::new(LambertParams { ni: 1, ..lambert() }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "lambert_dx0",
        &LambertProjector::new(LambertParams {
            dx_metres: 0.0,
            ..lambert()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "lambert_negdy",
        &LambertProjector::new(LambertParams {
            dy_metres: -81_271.0,
            ..lambert()
        }),
        0xe33d1c8c0e101c0c,
    );
    check(
        "polar_north",
        &PolarStereoProjector::new(cmc_polar()),
        0xc5f27eda623b4ee7,
    );
    check(
        "polar_south",
        &PolarStereoProjector::new(PolarStereoParams {
            south_pole: true,
            lat_first: -11.43,
            ..cmc_polar()
        }),
        0xd495da8d33b4bf50,
    );
    check(
        "polar_nj1",
        &PolarStereoProjector::new(PolarStereoParams {
            nj: 1,
            ..cmc_polar()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "polar_dy0",
        &PolarStereoProjector::new(PolarStereoParams {
            dy_metres: 0.0,
            ..cmc_polar()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "laea_efas",
        &LambertAzimuthalProjector::new(efas()),
        0x38c9da30a7138a4a,
    );
    check(
        "laea_ni1",
        &LambertAzimuthalProjector::new(LambertAzimuthalParams { ni: 1, ..efas() }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "laea_dx0",
        &LambertAzimuthalProjector::new(LambertAzimuthalParams {
            dx_metres: 0.0,
            ..efas()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "laea_negdy",
        &LambertAzimuthalProjector::new(LambertAzimuthalParams {
            dy_metres: -200_000.0,
            ..efas()
        }),
        0x3a68ac1f214a5f4e,
    );
    check(
        "laea_south_tangent",
        &LambertAzimuthalProjector::new(LambertAzimuthalParams {
            standard_parallel: -90.0,
            lat_first: -60.0,
            ..efas()
        }),
        0x7e17e890478be1d1,
    );
    check(
        "tmerc_ukv",
        &TransverseMercatorProjector::new(ukv()),
        0xd1782da097605475,
    );
    check(
        "tmerc_sf0",
        &TransverseMercatorProjector::new(TransverseMercatorParams {
            scale_factor: 0.0,
            ..ukv()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "tmerc_sfnan",
        &TransverseMercatorProjector::new(TransverseMercatorParams {
            scale_factor: f64::NAN,
            ..ukv()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "tmerc_nj1",
        &TransverseMercatorProjector::new(TransverseMercatorParams { nj: 1, ..ukv() }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "tmerc_dx0",
        &TransverseMercatorProjector::new(TransverseMercatorParams {
            dx_metres: 0.0,
            ..ukv()
        }),
        0xf69bbbe95fd1b93f,
    );
    check(
        "tmerc_posdy",
        &TransverseMercatorProjector::new(TransverseMercatorParams {
            dy_metres: 48_000.0,
            ..ukv()
        }),
        0x49406a239fba2285,
    );
}

/// Every degenerate parameter set must actually reach the reject path, and no
/// live one may. Without this the table above would still pass if a real
/// projector started refusing everything, since a hash is a hash.
#[test]
fn the_degenerate_grids_reject_and_the_live_ones_do_not() {
    let live: [&dyn PlanarGridProjector; 8] = [
        &LambertProjector::new(lambert()),
        &LambertProjector::new(LambertParams {
            dy_metres: -81_271.0,
            ..lambert()
        }),
        &PolarStereoProjector::new(cmc_polar()),
        &PolarStereoProjector::new(PolarStereoParams {
            south_pole: true,
            lat_first: -11.43,
            ..cmc_polar()
        }),
        &LambertAzimuthalProjector::new(efas()),
        &LambertAzimuthalProjector::new(LambertAzimuthalParams {
            standard_parallel: -90.0,
            lat_first: -60.0,
            ..efas()
        }),
        &TransverseMercatorProjector::new(ukv()),
        &TransverseMercatorProjector::new(TransverseMercatorParams {
            dy_metres: 48_000.0,
            ..ukv()
        }),
    ];
    for p in live {
        let mut h: u64 = 0xcbf29ce484222325;
        for (lat, lon) in probes(&p.grid_corners_lonlat()) {
            mix(&mut h, p.inverse(lat, lon));
        }
        assert_ne!(h, ALL_REJECTED, "a live grid placed no point at all");
    }
}

/// The point of the extraction: `inverse` is now the trait's, and every
/// projector's own `inverse` is the same call. A forwarder that stopped
/// forwarding would pass every hash above only if it happened to reproduce the
/// body exactly, which is the drift this checks for directly.
#[test]
fn the_inherent_inverse_is_the_trait_inverse() {
    macro_rules! same {
        ($p:expr) => {{
            let p = $p;
            for (lat, lon) in probes(&p.grid_corners_lonlat()) {
                assert_eq!(
                    p.inverse(lat, lon),
                    PlanarGridProjector::inverse(&p, lat, lon),
                    "{lat},{lon}"
                );
            }
        }};
    }
    same!(LambertProjector::new(lambert()));
    same!(PolarStereoProjector::new(cmc_polar()));
    same!(LambertAzimuthalProjector::new(efas()));
    same!(TransverseMercatorProjector::new(ukv()));
}

/// The edge snap exists so the outermost row and column are not dropped to
/// background. It is the one hook whose *unit* differs between projectors, so
/// it gets a direct assertion rather than only a hash: every grid must place
/// its own first and last points.
#[test]
fn every_grid_places_its_own_corners() {
    macro_rules! corners {
        ($name:expr, $p:expr) => {{
            let p = $p;
            let (ni, nj) = p.grid_dims();
            for (lat, lon) in p.grid_corners_lonlat() {
                let g = p
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("{} dropped its own corner {lat},{lon}", $name));
                assert!(
                    g.i >= 0.0 && g.i <= ni as f64 - 1.0 && g.j >= 0.0 && g.j <= nj as f64 - 1.0,
                    "{} corner {lat},{lon} landed at {},{}",
                    $name,
                    g.i,
                    g.j
                );
            }
        }};
    }
    corners!("lambert", LambertProjector::new(lambert()));
    // The reason `SnapEps::Metres` exists: a nanometre of a 200 km cell is
    // below this projection's own series error, and the outer row was dropped.
    corners!("laea", LambertAzimuthalProjector::new(efas()));
    corners!("tmerc", TransverseMercatorProjector::new(ukv()));
}

/// The polar case is held out of the test above because it fails, and it fails
/// on `master` too: this extraction is bit-identical and did not cause it. The
/// CMC 135x95 grid behind the GRIB1 fixtures reaches -4.718 degN, and
/// `PolarStereoProjector::accepts` refuses the whole opposite hemisphere to
/// guard against a singularity that is only at the antipodal pole — the forward
/// map is finite all the way to -89.999. 328 of its 12,825 points, 2.6%, are
/// unreachable by the warp and render as background.
///
/// Asserted as it currently behaves rather than deleted, so fixing #488 fails
/// this test and forces it to be moved up into `every_grid_places_its_own_corners`.
#[test]
fn a_polar_grid_across_the_equator_drops_its_own_corner_until_488() {
    let p = PolarStereoProjector::new(cmc_polar());
    let corners = p.grid_corners_lonlat();
    let southern = corners
        .iter()
        .find(|(lat, _)| *lat < 0.0)
        .expect("the CMC grid has a corner south of the equator");
    assert!(
        p.inverse(southern.0, southern.1).is_none(),
        "#488 is fixed — move the polar case back into \
         every_grid_places_its_own_corners and delete this test"
    );
}

/// `SnapEps::Metres` converts through the spacing, so a negative `dy` — the
/// scan direction, not a smaller cell — must not flip the tolerance negative
/// and turn the snap into a rejection.
#[test]
fn a_negative_spacing_does_not_invert_the_snap() {
    let p = LambertAzimuthalProjector::new(LambertAzimuthalParams {
        dy_metres: -200_000.0,
        lat_first: 60.0,
        ..efas()
    });
    for (lat, lon) in p.grid_corners_lonlat() {
        assert!(
            p.inverse(lat, lon).is_some(),
            "negative dy dropped corner {lat},{lon}"
        );
    }
}
