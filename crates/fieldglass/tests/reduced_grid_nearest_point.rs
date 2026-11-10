//! On a reduced grid, a pixel shows the grid point nearest it (#1759).
//!
//! A reduced Gaussian grid's rows are widened to its widest row before it is
//! drawn, and the widened raster is placed as its regular sibling. Rounding a
//! pixel to the nearest column of that raster, and the column to the point
//! nearest the column's center, is two roundings, and on a coarse grid drawn
//! finer than its widest row they disagree with the nearest point. The rows
//! were also chosen by latitude before any point was measured. The probe-parity
//! sweep found both against eccodes' `codes_grib_find_nearest` (#1754).
//!
//! The oracle here is the grid's own definition, not a second copy of the
//! search: point `q` of row `j` lies at the row's latitude and `λ₀ + q·360/PL[j]`
//! (§3 interpretation code 1, full circles), the nearest point is found by
//! measuring every point of the grid, and the point a cell shows is the one
//! `expand_reduced_to_regular` documents for it. The cell under a pixel comes
//! from the real readers and the real probe and render entry points.

#![cfg(all(feature = "grib1", feature = "grib2", feature = "render"))]

use std::sync::Arc;

use fieldglass::api::Field;
use fieldglass::render::{PixelProbe, probe_pixel, project};
use fieldglass::{DecodeOptions, RenderOptions, Session, WarpMemo, WarpOptions};

const N32: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1");
const O32: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2");
/// The N32 sample as a constant field: zero bits a value.
const N32_CONSTANT: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1");
const LATLON_10DEG: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/reduced_latlon_10deg.grib2");

fn decode(bytes: &[u8]) -> Field {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    session
        .decode(0, &DecodeOptions::default())
        .expect("the message decodes")
}

/// A field's values as the render reads them.
fn values(field: &Field) -> Vec<Option<f64>> {
    (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect()
}

fn options(resampling: &str) -> RenderOptions {
    RenderOptions::new("equirectangular", resampling)
}

/// The grid as its definition places it: each row's latitude and length, and
/// the longitude of its first point.
struct Rows {
    lat: Vec<f64>,
    len: Vec<u32>,
    lon_first: f64,
    width: u32,
}

impl Rows {
    fn of(field: &Field) -> Self {
        let pl = field
            .georef
            .points_per_row
            .clone()
            .expect("a reduced grid carries its rows");
        let geometry = &field.georef.geometry;
        let lat = (0..pl.len() as u32)
            .map(|j| geometry.forward(0, j).expect("row placed").0)
            .collect();
        let lon_first = geometry.forward(0, 0).expect("first point").1;
        let width = *pl.iter().max().expect("rows");
        assert_eq!(field.ni, width, "the raster is as wide as the widest row");
        Self {
            lat,
            len: pl,
            lon_first,
            width,
        }
    }

    /// The point column `i` of widened row `j` shows, as
    /// `expand_reduced_to_regular` documents it.
    fn shown(&self, i: u32, j: u32) -> u32 {
        let (w, n) = (u64::from(self.width), u64::from(self.len[j as usize]));
        ((u64::from(i) * n + w / 2) / w % n) as u32
    }

    fn position(&self, j: u32, q: u32) -> (f64, f64) {
        let n = f64::from(self.len[j as usize]);
        (
            self.lat[j as usize],
            self.lon_first + f64::from(q) * 360.0 / n,
        )
    }

    /// Every point of the grid as a unit vector, for a search that measures
    /// all of them.
    fn points(&self) -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        for j in 0..self.len.len() as u32 {
            for q in 0..self.len[j as usize] {
                let (la, lo) = self.position(j, q);
                out.push(unit(la, lo));
            }
        }
        out
    }
}

fn unit(lat: f64, lon: f64) -> [f64; 3] {
    let (p, l) = (lat.to_radians(), lon.to_radians());
    [p.cos() * l.cos(), p.cos() * l.sin(), p.sin()]
}

/// The chord between two unit vectors, squared: it orders points as the
/// great-circle distance does.
fn chord2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The probe of every `stride`-th pixel of the equirectangular render, with
/// the pixel it was asked at, through the free function, which resolves each
/// pixel through the inverse map.
fn probes(field: &Field, resampling: &str, stride: usize) -> Vec<((u32, u32), PixelProbe)> {
    let values = values(field);
    let source = field.source();
    let o = options(resampling);
    let shown = project(&source, &values, &o).expect("the field renders");
    let mut out = Vec::new();
    for k in (0..(shown.width * shown.height) as usize).step_by(stride) {
        let (px, py) = (k as u32 % shown.width, k as u32 / shown.width);
        let p = probe_pixel(&source, &values, &o, px, py)
            .expect("the probe runs")
            .expect("the pixel is on the raster");
        out.push(((px, py), p));
    }
    out
}

/// The probe of every pixel of the equirectangular nearest render, read from
/// the plan a memo kept for it, which names the cell the render painted.
fn planned_probes(field: &Field) -> Vec<PixelProbe> {
    let values = values(field);
    let source = field.source();
    let o = options("nearest");
    let memo = WarpMemo::new();
    let geometry = Arc::new(field.georef.geometry.clone());
    let shown = memo
        .project(&geometry, &source, &values, &o)
        .expect("the field renders");
    let mut out = Vec::new();
    for py in 0..shown.height {
        for px in 0..shown.width {
            out.push(
                memo.probe_pixel(&geometry, &source, &values, &o, px, py)
                    .expect("the probe runs")
                    .expect("the pixel is on the raster"),
            );
        }
    }
    assert_eq!(memo.probes_served(), out.len() as u64, "the plan answered");
    out
}

/// N32's row 6 holds 60 points, 6° apart. A pixel at 51.5°E falls in the
/// widened column at 50.625°E, which shows the point at 48°E, 138 km away;
/// the point at 54°E is 108 km away. The issue's own example.
#[test]
fn a_pixel_shows_the_nearer_point_of_its_row_not_its_columns() {
    let field = decode(N32);
    let rows = Rows::of(&field);
    assert_eq!(rows.len[6], 60, "row 6 of N32 holds 60 points");
    let lat = rows.lat[6];
    // The two points either side, on the field's own values.
    let shows = |q: u32| {
        let i = (0..rows.width)
            .find(|&i| rows.shown(i, 6) == q)
            .expect("every point is shown");
        field
            .values
            .get((6 * field.ni + i) as usize)
            .expect("present")
    };
    let (at_48, at_54) = (shows(8), shows(9));
    assert_ne!(
        at_48, at_54,
        "the two points must differ to tell them apart"
    );
    let here = unit(lat, 51.5);
    assert!(
        chord2(here, unit(lat, 54.0)) < chord2(here, unit(lat, 48.0)),
        "54°E is the nearer"
    );

    // The browser host's probe.
    let session = Session::open(N32.to_vec()).expect("opens");
    let probe = session.probe(&field, lat, 51.5).expect("on the grid");
    assert_eq!(probe.value, Some(at_54), "Session::probe reads 54°E");
    // `i` is still where the point lies on the widened raster, 18.31 columns
    // east, not the column the value came from.
    assert!((probe.i - 51.5 / 2.8125).abs() < 1e-9, "i = {}", probe.i);
    // And its nearest warp, one pixel centered there.
    let mut nearest = WarpOptions::default();
    nearest.bilinear = false;
    nearest.bounds = Some([lat - 1e-6, lat + 1e-6, 51.5 - 1e-6, 51.5 + 1e-6]);
    nearest.width = Some(1);
    nearest.height = Some(1);
    let warped = session.warp(&field, &nearest).expect("warps");
    assert_eq!(
        (warped.mask[0], f64::from(warped.values[0])),
        (1, f64::from(at_54 as f32)),
        "Session::warp reads 54°E"
    );

    // The Node addon's probe and render, at the pixel the sweep found it on:
    // the row of pixels nearest row 6's latitude, the column at 51.5°E.
    let values = values(&field);
    let source = field.source();
    let o = options("nearest");
    let shown = project(&source, &values, &o).expect("renders");
    let at = |px: u32, py: u32| probe_pixel(&source, &values, &o, px, py).unwrap().unwrap();
    let py = (0..shown.height)
        .min_by(|&a, &b| {
            let d = |py| (at(0, py).lat.unwrap() - lat).abs();
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    let px = (0..shown.width)
        .find(|&px| (at(px, py).lon.unwrap() - 51.5).abs() < 1e-9)
        .expect("a pixel centered on 51.5°E");
    let p = at(px, py);
    assert!((p.lat.unwrap() - lat).abs() < 0.25, "the pixel is on row 6");
    assert_eq!(p.grid_j, Some(6));
    let i = p.grid_i.expect("a cell") as u32;
    assert_eq!(
        rows.shown(i, 6),
        9,
        "the cell shows point 9, 54°E (column {i})"
    );
    assert_eq!(p.value, Some(at_54));
    let k = (py * shown.width + px) as usize;
    assert_eq!(
        (shown.mask[k], shown.values[k]),
        (1, at_54),
        "the render agrees"
    );
}

/// Every pixel's cell shows a point no farther than the nearest point of the
/// whole grid, for both coarse grids, and in the polar caps for every pixel:
/// where the rows are shortest, the nearest point is the most often on the
/// next row (#1759's second cause).
#[test]
fn every_pixel_shows_the_nearest_grid_point() {
    for (name, bytes) in [("N32", N32), ("O32", O32)] {
        let field = decode(bytes);
        let rows = Rows::of(&field);
        let points = rows.points();
        let mut adjacent_row = 0;
        for p in planned_probes(&field) {
            let (lat, lon) = (p.lat.unwrap(), p.lon.unwrap());
            // Every pixel in the caps, a sample elsewhere: the oracle measures
            // every point of the grid.
            let polar = lat.abs() > 80.0;
            let sampled = ((lat * 7.3 + lon * 3.1).rem_euclid(53.0)) < 1.0;
            if !(polar || sampled) {
                continue;
            }
            let (Some(i), Some(j)) = (p.grid_i, p.grid_j) else {
                // Poleward of the outermost row, off the grid, as before.
                continue;
            };
            let (i, j) = (i as u32, j as u32);
            let at = unit(lat, lon);
            let (la, lo) = rows.position(j, rows.shown(i, j));
            let got = chord2(at, unit(la, lo));
            let best = points
                .iter()
                .map(|&q| chord2(at, q))
                .fold(f64::INFINITY, f64::min);
            assert!(
                got <= best * (1.0 + 1e-9) + 1e-15,
                "{name}: pixel at ({lat}, {lon}) shows row {j} point {}, chord² {got}; \
                 the nearest is {best}",
                rows.shown(i, j)
            );
            // The row the regular map would have rounded to.
            let by_latitude = (0..rows.lat.len())
                .min_by(|&a, &b| {
                    (rows.lat[a] - lat)
                        .abs()
                        .total_cmp(&(rows.lat[b] - lat).abs())
                })
                .unwrap() as u32;
            adjacent_row += usize::from(j != by_latitude);
        }
        assert!(
            adjacent_row > 0,
            "{name}: no pixel's nearest point was on another row than its nearest row \
             by latitude, so the adjacent-row case went unchecked"
        );
    }
}

/// The probe reads the cell the render painted, pixel for pixel: through the
/// free functions, and through a memo's kept plan.
#[test]
fn the_probe_and_the_render_agree() {
    for bytes in [N32, O32] {
        let field = decode(bytes);
        let values = values(&field);
        let source = field.source();
        let o = options("nearest");
        let shown = project(&source, &values, &o).expect("renders");
        let memo = WarpMemo::new();
        let geometry = Arc::new(field.georef.geometry.clone());
        let planned = memo
            .project(&geometry, &source, &values, &o)
            .expect("renders");
        assert_eq!(
            planned.values, shown.values,
            "the plan paints as the warp does"
        );
        assert_eq!(planned.mask, shown.mask);
        for ((px, py), p) in probes(&field, "nearest", 3) {
            let at = (py * shown.width + px) as usize;
            let painted = (shown.mask[at] == 1).then_some(shown.values[at]);
            assert_eq!(p.value, painted, "pixel ({px}, {py})");
            let from_plan = memo
                .probe_pixel(&geometry, &source, &values, &o, px, py)
                .unwrap()
                .unwrap();
            assert_eq!(
                (from_plan.grid_i, from_plan.grid_j, from_plan.value),
                (p.grid_i, p.grid_j, p.value),
                "pixel ({px}, {py}) through the plan"
            );
        }
        assert!(memo.probes_served() > 0, "the plan answered the probes");
    }
}

/// A bilinear render blends, but its probe still names one cell, and it is the
/// nearest point's: the same cell the nearest render paints there, whether the
/// probe is answered from the kept bilinear plan's target or afresh.
#[test]
fn a_bilinear_probe_names_the_nearest_point_too() {
    let field = decode(O32);
    let values = values(&field);
    let source = field.source();
    let bilinear = options("bilinear");
    let memo = WarpMemo::new();
    let geometry = Arc::new(field.georef.geometry.clone());
    memo.project(&geometry, &source, &values, &bilinear)
        .expect("renders");
    let rows = Rows::of(&field);
    let points = rows.points();
    let nearest = probes(&field, "nearest", 5);
    let mut measured = 0;
    for ((px, py), p) in &nearest {
        for q in [
            probe_pixel(&source, &values, &bilinear, *px, *py),
            memo.probe_pixel(&geometry, &source, &values, &bilinear, *px, *py),
        ] {
            let q = q.unwrap().unwrap();
            assert_eq!(
                (q.grid_i, q.grid_j),
                (p.grid_i, p.grid_j),
                "pixel ({px}, {py})"
            );
        }
        // And the cell named shows the nearest point of the grid.
        let (Some(i), Some(j), Some(lat), Some(lon)) = (p.grid_i, p.grid_j, p.lat, p.lon) else {
            continue;
        };
        if lat.abs() < 75.0 {
            continue;
        }
        let at = unit(lat, lon);
        let (la, lo) = rows.position(j as u32, rows.shown(i as u32, j as u32));
        let got = chord2(at, unit(la, lo));
        let best = points
            .iter()
            .map(|&q| chord2(at, q))
            .fold(f64::INFINITY, f64::min);
        assert!(
            got <= best * (1.0 + 1e-9) + 1e-15,
            "pixel ({px}, {py}) at ({lat}, {lon}) names a cell that is not the nearest point's"
        );
        measured += 1;
    }
    assert!(measured > 0, "the polar caps were probed");
}

/// A bilinear render blends the widened raster as it always has: moving the
/// fractional index toward the nearest point would move the blend. Through
/// the render path and through `Session::warp`.
#[test]
fn a_bilinear_render_blends_the_widened_raster_as_before() {
    for bytes in [N32, O32] {
        let field = decode(bytes);
        let values = values(&field);
        let reduced = field.source();
        let mut regular = field.source();
        regular.points_per_row = None;
        let o = options("bilinear");
        let a = project(&reduced, &values, &o).expect("renders");
        let b = project(&regular, &values, &o).expect("renders");
        assert_eq!((a.values, a.mask), (b.values, b.mask), "the render");

        let session = Session::open(bytes.to_vec()).expect("opens");
        let mut widened = field.clone();
        widened.georef.points_per_row = None;
        let mut bilinear = WarpOptions::default();
        bilinear.bilinear = true;
        bilinear.width = Some(400);
        bilinear.height = Some(200);
        bilinear.bounds = Some([-90.0, 90.0, 0.0, 360.0]);
        let a = session.warp(&field, &bilinear).expect("warps");
        let b = session.warp(&widened, &bilinear).expect("warps");
        assert_eq!(a, b, "Session::warp");
        // And a nearest warp does tell them apart, so the two above are the
        // rows read alike rather than ignored.
        bilinear.bilinear = false;
        assert_ne!(
            session.warp(&field, &bilinear).unwrap(),
            session.warp(&widened, &bilinear).unwrap()
        );
    }
}

/// A reduced grid and its regular sibling have the same geometry and shape, so
/// a memo keyed on those alone served one's plan to the other.
#[test]
fn a_memo_does_not_serve_a_reduced_grids_plan_to_its_regular_sibling() {
    let field = decode(N32);
    let values = values(&field);
    let reduced = field.source();
    let mut regular = field.source();
    regular.points_per_row = None;
    let o = options("nearest");
    let memo = WarpMemo::new();
    let geometry = Arc::new(field.georef.geometry.clone());
    let a = memo.project(&geometry, &reduced, &values, &o).unwrap();
    let b = memo.project(&geometry, &regular, &values, &o).unwrap();
    assert_eq!(memo.plans_made(), 2, "the two are planned apart");
    assert_eq!(a.values, project(&reduced, &values, &o).unwrap().values);
    assert_eq!(b.values, project(&regular, &values, &o).unwrap().values);
    assert_ne!(a.values, b.values, "the two rules paint N32 differently");
}

/// The review's row list (#1781): 65,535 rows from pole to pole (GRIB1's
/// 16-bit `Nj`), two points on the first and one on every other.
fn rows_of_one_point() -> Vec<u16> {
    let mut pl = vec![1u16; 65_535];
    pl[0] = 2;
    pl
}

/// `reduced_gg_n32.grib1`, a constant field (zero bits a value, so no data
/// octets), with its GDS rewritten as a reduced lat/lon grid (type 0, `Ni`
/// missing) over [`rows_of_one_point`], 90°N to 90°S, 0°E to 180°E.
fn grib1_rows_of_one_point() -> Vec<u8> {
    let (pds_end, gds_len) = (8 + 52, 160);
    let mut gds = N32_CONSTANT[pds_end..pds_end + 32].to_vec();
    let pl = rows_of_one_point();
    let len = 32 + 2 * pl.len() as u32;
    gds[..3].copy_from_slice(&len.to_be_bytes()[1..]);
    gds[5] = 0; // lat/lon
    gds[8..10].copy_from_slice(&(pl.len() as u16).to_be_bytes());
    gds[10..13].copy_from_slice(&[0x01, 0x5F, 0x90]); // 90.000°N
    gds[13..16].copy_from_slice(&[0, 0, 0]); // 0°E
    gds[17..20].copy_from_slice(&[0x81, 0x5F, 0x90]); // 90.000°S
    gds[20..23].copy_from_slice(&[0x02, 0xBF, 0x20]); // 180.000°E
    gds[25..27].copy_from_slice(&[0, 3]); // Dj, unused for a reduced grid
    for n in &pl {
        gds.extend(n.to_be_bytes());
    }
    let mut out = N32_CONSTANT[..pds_end].to_vec();
    out.extend(gds);
    out.extend(&N32_CONSTANT[pds_end + gds_len..]);
    let total = out.len() as u32;
    out[4..7].copy_from_slice(&total.to_be_bytes()[1..]);
    out
}

/// `reduced_latlon_10deg.grib2` with §3 rewritten over [`rows_of_one_point`]
/// (0°E to 180°E), §5 a constant field of zero bits a value, no bitmap, and
/// an empty §7.
fn grib2_rows_of_one_point() -> Vec<u8> {
    let b = LATLON_10DEG;
    let section = |at: usize| {
        let len = u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        (at, len)
    };
    let (s1, l1) = section(16);
    let (s3, l3) = section(s1 + l1);
    let (s4, l4) = section(s3 + l3);
    let (s5, l5) = section(s4 + l4);
    assert_eq!((b[s3 + 4], b[s4 + 4], b[s5 + 4]), (3, 4, 5));
    let pl = rows_of_one_point();
    let points: u32 = pl.iter().map(|&n| u32::from(n)).sum();
    let mut gds = b[s3..s3 + 72].to_vec();
    gds[..4].copy_from_slice(&(72 + 2 * pl.len() as u32).to_be_bytes());
    gds[6..10].copy_from_slice(&points.to_be_bytes());
    gds[34..38].copy_from_slice(&(pl.len() as u32).to_be_bytes());
    gds[59..63].copy_from_slice(&180_000_000u32.to_be_bytes()); // Lo2 = 180°E
    for n in &pl {
        gds.extend(n.to_be_bytes());
    }
    let mut drs = b[s5..s5 + l5].to_vec();
    drs[5..9].copy_from_slice(&points.to_be_bytes());
    drs[19] = 0; // zero bits a value: every point is the reference value
    let mut out = b[..s1 + l1].to_vec();
    out.extend(gds);
    out.extend(&b[s4..s4 + l4]);
    out.extend(drs);
    out.extend([0, 0, 0, 6, 6, 255]); // §6: no bitmap
    out.extend([0, 0, 0, 5, 7]); // §7: no data octets
    out.extend(b"7777");
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

/// On a crafted row list whose rows are far shorter than their circles, the
/// search for the nearest point measured every row on the way to the pole,
/// about a millisecond a pixel (#1781). Both formats that read a reduced
/// lat/lon grid reach that search, so both are checked: the cell a nearest
/// sample or a probe reads is at most 32 rows from the point's own row, and
/// some points do move off their row, so the search runs rather than being
/// declined.
#[test]
fn rows_of_one_point_search_a_bounded_reach() {
    for (name, bytes) in [
        ("GRIB1", grib1_rows_of_one_point()),
        ("GRIB2", grib2_rows_of_one_point()),
    ] {
        let field = decode(&bytes);
        let pl: Vec<u32> = rows_of_one_point().into_iter().map(u32::from).collect();
        assert_eq!(
            field.georef.points_per_row.as_deref(),
            Some(&pl[..]),
            "{name}"
        );
        assert_eq!((field.ni, field.nj), (2, 65_535), "{name}");
        let geometry = &field.georef.geometry;
        let refined = geometry.reduced_inverse_at(field.georef.points_per_row.as_deref());
        let regular = geometry.inverse_at();
        let mut farthest = 0;
        for k in 0..=36 {
            let lat = -90.0 + 5.0 * f64::from(k);
            for lon in [10.0, 100.0, 150.0, 200.0, 280.0] {
                let (Some(at), Some(own)) = (refined(lat, lon), regular(lat, lon)) else {
                    continue;
                };
                let moved = (at.j.round() - own.j.round()).abs();
                assert!(
                    moved <= 32.0,
                    "{name}: ({lat}, {lon}) read a row {moved} rows away"
                );
                farthest = farthest.max(moved as u32);
            }
        }
        assert_eq!(
            farthest, 32,
            "{name}: the search moved no point as far as its reach"
        );
    }
}
