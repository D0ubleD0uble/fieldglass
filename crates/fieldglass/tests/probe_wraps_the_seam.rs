//! A probe on a global grid reads across the seam (#1759).
//!
//! A grid whose columns close on themselves owns the gap between its last
//! column and its first. A point in the eastern half of that gap lands at a
//! fractional column past `ni - 0.5`, which rounds to `ni`: column 0 again,
//! one cell east. Clamping it to `ni - 1` instead reads the point a cell west
//! of the nearest one. The render path wraps there since #332; the probes
//! `Session` answers with must wrap the same way.
//!
//! The oracle is the grid's own definition: every point of the grid measured
//! by great-circle distance, or, for a regular grid, the column whose
//! longitude is nearest.

#![cfg(all(feature = "grib1", feature = "grib2"))]

use fieldglass::api::Field;
use fieldglass::{DecodeOptions, Dtype, Session};

const N32: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1");
const O32: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2");
/// GFS at 2.5°: 144 columns from 0°E to 357.5°E, global.
const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
/// A spectral message past what its 0.5° synthesis grid carries, so
/// `probe_full_detail` answers.
const SPECTRAL: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/spectral_simple_t383.grib2");

fn open(bytes: &[u8]) -> (Session, Field) {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &DecodeOptions::new(Dtype::Auto))
        .expect("the message decodes");
    (session, field)
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

/// Every point of a reduced grid with the value it holds: point `q` of row
/// `j` lies at the row's latitude and `λ₀ + q·360/PL[j]`, and is the value
/// of the widened columns that show it, as `expand_reduced_to_regular`
/// documents.
fn reduced_points(field: &Field) -> Vec<([f64; 3], Option<f64>)> {
    let pl = field
        .georef
        .points_per_row
        .clone()
        .expect("a reduced grid carries its rows");
    let geometry = &field.georef.geometry;
    let w = u64::from(field.ni);
    let lon0 = geometry.forward(0, 0).expect("first point").1;
    let mut out = Vec::new();
    for (j, &n) in pl.iter().enumerate() {
        let lat = geometry.forward(0, j as u32).expect("row placed").0;
        let n = u64::from(n);
        for q in 0..n {
            let k = (0..w)
                .find(|&k| (k * n + w / 2) / w % n == q)
                .expect("every point is shown");
            let flat = j * field.ni as usize + k as usize;
            let value = (field.mask[flat] == 1)
                .then(|| field.values.get(flat))
                .flatten();
            out.push((unit(lat, lon0 + q as f64 * 360.0 / n as f64), value));
        }
    }
    out
}

/// The review's case: O32 at (-13.35°, 358.81°) lands at column 143.53 of
/// the 144-wide raster. That is point 0's cell across the seam, which holds
/// 26.0; the clamp read column 143, which holds 3.0.
#[test]
fn an_octahedral_probe_east_of_the_last_column_reads_point_zero() {
    let (session, field) = open(O32);
    let probe = session.probe(&field, -13.35, 358.81).expect("on the grid");
    assert!(probe.i > f64::from(field.ni) - 0.5, "i = {}", probe.i);
    let points = reduced_points(&field);
    let at = unit(-13.35, 358.81);
    let nearest = points
        .iter()
        .min_by(|a, b| chord2(at, a.0).total_cmp(&chord2(at, b.0)))
        .expect("points");
    assert_eq!(nearest.1, Some(26.0), "the oracle's nearest point");
    assert_eq!(probe.value, Some(26.0));
}

/// On both coarse reduced grids, every probe near the seam reads a point no
/// farther than the nearest point of the whole grid, and so does a lattice
/// over the rest of the globe.
#[test]
fn a_reduced_grid_probe_reads_the_nearest_point_across_the_seam() {
    for (name, bytes) in [("N32", N32), ("O32", O32)] {
        let (session, field) = open(bytes);
        let points = reduced_points(&field);
        let mut seam = 0;
        let lons = (0..60)
            .map(|k| 357.0 + 3.0 * f64::from(k) / 60.0)
            .chain((0..36).map(|k| 10.0 * f64::from(k) + 3.7));
        for lon in lons {
            for k in 0..60 {
                let lat = -87.0 + 174.0 * f64::from(k) / 59.0;
                let Some(probe) = session.probe(&field, lat, lon) else {
                    continue;
                };
                seam += usize::from(probe.i > f64::from(field.ni) - 0.5);
                let at = unit(lat, lon);
                let best = points
                    .iter()
                    .map(|p| chord2(at, p.0))
                    .fold(f64::INFINITY, f64::min);
                // Any point tied for nearest is a right answer.
                let read_a_nearest = points
                    .iter()
                    .filter(|p| chord2(at, p.0) <= best * (1.0 + 1e-6) + 1e-12)
                    .any(|p| p.1 == probe.value);
                assert!(
                    read_a_nearest,
                    "{name}: probe at ({lat}, {lon}), i = {}, read {:?}, not a nearest point's value",
                    probe.i, probe.value
                );
            }
        }
        assert!(seam > 0, "{name}: no probe landed in the seam gap");
    }
}

/// A regular global grid: 359.5°E is 0.5° from column 0 (0°E) and 2° from
/// column 143 (357.5°E), so the probe reads column 0. 358°E stays on
/// column 143.
#[test]
fn a_regular_global_probe_east_of_the_last_column_reads_column_zero() {
    let (session, field) = open(GFS);
    assert_eq!(field.ni, 144);
    let geometry = &field.georef.geometry;
    assert!(geometry.is_periodic_x(), "GFS 2.5° is global");
    let (ni, nj) = (field.ni as usize, field.nj as usize);
    // A masked cell probes as `None`.
    let column = |j: usize, i: usize| {
        let k = j * ni + i;
        (field.mask[k] == 1).then(|| field.values.get(k)).flatten()
    };
    // The rows where the two columns differ, so reading the wrong one shows.
    let rows: Vec<usize> = (0..nj)
        .filter(|&j| column(j, 0) != column(j, ni - 1))
        .collect();
    assert!(
        rows.len() > 10,
        "the seam columns differ on {} of {nj} rows",
        rows.len()
    );
    for j in rows {
        let lat = geometry.forward(0, j as u32).expect("row placed").0;
        let east = session.probe(&field, lat, 359.5).expect("on the grid");
        assert!((east.i - 143.8).abs() < 1e-9, "i = {}", east.i);
        assert_eq!(east.value, column(j, 0), "row {j} at 359.5°E reads 0°E");
        let west = session.probe(&field, lat, 358.0).expect("on the grid");
        assert_eq!(
            west.value,
            column(j, ni - 1),
            "row {j} at 358°E reads 357.5°E"
        );
    }
}

/// The full-detail probe picks its node by `probe`'s rule. On the 0.5°
/// synthesis grid, 359.9°E is 0.1° from the node at 0°E and 0.4° from the one
/// at 359.5°E, so both probes read the 0°E node.
#[test]
fn a_full_detail_probe_east_of_the_last_column_reads_column_zero() {
    let (session, field) = open(SPECTRAL);
    assert_eq!((field.ni, field.nj), (720, 361));
    for lat in [-41.5, 0.0, 23.0, 66.5] {
        let at_zero = session.probe(&field, lat, 0.0).expect("on the map");
        let east = session.probe(&field, lat, 359.9).expect("on the map");
        assert_eq!(east.value, at_zero.value, "probe at ({lat}, 359.9)");
        let detail = |lon: f64| {
            session
                .probe_full_detail(0, lat, lon)
                .expect("full detail")
                .expect("band-limited")
                .value
        };
        let west = detail(359.5);
        assert_ne!(
            detail(0.0).to_bits(),
            west.to_bits(),
            "the two nodes must differ to tell them apart"
        );
        assert_eq!(
            detail(359.9).to_bits(),
            detail(0.0).to_bits(),
            "full detail at ({lat}, 359.9)"
        );
    }
}
