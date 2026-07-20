//! Inverse spherical-harmonic transform — synthesize grid-point values from
//! the triangular spherical-harmonic coefficients stored by ECMWF/IFS spectral
//! GRIB fields (and their GRIB1 equivalents).
//!
//! Definitive reference — ECMWF's spectral representation
//! (<https://confluence.ecmwf.int/display/UDOC/How+to+access+the+data+values+of+a+spherical+harmonic+field+in+GRIB+-+ecCodes+GRIB+FAQ>):
//!
//! ```text
//! A(λ, μ) = Σ_{m=-T}^{T} Σ_{n=|m|}^{T} X_{n,m} P̄_n^m(μ) e^{i m λ},   μ = sin(lat)
//! ```
//!
//! with `X_{n,-m} = conj(X_{n,m}) / (-1)^m` and the normalisation
//! `(1/2) ∫_{-1}^{1} [P̄_n^m(μ)]² dμ = 1` (so `P̄_0^0 = 1`, `P̄_1^0 = √3·μ`).
//!
//! Coefficients are stored `m`-major: `Re(X₀₀), Im(X₀₀), Re(X₁₀), Im(X₁₀), …`,
//! `n` increasing from `m` to `T`, first `m = 0`, then `m = 1, …, T`. Collapsing
//! the ±m conjugate pairs for a real field yields the implemented form:
//!
//! ```text
//! A(λ, μ) = Σ_n X_{n,0} P̄_n^0(μ)
//!         + 2 Σ_{m≥1} Σ_n P̄_n^m(μ) [Re(X_{n,m}) cos(mλ) − Im(X_{n,m}) sin(mλ)]
//! ```
//!
//! eccodes cannot synthesise a grid from spectral coefficients, so correctness
//! is pinned by exact analytic single-coefficient cases derived straight from
//! the spec (e.g. `(0,0)` → constant `1`; `(1,0)` → `√3·sin(lat)`; `(1,1)` real
//! → `√6·cos(lat)cos(lon)`; `(2,0)` → `√5·(3μ²−1)/2`) — these pin the
//! normalisation and the ±m factor-of-2 with no library dependence — plus a
//! full-field oracle (`tools/build_grib2_spectral_render_oracle.py`) that an
//! independent pyshtools synthesis reproduces to ~5·10⁻⁸ once the ECMWF complex
//! coefficients are mapped to pyshtools real coefficients (m > 0 carries a `√2`
//! complex→real factor) and scaled by `√(4π)` for the physics-`ortho` vs
//! `(1/2)∫P̄²=1` normalisation difference.

use crate::error::FieldglassError;

/// Upper bound on the truncation `T` this transform will accept, bounding the
/// per-latitude `O(T²)` synthesis cost (the transform holds only an `O(T)`
/// Legendre column at a time, never a full table). `T` is derived from
/// attacker-controlled §3 fields, so it is capped up front. The largest
/// operational spectral truncation (~T3999) is far below this.
pub const MAX_TRUNCATION: u32 = 10_000;

/// Number of stored real values (real *and* imaginary parts) for a triangular
/// truncation `t`: `(t + 1)·(t + 2)`.
fn stored_len(t: u32) -> usize {
    let t = t as usize;
    (t + 1) * (t + 2)
}

/// Synthesize grid-point values from triangular spherical-harmonic coefficients.
///
/// `coefficients` is the flat `(real, imaginary)` `m`-major sequence (as decoded
/// from §7 by the spectral readers); `truncation` is `T` (`J = K = M`).
/// `latitudes_deg` / `longitudes_deg` give the target regular grid in degrees.
/// Returns `latitudes_deg.len() · longitudes_deg.len()` values, latitude-major
/// (outer) then longitude (inner) — the usual scan order.
pub fn synthesize_spherical_harmonic(
    coefficients: &[f64],
    truncation: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
) -> Result<Vec<f64>, FieldglassError> {
    if truncation > MAX_TRUNCATION {
        return Err(FieldglassError::Parse(format!(
            "spectral truncation T={truncation} exceeds the synthesis cap of {MAX_TRUNCATION}"
        )));
    }
    let expected = stored_len(truncation);
    if coefficients.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis: got {} coefficient values, expected (T+1)(T+2) = {expected} for T={truncation}",
            coefficients.len()
        )));
    }

    let t = truncation as usize;
    let nlon = longitudes_deg.len();
    let mut out = vec![0.0; latitudes_deg.len() * nlon];
    let lon_rad: Vec<f64> = longitudes_deg.iter().map(|l| l.to_radians()).collect();

    for (li, &lat) in latitudes_deg.iter().enumerate() {
        let mu = lat.to_radians().sin();
        let s = (1.0 - mu * mu).max(0.0).sqrt();
        let row = &mut out[li * nlon..(li + 1) * nlon];

        // Walk the columns m = 0..=T. `idx` reads the flat (real, imag) pairs in
        // storage order (n = m..=T within each m). `pmm` carries the sectoral
        // diagonal P̄_m^m from one column to the next. For each column we reduce
        // the coefficients against the Legendre column to a single complex
        // (re_m, im_m), then spread it over longitude.
        let mut idx = 0usize;
        let mut pmm = 1.0f64; // P̄_0^0
        for m in 0..=t {
            let mf = m as f64;
            // n = m (always present).
            let p_m = pmm;
            let mut re_m = p_m * coefficients[idx];
            let mut im_m = p_m * coefficients[idx + 1];
            idx += 2;

            if m < t {
                // n = m + 1: P̄_{m+1}^m = √(2m+3)·μ·P̄_m^m.
                let mut p_prev2 = p_m;
                let mut p_prev1 = (2.0 * mf + 3.0).sqrt() * mu * p_m;
                re_m += p_prev1 * coefficients[idx];
                im_m += p_prev1 * coefficients[idx + 1];
                idx += 2;

                // n = m + 2 ..= T via the two-term normalised recurrence.
                for n in (m + 2)..=t {
                    let nf = n as f64;
                    let a = ((2.0 * nf + 1.0) * (2.0 * nf - 1.0) / ((nf - mf) * (nf + mf))).sqrt();
                    let b = ((2.0 * nf + 1.0) * (nf + mf - 1.0) * (nf - mf - 1.0)
                        / ((2.0 * nf - 3.0) * (nf - mf) * (nf + mf)))
                        .sqrt();
                    let p_n = a * mu * p_prev1 - b * p_prev2;
                    re_m += p_n * coefficients[idx];
                    im_m += p_n * coefficients[idx + 1];
                    idx += 2;
                    p_prev2 = p_prev1;
                    p_prev1 = p_n;
                }
            }

            if m == 0 {
                // The m = 0 term is longitude-independent (imaginary part is
                // zero for a real field).
                for cell in row.iter_mut() {
                    *cell += re_m;
                }
            } else {
                // A += 2·[Re_m·cos(mλ) − Im_m·sin(mλ)].
                for (lo, cell) in row.iter_mut().enumerate() {
                    let ang = mf * lon_rad[lo];
                    *cell += 2.0 * (re_m * ang.cos() - im_m * ang.sin());
                }
            }

            // Advance the sectoral diagonal: P̄_{m+1}^{m+1} = √((2m+3)/(2m+2))·s·P̄_m^m.
            if m < t {
                pmm *= ((2.0 * mf + 3.0) / (2.0 * mf + 2.0)).sqrt() * s;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `(T+1)(T+2)`-length coefficient array with a single complex
    /// coefficient `(n, m)` set to `(re, im)`, in ECMWF m-major order.
    fn single(t: u32, target_n: u32, target_m: u32, re: f64, im: f64) -> Vec<f64> {
        let mut out = Vec::with_capacity(stored_len(t));
        for m in 0..=t {
            for n in m..=t {
                if n == target_n && m == target_m {
                    out.push(re);
                    out.push(im);
                } else {
                    out.push(0.0);
                    out.push(0.0);
                }
            }
        }
        out
    }

    const LATS: [f64; 3] = [90.0, 30.0, -45.0];
    const LONS: [f64; 4] = [0.0, 45.0, 120.0, 270.0];

    fn synth(coeffs: &[f64], t: u32) -> Vec<f64> {
        synthesize_spherical_harmonic(coeffs, t, &LATS, &LONS).expect("synthesize")
    }

    #[test]
    fn constant_from_00_coefficient() {
        // X_{0,0} = 1 → A = P̄_0^0 = 1 everywhere.
        let c = single(2, 0, 0, 1.0, 0.0);
        for v in synth(&c, 2) {
            assert!((v - 1.0).abs() < 1e-9, "constant field 1.0, got {v}");
        }
    }

    #[test]
    fn zonal_from_10_coefficient() {
        // X_{1,0} = 1 → A = √3·sin(lat), independent of longitude.
        let c = single(2, 1, 0, 1.0, 0.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            let want = 3f64.sqrt() * lat.to_radians().sin();
            for j in 0..LONS.len() {
                let v = got[i * LONS.len() + j];
                assert!((v - want).abs() < 1e-9, "√3·sin({lat})={want}, got {v}");
            }
        }
    }

    #[test]
    fn sectoral_from_11_coefficient() {
        // X_{1,1} = 1 (real) → A = 2·P̄_1^1·cos(λ) = √6·cos(lat)·cos(lon).
        let c = single(2, 1, 1, 1.0, 0.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            for (j, &lon) in LONS.iter().enumerate() {
                let want = 6f64.sqrt() * lat.to_radians().cos() * lon.to_radians().cos();
                let v = got[i * LONS.len() + j];
                assert!(
                    (v - want).abs() < 1e-9,
                    "√6·cos({lat})cos({lon})={want}, got {v}"
                );
            }
        }
    }

    #[test]
    fn imaginary_from_11_coefficient() {
        // X_{1,1} = i (imag) → A = 2·P̄_1^1·(−sin(λ)) = −√6·cos(lat)·sin(lon).
        let c = single(2, 1, 1, 0.0, 1.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            for (j, &lon) in LONS.iter().enumerate() {
                let want = -(6f64.sqrt()) * lat.to_radians().cos() * lon.to_radians().sin();
                let v = got[i * LONS.len() + j];
                assert!(
                    (v - want).abs() < 1e-9,
                    "−√6·cos({lat})sin({lon})={want}, got {v}"
                );
            }
        }
    }

    #[test]
    fn zonal_degree2_from_20_coefficient() {
        // X_{2,0} = 1 → A = P̄_2^0 = √5·(3μ²−1)/2. Exercises the two-term
        // recurrence (n = m+2) at m = 0, independent of the T63 oracle.
        let c = single(2, 2, 0, 1.0, 0.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            let mu = lat.to_radians().sin();
            let want = 5f64.sqrt() * (3.0 * mu * mu - 1.0) / 2.0;
            for j in 0..LONS.len() {
                assert!(
                    (got[i * LONS.len() + j] - want).abs() < 1e-9,
                    "P̄_2^0={want}"
                );
            }
        }
    }

    #[test]
    fn tesseral_degree2_from_21_coefficient() {
        // X_{2,1} = 1 (real) → A = 2·P̄_2^1·cos(λ). Under the ECMWF normalisation
        // (1/2)∫P̄²=1, P̄_2^1 = √(15/2)·μ·√(1−μ²). Exercises the first
        // off-diagonal (n = m+1) at m ≥ 1.
        let c = single(2, 2, 1, 1.0, 0.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            let mu = lat.to_radians().sin();
            let s = (1.0 - mu * mu).sqrt();
            for (j, &lon) in LONS.iter().enumerate() {
                let want = 2.0 * (15f64 / 2.0).sqrt() * mu * s * lon.to_radians().cos();
                assert!(
                    (got[i * LONS.len() + j] - want).abs() < 1e-9,
                    "2·P̄_2^1·cos(λ)={want}"
                );
            }
        }
    }

    #[test]
    fn sectoral_degree2_from_22_coefficient() {
        // X_{2,2} = 1 (real) → A = 2·P̄_2^2·cos(2λ). Under (1/2)∫P̄²=1,
        // P̄_2^2 = √(15/8)·(1−μ²). Exercises a second sectoral advance (m = 2).
        let c = single(2, 2, 2, 1.0, 0.0);
        let got = synth(&c, 2);
        for (i, &lat) in LATS.iter().enumerate() {
            let mu = lat.to_radians().sin();
            for (j, &lon) in LONS.iter().enumerate() {
                let want =
                    2.0 * (15f64 / 8.0).sqrt() * (1.0 - mu * mu) * (2.0 * lon.to_radians()).cos();
                assert!(
                    (got[i * LONS.len() + j] - want).abs() < 1e-9,
                    "2·P̄_2^2·cos(2λ)={want}"
                );
            }
        }
    }

    #[test]
    fn zonal_degree3_from_30_coefficient() {
        // X_{3,0} = 1 → A = P̄_3^0 = √7·(5μ³−3μ)/2. Runs the two-term recurrence
        // one step further (n = 3), with T = 3.
        let c = single(3, 3, 0, 1.0, 0.0);
        let got = synth(&c, 3);
        for (i, &lat) in LATS.iter().enumerate() {
            let mu = lat.to_radians().sin();
            let want = 7f64.sqrt() * (5.0 * mu * mu * mu - 3.0 * mu) / 2.0;
            for j in 0..LONS.len() {
                assert!(
                    (got[i * LONS.len() + j] - want).abs() < 1e-9,
                    "P̄_3^0={want}"
                );
            }
        }
    }

    #[test]
    fn rejects_wrong_coefficient_count() {
        assert!(synthesize_spherical_harmonic(&[0.0; 5], 2, &LATS, &LONS).is_err());
    }

    #[test]
    fn rejects_truncation_over_cap() {
        assert!(synthesize_spherical_harmonic(&[], MAX_TRUNCATION + 1, &LATS, &LONS).is_err());
    }
}
