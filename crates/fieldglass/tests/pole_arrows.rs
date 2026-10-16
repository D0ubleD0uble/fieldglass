//! No arrow starts at a pole (#1045).
//!
//! A global grid's first and last rows are often the poles themselves: every
//! cell of the row is the same point. Its components are no single vector in
//! one frame. GRIB states them "relative to easterly and northerly directions"
//! (WMO Code table 3.3, bit 5), and at a pole there is no east or north, so a
//! writer such as NCEP's GFS gives each cell the one wind resolved along its
//! own meridian. The values turn with longitude while the wind does not. Each
//! sampled pole cell drew an arrow down its own meridian, which smeared a band
//! along the top and bottom of an Equirectangular map and drew a fan at the
//! pole of a polar view.
//!
//! The grid is a real one: `gfs_c255_latlon.grib2`, a 2.5° global GFS message
//! decoded through `Session`, whose rows 0 and 72 are lat 90 and −90. The
//! components are a field whose direction is known everywhere and which, like
//! a real one, blows across the pole: solid-body rotation about the axis
//! through (0°, 90°E). Resolved per cell, that is `u = −sin λ sin φ`,
//! `v = −cos λ`, so at either pole `(u, v)` sweeps a full turn along the row,
//! as GFS's does.

use fieldglass::render::{ARROW_VERTICES, VectorArrows, VectorOptions, vector_polylines};
use fieldglass::{DecodeOptions, Field, RenderOptions, Scan, Session, Source};
use fieldglass_core::{GridGeometry, RotatedLatLonParams};

/// Embedded rather than read, so the test runs under wasm32 too, where only
/// the crate's own directory is mapped in.
const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");

fn gfs() -> Field {
    let session = Session::open(GFS.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("the fixture decodes");
    // The precondition the test is about: a regular global grid whose first
    // and last rows are the poles.
    match &field.georef.geometry {
        GridGeometry::LatLon(p) => {
            assert_eq!((p.ni, p.nj), (144, 73));
            assert_eq!((p.lat_first, p.lat_last), (90.0, -90.0));
        }
        other => panic!("expected a regular lat/lon grid, got {other:?}"),
    }
    field
}

/// Solid-body rotation about the axis through (0°, 90°E), resolved east and
/// north at every cell of the 2.5° grid: `u = −sin λ sin φ`, `v = −cos λ`.
fn across_the_pole() -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let (mut u, mut v) = (Vec::new(), Vec::new());
    for j in 0..73 {
        let lat = (90.0 - 2.5 * f64::from(j)).to_radians();
        for i in 0..144 {
            let lon = (2.5 * f64::from(i)).to_radians();
            u.push(Some(-lon.sin() * lat.sin()));
            v.push(Some(-lon.cos()));
        }
    }
    (u, v)
}

fn options(projection: &str, center_lat: Option<f64>) -> RenderOptions {
    let mut o = RenderOptions::new(projection, "nearest");
    o.width = Some(720);
    o.height = Some(360);
    o.center_lat = center_lat;
    o
}

/// Where one geographic point lands, through the same pipeline the arrows go
/// through, rather than assumed from the raster's layout.
fn pixel(source: &Source<'_>, options: &RenderOptions, lat: f64, lon: f64) -> (f64, f64) {
    let p = fieldglass::render::overlay_polylines(source, options, &[lat, lon, lat, lon], &[2])
        .expect("the point projects");
    assert!(!p.xy.is_empty(), "({lat}, {lon}) is on the raster");
    (p.xy[0], p.xy[1])
}

/// The tail of every arrow that came back whole. A run split at the
/// antimeridian or the disc's edge is shorter and is not an arrow's start.
fn tails(arrows: &VectorArrows) -> Vec<(f64, f64)> {
    let p = &arrows.runs;
    let mut out = Vec::new();
    let mut at = 0usize;
    for &len in &p.seg_lengths {
        if len == ARROW_VERTICES {
            out.push((p.xy[at * 2], p.xy[at * 2 + 1]));
        }
        at += len as usize;
    }
    out
}

fn arrows(field: &Field, u: &[Option<f64>], v: &[Option<f64>], o: &RenderOptions) -> VectorArrows {
    let source = field.source();
    vector_polylines(&source, u, &source, v, o, &VectorOptions::new()).expect("arrows project")
}

/// Equirectangular: nothing along the top or bottom edge, and every row that
/// is not a pole keeps all of its arrows.
#[test]
fn no_arrow_runs_along_the_top_or_bottom_of_an_equirectangular_map() {
    let field = gfs();
    let (u, v) = across_the_pole();
    let o = options("equirectangular", None);
    let arrows = arrows(&field, &u, &v, &o);
    let source = field.source();

    // Latitude is linear in y here, so the two poles fix the whole map.
    let (_, top) = pixel(&source, &o, 90.0, 0.0);
    let (_, bottom) = pixel(&source, &o, -90.0, 0.0);
    let lat_at = |y: f64| 90.0 - 180.0 * (y - top) / (bottom - top);

    // No vertex of any arrow, whole or split, reaches either edge. The
    // nearest drawn row is lat ±75, and a full-length arrow, which is the mean
    // sampled spacing (about 10° of arc), can run poleward from it to ±85.
    for &[_, y] in arrows.runs.xy.as_chunks::<2>().0 {
        let lat = lat_at(y);
        assert!(
            lat.abs() < 88.0,
            "an arrow reaches lat {lat:.2}, at the map's edge (pixel y {y})"
        );
    }

    // Auto spacing on 144 × 73 samples every 6th cell: rows 0, 6, …, 72, the
    // first and last of which are the poles. The other 11 rows are lat 75 to
    // −75, and each keeps its arrows.
    let mut per_row = std::collections::BTreeMap::<i64, usize>::new();
    for (_, y) in tails(&arrows) {
        let lat = lat_at(y);
        let row = (lat / 15.0).round() as i64;
        assert!(
            (lat - 15.0 * row as f64).abs() < 0.5,
            "an arrow starts at lat {lat:.3}, not on a sampled row"
        );
        *per_row.entry(row).or_default() += 1;
    }
    assert!(
        !per_row.contains_key(&6) && !per_row.contains_key(&-6),
        "no arrow starts at a pole: {per_row:?}"
    );
    assert_eq!(
        per_row.keys().copied().collect::<Vec<_>>(),
        (-5..=5).collect::<Vec<_>>(),
        "every row between the poles is drawn"
    );
    for (row, n) in &per_row {
        // 24 columns are sampled; one whose arrow crosses lon 180 is split
        // and so not counted whole.
        assert!(
            (22..=24).contains(n),
            "row at lat {} has {n} whole arrows",
            15 * row
        );
    }
    // The scale is a drawn cell's: speed 1 along lon 0 and 180 at every lat.
    let speed = arrows.reference_speed.expect("arrows were drawn");
    assert!((speed - 1.0).abs() < 1e-9, "reference speed {speed}");
}

/// Orthographic centred on each pole: no arrow starts at the pole, so no fan.
#[test]
fn no_fan_of_arrows_at_either_pole_of_an_orthographic_globe() {
    let field = gfs();
    let (u, v) = across_the_pole();
    for pole_lat in [90.0, -90.0] {
        let o = options("orthographic", Some(pole_lat));
        let arrows = arrows(&field, &u, &v, &o);
        let source = field.source();
        let pole = pixel(&source, &o, pole_lat, 0.0);
        // How far from the pole, on screen, lat ±85 is: well inside the
        // nearest drawn row (±75) and well outside the pole itself.
        let near = pixel(&source, &o, pole_lat.signum() * 85.0, 0.0);
        let radius = (near.0 - pole.0).hypot(near.1 - pole.1);
        let tails = tails(&arrows);
        assert!(
            tails.len() > 100,
            "pole {pole_lat}: only {} arrows",
            tails.len()
        );
        for (x, y) in &tails {
            let r = (x - pole.0).hypot(y - pole.1);
            assert!(
                r > radius,
                "pole {pole_lat}: an arrow starts {r:.2} px from the pole (lat ±85 is {radius:.2} px)"
            );
        }
        // And no two arrows share a tail, which is what a fan is.
        let mut sorted = tails.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        assert!(
            sorted
                .windows(2)
                .all(|w| (w[0].0 - w[1].0).hypot(w[0].1 - w[1].1) > 0.5),
            "pole {pole_lat}: two arrows start at one point"
        );
    }
}

/// A rotated grid whose rows reach its own pole. That pole is not a
/// geographic one, but the row is still one point, and grid-relative
/// components there are measured along axes that turn with the column: the
/// same defect, which a latitude test alone would miss.
#[test]
fn no_arrow_starts_at_a_rotated_grid_s_own_pole() {
    let (ni, nj) = (72u32, 37u32);
    let geometry = GridGeometry::RotatedLatLon(RotatedLatLonParams {
        ni,
        nj,
        lat_first: 90.0,
        lon_first: 0.0,
        lat_last: -90.0,
        lon_last: 355.0,
        south_pole_lat: -40.0,
        south_pole_lon: 10.0,
        angle_of_rotation: 0.0,
    });
    let source = Source {
        geometry: Ok(&geometry),
        ni,
        nj,
        scan: Scan::north_down(),
        family: "rotated_ll",
        points_per_row: None,
    };
    // Along the grid's x everywhere, which at its pole turns with the column.
    let cells = (ni * nj) as usize;
    let (u, v) = (vec![Some(1.0); cells], vec![Some(0.0); cells]);
    let mut vectors = VectorOptions::new();
    vectors.grid_relative = true;
    let o = options("equirectangular", None);
    let arrows = vector_polylines(&source, &u, &source, &v, &o, &vectors).expect("arrows project");

    // The rotated north pole is the antipode of the south one, (40°, −170°);
    // the rotated south pole is (−40°, 10°).
    for (lat, lon) in [(40.0, -170.0), (-40.0, 10.0)] {
        let pole = pixel(&source, &o, lat, lon);
        for (x, y) in tails(&arrows) {
            assert!(
                (x - pole.0).hypot(y - pole.1) > 1.0,
                "an arrow starts at the grid's pole ({lat}, {lon})"
            );
        }
    }
    // Every other sampled row still draws (auto spacing 3 on 72 × 37 samples
    // rows 0, 3, …, 36; 0 and 36 are the poles).
    assert!(
        tails(&arrows).len() > 200,
        "only {} arrows",
        tails(&arrows).len()
    );
}
