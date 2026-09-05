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
use crate::global_grid::{GlobalGrid, SYNTHESIS_NI, SYNTHESIS_NJ};

/// Upper bound on the truncation `T` this transform will accept. `T` is derived
/// from attacker-controlled §3 fields, so it is capped up front to bound both
/// the `O(T²)` per-latitude synthesis cost and the latitude-invariant tables the
/// synthesis precomputes: an `O(T²)` Legendre-recurrence coefficient table — the
/// same order as the input coefficient array, which is itself `(T+1)(T+2)`
/// values — and an `O(T·nlon)` longitude-phase table. The largest operational
/// spectral truncation (~T3999) is far below this cap.
pub const MAX_TRUNCATION: u32 = 10_000;

/// Choose the global regular lat/lon grid to synthesize a spectral field onto.
///
/// `2(T+1)` latitudes is the smallest grid that holds everything a truncation
/// `T` carries (≈ two grid points per wavenumber), and that used to be the grid
/// itself below the cap — which put a T63 field on 256×128, a postage-stamp
/// render whose PNG exported 382 pixels wide.
///
/// But a spectral message is a band-limited function, not a sampled grid: its
/// coefficients can be evaluated anywhere, so any grid at or above the minimum
/// reproduces the same field exactly, and a finer one is a sharper picture of
/// it rather than interpolation between samples. Every field is therefore
/// synthesized at [`SYNTHESIS_STEP_DEG`](crate::global_grid::SYNTHESIS_STEP_DEG)
/// — which is also the ceiling a large truncation was already downsampled to,
/// so `T ≥ 180` is unchanged and the cost of the densest case is unchanged with
/// it.
///
/// Ignoring the truncation is what lets two spectral fields at different
/// truncations land on the same raster and genuinely combine
/// (`docs/architecture/planned/03-composition.md`). The parameter stays in the
/// signature because that is the question a caller is asking.
pub fn spectral_render_dims(_truncation: u32) -> (usize, usize) {
    (SYNTHESIS_NI, SYNTHESIS_NJ)
}

/// [`spectral_render_dims`] as the grid itself, which is what the synthesis
/// call and the render meta both want.
pub fn spectral_render_grid(truncation: u32) -> GlobalGrid {
    GlobalGrid::from(spectral_render_dims(truncation))
}

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

    // ── Latitude-invariant tables, hoisted out of the latitude loop ──────────
    // The longitude phases and the Legendre-recurrence coefficients depend only
    // on the column `m` (and the longitude, resp. `n`), never on `μ` — so
    // recomputing them per latitude row wasted ~nlat× the trig and sqrt work
    // (hundreds of millions of calls at high truncation). Each table entry is
    // built with the identical expression it replaces, so the synthesis stays
    // bit-for-bit unchanged; only the arithmetic count drops. See
    // `MAX_TRUNCATION` for the table sizes.

    // cos(mλ), sin(mλ) for every column `m` and longitude, laid out `m`-major.
    let mut cos_tab = vec![0.0f64; (t + 1) * nlon];
    let mut sin_tab = vec![0.0f64; (t + 1) * nlon];
    for m in 0..=t {
        let mf = m as f64;
        let base = m * nlon;
        for (lo, &lr) in lon_rad.iter().enumerate() {
            let ang = mf * lr;
            cos_tab[base + lo] = ang.cos();
            sin_tab[base + lo] = ang.sin();
        }
    }

    // The two-term normalised recurrence coefficients `a`, `b` for every
    // `(n, m)` with `n ≥ m + 2`, pushed in the same `m`-major / `n`-ascending
    // order the latitude loop reads them back (via a running cursor), so
    // addressing them needs no `(n, m)` arithmetic. `T(T−1)/2` `(f64, f64)`.
    let mut ab = Vec::with_capacity(t.saturating_sub(1) * t / 2);
    for m in 0..=t {
        let mf = m as f64;
        for n in (m + 2)..=t {
            let nf = n as f64;
            let a = ((2.0 * nf + 1.0) * (2.0 * nf - 1.0) / ((nf - mf) * (nf + mf))).sqrt();
            let b = ((2.0 * nf + 1.0) * (nf + mf - 1.0) * (nf - mf - 1.0)
                / ((2.0 * nf - 3.0) * (nf - mf) * (nf + mf)))
                .sqrt();
            ab.push((a, b));
        }
    }

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
        let mut abk = 0usize; // read cursor into `ab`, in lockstep with its build order
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

                // n = m + 2 ..= T via the two-term normalised recurrence, its
                // latitude-invariant coefficients read from the precomputed table.
                for _ in (m + 2)..=t {
                    let (a, b) = ab[abk];
                    abk += 1;
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
                // A += 2·[Re_m·cos(mλ) − Im_m·sin(mλ)], phases from the table.
                let base = m * nlon;
                for (lo, cell) in row.iter_mut().enumerate() {
                    *cell += 2.0 * (re_m * cos_tab[base + lo] - im_m * sin_tab[base + lo]);
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

    #[test]
    fn the_synthesis_grid_is_half_degree_for_every_truncation() {
        // The floor and the ceiling are the same grid, so a small truncation is
        // synthesized as densely as a large one. T63 used to land on 256×128 —
        // faithful to the truncation, but a postage-stamp picture of it.
        for truncation in [2_u32, 63, 106, 179, 180, 639, 1279] {
            assert_eq!(
                spectral_render_dims(truncation),
                (720, 361),
                "truncation {truncation}"
            );
            assert_eq!(
                spectral_render_grid(truncation),
                GlobalGrid::FINEST,
                "truncation {truncation}"
            );
        }
        // The grid the coordinates are built on agrees with the declared dims,
        // pole to pole and without a duplicated wrap column.
        let (lats, lons) = spectral_render_grid(63).axes();
        assert_eq!((lons.len(), lats.len()), (720, 361));
        assert_eq!((lats[0], lats[lats.len() - 1]), (90.0, -90.0));
        assert_eq!(lons[0], 0.0);
        assert!(lons[lons.len() - 1] < 360.0);
    }

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
