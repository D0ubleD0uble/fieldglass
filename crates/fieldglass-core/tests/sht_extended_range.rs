//! The exact point evaluation stays exact past where `f64`'s exponent runs out
//! (#637).
//!
//! A probe evaluates the full spherical-harmonic sum at one point, at whatever
//! truncation the file declares — up to T7999, the highest that exists. The
//! sectoral term `P̄_m^m` falls like `cos(φ)^m` and leaves `f64`'s range by
//! `m ≈ 1030` at 60°; the plain recurrence then amplifies a denormal into
//! garbage (at 60° it returns ~1e155 at T3000). The oracle here is the same
//! sum at T3000 in 80-bit `long double`, whose exponent holds every term, from
//! `tools/build_spectral_truncation_oracle.py`. The coefficients are generated
//! on both sides by the same 64-bit LCG, so nothing large is committed.

use fieldglass_core::sht::{coefficient_count, evaluate_spherical_harmonic};
use serde_json::Value;

const ORACLE: &str = include_str!("fixtures/sht_t3000_extended_range.oracle.json");

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
