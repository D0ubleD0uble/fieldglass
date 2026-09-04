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

/// The five families added in #464, each the real grid its round-trip test
/// uses (`grid_round_trip.rs`) rather than a convenient synthetic one — #488
/// hid in a synthetic grid that never left the northern hemisphere.
fn maritime_mercator() -> MercatorParams {
    MercatorParams {
        ni: 40,
        nj: 40,
        lat_first: -40.0,
        lon_first: 100.0,
        lat_last: 40.0,
        lon_last: 140.0,
    }
}

/// A COSMO-EU-shaped rotated grid, coarsened: the pole is moved to 40S 10E and
/// the grid is laid out around the new equator. Its corners are rotated-frame
/// degrees, so the region it actually covers — western and central Europe — is
/// nowhere near the numbers below.
fn cosmo_rotated() -> RotatedLatLonParams {
    RotatedLatLonParams {
        ni: 40,
        nj: 42,
        lat_first: -20.0,
        lon_first: -18.0,
        lat_last: 21.0,
        lon_last: 21.0,
        south_pole_lat: -40.0,
        south_pole_lon: 10.0,
        angle_of_rotation: 0.0,
    }
}

fn ukv_transverse_mercator() -> TransverseMercatorParams {
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

fn efas_lambert_azimuthal() -> LambertAzimuthalParams {
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

/// A GOES-16 ABI mesoscale window, well inside the limb so every grid point is
/// a place on Earth. The near-limb case is `grid_geometry_proj.rs`'s.
fn abi_mesoscale() -> GeostationaryParams {
    GeostationaryParams {
        ni: 100,
        nj: 100,
        h_metres: 42_164_160.0,
        r_eq: 6_378_137.0,
        r_pol: 6_356_752.314_14,
        sub_lon_deg: -75.0,
        sweep_x: true,
        x0: -0.028,
        dx_rad: 0.056 / 99.0,
        y0: 0.078,
        dy_rad: -0.056 / 99.0,
    }
}

/// Every variant, for the tests that must not quietly stop covering one.
fn every_modelled_family() -> Vec<GridGeometry> {
    vec![
        GridGeometry::LatLon(ecmwf_wrapping()),
        GridGeometry::Gaussian(n80_gaussian()),
        GridGeometry::Mercator(maritime_mercator()),
        GridGeometry::RotatedLatLon(cosmo_rotated()),
        GridGeometry::Lambert(eta_lambert()),
        GridGeometry::PolarStereo(cmc_polar()),
        GridGeometry::TransverseMercator(ukv_transverse_mercator()),
        GridGeometry::LambertAzimuthal(efas_lambert_azimuthal()),
        GridGeometry::Geostationary(abi_mesoscale()),
    ]
}

fn n80_gaussian() -> GaussianParams {
    GaussianParams {
        ni: 320,
        nj: 160,
        lat_first: 89.142,
        lon_first: 0.0,
        lat_last: -89.142,
        lon_last: 358.875,
        n_parallels: 80,
    }
}

#[test]
fn each_variant_reports_the_tag_the_hosts_already_use() {
    // These strings are load-bearing: #464 swaps napi's `grid_type` string
    // dispatch for this enum, and a rename here would be a host-visible break.
    let families = every_modelled_family();
    let tags: Vec<&str> = families.iter().map(|g| g.kind()).collect();
    assert_eq!(
        tags,
        [
            "latlon",
            "gaussian",
            "mercator",
            "rotated_latlon",
            "lambert",
            "polar_stereo",
            "transverse_mercator",
            "lambert_azimuthal",
            // The variant is named for the projection and the tag for the
            // template, because `space_view` is what both readers and the hosts
            // already print for a §3.90 message.
            "space_view",
        ],
    );
    assert_eq!(
        GridGeometry::Unsupported {
            label: "spherical_harmonic".into()
        }
        .kind(),
        "unsupported",
        "kind() is the serde tag, so an unmodelled family reports the tag",
    );
    assert_eq!(
        GridGeometry::Unsupported {
            label: "spherical_harmonic".into()
        }
        .label(),
        "spherical_harmonic",
        "label() is what says which grid was declined",
    );
}

#[test]
fn every_grid_point_round_trips_through_the_enum() {
    // The dispatch is what is under test, not the projections: if `forward`
    // and `inverse` reached different families the indices would not come back.
    for geom in every_modelled_family() {
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
        label: "spherical_harmonic".into(),
    };
    assert!((none.inverse_at())(45.0, 0.0).is_none());
}

#[test]
fn the_enum_survives_a_json_round_trip() {
    // ADR-0006 requires serde on every API type; a host derives its own DTO
    // from this one, so the tag has to be stable and the payload complete.
    for geom in every_modelled_family()
        .into_iter()
        .chain([GridGeometry::Unsupported {
            label: "spherical_harmonic".into(),
        }])
    {
        let json = serde_json::to_string(&geom).expect("serialises");
        assert!(
            json.contains(&format!("\"kind\":\"{}\"", geom.kind())),
            "the tag must be the same string `kind()` reports: {json}",
        );
        let back: GridGeometry = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, geom);
    }
}

#[test]
fn a_rotated_grid_reports_where_it_really_is_not_where_its_corners_say() {
    // The corners (-18..21E, -20..21N in the rotated frame) are not a place:
    // read as geographic they would put half the grid in the Sahara. The box
    // has to come from unrotating the perimeter, which puts it over Europe.
    let geom = GridGeometry::RotatedLatLon(cosmo_rotated());
    let (lat_min, lat_max, lon_min, lon_max) =
        geom.lonlat_bbox().expect("a rotated grid is placed");
    let (lat, lon) = geom.forward(0, 0).expect("first point");
    // The box runs east from `lon_min` and may pass 180 rather than wrapping —
    // the convention `lonlat_bbox` documents — so bring the point into that
    // turn before comparing.
    let lon = lon_min + (lon - lon_min).rem_euclid(360.0);
    assert!(
        (lat_min..=lat_max).contains(&lat) && (lon_min..=lon_max).contains(&lon),
        "the box ({lat_min}..{lat_max}, {lon_min}..{lon_max}) must contain its own \
         first point ({lat}, {lon})",
    );
    assert!(
        lat_min > 25.0 && lat_max < 75.0,
        "the box ({lat_min}..{lat_max}) is not over Europe, so the perimeter was \
         read as geographic instead of unrotated",
    );
}

/// The one modelled family with no CRS. It places its points perfectly well;
/// what it cannot yet do is name the frame they are laid out in, so it says so
/// rather than naming an unchecked one. See `GridGeometry::proj4`.
#[test]
fn a_rotated_grid_names_no_crs_but_still_places_its_points() {
    let geom = GridGeometry::RotatedLatLon(cosmo_rotated());
    assert_eq!(geom.proj4(), None);
    assert_eq!(geom.plane_affine(), None);
    let (lat, lon) = geom
        .forward(3, 4)
        .expect("a rotated grid places its points");
    let idx = geom.inverse(lat, lon).expect("and finds them again");
    assert!(
        (idx.i - 3.0).abs() < 1e-9 && (idx.j - 4.0).abs() < 1e-9,
        "{idx:?}"
    );
}

/// A message can declare a spheroid that is not one — GRIB2 §3 lets it — and
/// the Krüger and authalic series stay arithmetically finite when it does. So
/// the two spheroidal families check their own constants before answering;
/// without that gate a nonsense §3.12 would geolocate every point somewhere
/// plausible and wrong.
#[test]
fn a_spheroid_that_is_not_one_is_declined_rather_than_projected() {
    let broken = [
        GridGeometry::TransverseMercator(TransverseMercatorParams {
            semi_major_m: 1.0,
            semi_minor_m: -1.0,
            ..ukv_transverse_mercator()
        }),
        GridGeometry::LambertAzimuthal(LambertAzimuthalParams {
            semi_major_m: 0.0,
            semi_minor_m: 0.0,
            ..efas_lambert_azimuthal()
        }),
    ];
    for geom in broken {
        assert!(geom.forward(0, 0).is_none(), "{}: forward", geom.kind());
        assert!(geom.lonlat_bbox().is_none(), "{}: bbox", geom.kind());
        assert!(geom.plane_affine().is_none(), "{}: affine", geom.kind());
        // `dims` still answers: the raster shape is a fact about the message,
        // not about whether its projection resolves. So does `proj4` — the
        // plane belongs to the projection, and it is this grid that cannot be
        // put in it.
        assert!(geom.dims().is_some(), "{}: dims", geom.kind());
        assert!(geom.proj4().is_some(), "{}: proj4", geom.kind());
    }
}

/// An affine is a position in a plane, so there has to be a plane to name.
/// The converse does not hold and is not asserted: a grid whose projection
/// does not resolve still belongs to a projection, so `proj4` keeps naming it
/// while `plane_affine` declines — see the two tests below.
///
/// `grid_geometry_proj.rs` checks the numbers themselves against PROJ; what is
/// checked here is that the two cannot disagree about whether there is a plane.
#[test]
fn an_affine_is_never_reported_without_a_crs_to_measure_it_in() {
    for geom in every_modelled_family() {
        if geom.plane_affine().is_some() {
            assert!(
                geom.proj4().is_some(),
                "{}: an affine with no CRS is a number with no plane",
                geom.kind(),
            );
        }
    }
}

/// §3.10 states its corner latitudes, and nothing stops a message stating one
/// at a pole — where the Mercator ordinate diverges. The forward and inverse
/// maps have always refused such a grid; the affine has to refuse it too, or a
/// host receives an infinite origin and a NaN step, which JSON renders as
/// `null` and which place the raster nowhere.
#[test]
fn a_mercator_corner_at_a_pole_has_no_affine() {
    let geom = GridGeometry::Mercator(MercatorParams {
        lat_first: -90.0,
        ..maritime_mercator()
    });
    assert_eq!(geom.plane_affine(), None);
    assert_eq!(geom.forward(0, 0), None);
    assert_eq!(geom.inverse(0.0, 120.0), None);
    // The plane is still `+proj=merc`; it is this grid that cannot be put in
    // it, which is what the one-way implication above is about.
    assert!(geom.proj4().is_some());
}

#[test]
fn the_geographic_families_measure_their_affine_in_degrees() {
    let latlon = GridGeometry::LatLon(ecmwf_wrapping())
        .plane_affine()
        .expect("a lat/lon grid has an affine");
    assert_eq!(latlon.units, PlaneUnits::Degrees);
    assert_eq!(
        latlon.x0, 180.0,
        "the stated first point, not a normalised one"
    );
    assert_eq!(latlon.dx, Some(0.25));
    assert_eq!(latlon.dy, Some(-0.25), "the grid scans north to south");

    // Gaussian rows are Gauss-Legendre nodes: there is no constant spacing,
    // and a mean one would misplace every row but the middle.
    let gaussian = GridGeometry::Gaussian(n80_gaussian())
        .plane_affine()
        .expect("a Gaussian grid has an affine");
    assert_eq!(gaussian.units, PlaneUnits::Degrees);
    assert!(gaussian.dx.is_some());
    assert_eq!(gaussian.dy, None);
}

#[test]
fn a_family_with_no_plane_reports_no_affine() {
    assert_eq!(
        GridGeometry::Unsupported {
            label: "bifourier".into()
        }
        .plane_affine(),
        None,
    );
}
