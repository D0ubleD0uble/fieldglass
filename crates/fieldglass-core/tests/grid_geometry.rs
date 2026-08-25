//! `GridGeometry`'s own behaviour: dispatch, bounds, and the states it refuses.
//!
//! The projection arithmetic behind it is checked elsewhere — against PROJ in
//! `grid_geometry_proj.rs` and point-by-point in `grid_round_trip.rs`. What is
//! left, and what this file covers, is the lid: that each variant reaches its
//! own family's map, that a bounding box survives the two places a naive
//! min/max loses it (the antimeridian and a pole inside the domain), and that
//! an unmodelled family declines rather than guesses.

use fieldglass_core::projection::*;

/// ECMWF open data: 0.25° global, published starting at 180°E, so `lon_last`
/// (179.75) reads numerically *below* `lon_first`. The grid that
/// `eastward_lon_span` exists for.
fn ecmwf_wrapping() -> LatLonParams {
    LatLonParams {
        ni: 1440,
        nj: 721,
        lat_first: 90.0,
        lon_first: 180.0,
        lat_last: -90.0,
        lon_last: 179.75,
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

fn eta_lambert() -> LambertParams {
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

#[test]
fn each_variant_reports_the_tag_the_hosts_already_use() {
    // These strings are load-bearing: #464 swaps napi's `grid_type` string
    // dispatch for this enum, and a rename here would be a host-visible break.
    assert_eq!(GridGeometry::LatLon(ecmwf_wrapping()).kind(), "latlon");
    assert_eq!(GridGeometry::Lambert(eta_lambert()).kind(), "lambert");
    assert_eq!(
        GridGeometry::PolarStereo(cmc_polar()).kind(),
        "polar_stereo"
    );
    assert_eq!(
        GridGeometry::Unsupported {
            label: "space_view".into()
        }
        .kind(),
        "unsupported",
        "kind() is the serde tag, so an unmodelled family reports the tag",
    );
    assert_eq!(
        GridGeometry::Unsupported {
            label: "space_view".into()
        }
        .label(),
        "space_view",
        "label() is what says which grid was declined",
    );
}

#[test]
fn every_grid_point_round_trips_through_the_enum() {
    // The dispatch is what is under test, not the projections: if `forward`
    // and `inverse` reached different families the indices would not come back.
    for geom in [
        GridGeometry::LatLon(ecmwf_wrapping()),
        GridGeometry::Lambert(eta_lambert()),
        GridGeometry::PolarStereo(cmc_polar()),
        GridGeometry::Gaussian(GaussianParams {
            ni: 320,
            nj: 160,
            lat_first: 89.142,
            lon_first: 0.0,
            lat_last: -89.142,
            lon_last: 358.875,
            n_parallels: 80,
        }),
    ] {
        let (ni, nj) = geom.dims().expect("a modelled family has dimensions");
        let mut worst = 0.0f64;
        // Stride so the global grids stay quick; the exhaustive walk is
        // `grid_round_trip.rs`'s job.
        for j in (0..nj).step_by(7) {
            for i in (0..ni).step_by(7) {
                let (lat, lon) = geom
                    .forward(i, j)
                    .unwrap_or_else(|| panic!("{}: no position at ({i}, {j})", geom.kind()));
                let idx = geom
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("{}: ({i}, {j}) did not invert", geom.kind()));
                worst = worst
                    .max((idx.i - i as f64).abs())
                    .max((idx.j - j as f64).abs());
            }
        }
        assert!(
            worst < 1e-6,
            "{}: worst index error {worst:e} cells",
            geom.kind()
        );
    }
}

#[test]
fn an_index_off_the_grid_has_no_position() {
    let geom = GridGeometry::Lambert(eta_lambert());
    let (ni, nj) = geom.dims().unwrap();
    assert!(geom.forward(ni - 1, nj - 1).is_some());
    assert!(geom.forward(ni, nj - 1).is_none(), "i past the last column");
    assert!(geom.forward(ni - 1, nj).is_none(), "j past the last row");
}

#[test]
fn an_unsupported_family_declines_every_question_but_its_name() {
    let geom = GridGeometry::Unsupported {
        label: "reduced_gaussian".into(),
    };
    assert_eq!(geom.kind(), "unsupported");
    assert_eq!(geom.label(), "reduced_gaussian");
    assert!(geom.dims().is_none());
    assert!(geom.forward(0, 0).is_none());
    assert!(geom.inverse(0.0, 0.0).is_none());
    assert!(geom.lonlat_bbox().is_none());
    assert!(
        geom.proj4().is_none(),
        "a CRS we cannot name must be absent, never a plausible default",
    );
}

#[test]
fn a_global_grid_published_from_the_antimeridian_keeps_its_full_span() {
    let (lat_min, lat_max, lon_min, lon_max) = GridGeometry::LatLon(ecmwf_wrapping())
        .lonlat_bbox()
        .expect("lat/lon has a box");
    // The failure this guards is the span collapsing to one 0.25 deg cell,
    // which is what min/max of the two stated corners produces. `lon_max` runs
    // past 180 on purpose — see the method's own note.
    assert!((lon_min - 180.0).abs() < 1e-9, "lon_min was {lon_min}");
    assert!(
        ((lon_max - lon_min) - 359.75).abs() < 1e-9,
        "span was {}",
        lon_max - lon_min
    );
    assert!((lat_max - 90.0).abs() < 1e-9 && (lat_min - -90.0).abs() < 1e-9);
}

#[test]
fn a_regional_grid_that_straddles_the_antimeridian_keeps_a_continuous_span() {
    // A Pacific tile running 170 degE to 170 degW is 20 degrees wide. Reported
    // as 170..190 rather than -170..170, which would be the 340 degrees of
    // world the grid does *not* cover.
    let (_, _, lon_min, lon_max) = GridGeometry::LatLon(LatLonParams {
        ni: 81,
        nj: 41,
        lat_first: 20.0,
        lon_first: 170.0,
        lat_last: 10.0,
        lon_last: -170.0,
    })
    .lonlat_bbox()
    .unwrap();
    assert!((lon_min - 170.0).abs() < 1e-9, "lon_min was {lon_min}");
    assert!((lon_max - 190.0).abs() < 1e-9, "lon_max was {lon_max}");
}

#[test]
fn an_ordinary_grid_reports_its_corners_unchanged() {
    let (lat_min, lat_max, lon_min, lon_max) = GridGeometry::LatLon(LatLonParams {
        ni: 100,
        nj: 50,
        lat_first: 60.0,
        lon_first: -20.0,
        lat_last: 30.0,
        lon_last: 40.0,
    })
    .lonlat_bbox()
    .unwrap();
    // Nothing to unwrap here, so the box is exactly the stated corners and
    // stays inside [-180, 180].
    assert!((lon_min - -20.0).abs() < 1e-9 && (lon_max - 40.0).abs() < 1e-9);
    assert!((lat_min - 30.0).abs() < 1e-9 && (lat_max - 60.0).abs() < 1e-9);
}

#[test]
fn a_projected_grid_crossing_the_equator_reports_the_half_below_it() {
    // The #488 grid: its far corner sits at 4.718 degS. A bounds that stopped
    // at the equator would be describing a grid the file does not contain.
    let (lat_min, lat_max, ..) = GridGeometry::PolarStereo(cmc_polar())
        .lonlat_bbox()
        .expect("polar stereo has a box");
    // Measured, not guessed: -4.71787 degN is the corner #488 was about. The
    // edge walk subdivides, so it finds a hair further south than the corner
    // grid point itself does — which is the reason it subdivides.
    assert!(
        (lat_min - -4.717_874_643_538_869).abs() < 1e-9,
        "southern extent was {lat_min}"
    );
    assert!(
        (lat_max - 51.937_317_605_591_59).abs() < 1e-6,
        "northern extent was {lat_max}"
    );
}

#[test]
fn a_grid_containing_the_pole_covers_every_meridian() {
    // Meridians converge inside the domain, so no walk of the outer ring can
    // find the longitude range — every value is present. A hemispheric grid
    // centred on the pole is the case: 4000 km each way at 60 km spacing.
    let geom = GridGeometry::PolarStereo(PolarStereoParams {
        ni: 135,
        nj: 135,
        lat_first: 20.0,
        lon_first: 247.0 - 45.0,
        ..cmc_polar()
    });
    assert!(
        geom.inverse(90.0, 0.0).is_some(),
        "test grid must actually contain the pole for this to mean anything",
    );
    let (_, lat_max, lon_min, lon_max) = geom.lonlat_bbox().unwrap();
    assert!(
        (lon_min - -180.0).abs() < 1e-9 && (lon_max - 180.0).abs() < 1e-9,
        "every meridian is present; got {lon_min}..{lon_max}"
    );
    assert!(
        (lat_max - 90.0).abs() < 1e-9,
        "the pole is in the grid, so it is the northern bound; got {lat_max}"
    );
}

#[test]
fn the_lambert_perimeter_bulges_past_its_corners() {
    // A conic's edges are curves. Taking the four corners would understate the
    // box, which is the reason the projector's walk subdivides each edge.
    let geom = GridGeometry::Lambert(eta_lambert());
    let (lat_min, lat_max, ..) = geom.lonlat_bbox().unwrap();
    let (ni, nj) = geom.dims().unwrap();
    let corners = [(0, 0), (ni - 1, 0), (0, nj - 1), (ni - 1, nj - 1)];
    let corner_north = corners
        .iter()
        .map(|&(i, j)| geom.forward(i, j).unwrap().0)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        lat_max > corner_north,
        "the top edge should reach north of every corner: box {lat_max} vs corners {corner_north}"
    );
    // And every grid point must be inside the reported box.
    for j in (0..nj).step_by(5) {
        for i in (0..ni).step_by(5) {
            let (lat, _) = geom.forward(i, j).unwrap();
            assert!(
                lat >= lat_min - 1e-9 && lat <= lat_max + 1e-9,
                "({i}, {j}) at {lat} is outside {lat_min}..{lat_max}"
            );
        }
    }
}

#[test]
fn the_box_is_the_projectors_own_and_not_a_second_implementation() {
    // `PlanarGridProjector::lonlat_bbox` subdivides each edge 512 times, skips
    // perimeter samples that are not on the Earth, and returns the empty box
    // rather than infinities. Re-deriving any of that here would be a second
    // thing to keep right; this pins the delegation instead.
    assert_eq!(
        GridGeometry::Lambert(eta_lambert()).lonlat_bbox().unwrap(),
        LambertProjector::new(eta_lambert()).lonlat_bbox(),
    );
    let polar = cmc_polar();
    assert_eq!(
        GridGeometry::PolarStereo(polar).lonlat_bbox().unwrap(),
        PolarStereoProjector::new(polar).lonlat_bbox(),
        "a grid with the pole outside it passes the projector's answer straight through",
    );
}

#[test]
fn the_closure_and_the_one_shot_inverse_agree() {
    // `inverse` routes through `inverse_at`, and this is what keeps that true:
    // the closure is the one a warp uses, so a divergence would show up only
    // in rendered output.
    let geom = GridGeometry::Lambert(eta_lambert());
    let at = geom.inverse_at();
    let (ni, nj) = geom.dims().unwrap();
    for j in (0..nj).step_by(11) {
        for i in (0..ni).step_by(11) {
            let (lat, lon) = geom.forward(i, j).unwrap();
            assert_eq!(at(lat, lon), geom.inverse(lat, lon));
        }
    }
    // And a family with no map hands back a closure rather than making the
    // caller branch.
    let none = GridGeometry::Unsupported {
        label: "space_view".into(),
    };
    assert!((none.inverse_at())(45.0, 0.0).is_none());
}

#[test]
fn the_enum_survives_a_json_round_trip() {
    // ADR-0006 requires serde on every API type; a host derives its own DTO
    // from this one, so the tag has to be stable and the payload complete.
    for geom in [
        GridGeometry::LatLon(ecmwf_wrapping()),
        GridGeometry::Lambert(eta_lambert()),
        GridGeometry::PolarStereo(cmc_polar()),
        GridGeometry::Unsupported {
            label: "space_view".into(),
        },
    ] {
        let json = serde_json::to_string(&geom).expect("serialises");
        assert!(
            json.contains(&format!("\"kind\":\"{}\"", geom.kind())),
            "the tag must be the same string `kind()` reports: {json}",
        );
        let back: GridGeometry = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, geom);
    }
}
