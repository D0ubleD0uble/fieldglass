//! Build cost and memory for the lookup index, at the scale a real grid reaches.
//!
//! #437 asks for the build to be "bounded and measured" at about a million
//! cells — an ORCA025 tripolar ocean grid is 1442 x 1021, so this is the actual
//! working size, not a stress test.
//!
//! Wall-clock alone would be a machine-speed assertion, and flaky on a shared
//! runner. What is asserted instead is the *shape*: quadrupling the cell count
//! must not quadruple-and-then-some the time. An O(n log n) build predicts about
//! 4.3x, an accidental O(n^2) predicts 16x, and the ceiling sits between them
//! with room for a noisy runner. The absolute numbers are printed with
//! `--nocapture` for whoever wants them.

use fieldglass_core::spatial_index::SpatialIndex;
use std::time::Instant;

/// A curvilinear-ish grid: a lattice bent enough that the centres are not
/// axis-aligned, so the tree does real partitioning work rather than splitting
/// an already-sorted list.
fn bent(ni: u32, nj: u32) -> (Vec<f64>, Vec<f64>) {
    let n = (ni * nj) as usize;
    let mut lats = Vec::with_capacity(n);
    let mut lons = Vec::with_capacity(n);
    for j in 0..nj {
        let fj = j as f64 / nj as f64;
        for i in 0..ni {
            let fi = i as f64 / ni as f64;
            // A shear plus a ripple: no row shares a latitude, no column a
            // longitude.
            lats.push(-80.0 + 160.0 * fj + 4.0 * (fi * 12.0).sin());
            lons.push(-180.0 + 360.0 * fi + 6.0 * (fj * 9.0).cos());
        }
    }
    (lats, lons)
}

fn build_millis(ni: u32, nj: u32) -> (u128, usize) {
    let (lats, lons) = bent(ni, nj);
    let t = Instant::now();
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    (t.elapsed().as_millis(), ix.len())
}

#[test]
fn build_cost_scales_sub_quadratically() {
    // 500x500 = 250k, then 1000x1000 = 1M: a 4x step.
    let (small_ms, small_n) = build_millis(500, 500);
    let (big_ms, big_n) = build_millis(1000, 1000);
    assert_eq!(small_n, 250_000);
    assert_eq!(big_n, 1_000_000);

    println!("spatial index build: {small_n} cells in {small_ms} ms, {big_n} cells in {big_ms} ms");

    // Memory, stated rather than measured: 24 bytes of unit vector plus 4 bytes
    // of tree index per cell, so about 28 MB at a million cells. The
    // coordinates the caller already holds are not copied.
    println!(
        "spatial index memory at {big_n} cells: ~{} MB",
        big_n * 28 / 1_000_000
    );

    // Guard against a hang or an accidental quadratic, without asserting a
    // machine speed. `max(1)` keeps a sub-millisecond small case from making
    // the ratio meaningless.
    let ratio = big_ms as f64 / small_ms.max(1) as f64;
    assert!(
        ratio < 10.0,
        "build went from {small_ms} ms at {small_n} cells to {big_ms} ms at {big_n} \
         (x{ratio:.1}); O(n log n) predicts about 4.3x and O(n^2) about 16x"
    );
}

#[test]
fn a_million_cell_index_answers_quickly() {
    let (ni, nj) = (1000, 1000);
    let (lats, lons) = bent(ni, nj);
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");

    // 10k queries spread over the grid. At O(log n) this is trivial; if the
    // search degenerated to a scan it would be 1e10 distance computations and
    // would not finish.
    let t = Instant::now();
    let mut found = 0;
    for k in 0..10_000 {
        let f = k as f64 / 10_000.0;
        if ix.nearest(-70.0 + 140.0 * f, -170.0 + 340.0 * f).is_some() {
            found += 1;
        }
    }
    let ms = t.elapsed().as_millis();
    println!(
        "spatial index: 10000 queries against {} cells in {ms} ms",
        ix.len()
    );
    assert!(
        found > 9_000,
        "most query points are inside the grid: {found}"
    );
    assert!(
        ms < 10_000,
        "10k queries took {ms} ms — the search is not pruning"
    );
}
