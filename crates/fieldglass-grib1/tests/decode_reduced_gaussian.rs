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
