//! `Session::contours_lonlat` (#1750): the isolines `contours` traces, each
//! vertex placed on the Earth, for a host that draws its own map.
//!
//! The oracle for a vertex's position is the session's own `probe`, which
//! inverts a latitude and longitude through the grid's projection to a
//! fractional grid index. A vertex placed right probes back to the grid index
//! the tracer put it at, and the field interpolated there is the level. A
//! vertex given in the wrong order, off by half a cell, or through the wrong
//! projection does not.
//!
//! Committed fixtures only, read by relative path so the wasm32-wasip1 run
//! (`wasmtime --dir=. --dir=..`) reads them too.

use fieldglass::{
    ContourOptions, DecodeOptions, Error, Field, Isoline, LonLatIsoline, RenderOptions, Session,
};

const G2: &str = "../fieldglass-grib2/tests/fixtures/";
const G1: &str = "../fieldglass-grib1/tests/fixtures/";

/// Open `path` and decode its first message.
fn decoded(path: &str) -> (Session, Field) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let session = Session::open(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
    let field = session
        .decode(0, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    (session, field)
}

/// The field at a fractional grid index, bilinear between the grid points
/// around it, and `None` if one it needs is masked. A contour vertex sits on a
/// cell edge, where this is linear along the edge, which is the interpolation
/// the tracer placed the vertex by; the points off that edge carry no weight
/// and are not read, since the cell across the edge may be masked. On a grid
/// whose tracer crosses the seam, an `i` past the last column is in the seam
/// cell, whose east side is column 0.
fn value_at(field: &Field, i: f64, j: f64, periodic: bool) -> Option<f64> {
    let (ni, nj) = (field.ni as usize, field.nj as usize);
    let (i0, i1) = if periodic && i > (ni - 1) as f64 {
        (ni - 1, 0)
    } else {
        let i0 = (i.floor().max(0.0) as usize).min(ni - 2);
        (i0, i0 + 1)
    };
    let j0 = (j.floor().max(0.0) as usize).min(nj - 2);
    let (fx, fy) = (i - i0 as f64, j - j0 as f64);
    let at = |i: usize, j: usize| {
        let k = j * ni + i;
        (field.mask[k] == 1).then(|| field.values.get(k)).flatten()
    };
    // A weight within float noise of zero (a probe's round trip) is zero.
    let none = |w: f64| w.abs() < 1e-9;
    let row = |j: usize| -> Option<f64> {
        let a = if none(1.0 - fx) { 0.0 } else { at(i0, j)? };
        let b = if none(fx) { 0.0 } else { at(i1, j)? };
        Some(a * (1.0 - fx) + b * fx)
    };
    let top = if none(1.0 - fy) { 0.0 } else { row(j0)? };
    let bot = if none(fy) { 0.0 } else { row(j0 + 1)? };
    Some(top * (1.0 - fy) + bot * fy)
}

/// What [`pair`] found.
#[derive(Debug, Default)]
struct Paired {
    /// Vertices probed back to their traced index.
    vertices: usize,
    /// Segments cut on the ±180° meridian into one piece per side.
    cuts: usize,
    /// The farthest a vertex probed back from its traced index, in cells.
    worst: f64,
}

/// Walk `contours` and `contours_lonlat` of the same field in step, and hold
/// every lon/lat vertex to the grid vertex it came from.
///
/// Each vertex probes back to the grid index the tracer gave it, within
/// `index_tol` cells (modulo the columns on a grid whose tracer crosses the
/// seam, where index `ni` is column `0`), and the field there is the level
/// within `value_tol` of the field's range. A grid segment whose far end does
/// not probe back from the first lon/lat segment's end was cut on the ±180°
/// meridian: that end must be on it, and the next lon/lat segment must start
/// on the other side at the same latitude and end where the grid segment does.
fn pair(
    name: &str,
    session: &Session,
    field: &Field,
    grid: &[Isoline],
    placed: &[LonLatIsoline],
    index_tol: f64,
    value_tol: f64,
) -> Paired {
    let ni = f64::from(field.ni);
    let periodic = field.georef.geometry.contour_seam_wraps();
    let nj = f64::from(field.nj);
    let span = field.stats.max.unwrap() - field.stats.min.unwrap();
    let mut found = Paired::default();
    // The inverse out to the outer cells' edges, for a vertex the tracer put
    // on the grid's outer row or column: placed linearly between two grid
    // points of a projected grid, it can land a hair outside the grid, where
    // `probe` answers nothing. Only those vertices ask it.
    let past_edge = field.georef.geometry.cell_inverse_at();
    // How far `(lon, lat)` probes back from grid index `(gi, gj)`, and where.
    let off = |lon: f64, lat: f64, gi: f64, gj: f64| -> (f64, f64, f64) {
        assert!(
            (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat),
            "{name}: ({lon}, {lat}) is a longitude then a latitude"
        );
        let on_edge = gi == 0.0 || gj == 0.0 || gi == ni - 1.0 || gj == nj - 1.0;
        let (pi, pj) = match session.probe(field, lat, lon) {
            Some(p) => (p.i, p.j),
            None if on_edge => past_edge(lat, lon)
                .map(|x| (x.i, x.j))
                .unwrap_or_else(|| panic!("{name}: edge vertex ({lon}, {lat}) is off the grid")),
            None => panic!("{name}: vertex ({lon}, {lat}) is off the grid"),
        };
        let mut di = (pi - gi).abs();
        if periodic {
            di = di.min((pi - gi + ni).abs()).min((pi - gi - ni).abs());
        }
        (di.max((pj - gj).abs()), pi, pj)
    };
    assert_eq!(grid.len(), placed.len(), "{name}: the same levels");
    for (g, p) in grid.iter().zip(placed) {
        assert_eq!(g.value, p.value, "{name}: the same level");
        let mut out = p.segments.iter();
        for gs in &g.segments {
            let first = out
                .next()
                .unwrap_or_else(|| panic!("{name}: level {} lost a segment", g.value));
            assert!(
                (first[2] - first[0]).abs() <= 180.0,
                "{name}: a segment across the whole map: {first:?}"
            );
            let mut ends = vec![(first[0], first[1], gs[0], gs[1])];
            // A cut piece ends on the meridian, and the next piece starts on
            // the other side of it at the same latitude and ends where this
            // grid segment does. A segment that only reaches the meridian
            // (GFS's column 72 is 180°) can be followed by the next grid
            // segment starting there, so the far end decides.
            let next = out.clone().next();
            let cut = first[2].abs() == 180.0
                && next.is_some_and(|second| {
                    (second[0], second[1]) == (-first[2], first[3])
                        && off(second[2], second[3], gs[2], gs[3]).0
                            < off(first[2], first[3], gs[2], gs[3]).0
                });
            if !cut {
                ends.push((first[2], first[3], gs[2], gs[3]));
            } else {
                let second = out.next().expect("peeked");
                assert!(
                    (second[2] - second[0]).abs() <= 180.0,
                    "{name}: a segment across the whole map: {second:?}"
                );
                ends.push((second[2], second[3], gs[2], gs[3]));
                found.cuts += 1;
            }
            for (lon, lat, gi, gj) in ends {
                let (d, pi, pj) = off(lon, lat, gi, gj);
                found.worst = found.worst.max(d);
                assert!(
                    d <= index_tol,
                    "{name}: vertex ({lon}, {lat}) probes back to ({pi}, {pj}), traced at \
                     ({gi}, {gj}): {d} cells away"
                );
                let (pi, pj) = (pi.clamp(0.0, ni), pj.clamp(0.0, nj - 1.0));
                let value = value_at(field, pi, pj, periodic).unwrap_or_else(|| {
                    panic!(
                        "{name}: no value at ({pi}, {pj}) for ({lon}, {lat}) traced at ({gi}, {gj})"
                    )
                });
                assert!(
                    (value - g.value).abs() <= value_tol * span,
                    "{name}: the field at vertex ({lon}, {lat}) is {value}, not the level {}",
                    g.value
                );
                found.vertices += 1;
            }
        }
        assert!(
            out.next().is_none(),
            "{name}: level {} has lon/lat segments no grid segment accounts for",
            g.value
        );
    }
    found
}

/// [`pair`] over the automatic levels of the first message of `path`.
fn placed_like_probe(path: &str, index_tol: f64, value_tol: f64) -> Paired {
    let (session, field) = decoded(path);
    assert!(field.georef.supports.contours, "{path} places contours");
    let options = ContourOptions::default();
    let grid = session.contours(&field, &[], &options).expect("contours");
    let placed = session
        .contours_lonlat(&field, &[], &options)
        .expect("contours_lonlat");
    let found = pair(path, &session, &field, &grid, &placed, index_tol, value_tol);
    assert!(found.vertices > 100, "{path}: only {found:?}");
    eprintln!("{path}: {found:?}");
    found
}

#[test]
fn a_lat_lon_grid_places_each_vertex_where_probe_reads_the_level() {
    // 16 × 31, 2° spacing. The forward map and the inverse are each other's
    // arithmetic, so the round trip closes to float noise.
    let found = placed_like_probe(&format!("{G2}regular_latlon_surface.grib2"), 1e-9, 1e-9);
    assert_eq!(found.cuts, 0);
}

#[test]
fn a_lambert_grid_places_each_vertex_where_probe_reads_the_level() {
    // NCEP's 32 km Eta grid. A vertex between two grid points is placed
    // linearly between their latitudes and longitudes, as the Node addon
    // places it before projecting, and the projection is not linear in them:
    // measured, the worst vertex probes back 1.1e-3 cells from where it was
    // traced, at the grid's northern edge. A half-cell shift or swapped axes
    // is hundreds of times past the tolerance.
    let found = placed_like_probe(&format!("{G2}eta_lambert_msg0.grib2"), 2e-3, 2e-3);
    assert_eq!(found.cuts, 0);
}

#[test]
fn a_polar_stereographic_grid_places_each_vertex_where_probe_reads_the_level() {
    // CMC's GRIB1 polar stereographic grid, a second projected family. As on
    // the Lambert grid, a vertex is placed linearly in latitude and longitude
    // between two grid points; measured, the worst probes back 7.1e-3 cells
    // from where it was traced.
    let found = placed_like_probe(
        &format!("{G1}cmc_wind_300_2010052400_p012.grib"),
        1e-2,
        1e-2,
    );
    assert_eq!(found.cuts, 0);
}

#[test]
fn a_global_grid_traces_its_seam_and_lines_run_on_across_180() {
    // GFS at 2.5°, columns 0 to 357.5: the grid's seam is at 0°, inside the
    // map, and the ±180° meridian is column 72, a cell edge, so no cell
    // straddles it and nothing is cut.
    let path = format!("{G2}gfs_c255_latlon.grib2");
    let (session, field) = decoded(&path);
    assert!(
        field.georef.geometry.contour_seam_wraps(),
        "global, west to east"
    );
    let options = ContourOptions::default();
    let grid = session.contours(&field, &[], &options).expect("contours");
    let placed = session
        .contours_lonlat(&field, &[], &options)
        .expect("contours_lonlat");
    let found = pair(&path, &session, &field, &grid, &placed, 1e-9, 1e-9);
    assert_eq!(found.cuts, 0, "{found:?}");

    // The seam cell is traced, as the addon's contours trace it, and its
    // segments come back just west of 0°. A line through it runs on into
    // column 0 with no gap.
    let ni = f64::from(field.ni);
    let seam_cell = grid
        .iter()
        .flat_map(|l| l.segments.iter())
        .filter(|s| s[0] > ni - 1.0 || s[2] > ni - 1.0)
        .count();
    assert!(seam_cell > 0, "a level crosses the seam cell");

    // A line crossing 180° comes back once per side: it ends on +180 in the
    // cell west of column 72 and goes on from -180 in the cell east of it, at
    // the same latitude. A vertex on column 72 takes its partner's side, so
    // no segment spans the map and none has zero length.
    let column = ni / 2.0;
    let (mut east, mut west) = (Vec::new(), Vec::new());
    for (g, p) in grid.iter().zip(&placed) {
        for (gs, ps) in g.segments.iter().zip(&p.segments) {
            for (here, there, lon, lat) in
                [(gs[0], gs[2], ps[0], ps[1]), (gs[2], gs[0], ps[2], ps[3])]
            {
                if here != column {
                    continue;
                }
                if there < column {
                    assert_eq!(lon, 180.0, "the west cell's end of 180° is +180");
                    east.push(lat.to_bits());
                } else if there > column {
                    assert_eq!(lon, -180.0, "the east cell's end of 180° is -180");
                    west.push(lat.to_bits());
                }
            }
        }
    }
    assert!(
        east.iter().any(|lat| west.contains(lat)),
        "a line ends on +180 and goes on from -180 at the same latitude"
    );
}

#[test]
fn a_global_grid_whose_seam_is_the_180_meridian_runs_on_across_it() {
    // ECMWF open data at 0.25°, columns from 180° east round to 179.75°: the
    // grid's own seam is the map's edge. The seam cell lies west of 180° and
    // the cell after column 0 east of it, so a line crossing there ends on
    // +180 in one cell and starts on -180 in the next, once per side.
    let path = format!("{G2}ecmwf_ccsds_latlon.grib2");
    let (session, field) = decoded(&path);
    assert!(
        field.georef.geometry.contour_seam_wraps(),
        "global, west to east"
    );
    let options = ContourOptions::default();
    let grid = session.contours(&field, &[], &options).expect("contours");
    let placed = session
        .contours_lonlat(&field, &[], &options)
        .expect("contours_lonlat");
    let found = pair(&path, &session, &field, &grid, &placed, 1e-9, 1e-9);
    assert_eq!(found.cuts, 0, "{found:?}");
    let ni = f64::from(field.ni);
    let (mut west, mut east) = (0, 0);
    for (g, p) in grid.iter().zip(&placed) {
        for (gs, ps) in g.segments.iter().zip(&p.segments) {
            for (i, lon) in [(gs[0], ps[0]), (gs[2], ps[2])] {
                if i > ni - 1.0 {
                    // The seam cell, `i` past the last column: index `ni` is
                    // column 0 again, 180°, on the west side here.
                    assert!(lon > 179.75 && lon <= 180.0, "seam cell at {lon}");
                    west += 1;
                } else if i > 0.0 && i < 1.0 {
                    assert!(lon > -180.0 && lon < -179.75, "first cell at {lon}");
                    east += 1;
                }
            }
        }
    }
    assert!(
        west > 0 && east > 0,
        "lines reach both sides ({west}, {east})"
    );
}

#[test]
fn a_grid_with_no_position_on_a_map_is_refused_as_the_addon_refuses_it() {
    // Dx = Dy = 0: the conformance suite's grid that places no point.
    let (session, field) = decoded(&format!("{G2}polar_stereographic_surface.grib2"));
    assert!(
        !field.georef.supports.contours,
        "the grid places no contour"
    );
    let options = ContourOptions::default();
    // The grid-index call has no position to give, so it still traces.
    session
        .contours(&field, &[], &options)
        .expect("grid-space contours need no placement");
    let Err(ours) = session.contours_lonlat(&field, &[], &options) else {
        panic!("a grid with no position on a map is refused");
    };
    let values: Vec<Option<f64>> = (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let Err(addon) = fieldglass::render::contour_polylines(
        &field.source(),
        &values,
        &RenderOptions::default(),
        None,
    ) else {
        panic!("the addon's contours refuse the grid too");
    };
    assert!(matches!(ours, Error::Unsupported { .. }), "{ours:?}");
    assert_eq!(ours, addon, "the same refusal, in the same words");

    // Asked before the range, the order the addon asks in, so a host hears
    // that the grid cannot be placed rather than that its range is wrong.
    let mut inverted = ContourOptions::default();
    inverted.min = Some(10.0);
    inverted.max = Some(1.0);
    let err = session
        .contours_lonlat(&field, &[], &inverted)
        .expect_err("refused");
    assert_eq!(err, addon);
}
