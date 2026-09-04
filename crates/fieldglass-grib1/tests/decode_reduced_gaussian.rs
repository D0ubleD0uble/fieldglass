//! End-to-end decode of a reduced (quasi-regular) Gaussian GRIB1 grid.
//!
//! Fixture `reduced_gg_n32.grib1` is the eccodes 2.34.1 `reduced_gg_pl_32`
//! sample with every value set to a constant 285.5 (see `fixtures/NOTICE.md`).
//! It pins the reader's native-count path: a reduced grid stores `sum(PL)`
//! points, not `Ni·Nj`, so `decode_message_values` must size its output to the
//! `PL` list. `grib_get_data` (eccodes) is the oracle — 6114 points, all 285.5.

use fieldglass_grib1::{Grib1Reader, GridDescription};

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gg_n32.grib1");

/// The 64-row `PL` list dumped from the fixture by `grib_get_data` (point
/// counts per parallel, symmetric pole-to-pole).
const PL: [u32; 64] = [
    20, 27, 36, 40, 45, 50, 60, 64, 72, 75, 80, 90, 90, 96, 100, 108, 108, 120, 120, 120, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 120, 120, 120, 108, 108, 100, 96, 90, 90, 80, 75, 72, 64, 60, 50, 45, 40, 36,
    27, 20,
];

#[test]
fn reduced_gaussian_gds_reports_geometry() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let gds = reader.messages[0].gds.as_ref().expect("message has a GDS");
    assert_eq!(gds.grid_type_name(), "reduced_gaussian");
    // Ni is the widest row (128); Nj is the 64-row count.
    assert_eq!(gds.dimensions(), Some((128, 64)));
    assert_eq!(gds.points_per_row(), Some(PL.as_slice()));
    assert_eq!(gds.num_data_points(), Some(6114), "sum of PL");
    let GridDescription::ReducedGaussian(g) = gds else {
        panic!("expected ReducedGaussian");
    };
    assert_eq!(g.n_gaussians, 32);
    // First parallel near the north pole; box spans the full longitude circle.
    let (la1, lo1, _, lo2) = gds.bounds().expect("reduced grid has bounds");
    assert!((la1 - 87.864).abs() < 1e-3, "lat_first: {la1}");
    assert_eq!(lo1, 0.0);
    assert!((lo2 - 357.188).abs() < 1e-3, "lon_last: {lo2}");
}

#[test]
fn decode_yields_native_point_count_matching_eccodes() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let values = reader
        .decode_message_values(0)
        .expect("reduced Gaussian decode succeeds");
    // eccodes reports 6114 points (sum of PL), not 128·64 = 8192.
    assert_eq!(values.len(), 6114, "decoded sum(PL) points, not Ni·Nj");
    assert!(
        values.iter().all(|v| v.is_some()),
        "no bitmap → all present"
    );
    for (i, v) in values.iter().enumerate() {
        let v = v.expect("present");
        assert!((v - 285.5).abs() < 1e-6, "value[{i}] = {v}, expected 285.5");
    }
}

/// `decode_message_raster` hands back the rectangle `dimensions()` promises,
/// with no expansion step left to the caller (#543).
///
/// The distinction this pins is the one a standalone consumer gets wrong: the
/// same message decodes to 6114 values in storage order and 8192 on the raster,
/// and only the second can be indexed as `raster[j*ni + i]`. Row 0 is 20 points
/// widened to 128, so a correct expansion repeats each of them; the wrong one —
/// treating storage order as row-major — would put row 1's data in row 0's tail.
#[test]
fn decode_message_raster_fills_the_shape_dimensions_promises() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let gds = reader.messages[0].gds.as_ref().expect("message has a GDS");
    let (ni, nj) = gds.dimensions().expect("a row-expanded raster shape");
    assert_eq!((ni, nj), (128, 64));

    let stored = reader
        .decode_message_values(0)
        .expect("storage-order decode");
    let raster = reader.decode_message_raster(0).expect("raster decode");
    assert_eq!(stored.len(), 6114, "sum(PL), the layout the message stores");
    assert_eq!(raster.len(), (ni * nj) as usize, "Ni*Nj, the raster");
    assert!(raster.len() > stored.len(), "the two are not the same call");

    // Every raster value came from the field; the fixture is constant 285.5.
    assert!(
        raster.iter().all(|v| v.is_some()),
        "no holes after widening"
    );
    for (i, v) in raster.iter().enumerate() {
        let v = v.expect("present");
        assert!((v - 285.5).abs() < 1e-6, "raster[{i}] = {v}");
    }
}

/// The raster the values land on and the box they are placed in come from the
/// same pair of calls (#543).
///
/// `bounds()` still reports the file's own `Lo2`; `raster_bounds()` reports the
/// corner 128 columns around the circle actually reach. For this classic `N32`
/// the widest row *is* the `4N` reference width, so the two agree — the point
/// here is that the derived value is right where it can be checked against the
/// file, with `gds.rs`'s octahedral unit test covering where they diverge.
#[test]
fn raster_bounds_places_the_expanded_raster() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let gds = reader.messages[0].gds.as_ref().expect("message has a GDS");
    let (width, _) = gds.dimensions().expect("a raster shape");
    assert_eq!(width, 128, "N32: the widest row is the 4N reference width");

    let (_, _, _, declared) = gds.bounds().expect("a reduced grid has bounds");
    let (la1, lo1, la2, lo2) = gds.raster_bounds().expect("and a raster extent");
    assert!((la1 - 87.864).abs() < 1e-3, "lat_first: {la1}");
    assert_eq!(lo1, 0.0);
    assert!((la2 + 87.864).abs() < 1e-3, "lat_last: {la2}");
    // 0 + 127 * 360/128, which for this grid is also what the file declares.
    assert!((lo2 - 357.1875).abs() < 1e-9, "raster east edge {lo2}");
    assert!(
        (lo2 - declared).abs() < 1e-3,
        "N32: derived matches declared"
    );
}

/// A reduced Gaussian grid is named, not measured (#500).
///
/// This grid *does* have `dimensions()` — the widest row paired with the row
/// count, which is the raster a row-expanded field needs — but that pair is a
/// shape this crate computes, not one the file states. The file has 6114 points
/// in rows of differing width, not 128 × 64 = 8192, so reporting the raster as
/// the grid's size overstates it by a quarter. eccodes calls this grid `N32`,
/// and so does GRIB2's copy of the same grid; a display should prefer the name.
#[test]
fn a_reduced_gaussian_grid_is_named_as_well_as_shaped() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("parse");
    let gds = reader.messages[0].gds.as_ref().expect("GDS");

    // eccodes 2.34.1: gridName = N32, isOctahedral = 0.
    assert_eq!(gds.size_label().as_deref(), Some("N32"));

    let (ni, nj) = gds.dimensions().expect("a row-expanded raster shape");
    assert_eq!((ni, nj), (128, 64), "the widest row and the row count");
    let stored: u32 = gds
        .points_per_row()
        .expect("a reduced grid lists its row widths")
        .iter()
        .sum();
    assert!(
        stored < ni * nj,
        "the raster is larger than the field: {stored} points in a {ni}x{nj} shape"
    );
}

/// A reduced Gaussian field can be contoured once it is row-expanded (#503).
///
/// The sibling fixture `reduced_gg_n32.grib1` is a *constant* field, which is
/// the right shape for the reference-value decode path it pins but useless
/// here: a constant has no isolines, so contouring it returns nothing whether
/// the reduced path works or not. That left "contours draw on a GRIB1 reduced
/// grid" with no fixture that could fail, and the manual release plan asked a
/// tester to confirm it by eye against a picture that is empty by construction.
///
/// `reduced_gg_n32_smooth.grib1` is the same grid carrying a smooth analytic
/// field (see `fixtures/NOTICE.md`), so the isolines are real and their absence
/// is a bug rather than a property of the data.
#[test]
fn a_reduced_gaussian_field_contours_once_expanded() {
    use fieldglass_core::{contour_segments_global, expand_reduced_to_regular, nice_levels};

    const SMOOTH: &[u8] = include_bytes!("fixtures/reduced_gg_n32_smooth.grib1");

    let reader = Grib1Reader::from_bytes(SMOOTH.to_vec()).expect("fixture parses");
    let gds = reader.messages[0].gds.as_ref().expect("message has a GDS");
    assert_eq!(
        gds.points_per_row(),
        Some(PL.as_slice()),
        "same grid as its sibling"
    );

    let values = reader.decode_message_values(0).expect("decode succeeds");
    assert_eq!(values.len(), 6114, "sum(PL) points");

    // The field must vary, or this test would pass on the constant fixture too.
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    let (min, max) = present
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    assert!(max - min > 40.0, "field spans only {:.3} K", max - min);

    let (ni, nj) = gds.dimensions().expect("row-expanded raster shape");
    let raster = reader.decode_message_raster(0).expect("raster decode");
    assert_eq!(raster.len(), (ni * nj) as usize);
    // The entry point is the expansion, not a second implementation of it.
    assert_eq!(
        raster,
        expand_reduced_to_regular(&values, PL.as_slice(), ni as usize)
    );

    // Global west-to-east grid, so the seam-spanning entry point is the correct
    // one; a bounded march would break every isoline at the antimeridian.
    let levels = nice_levels(min, max, 10);
    let contours = contour_segments_global(&raster, ni as usize, nj as usize, &levels);

    let total: usize = contours.iter().map(|c| c.segments.len()).sum();
    assert!(
        total > 0,
        "a varying reduced Gaussian field produced no contour segments"
    );
    let drawn = contours.iter().filter(|c| !c.segments.is_empty()).count();
    assert!(
        drawn >= levels.len() / 2,
        "only {drawn} of {} levels drew anything",
        levels.len()
    );
}
