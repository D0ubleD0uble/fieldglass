//! The spacing a probe reports for its cell (#1466), on grids whose rows are
//! not circles of latitude.
//!
//! A host writes the probed position with the decimals this spacing needs. On a
//! Lambert or rotated grid a row is tangent to a parallel somewhere, so the
//! latitude barely moves from one cell to the next there (a few millionths of a
//! degree on HRRR). A spacing that took that change would make the decimals
//! vary by column. The oracle is the grid's own nominal spacing, which the
//! message states (3 km on HRRR, 2 degrees in the rotated frame), not a second
//! copy of the forward map.
//!
//! Committed fixtures only: `samples/` is git-ignored.

use fieldglass::{DecodeOptions, RenderOptions, Session};

const HRRR: &str = "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2";
const ROTATED: &str = "../fieldglass-grib2/tests/fixtures/rotated_latlon_surface.grib2";

/// The `cell_step` the source-view probe reports for every cell of `row`,
/// and for every cell of `column`, of the first message of `path`.
fn steps(path: &str, cells: &[(u32, u32)]) -> Vec<f64> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let session = Session::open(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
    let field = session
        .decode(0, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    let values: Vec<Option<f64>> = (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let source = field.source();
    let options = RenderOptions::default();
    assert_eq!(
        options.projection, "source",
        "the default view is the source"
    );
    cells
        .iter()
        .map(|&(i, j)| {
            // The source view paints cell (i, j) at pixel (i, j) before any
            // flip, so ask for the cell by the probe's own report.
            let flip = source.scan.flips_source_rows(false);
            let py = if flip { source.nj - 1 - j } else { j };
            let probe = fieldglass::render::probe_pixel(&source, &values, &options, i, py)
                .expect("the probe runs")
                .expect("the pixel is on the raster");
            assert_eq!(
                (probe.grid_i, probe.grid_j),
                (Some(i as i32), Some(j as i32))
            );
            probe
                .cell_step
                .unwrap_or_else(|| panic!("{path}: cell ({i}, {j}) has no step"))
        })
        .collect()
}

#[test]
fn a_lambert_grid_reports_its_nominal_spacing_in_every_column() {
    // HRRR is 3 km: 0.027 degrees. Across the row the latitude along a row
    // changes by far less than that near the central meridian.
    let nominal = 3000.0 / 111_195.0;
    let (ni, nj) = (1799, 1059);
    let cells: Vec<(u32, u32)> = [
        0,
        1,
        ni / 4,
        ni / 2 - 1,
        ni / 2,
        ni / 2 + 1,
        3 * ni / 4,
        ni - 2,
        ni - 1,
    ]
    .into_iter()
    .map(|i| (i, nj / 2))
    .chain([(ni / 2, 0), (ni / 2, nj / 2), (ni / 2, nj - 1)])
    .collect();
    for (cell, step) in cells.iter().zip(steps(HRRR, &cells)) {
        assert!(
            step > nominal * 0.6 && step < nominal * 1.6,
            "cell {cell:?}: step {step} degrees against a nominal {nominal}"
        );
    }
}

#[test]
fn a_rotated_grid_reports_its_spacing_in_every_column() {
    // 2 degrees in the rotated frame: no more than that on the sphere, and
    // within a factor of two of it at the grid's latitudes.
    let cells: Vec<(u32, u32)> = (0..16).map(|i| (i, 15)).chain([(8, 0), (8, 30)]).collect();
    for (cell, step) in cells.iter().zip(steps(ROTATED, &cells)) {
        assert!(
            step < 2.5 && step > 1.0,
            "cell {cell:?}: step {step} degrees"
        );
    }
}
