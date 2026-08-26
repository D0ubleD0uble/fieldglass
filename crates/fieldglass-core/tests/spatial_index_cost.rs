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

/// A thin grid costs about what a compact one of the same size costs.
///
/// A satellite swath is a ribbon — 96 cells across, hundreds long — which in
/// three dimensions is nearly a curve. The splitting planes barely separate it,
/// so the k-d tree's prune test holds almost everywhere and both branches get
/// searched. Queries *off* the ribbon are the worst case, and on a world
/// projection nearly every output pixel is one: warping a 73k-cell swath took
/// 6.5 seconds before `nearest` seeded its search with the cutoff instead of
/// infinity.
///
/// Asserted as a ratio against a compact grid of the same cell count, on the
/// same machine in the same run, because an absolute time is a machine-speed
/// assertion. A degenerate search shows up as hundreds of times slower, so the
/// ceiling has room for a noisy runner and still fails the thing it is for.
#[test]
fn a_thin_grid_is_not_pathologically_slower_than_a_compact_one() {
    const CELLS: u32 = 96 * 768;

    // Off-grid queries: the case that used to walk the whole tree.
    let probes: Vec<(f64, f64)> = (0..20_000)
        .map(|k| {
            let t = k as f64 / 20_000.0;
            (-90.0 + 180.0 * t, -180.0 + 360.0 * ((t * 7.3) % 1.0))
        })
        .collect();

    let time = |ni: u32, nj: u32| {
        let (lats, lons) = bent(ni, nj);
        let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
        let start = Instant::now();
        let mut found = 0usize;
        for &(lat, lon) in &probes {
            if ix.nearest(lat, lon).is_some() {
                found += 1;
            }
        }
        (start.elapsed(), found)
    };

    let (thin, thin_hits) = time(96, CELLS / 96);
    let (compact, compact_hits) = time(256, CELLS / 256);
    println!("thin {thin:?} ({thin_hits} hits), compact {compact:?} ({compact_hits} hits)");

    let ratio = thin.as_secs_f64() / compact.as_secs_f64().max(1e-9);
    assert!(
        ratio < 25.0,
        "a thin grid took {ratio:.1}x what a compact one of the same cell count \
         took ({thin:?} vs {compact:?}) — the search is degenerating on its shape"
    );
}
