//! The lookup seam, checked against brute force.
//!
//! A k-d tree is an optimisation of "compare against every cell", so the
//! reference is that comparison, run over grids chosen for the cases where a
//! lat/lon-space search would go wrong: the antimeridian, the poles, and a fold
//! where index-adjacent cells are far apart on the ground.

use fieldglass_core::projection::{GridGeometry, GridResampling};
use fieldglass_core::spatial_index::SpatialIndex;

/// Great-circle angle between two `(lat, lon)` pairs, in radians. The metric
/// the index claims to minimise.
fn central_angle(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let d =
        (lat1.sin() * lat2.sin() + lat1.cos() * lat2.cos() * (lon1 - lon2).cos()).clamp(-1.0, 1.0);
    d.acos()
}

/// The reference implementation: the nearest centre by great-circle distance,
/// found by looking at all of them.
fn brute_force(lats: &[f64], lons: &[f64], q: (f64, f64)) -> usize {
    (0..lats.len())
        .filter(|&k| lats[k].is_finite())
        .min_by(|&a, &b| {
            central_angle((lats[a], lons[a]), q)
                .partial_cmp(&central_angle((lats[b], lons[b]), q))
                .expect("angles are finite")
        })
        .expect("at least one finite centre")
}

/// A regular lat/lon lattice, but handed over as a bare list of centres so the
/// index cannot exploit its regularity.
fn lattice(ni: u32, nj: u32, lat: (f64, f64), lon: (f64, f64)) -> (Vec<f64>, Vec<f64>) {
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for j in 0..nj {
        for i in 0..ni {
            let fi = if ni > 1 {
                i as f64 / (ni - 1) as f64
            } else {
                0.0
            };
            let fj = if nj > 1 {
                j as f64 / (nj - 1) as f64
            } else {
                0.0
            };
            lats.push(lat.0 + fj * (lat.1 - lat.0));
            lons.push(lon.0 + fi * (lon.1 - lon.0));
        }
    }
    (lats, lons)
}

#[test]
fn the_tree_agrees_with_brute_force_everywhere() {
    let (ni, nj) = (40, 30);
    let (lats, lons) = lattice(ni, nj, (-60.0, 60.0), (-100.0, 100.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");

    // Query points deliberately off the lattice, so the answer is a real
    // choice rather than an exact hit.
    let mut checked = 0;
    for qj in 0..29 {
        for qi in 0..39 {
            let q = (
                -59.0 + qj as f64 * (118.0 / 28.0),
                -99.0 + qi as f64 * (198.0 / 38.0),
            );
            let got = ix.nearest(q.0, q.1).expect("inside the grid");
            let cell = got.j as usize * ni as usize + got.i as usize;
            let want = brute_force(&lats, &lons, q);
            // Compare by distance, not by index: two centres can tie, and
            // either is a correct answer.
            let d_got = central_angle((lats[cell], lons[cell]), q);
            let d_want = central_angle((lats[want], lons[want]), q);
            assert!(
                (d_got - d_want).abs() < 1e-12,
                "at {q:?}: tree gave cell {cell} at {d_got} rad, brute force {want} at {d_want}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 39 * 29);
}

/// The property the whole three-dimensional construction rests on: minimising
/// the chord minimises the great-circle angle, so a Euclidean tree gives the
/// geodesic answer. Asserted rather than taken on trust.
#[test]
fn nearest_by_chord_agrees_with_great_circle() {
    for k in 0..2000 {
        // A deterministic spread of angle pairs across the full range.
        let t1 = k as f64 * std::f64::consts::PI / 2000.0;
        let t2 = ((k * 7) % 2000) as f64 * std::f64::consts::PI / 2000.0;
        let (c1, c2) = (2.0 * (t1 / 2.0).sin(), 2.0 * (t2 / 2.0).sin());
        assert_eq!(
            t1.partial_cmp(&t2),
            c1.partial_cmp(&c2),
            "chord ordering diverged from angle ordering at {t1} vs {t2}"
        );
    }
}

#[test]
fn a_grid_straddling_the_antimeridian_needs_no_special_case() {
    // 170 degE to 170 degW. In lat/lon space these centres sit at both ends of
    // the number line; in three dimensions they are simply adjacent.
    let (ni, nj) = (21, 11);
    let (lats, lons) = lattice(ni, nj, (-5.0, 5.0), (170.0, 190.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");

    for q in [(0.0, 179.9), (0.0, -179.9), (0.0, 180.0), (2.5, -175.0)] {
        let got = ix.nearest(q.0, q.1).expect("inside the grid");
        let cell = got.j as usize * ni as usize + got.i as usize;
        let want = brute_force(&lats, &lons, q);
        let d_got = central_angle((lats[cell], lons[cell]), q);
        let d_want = central_angle((lats[want], lons[want]), q);
        assert!(
            (d_got - d_want).abs() < 1e-12,
            "at {q:?}: {d_got} vs {d_want}"
        );
    }
}

#[test]
fn a_grid_over_the_pole_needs_no_special_case() {
    // Every centre within 5 degrees of the north pole, where all meridians meet.
    let (ni, nj) = (36, 6);
    let (lats, lons) = lattice(ni, nj, (85.0, 90.0), (0.0, 350.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    for q in [(89.9, 0.0), (89.9, 180.0), (90.0, 0.0), (87.0, 271.0)] {
        let got = ix.nearest(q.0, q.1).expect("inside the grid");
        let cell = got.j as usize * ni as usize + got.i as usize;
        let want = brute_force(&lats, &lons, q);
        let d_got = central_angle((lats[cell], lons[cell]), q);
        let d_want = central_angle((lats[want], lons[want]), q);
        assert!(
            (d_got - d_want).abs() < 1e-12,
            "at {q:?}: {d_got} vs {d_want}"
        );
    }
}

/// The case that makes `NearestOnly` necessary rather than tidy: a fold where
/// `(i, j)` and `(i + 1, j)` are index-adjacent but on opposite sides of the
/// globe, as a tripolar ocean grid is along its seam.
#[test]
fn index_adjacent_cells_can_be_far_apart_on_the_ground() {
    // Two columns of a folded grid: column 0 near 0 degE, column 1 near 180 degE.
    let (ni, nj) = (2, 5);
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for j in 0..nj {
        lats.push(60.0 + j as f64);
        lons.push(0.0);
        lats.push(60.0 + j as f64);
        lons.push(180.0);
    }
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");

    // A point by column 0 must get column 0 — never something blended with the
    // cell on the far side of the fold.
    let got = ix.nearest(62.0, 0.5).expect("near column 0");
    assert_eq!((got.i, got.j), (0.0, 2.0));
    assert_eq!(
        ix.resampling(),
        GridResampling::NearestOnly,
        "a folded grid must never report Any"
    );
    // And the index is integral: there is no position-within-cell to report.
    assert_eq!(got.i.fract(), 0.0);
    assert_eq!(got.j.fract(), 0.0);
}

#[test]
fn a_point_well_outside_the_grid_is_refused() {
    // Without a cutoff every query returns some cell, and a regional grid
    // warped onto a world map paints the whole map with its edge cells.
    let (ni, nj) = (20, 20);
    let (lats, lons) = lattice(ni, nj, (40.0, 50.0), (-10.0, 10.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    assert!(ix.nearest(45.0, 0.0).is_some(), "the middle is inside");
    assert!(ix.nearest(40.0, -10.0).is_some(), "a corner is inside");
    assert!(
        ix.nearest(-45.0, 0.0).is_none(),
        "the far hemisphere is not"
    );
    assert!(
        ix.nearest(45.0, 120.0).is_none(),
        "nor is another continent"
    );
}

#[test]
fn a_cell_centre_the_file_left_missing_is_never_returned() {
    // NetCDF coordinate variables carry fill values. Such a cell has no
    // position, so it must not be the answer to any query — and the indices of
    // the cells around it must not shift.
    let (ni, nj) = (5, 5);
    let (mut lats, lons) = lattice(ni, nj, (0.0, 4.0), (0.0, 4.0));
    lats[12] = f64::NAN; // the centre cell, (2, 2)
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    assert_eq!(ix.len(), 24, "one cell is unsearchable");
    let got = ix.nearest(2.0, 2.0).expect("something is still nearby");
    assert_ne!(
        (got.i, got.j),
        (2.0, 2.0),
        "the missing cell was returned anyway"
    );
    // A cell past the hole still reports its own index.
    let far = ix.nearest(4.0, 4.0).expect("corner");
    assert_eq!((far.i, far.j), (4.0, 4.0));
}

#[test]
fn a_malformed_grid_is_refused_rather_than_half_built() {
    let (lats, lons) = lattice(4, 4, (0.0, 3.0), (0.0, 3.0));
    assert!(
        SpatialIndex::new(4, 4, &lats[..15], &lons).is_none(),
        "short lats"
    );
    assert!(
        SpatialIndex::new(0, 4, &[], &[]).is_none(),
        "zero dimension"
    );
    assert!(
        SpatialIndex::new(2, 2, &[f64::NAN; 4], &[f64::NAN; 4]).is_none(),
        "no finite centre at all"
    );
}

#[test]
fn the_index_survives_a_json_round_trip() {
    // ADR-0006 wants every API type serde-derivable. Only the centres cross the
    // wire; the tree is rebuilt, so a payload cannot claim one that disagrees
    // with its own points.
    let (ni, nj) = (12, 9);
    let (lats, lons) = lattice(ni, nj, (10.0, 30.0), (-20.0, 20.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    let json = serde_json::to_string(&ix).expect("serialises");
    assert!(
        !json.contains("tree"),
        "the derived tree must not be on the wire"
    );
    let back: SpatialIndex = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(back.dims(), (ni, nj));
    for q in [(15.0, -5.0), (28.0, 19.0), (11.0, -19.5)] {
        assert_eq!(back.nearest(q.0, q.1), ix.nearest(q.0, q.1), "at {q:?}");
    }
}

#[test]
fn a_lookup_geometry_reports_itself_correctly() {
    let (ni, nj) = (8, 8);
    let (lats, lons) = lattice(ni, nj, (0.0, 7.0), (0.0, 7.0));
    let g = GridGeometry::Lookup(SpatialIndex::new(ni, nj, &lats, &lons).expect("builds"));
    assert_eq!(g.kind(), "lookup");
    assert_eq!(g.dims(), Some((ni, nj)));
    assert_eq!(g.resampling(), GridResampling::NearestOnly);
    // The closure a warp actually uses reaches the index.
    let at = g.inverse_at();
    assert_eq!(at(3.0, 3.0), g.inverse(3.0, 3.0));
    assert!(at(3.0, 3.0).is_some());
    // Every formula family stays `Any`, so nothing was downgraded by accident.
    assert_eq!(
        GridGeometry::Unsupported { label: "x".into() }.resampling(),
        GridResampling::Any
    );
}

#[test]
fn the_fingerprint_distinguishes_grids_without_reading_them_all() {
    // The render cache asks "same geometry?" on every repaint, and `==` on a
    // million-cell index compares three million floats. `fingerprint` is what a
    // cache should key on, so it has to actually separate grids that differ.
    let (ni, nj) = (16, 12);
    let (lats, lons) = lattice(ni, nj, (0.0, 11.0), (0.0, 15.0));
    let a = SpatialIndex::new(ni, nj, &lats, &lons).expect("builds");
    let b = SpatialIndex::new(ni, nj, &lats, &lons).expect("builds");
    assert_eq!(
        a.fingerprint(),
        b.fingerprint(),
        "same centres, same fingerprint"
    );
    assert_eq!(a, b, "and `==` agrees");

    // One centre moved by a hair.
    let mut moved = lats.clone();
    moved[100] += 1e-9;
    let c = SpatialIndex::new(ni, nj, &moved, &lons).expect("builds");
    assert_ne!(
        a.fingerprint(),
        c.fingerprint(),
        "a moved centre must not match"
    );

    // Same centres, different grid shape: the dims are part of the hash, so a
    // reshaped grid is a different grid.
    let d = SpatialIndex::new(nj, ni, &lats, &lons).expect("builds");
    assert_ne!(a.fingerprint(), d.fingerprint(), "a reshape must not match");
}

#[test]
fn an_explicit_cutoff_overrides_the_measured_one() {
    // `with_max_distance` is for a caller that knows the grid's real cell size
    // better than its centres do — a swath whose along-track and across-track
    // spacing differ, where the measured 95th percentile is not the right
    // radius in every direction.
    let (ni, nj) = (10, 10);
    let (lats, lons) = lattice(ni, nj, (0.0, 9.0), (0.0, 9.0));
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("builds");
    // Roughly 111 km per degree on this sphere.
    const R: f64 = 6_371_229.0;

    // A point 2 degrees off the corner: inside a 400 km radius, outside a 50 km one.
    let q = (-2.0, 0.0);
    let generous = ix.clone().with_max_distance(400_000.0, R);
    let tight = ix.clone().with_max_distance(50_000.0, R);
    assert!(
        generous.nearest(q.0, q.1).is_some(),
        "400 km should reach it"
    );
    assert!(tight.nearest(q.0, q.1).is_none(), "50 km should not");

    // The override does not change which cell is nearest, only whether it is
    // close enough to count.
    assert_eq!(generous.nearest(4.4, 4.4), ix.nearest(4.4, 4.4));

    // A cutoff wider than the sphere accepts everything; one of zero accepts
    // only an exact hit.
    let all = ix.clone().with_max_distance(f64::MAX, R);
    assert!(
        all.nearest(-80.0, 170.0).is_some(),
        "clamped to the whole sphere"
    );
    let none = ix.with_max_distance(0.0, R);
    assert!(
        none.nearest(4.0, 4.0).is_some(),
        "an exact centre is at distance 0"
    );
    assert!(none.nearest(4.4, 4.4).is_none(), "anything else is too far");
}

/// A centre reads back as the position it was built from.
///
/// #445 settled the storage question by keeping only the unit vectors and
/// deriving degrees from them, so this is the round trip that claim rests on:
/// build from degrees, read back through `centre`, and land within 1e-9° —
/// several orders tighter than any consumer needs and far tighter than the
/// float32 the source files store.
#[test]
fn a_centre_reads_back_the_position_it_was_built_from() {
    let (lats, lons) = lattice(17, 13, (-80.0, 80.0), (-170.0, 170.0));
    let ix = SpatialIndex::new(17, 13, &lats, &lons).expect("index builds");
    for j in 0..13u32 {
        for i in 0..17u32 {
            let k = (j as usize) * 17 + i as usize;
            let (lat, lon) = ix.centre(i, j).expect("every centre is finite");
            assert!(
                (lat - lats[k]).abs() < 1e-9 && (lon - lons[k]).abs() < 1e-9,
                "({i},{j}): read ({lat}, {lon}), built from ({}, {})",
                lats[k],
                lons[k]
            );
        }
    }
    assert_eq!(ix.centre(17, 0), None, "off the east edge");
    assert_eq!(ix.centre(0, 13), None, "off the north edge");
}

/// The longitude comes back normalised, because a unit vector has no turn.
///
/// RTOFS writes its tripolar longitudes unwrapped, past 360°. Those are the
/// same positions on the globe, and `centre` says so — which is the documented
/// consequence of not storing the original degrees, and the thing a test
/// comparing against the file's raw numbers has to know.
#[test]
fn a_centre_normalises_a_longitude_the_file_wrote_unwrapped() {
    let lats = vec![10.0, 10.0];
    let lons = vec![370.0, 1019.0];
    let ix = SpatialIndex::new(2, 1, &lats, &lons).expect("index builds");
    let (_, first) = ix.centre(0, 0).expect("finite");
    let (_, second) = ix.centre(1, 0).expect("finite");
    assert!((first - 10.0).abs() < 1e-9, "370° is 10°, got {first}");
    // 1019 = 2·360 + 299; 299° normalises to -61°.
    assert!(
        (second - (-61.0)).abs() < 1e-9,
        "1019° is -61°, got {second}"
    );
}

/// A missing centre has no position, and never becomes one.
#[test]
fn a_centre_the_file_left_missing_reports_no_position() {
    let (mut lats, lons) = lattice(4, 3, (-10.0, 10.0), (-10.0, 10.0));
    lats[5] = f64::NAN;
    let ix = SpatialIndex::new(4, 3, &lats, &lons).expect("index builds");
    assert_eq!(ix.centre(1, 1), None, "cell 5 is (i=1, j=1)");
    assert!(ix.centre(0, 1).is_some(), "its neighbour is fine");
}

/// A regional grid's box is the box its cells occupy.
#[test]
fn the_bounding_box_is_the_extent_of_the_centres() {
    let (lats, lons) = lattice(20, 10, (-40.0, 40.0), (100.0, 140.0));
    let ix = SpatialIndex::new(20, 10, &lats, &lons).expect("index builds");
    let (lat_min, lat_max, lon_min, lon_max) = ix.lonlat_bbox().expect("a box");
    assert!((lat_min + 40.0).abs() < 1e-9, "{lat_min}");
    assert!((lat_max - 40.0).abs() < 1e-9, "{lat_max}");
    assert!((lon_min - 100.0).abs() < 1e-9, "{lon_min}");
    assert!((lon_max - 140.0).abs() < 1e-9, "{lon_max}");
}

/// A grid spanning the antimeridian reports the span, not the globe.
///
/// The naive box over normalised longitudes runs -180 to 180 and paints the
/// whole world for a grid occupying 40° of it. The enclosing arc reports a
/// `lon_min` below -180 instead, which is the convention the warp already reads
/// through periodic trig.
#[test]
fn a_box_across_the_antimeridian_keeps_its_span() {
    let (lats, lons) = lattice(20, 10, (-10.0, 10.0), (160.0, 200.0));
    let ix = SpatialIndex::new(20, 10, &lats, &lons).expect("index builds");
    let (_, _, lon_min, lon_max) = ix.lonlat_bbox().expect("a box");
    assert!(
        (lon_max - lon_min - 40.0).abs() < 1e-6,
        "the span should stay 40°, got {lon_min}..{lon_max}"
    );
    // `enclosing_lon_arc` recentres so the arc's midpoint lands in range, so
    // this grid comes back as -200..-160 rather than 160..200. Same arc, and
    // either way it crosses ±180, which is what the warp needs to see.
    assert!(
        lon_min < -180.0 || lon_max > 180.0,
        "the box should cross the antimeridian, got {lon_min}..{lon_max}"
    );
}

/// A grid surrounding a pole covers every meridian, so its box says so.
///
/// Near a pole the cells reach all longitudes, and the widest gap between
/// neighbouring centres is ordinary cell spacing rather than a real absence of
/// data. The arc that gap implies is arbitrary — it would leave a sliver of the
/// map unpainted at whichever meridian happened to have the largest gap — so
/// the box widens to the full circle. The same degeneracy the polar
/// stereographic bbox handles with `pole_inside_grid`.
#[test]
fn a_box_around_a_pole_widens_to_every_meridian() {
    // A polar cap: 36 meridians × 5 rings from 80°N to the pole.
    let (ni, nj) = (36u32, 5u32);
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for j in 0..nj {
        for i in 0..ni {
            lats.push(80.0 + 10.0 * (j as f64) / (nj as f64 - 1.0));
            lons.push(-180.0 + 360.0 * (i as f64) / (ni as f64));
        }
    }
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    let (lat_min, lat_max, lon_min, lon_max) = ix.lonlat_bbox().expect("a box");
    assert!((lat_min - 80.0).abs() < 1e-6, "{lat_min}");
    assert!((lat_max - 90.0).abs() < 1e-6, "{lat_max}");
    assert_eq!((lon_min, lon_max), (-180.0, 180.0), "every meridian");
}

/// `GridGeometry::Lookup` answers both questions #445 left open on it.
#[test]
fn the_lookup_geometry_forwards_and_bounds_like_its_index() {
    let (lats, lons) = lattice(8, 6, (-20.0, 20.0), (30.0, 70.0));
    let ix = SpatialIndex::new(8, 6, &lats, &lons).expect("index builds");
    let geometry = GridGeometry::Lookup(ix.clone());
    assert_eq!(geometry.forward(3, 2), ix.centre(3, 2));
    assert!(geometry.forward(3, 2).is_some(), "an interior cell");
    assert_eq!(geometry.lonlat_bbox(), ix.lonlat_bbox());
    assert!(geometry.lonlat_bbox().is_some(), "a placeable grid");
    assert_eq!(geometry.resampling(), GridResampling::NearestOnly);
}
