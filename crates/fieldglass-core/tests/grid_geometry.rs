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
    assert!(geom.bounds_lonlat().is_none());
    assert!(
        geom.proj4().is_none(),
        "a CRS we cannot name must be absent, never a plausible default",
    );
}

#[test]
fn a_global_grid_published_from_the_antimeridian_keeps_its_full_span() {
    let b = GridGeometry::LatLon(ecmwf_wrapping())
        .bounds_lonlat()
        .expect("lat/lon has bounds");
    // The failure this guards is the span collapsing to one 0.25 deg cell,
    // which is what min/max of the two stated corners produces.
    assert!((b.west - -180.0).abs() < 1e-9, "west was {}", b.west);
    assert!((b.east - 179.75).abs() < 1e-9, "east was {}", b.east);
    assert!((b.north - 90.0).abs() < 1e-9);
    assert!((b.south - -90.0).abs() < 1e-9);
    // It *starts* at the antimeridian rather than crossing it: 180 and -180
    // are the same meridian, so the box is the ordinary [-180, 179.75].
    assert!(
        !b.crosses_antimeridian(),
        "got west={} east={}",
        b.west,
        b.east
    );
}

#[test]
fn a_regional_grid_that_straddles_the_antimeridian_reports_the_wrap() {
    // A Pacific tile running 170 degE to 170 degW: 20 degrees wide, and the
    // only honest box has west numerically east of east.
    let b = GridGeometry::LatLon(LatLonParams {
        ni: 81,
        nj: 41,
        lat_first: 20.0,
        lon_first: 170.0,
        lat_last: 10.0,
        lon_last: -170.0,
    })
    .bounds_lonlat()
    .unwrap();
    assert!(
        b.crosses_antimeridian(),
        "got west={} east={}",
        b.west,
        b.east
    );
    assert!((b.west - 170.0).abs() < 1e-9, "west was {}", b.west);
    assert!((b.east - -170.0).abs() < 1e-9, "east was {}", b.east);
}

#[test]
fn an_ordinary_grid_does_not_claim_to_wrap() {
    let b = GridGeometry::LatLon(LatLonParams {
        ni: 100,
        nj: 50,
        lat_first: 60.0,
        lon_first: -20.0,
        lat_last: 30.0,
        lon_last: 40.0,
    })
    .bounds_lonlat()
    .unwrap();
    assert!(!b.crosses_antimeridian());
    assert!((b.west - -20.0).abs() < 1e-9 && (b.east - 40.0).abs() < 1e-9);
    assert!((b.south - 30.0).abs() < 1e-9 && (b.north - 60.0).abs() < 1e-9);
}

#[test]
fn a_projected_grid_crossing_the_equator_reports_the_half_below_it() {
    // The #488 grid: its far corner sits at 4.718 degS. A bounds that stopped
    // at the equator would be describing a grid the file does not contain.
    let b = GridGeometry::PolarStereo(cmc_polar())
        .bounds_lonlat()
        .expect("polar stereo has bounds");
    assert!(
        b.south < -4.0,
        "the southern edge reaches past the equator; got {}",
        b.south
    );
    // Measured: the grid reaches 51.937 degN. Pinned rather than guessed at,
    // so a change to the perimeter walk has to be looked at.
    assert!(
        (b.north - 51.937_317_605_591_59).abs() < 1e-9,
        "northern extent was {}",
        b.north
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
    let b = geom.bounds_lonlat().unwrap();
    assert!((b.west - -180.0).abs() < 1e-9 && (b.east - 180.0).abs() < 1e-9);
    assert!(
        (b.north - 90.0).abs() < 1e-9,
        "the pole is in the grid, so it is the northern bound; got {}",
        b.north
    );
}

#[test]
fn the_lambert_perimeter_bulges_past_its_corners() {
    // A conic's edges are curves. Taking the four corners would understate the
    // box, which is the reason `bounds_lonlat` walks the whole ring.
    let geom = GridGeometry::Lambert(eta_lambert());
    let b = geom.bounds_lonlat().unwrap();
    let (ni, nj) = geom.dims().unwrap();
    let corners = [(0, 0), (ni - 1, 0), (0, nj - 1), (ni - 1, nj - 1)];
    let corner_north = corners
        .iter()
        .map(|&(i, j)| geom.forward(i, j).unwrap().0)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        b.north > corner_north,
        "the top edge should reach north of every corner: bounds {} vs corners {}",
        b.north,
        corner_north
    );
    // And every grid point must actually be inside the reported box.
    for j in (0..nj).step_by(5) {
        for i in (0..ni).step_by(5) {
            let (lat, lon) = geom.forward(i, j).unwrap();
            assert!(
                lat >= b.south - 1e-9 && lat <= b.north + 1e-9,
                "({i}, {j}) at {lat} is outside {}..{}",
                b.south,
                b.north
            );
            let lon = normalise_lon(lon);
            assert!(
                lon >= b.west - 1e-9 && lon <= b.east + 1e-9,
                "({i}, {j}) at {lon} is outside {}..{}",
                b.west,
                b.east
            );
        }
    }
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
