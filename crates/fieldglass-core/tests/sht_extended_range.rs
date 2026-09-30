//! The transform stays correct past where plain `f64` does (#637).
//!
//! A probe's full-detail line evaluates the full spherical-harmonic sum at one
//! point, and a caller's grid may ask for it everywhere, at whatever truncation
//! the file declares — up to T7999, the highest that exists. The sectoral term
//! `P̄_m^m` falls like `cos(φ)^m` and leaves `f64`'s range by `m ≈ 1030` at 60°;
//! the plain recurrence then amplifies a denormal into garbage, and it is
//! measurably wrong from about T1820. Two oracles, both from
//! `tools/build_spectral_truncation_oracle.py`, both past that:
//!
//! * the same sum at T3000 in 80-bit `long double`, whose exponent holds every
//!   term;
//! * pyshtools at T2500, a different algorithm (Holmes–Featherstone scaling)
//!   inside its documented range, so an error the recurrence shares with its
//!   own widened copy cannot hide.
//!
//! Both run through the grid transform and the point evaluation, which share
//! one kernel. The coefficients are generated on both sides by the same 64-bit
//! LCG, so nothing large is committed.

use fieldglass_core::sht::{
    coefficient_count, evaluate_spherical_harmonic, synthesize_spherical_harmonic,
};
use serde_json::Value;

const ORACLE: &str = include_str!("fixtures/sht_t3000_extended_range.oracle.json");
const PYSHTOOLS: &str = include_str!("fixtures/sht_t2500_pyshtools.oracle.json");

/// The generator the oracle builder spells the same way (`lcg_coefficients`).
fn lcg_coefficients(t: u32) -> Vec<f64> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ u64::from(t);
    (0..coefficient_count(t).expect("truncation"))
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

#[test]
fn the_point_evaluation_matches_an_extended_precision_sum_at_t3000() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle parses");
    let t = u32::try_from(oracle["truncation"].as_u64().expect("truncation")).expect("u32");
    assert_eq!(t, 3000);
    let coefficients = lcg_coefficients(t);
    for point in oracle["points"].as_array().expect("points") {
        let lat = point["lat"].as_f64().expect("lat");
        let lon = point["lon"].as_f64().expect("lon");
        let want = point["value"].as_f64().expect("value");
        let got = evaluate_spherical_harmonic(&coefficients, t, lat, lon).expect("evaluate");
        let grid = synthesize_spherical_harmonic(&coefficients, t, &[lat], &[lon]).expect("grid");
        assert_eq!(got.to_bits(), grid[0].to_bits(), "one kernel");
        // 4.5 million terms of magnitude up to ~1 summed in a different order
        // and precision: agreement to 1e-9 of the field's scale is thousands of
        // times tighter than the failure it guards against.
        let tolerance = 1e-9 * want.abs().max(1.0);
        eprintln!("({lat}, {lon}): Δ = {:.3e}", (got - want).abs());
        assert!(
            (got - want).abs() < tolerance,
            "({lat}, {lon}): {got} vs long double {want} (Δ={})",
            (got - want).abs()
        );
    }
}

/// The sum as the plain `f64` column recurrence computes it, with no extended
/// range: the method every entry point used before #637, written out here so
/// the test can show the oracle below is one it fails.
fn plain_f64_sum(c: &[f64], t: u32, lat: f64, lon: f64) -> f64 {
    let t = t as usize;
    let mu = lat.to_radians().sin();
    let s = (1.0 - mu * mu).max(0.0).sqrt();
    let (mut value, mut idx, mut pmm) = (0.0, 0usize, 1.0f64);
    for m in 0..=t {
        let mf = m as f64;
        let (mut re, mut im) = (pmm * c[idx], pmm * c[idx + 1]);
        idx += 2;
        if m < t {
            let (mut p2, mut p1) = (pmm, (2.0 * mf + 3.0).sqrt() * mu * pmm);
            re += p1 * c[idx];
            im += p1 * c[idx + 1];
            idx += 2;
            for n in (m + 2)..=t {
                let nf = n as f64;
                let a = ((2.0 * nf + 1.0) * (2.0 * nf - 1.0) / ((nf - mf) * (nf + mf))).sqrt();
                let b = ((2.0 * nf + 1.0) * (nf + mf - 1.0) * (nf - mf - 1.0)
                    / ((2.0 * nf - 3.0) * (nf - mf) * (nf + mf)))
                    .sqrt();
                let p = a * mu * p1 - b * p2;
                re += p * c[idx];
                im += p * c[idx + 1];
                idx += 2;
                (p2, p1) = (p1, p);
            }
            pmm *= ((2.0 * mf + 3.0) / (2.0 * mf + 2.0)).sqrt() * s;
        }
        let ang = mf * lon.to_radians();
        value += if m == 0 {
            re
        } else {
            2.0 * (re * ang.cos() - im * ang.sin())
        };
    }
    value
}

/// T2500 against pyshtools, on a grid and at each of its points — and the
/// plain `f64` recurrence, on the same coefficients, is wrong there: by 3·10⁷⁵
/// at 60° and by 70 (on a value of 8) at 80°, where the transform is within
/// 4·10⁻⁹. So this oracle is one the old kernel fails and the range-safe one
/// passes.
#[test]
fn the_transform_matches_pyshtools_at_t2500_where_plain_f64_fails() {
    let oracle: Value = serde_json::from_str(PYSHTOOLS).expect("oracle parses");
    let t = u32::try_from(oracle["truncation"].as_u64().expect("truncation")).expect("u32");
    assert_eq!(t, 2500);
    let axis = |key: &str| -> Vec<f64> {
        oracle[key]
            .as_array()
            .expect(key)
            .iter()
            .map(|v| v.as_f64().expect("degrees"))
            .collect()
    };
    let (lats, lons) = (axis("lats"), axis("lons"));
    let want = axis("values");
    let coefficients = lcg_coefficients(t);
    let grid = synthesize_spherical_harmonic(&coefficients, t, &lats, &lons).expect("grid");
    assert_eq!(grid.len(), want.len());
    for (k, (&got, &want)) in grid.iter().zip(&want).enumerate() {
        let (lat, lon) = (lats[k / lons.len()], lons[k % lons.len()]);
        // Three million terms of magnitude up to ~1, summed by two different
        // algorithms: 1e-9 of the field's scale is far inside the failure below.
        let tolerance = 1e-9 * want.abs().max(1000.0);
        eprintln!("({lat}, {lon}): Δ = {:.3e}", (got - want).abs());
        assert!(
            (got - want).abs() < tolerance,
            "({lat}, {lon}): {got} vs pyshtools {want}"
        );
        let point = evaluate_spherical_harmonic(&coefficients, t, lat, lon).expect("point");
        assert_eq!(point.to_bits(), got.to_bits(), "({lat}, {lon}): one kernel");
    }
    for (lat, lon, k) in [(60.0, 120.0, 7), (80.0, 10.0, 3)] {
        assert_eq!((lats[k / lons.len()], lons[k % lons.len()]), (lat, lon));
        let plain = plain_f64_sum(&coefficients, t, lat, lon);
        let want = want[k];
        eprintln!("plain f64 at ({lat}, {lon}): {plain:e} against {want}");
        // Wrong by more than one unit of a field whose values run to the
        // thousands, where the transform above is within 1e-6 of it.
        assert!(
            !plain.is_finite() || (plain - want).abs() > 1.0,
            "({lat}, {lon}): plain f64 gave {plain}, oracle {want}: this case no longer \
             shows the failure it exists for"
        );
    }
    // And where nothing leaves `f64`'s range the plain recurrence agrees, so
    // the check above is about range and not about the reference's spelling.
    let equator = plain_f64_sum(&coefficients, t, 0.25, 120.0);
    let k = 4 * lons.len() + 1;
    assert_eq!((lats[k / lons.len()], lons[k % lons.len()]), (0.25, 120.0));
    assert!((equator - grid[k]).abs() < 1e-9 * equator.abs().max(1000.0));
}
