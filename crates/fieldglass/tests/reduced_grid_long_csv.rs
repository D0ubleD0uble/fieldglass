//! A reduced grid's long CSV reports the points the file holds, where eccodes
//! places them (#244).
//!
//! A reduced grid stores `PL[j]` points on row `j`, fewer toward the poles.
//! Every consumer receives it widened to the widest row, which is what lets it
//! warp and reproject like a regular grid — and the long `lat,lon,value` CSV
//! used to walk that widened raster, exporting each short row's points once per
//! column they had been repeated into. On the N32 fixtures that is 8,192 rows for
//! a file holding 6,114: 2,078 copies, indistinguishable from the originals,
//! at longitudes the file never samples.
//!
//! **The oracle is eccodes 2.34.1** — the pinned version — as
//! `grib_get_data -L "%.9f %.9f" <fixture>`. Three things are held to it per
//! fixture, each catching a different way to be wrong:
//!
//! - the **total** row count, which is the whole of the original defect;
//! - the **per-row** counts, all 64 of them, which a correct total can hide —
//!   dropping one point from a long row and duplicating one on a short row
//!   still sums right;
//! - **sample points** by position and value, across the shortest row, a
//!   quarter-way row, the widest row and the last. Position alone would pass a
//!   CSV that put the right coordinates beside the wrong values.
//!
//! The octahedral GRIB2 fixture carries the strongest version of the last check:
//! its values are an index ramp, so row 0 holds exactly `0, 1, …, 19`, and a
//! value read from even one column off is a different integer.
//!
//! Three fixtures because the pattern spans both editions: GRIB1 `reduced_gg`,
//! and GRIB2 in both its reduced Gaussian and octahedral forms. The constants
//! below were generated from that output rather than typed.

use fieldglass::{DecodeOptions, Dtype, Session};

const GRIB1_SMOOTH_PL: [usize; 64] = [
    20, 27, 36, 40, 45, 50, 60, 64, 72, 75, 80, 90, 90, 96, 100, 108, 108, 120, 120, 120, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 120, 120, 120, 108, 108, 100, 96, 90, 90, 80, 75, 72, 64, 60, 50, 45, 40, 36,
    27, 20,
];
/// `(row, point in row, lat, lon, value)` from `grib_get_data -L "%.9f %.9f"`.
const GRIB1_SMOOTH_SAMPLES: [(usize, usize, f64, f64, f64); 20] = [
    (0, 0, 87.863798839, 0.000000000, 2.4805561829e+02),
    (0, 1, 87.863798839, 18.000000000, 2.4827436829e+02),
    (0, 10, 87.863798839, 180.000000000, 2.4805561829e+02),
    (0, 19, 87.863798839, 342.000000000, 2.4783686829e+02),
    (1, 0, 85.096526988, 0.000000000, 2.4829194641e+02),
    (1, 1, 85.096526988, 13.333333333, 2.4867573547e+02),
    (1, 13, 85.096526988, 173.333333333, 2.4809468079e+02),
    (1, 26, 85.096526988, 346.666666667, 2.4790815735e+02),
    (16, 0, 43.254194665, 0.000000000, 2.6921772766e+02),
    (16, 1, 43.254194665, 3.333333333, 2.7006343079e+02),
    (16, 54, 43.254194665, 180.000000000, 2.6921772766e+02),
    (16, 107, 43.254194665, 356.666666667, 2.6837300110e+02),
    (20, 0, 32.091943882, 0.000000000, 2.7670991516e+02),
    (20, 1, 32.091943882, 2.812500000, 2.7753999329e+02),
    (20, 64, 32.091943882, 180.000000000, 2.7670991516e+02),
    (20, 127, 32.091943882, 357.187500000, 2.7587886047e+02),
    (63, 0, -87.863798839, 0.000000000, 2.4805561829e+02),
    (63, 1, -87.863798839, 18.000000000, 2.4827436829e+02),
    (63, 10, -87.863798839, 180.000000000, 2.4805561829e+02),
    (63, 19, -87.863798839, 342.000000000, 2.4783686829e+02),
];

const GRIB2_OCTAHEDRAL_PL: [usize; 64] = [
    20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108,
    112, 116, 120, 124, 128, 132, 136, 140, 144, 144, 140, 136, 132, 128, 124, 120, 116, 112, 108,
    104, 100, 96, 92, 88, 84, 80, 76, 72, 68, 64, 60, 56, 52, 48, 44, 40, 36, 32, 28, 24, 20,
];
/// `(row, point in row, lat, lon, value)` from `grib_get_data -L "%.9f %.9f"`.
const GRIB2_OCTAHEDRAL_SAMPLES: [(usize, usize, f64, f64, f64); 20] = [
    (0, 0, 87.863798839, 0.000000000, 0.0000000000e+00),
    (0, 1, 87.863798839, 18.000000000, 1.0000000000e+00),
    (0, 10, 87.863798839, 180.000000000, 1.0000000000e+01),
    (0, 19, 87.863798839, 342.000000000, 1.9000000000e+01),
    (1, 0, 85.096526988, 0.000000000, 2.0000000000e+01),
    (1, 1, 85.096526988, 15.000000000, 2.1000000000e+01),
    (1, 12, 85.096526988, 180.000000000, 3.2000000000e+01),
    (1, 23, 85.096526988, 345.000000000, 4.3000000000e+01),
    (16, 0, 43.254194665, 0.000000000, 0.0000000000e+00),
    (16, 1, 43.254194665, 4.285714286, 1.0000000000e+00),
    (16, 42, 43.254194665, 180.000000000, 4.2000000000e+01),
    (16, 83, 43.254194665, 355.714285714, 3.3000000000e+01),
    (31, 0, 1.395306911, 0.000000000, 3.0000000000e+01),
    (31, 1, 1.395306911, 2.500000000, 3.1000000000e+01),
    (31, 72, 1.395306911, 180.000000000, 2.0000000000e+00),
    (31, 143, 1.395306911, 357.500000000, 2.3000000000e+01),
    (63, 0, -87.863798839, 0.000000000, 2.8000000000e+01),
    (63, 1, -87.863798839, 18.000000000, 2.9000000000e+01),
    (63, 10, -87.863798839, 180.000000000, 3.8000000000e+01),
    (63, 19, -87.863798839, 342.000000000, 4.7000000000e+01),
];

const GRIB2_PRESSURE_PL: [usize; 64] = [
    20, 27, 36, 40, 45, 50, 60, 64, 72, 75, 80, 90, 90, 96, 100, 108, 108, 120, 120, 120, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 120, 120, 120, 108, 108, 100, 96, 90, 90, 80, 75, 72, 64, 60, 50, 45, 40, 36,
    27, 20,
];
/// `(row, point in row, lat, lon, value)` from `grib_get_data -L "%.9f %.9f"`.
const GRIB2_PRESSURE_SAMPLES: [(usize, usize, f64, f64, f64); 20] = [
    (0, 0, 87.863798839, 0.000000000, 2.4746484375e+02),
    (0, 1, 87.863798839, 18.000000000, 2.4738671875e+02),
    (0, 10, 87.863798839, 180.000000000, 2.4463671875e+02),
    (0, 19, 87.863798839, 342.000000000, 2.4680859375e+02),
    (1, 0, 85.096526988, 0.000000000, 2.5663671875e+02),
    (1, 1, 85.096526988, 13.333333333, 2.5749609375e+02),
    (1, 13, 85.096526988, 173.333333333, 2.4412109375e+02),
    (1, 26, 85.096526988, 346.666666667, 2.5566796875e+02),
    (16, 0, 43.254194665, 0.000000000, 2.8557421875e+02),
    (16, 1, 43.254194665, 3.333333333, 2.8621484375e+02),
    (16, 54, 43.254194665, 180.000000000, 2.7596484375e+02),
    (16, 107, 43.254194665, 356.666666667, 2.8485546875e+02),
    (20, 0, 32.091943882, 0.000000000, 2.9230859375e+02),
    (20, 1, 32.091943882, 2.812500000, 2.9119921875e+02),
    (20, 64, 32.091943882, 180.000000000, 2.8980859375e+02),
    (20, 127, 32.091943882, 357.187500000, 2.9329296875e+02),
    (63, 0, -87.863798839, 0.000000000, 2.5602734375e+02),
    (63, 1, -87.863798839, 18.000000000, 2.5632421875e+02),
    (63, 10, -87.863798839, 180.000000000, 2.5696484375e+02),
    (63, 19, -87.863798839, 342.000000000, 2.5557421875e+02),
];

/// Decode message 0 and export it as the long CSV, through the path a host uses.
fn long_csv(path: &str) -> (Vec<(f64, f64, Option<f64>)>, fieldglass::Field) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path} is committed: {e}"));
    let session = Session::open(bytes).expect("the fixture opens");
    let field = session
        .decode(0, &DecodeOptions::new(Dtype::Auto))
        .expect("the fixture decodes");
    assert!(
        field.georef.points_per_row.is_some(),
        "{path}: a reduced grid states its points per row"
    );
    let values: Vec<Option<f64>> = field_values(&field);
    let csv = fieldglass::render::field_csv(&field.source(), &values, "long")
        .expect("a reduced grid exports as a long CSV");
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("lat,lon,value"));
    let rows = lines
        .map(|line| {
            let mut cells = line.split(',');
            let lat = cells.next().and_then(|c| c.parse().ok()).expect("lat");
            let lon = cells.next().and_then(|c| c.parse().ok()).expect("lon");
            let value = cells.next().and_then(|c| c.parse().ok());
            (lat, lon, value)
        })
        .collect();
    (rows, field)
}

/// The field's values, masked cells as `None`, in raster order.
fn field_values(field: &fieldglass::Field) -> Vec<Option<f64>> {
    let dense: Vec<f64> = match &field.values {
        fieldglass::Values::F64(v) => v.clone(),
        fieldglass::Values::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
        other => panic!("a values width this test does not read: {other:?}"),
    };
    field
        .mask
        .iter()
        .zip(dense)
        .map(|(&m, v)| (m == 1).then_some(v))
        .collect()
}

/// The CSV's rows grouped into the grid's rows, by latitude, in order.
fn by_row(rows: &[(f64, f64, Option<f64>)]) -> Vec<Vec<(f64, f64, Option<f64>)>> {
    let mut out: Vec<Vec<(f64, f64, Option<f64>)>> = Vec::new();
    for &row in rows {
        match out.last_mut() {
            Some(last) if last[0].0 == row.0 => last.push(row),
            _ => out.push(vec![row]),
        }
    }
    out
}

fn holds_to_eccodes(path: &str, pl: &[usize], samples: &[(usize, usize, f64, f64, f64)]) {
    let (rows, _) = long_csv(path);
    assert_eq!(
        rows.len(),
        pl.iter().sum::<usize>(),
        "{path}: one CSV row per point the file holds, and no copies"
    );
    let grouped = by_row(&rows);
    let counts: Vec<usize> = grouped.iter().map(Vec::len).collect();
    assert_eq!(
        counts, pl,
        "{path}: each row exports exactly the points it stores"
    );
    for &(ri, k, lat, lon, value) in samples {
        let (got_lat, got_lon, got_value) = grouped[ri][k];
        assert!(
            (got_lat - lat).abs() < 1e-6 && (got_lon - lon).abs() < 1e-6,
            "{path} row {ri} point {k}: exported ({got_lat}, {got_lon}), eccodes says ({lat}, {lon})"
        );
        let got_value =
            got_value.unwrap_or_else(|| panic!("{path} row {ri} point {k} has a value"));
        assert!(
            (got_value - value).abs() <= 1e-6 * value.abs().max(1.0),
            "{path} row {ri} point {k}: exported {got_value}, eccodes says {value}"
        );
    }
}

#[test]
fn a_grib1_reduced_grid_exports_the_points_eccodes_reports() {
    holds_to_eccodes(
        "../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1",
        &GRIB1_SMOOTH_PL,
        &GRIB1_SMOOTH_SAMPLES,
    );
}

#[test]
fn a_grib2_reduced_gaussian_grid_exports_the_points_eccodes_reports() {
    holds_to_eccodes(
        "../fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2",
        &GRIB2_PRESSURE_PL,
        &GRIB2_PRESSURE_SAMPLES,
    );
}

#[test]
fn a_grib2_octahedral_grid_exports_the_points_eccodes_reports() {
    holds_to_eccodes(
        "../fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2",
        &GRIB2_OCTAHEDRAL_PL,
        &GRIB2_OCTAHEDRAL_SAMPLES,
    );
}

/// The octahedral fixture's values are an index ramp, so a whole row can be held
/// to it: row 0's twenty points are `0, 1, …, 19`, at 18° steps from 0°. This is
/// the check that notices a value read from the column beside the right one,
/// which a sample of a smooth field could round past.
#[test]
fn every_point_of_the_octahedral_polar_row_is_its_own_value() {
    let (rows, _) = long_csv("../fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2");
    let polar = &by_row(&rows)[0];
    assert_eq!(polar.len(), 20);
    for (k, &(_, lon, value)) in polar.iter().enumerate() {
        assert!((lon - 18.0 * k as f64).abs() < 1e-9, "point {k} at {lon}°");
        assert_eq!(value, Some(k as f64), "point {k} carries its own value");
    }
}

/// The matrix layout is the raster, and stays widened: it has no coordinates to
/// be wrong about, and a caller asking for a `ni × nj` table gets one.
#[test]
fn the_matrix_layout_is_still_the_widened_raster() {
    let (_, field) = long_csv("../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1");
    let values = field_values(&field);
    let csv = fieldglass::render::field_csv(&field.source(), &values, "matrix").expect("matrix");
    assert_eq!(csv.lines().count(), field.nj as usize);
    assert_eq!(
        csv.lines().next().map(|l| l.split(',').count()),
        Some(field.ni as usize)
    );
}
