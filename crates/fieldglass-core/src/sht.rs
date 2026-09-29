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
//!
//! # A map and a point are different questions (#637)
//!
//! A map is synthesised only up to the wavenumbers its grid can carry
//! ([`grid_band_limit`]; T359 on the 0.5° grid every host renders onto), by
//! [`synthesize_band_limited`]: past that, the grid's points alias the rest onto
//! what they can show. The field a file declares above the limit is therefore
//! drawn band-limited, and [`SpectralTruncation`] is the label that says so. A
//! point is evaluated in full by [`evaluate_spherical_harmonic`], which carries
//! extended-range arithmetic because the plain recurrence runs out of `f64`
//! exponent above about T1927. Two more oracles pin these: a T383 fixture per
//! GRIB edition whose band-limited map and full point sums pyshtools computes
//! (`tools/build_spectral_truncation_oracle.py`), and a T3000 point sum in
//! 80-bit `long double` from the same script, past where pyshtools' own
//! Legendre routine is documented to hold.

use crate::error::FieldglassError;
use crate::global_grid::{GlobalGrid, SYNTHESIS_NI, SYNTHESIS_NJ};

/// Upper bound on the truncation `T` any spectral decode or synthesis will
/// accept — the ceiling on the coefficient array, which is what a declared
/// truncation turns into (#631).
///
/// `T` is read from attacker-controlled §3 fields and the array it sizes is
/// `(T+1)(T+2)` `f64`, so it is capped before anything is allocated. The
/// grid transform's tables are sized by the band limit it runs at, not by this
/// (see [`MAX_SYNTHESIS_CELLS`]), and the point evaluation has none.
///
/// # Why 8192
///
/// The cap refuses what does not exist rather than rationing what does.
/// `T7999` is the highest spectral truncation any model has produced — ECMWF's
/// 2.5 km IFS forecasts, made feasible by the fast Legendre transform of Wedi
/// et al. (2013) — so 8192 is the smallest power of two above the real
/// ceiling. (An earlier revision of this comment said `~T3999`, which is the
/// 5 km configuration, not the highest one.) Below the cap the cost is the
/// data's own; above it, no encoder has ever written a message, so the
/// declaration is corruption or attack.
///
/// # What the cap is worth
///
/// The *full* sum on the pinned 720×361 grid ([`spectral_render_grid`]) —
/// what a map cost until #637 — measured in a release build, one message:
///
/// | `T` | stored values | full-sum synthesis |
/// |---:|---:|---:|
/// | 63 | 4,160 | 9.8 ms |
/// | 250 | 63,252 | 47 ms |
/// | 500 | 251,502 | 138 ms |
/// | 1,000 | 1,003,002 | 517 ms |
/// | 2,000 | 4,006,002 | 1.94 s |
/// | 8,192 (cap) | 67,133,442 | 27.0 s |
///
/// At the cap that transform's own peak resident set was 1.03 GB, measured: the
/// coefficient array and the recurrence table are 537 MB each and overlap in
/// time. Past about T1927 it was also wrong: the recurrence leaves `f64`'s
/// exponent range at mid and high latitudes (see
/// [`evaluate_spherical_harmonic`]), and on a synthetic T7999 field 38% of the
/// map's cells came back non-finite or absurd.
///
/// A map now synthesises only the T359 the grid carries
/// ([`spectral_render_band_limit`]), so above T359 its transform costs the same
/// at every truncation — the budgets' 140,356,800 units and 907,562 values —
/// and what is left of the cap's cost is the coefficient array itself. On that
/// T7999 field, measured on a heavily loaded machine: the whole decode and map
/// peaks at 635 MB (the 512 MB array and the file) against 1.22 GB, the
/// transform takes 0.27 to 0.57 s against 121 s on the same machine, and the
/// exact probe of one point 0.4 to 0.6 s. Decoding the array, about a second,
/// is now most of it.
///
/// The previous cap of `10_000` bounded correctness, not cost: it admitted a
/// 1.6 GB transient and a ~38 s synthesis. It was also the *only* such bound —
/// `fieldglass-grib1`'s spectral decode had none at all, and its `J` is a bare
/// `u16`, so a 112-byte message declaring `J = K = M = 65535` sized a `Vec` at
/// 34 GB before the short data section was ever noticed. Measured on the
/// zero-bit-width (constant-field) path, where no §7 bit budget constrains the
/// count: `T = 10000` turned 112 bytes into 763 MB, an amplification of
/// 7,145,000×. `fieldglass-grib2` capped the same shape at 200 M values
/// (1.6 GB). Both now share this ceiling, via [`coefficient_count`].
///
/// The check has to be here and at each decoder rather than at the
/// `fieldglass::Session` seam: the decoder allocates the coefficient array
/// strictly before the transform or the session sees a value, and
/// `fieldglass-napi` calls `Grib{1,2}Reader::synthesize_message_global`
/// directly, as may any consumer of the published format crates.
pub const MAX_TRUNCATION: u32 = 8_192;

/// The largest coefficient array any spectral family may declare — the value
/// count at [`MAX_TRUNCATION`], `(T+1)(T+2)` = 67,133,442 `f64` (537 MB).
///
/// Named separately because the spherical-harmonic families reach it through a
/// triangular truncation ([`coefficient_count`]) while GRIB2's bi-Fourier
/// packing counts `4·NI·NJ` over a rectangle, ellipse or diamond. One envelope
/// for all of them keeps a single number to reason about rather than one per
/// packing template.
pub const MAX_COEFFICIENTS: usize = (MAX_TRUNCATION as usize + 1) * (MAX_TRUNCATION as usize + 2);

/// Stored value count (real *and* imaginary parts) of a triangular truncation
/// `t`: `(t + 1)·(t + 2)`, bounded by [`MAX_TRUNCATION`].
///
/// The one place a declared truncation becomes an allocation size. Both GRIB
/// editions' spectral decoders and [`synthesize_spherical_harmonic`] itself
/// call it, so a truncation that would size a `Vec` past the ceiling is refused
/// once, in one place, with one message — rather than by a per-crate constant
/// that one crate can be missing entirely (which is what happened: see
/// [`MAX_TRUNCATION`]).
///
/// # Errors
///
/// [`FieldglassError::Parse`] when `t` exceeds [`MAX_TRUNCATION`], which is the
/// same error every other size ceiling in the stack reports through, so a host
/// sees one `code` for "this file declares more than this build will allocate".
pub fn coefficient_count(truncation: u32) -> Result<usize, FieldglassError> {
    if truncation > MAX_TRUNCATION {
        return Err(FieldglassError::Parse(format!(
            "spectral truncation T={truncation} exceeds the cap of {MAX_TRUNCATION}"
        )));
    }
    let t = truncation as usize;
    // `MAX_TRUNCATION` bounds the product at 67,133,442, so it fits `usize` on
    // a 32-bit target and the multiply cannot overflow.
    Ok((t + 1) * (t + 2))
}

/// The inner-loop count [`synthesize_spherical_harmonic`] will run for a
/// truncation and a target grid, against which [`MAX_SYNTHESIS_WORK`] is the
/// ceiling: one unit per `(latitude, column)` pair times the work that column
/// costs — the `n`-reduction over at most `T+1` coefficients, then the spread
/// over `nlon` longitudes.
///
/// This models the measured cost; `output points × coefficients` does not.
/// Across T63 → T2000 on the pinned grid the time per unit of *this* metric
/// moves 0.54 → 0.99 ns (1.8×, the recurrence table falling out of cache),
/// while the time per unit of `output points × coefficients` moves 9.1 → 1.9 ps
/// (4.9×, and in the direction that under-charges the expensive end).
///
/// Saturating rather than wrapping: the slice lengths are caller-supplied and
/// only ever compared against a ceiling, so a product too large to represent
/// must read as "over budget", not wrap to a small number.
#[must_use]
pub const fn synthesis_work(truncation: u32, nlat: usize, nlon: usize) -> u64 {
    let columns = truncation as u64 + 1;
    (nlat as u64).saturating_mul(columns.saturating_mul(columns.saturating_add(nlon as u64)))
}

/// Ceiling on [`synthesis_work`] — the transform's running cost, across both
/// the axes it has.
///
/// [`MAX_TRUNCATION`] bounds the truncation but says nothing about the target
/// grid, and `latitudes_deg` / `longitudes_deg` are caller-supplied slices that
/// nothing else bounds: the output `Vec` alone is `nlat · nlon`. The two
/// multiply, so capping them separately would still admit a legitimate
/// truncation on an enormous grid.
///
/// Defined as the cost of rendering the largest truncation that exists: its map
/// on the grid every host renders onto ([`spectral_render_grid`]), which
/// synthesises only the wavenumbers that grid can carry
/// ([`spectral_render_band_limit`], T359). That is 140,356,800 units, ~0.1 s,
/// rather than a magic number, so it cannot drift from [`MAX_TRUNCATION`] or
/// from the grid and needs no separate justification.
///
/// It was the *full* sum at the cap on the same grid until #637 — 26,361,739,449
/// units and ~27 s — because the map evaluated every wavenumber the file
/// declared. The map no longer does, so the budget fell with it by 188×. A
/// caller-supplied grid is still admitted exactly as far as it costs no more
/// than the map does, which now means that an exact full sum onto a whole
/// grid at a high truncation is refused; the one exact evaluation the project
/// runs at every truncation is a single point, and
/// [`evaluate_spherical_harmonic`] does that with no tables and no budget.
pub const MAX_SYNTHESIS_WORK: u64 = {
    let (ni, nj) = spectral_render_dims(MAX_TRUNCATION);
    synthesis_work(spectral_render_band_limit(MAX_TRUNCATION), nj, ni)
};

/// Every `f64`-sized value [`synthesize_spherical_harmonic`] allocates for a
/// truncation and a target grid, against which [`MAX_SYNTHESIS_CELLS`] is the
/// ceiling.
///
/// Space is a different function of the same two inputs than time is, which is
/// why [`synthesis_work`] does not bound it. Work charges `nlat` for every
/// column-longitude pair; the tables are built once and do not, so a grid with
/// one latitude and a million longitudes is cheap by the work metric and
/// allocates a phase table of `2(T+1)·nlon`. In the other direction `T = 0`
/// makes every column term vanish while the output raster `nlat·nlon` remains.
/// The terms, in the order the function allocates them:
///
/// | table | values |
/// |---|---|
/// | output raster | `nlat·nlon` |
/// | longitudes in radians | `nlon` |
/// | `cos(mλ)` and `sin(mλ)` | `2(T+1)·nlon` |
/// | Legendre recurrence `(a, b)` | `T(T−1)` |
///
/// The coefficient array is the caller's and is bounded separately, by
/// [`MAX_TRUNCATION`] at each decoder.
///
/// Saturating, for the reason [`synthesis_work`] gives.
#[must_use]
pub const fn synthesis_cells(truncation: u32, nlat: usize, nlon: usize) -> u64 {
    let t = truncation as u64;
    let (nlat, nlon) = (nlat as u64, nlon as u64);
    let raster = nlat.saturating_mul(nlon);
    let phases = nlon.saturating_mul(2u64.saturating_mul(t + 1).saturating_add(1));
    let recurrence = t.saturating_mul(t.saturating_sub(1));
    raster.saturating_add(phases).saturating_add(recurrence)
}

/// Ceiling on [`synthesis_cells`] — the transform's own peak allocation.
///
/// Defined the same way [`MAX_SYNTHESIS_WORK`] is, from the map of the largest
/// truncation that exists on the grid every host renders onto: 907,562 values,
/// 7.3 MB (79,159,232 values and 633 MB before the map was band-limited, #637).
/// Holding both to the same worst case means one configuration decides both
/// budgets, and a caller-supplied grid is admitted exactly as far as it costs
/// no more than the case the project has already accepted.
///
/// The output raster is a term of it, so a grid with more points than the
/// pinned one is refused whatever its truncation.
pub const MAX_SYNTHESIS_CELLS: u64 = {
    let (ni, nj) = spectral_render_dims(MAX_TRUNCATION);
    synthesis_cells(spectral_render_band_limit(MAX_TRUNCATION), nj, ni)
};

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
#[must_use]
pub const fn spectral_render_dims(_truncation: u32) -> (usize, usize) {
    (SYNTHESIS_NI, SYNTHESIS_NJ)
}

/// [`spectral_render_dims`] as the grid itself, which is what the synthesis
/// call and the render meta both want.
pub fn spectral_render_grid(truncation: u32) -> GlobalGrid {
    GlobalGrid::from(spectral_render_dims(truncation))
}

/// The highest total wavenumber a global lat/lon grid of `ni × nj` points can
/// carry, in the [`crate::global_grid`] convention: longitudes `0 … 360 − Δ`
/// with no wrap column, latitudes pole to pole with both poles on the grid.
///
/// Two limits, one per axis, and the grid carries the smaller:
///
/// - **Longitude.** `ni` points round a circle sample `e^{imλ}` without
///   aliasing while `m < ni/2`, so the highest order is `ni/2 − 1`. A
///   truncation `T` needs `2(T + 1)` longitudes, which is the rule
///   [`spectral_render_dims`] has always stated the other way round.
/// - **Latitude.** `P̄_n^m(sin φ)` is a trigonometric polynomial of degree `n`
///   in colatitude, and `nj` points from pole to pole space colatitude at
///   `π/(nj − 1)`: `2(nj − 1)` samples per full turn, so `n < nj − 1`.
///
/// On the pinned 720 × 361 grid both give 359.
///
/// Evaluating the full sum at those points instead of truncating is not a
/// sharper picture: wavenumbers past the limit alias onto the ones below it, so
/// the map would carry structure the grid cannot hold, folded into structure it
/// can. Band-limiting to this is what a grid of this resolution shows of the
/// field.
///
/// Saturating: a grid too small to carry even `T = 0` answers 0.
#[must_use]
pub const fn grid_band_limit(ni: usize, nj: usize) -> u32 {
    let by_longitude = (ni / 2).saturating_sub(1);
    let by_latitude = nj.saturating_sub(2);
    let limit = if by_longitude < by_latitude {
        by_longitude
    } else {
        by_latitude
    };
    if limit > u32::MAX as usize {
        u32::MAX
    } else {
        limit as u32
    }
}

/// The truncation a spectral field's **map** is synthesised at: its declared
/// `truncation`, or the most [`spectral_render_grid`] can carry
/// ([`grid_band_limit`]) when that is lower — T359 on the pinned grid (#637).
///
/// Derived from [`spectral_render_dims`] rather than stated, so the limit moves
/// with the grid if the grid ever does. A truncation at or below it is
/// synthesised in full, exactly as before; above it, the map is the field
/// band-limited to what the raster can show, and [`SpectralTruncation`] is the
/// label that says so.
#[must_use]
pub const fn spectral_render_band_limit(truncation: u32) -> u32 {
    let (ni, nj) = spectral_render_dims(truncation);
    let limit = grid_band_limit(ni, nj);
    if truncation < limit {
        truncation
    } else {
        limit
    }
}

/// That a spectral field's map shows fewer wavenumbers than the file holds:
/// the truncation it declares, and the one it was synthesised at (#637).
///
/// **A smoothed field is never shown silently.** The map of a field whose
/// declared truncation is past [`spectral_render_band_limit`] is band-limited
/// to what the raster can carry, which is a different — smoother — field from
/// the one the file holds. This is the fact a host shows beside it ("shown at
/// T359 of T7999"), carried as data so every host sees it, not only the one
/// that thought to write the caption.
///
/// Built only where truncation removes something: see [`Self::of_render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpectralTruncation {
    /// The truncation `T` the message declares — what its coefficients hold.
    pub declared: u32,
    /// The truncation the map was synthesised at, always below `declared`.
    pub truncated_to: u32,
}

impl SpectralTruncation {
    /// The label a map of a field declaring `declared` needs, or `None` when
    /// the map carries every wavenumber the field has.
    ///
    /// Asked of the declaration alone, so a host can know before decoding —
    /// which is when a message list is drawn.
    #[must_use]
    pub const fn of_render(declared: u32) -> Option<Self> {
        let truncated_to = spectral_render_band_limit(declared);
        if truncated_to < declared {
            Some(Self {
                declared,
                truncated_to,
            })
        } else {
            None
        }
    }
}

/// Synthesize grid-point values from triangular spherical-harmonic coefficients.
///
/// `coefficients` is the flat `(real, imaginary)` `m`-major sequence (as decoded
/// from §7 by the spectral readers); `truncation` is `T` (`J = K = M`).
/// `latitudes_deg` / `longitudes_deg` give the target regular grid in degrees.
/// Returns `latitudes_deg.len() · longitudes_deg.len()` values, latitude-major
/// (outer) then longitude (inner) — the usual scan order.
///
/// The full sum over every `n ≤ T`: exact at every point, and
/// [`synthesize_band_limited`] with the band limit at `T`. A map is the
/// band-limited form (#637); this is for a caller who wants the file's own
/// values on a grid of their own, and who pays the full sum for it.
///
/// # Errors
///
/// [`FieldglassError::Parse`] when the truncation is past [`MAX_TRUNCATION`],
/// when `coefficients` is not `(T+1)(T+2)` long, or when the truncation and the
/// target grid together exceed [`MAX_SYNTHESIS_WORK`] or
/// [`MAX_SYNTHESIS_CELLS`].
pub fn synthesize_spherical_harmonic(
    coefficients: &[f64],
    truncation: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
) -> Result<Vec<f64>, FieldglassError> {
    synthesize_band_limited(
        coefficients,
        truncation,
        truncation,
        latitudes_deg,
        longitudes_deg,
    )
}

/// [`synthesize_spherical_harmonic`] over only the wavenumbers `n ≤ band_limit`
/// — the field triangularly truncated to `min(band_limit, truncation)`.
///
/// This is how a map is synthesised (#637): a grid resolves only so many
/// wavenumbers ([`grid_band_limit`]), and evaluating the rest at its points
/// aliases them onto the ones it can show rather than adding detail. The
/// coefficients stay the file's `(T+1)(T+2)`, read in place; the ones past the
/// band limit are skipped, not copied out. A `band_limit` at or past
/// `truncation` is the full sum, bit for bit.
///
/// The cost is the band limit's rather than the file's, and so are both
/// budgets: the recurrence table is `L(L−1)/2` pairs and the phase tables
/// `(L+1)·nlon` for `L = min(band_limit, truncation)`.
///
/// Plain `f64` is enough here, and the budgets are why. The sectoral term
/// `P̄_m^m ≈ cos(φ)^m` leaves `f64`'s range once `m·(−ln cos φ) > 709`, and a
/// column that starts there is only amplified into view if it climbs back to
/// order one before `n = L`, which it does past `n ≈ m / cos φ`. Both at once
/// need `L·cos φ·(−ln cos φ) > 709`, and `c·(−ln c)` never exceeds `1/e`, so
/// every `L < 709·e ≈ 1927` is safe at every latitude. The allocation budget
/// holds `L(L−1)` to 907,562, so nothing it admits is past T953.
/// [`evaluate_spherical_harmonic`], which runs at any truncation, carries the
/// extended range this does not need.
///
/// # Errors
///
/// As [`synthesize_spherical_harmonic`], with the budgets charged at `L`.
pub fn synthesize_band_limited(
    coefficients: &[f64],
    truncation: u32,
    band_limit: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
) -> Result<Vec<f64>, FieldglassError> {
    let expected = coefficient_count(truncation)?;
    let limit = band_limit.min(truncation);
    let (nlat, nlon) = (latitudes_deg.len(), longitudes_deg.len());
    // Both budgets, before the output `Vec` or any table is allocated. The
    // truncation is bounded above; the target grid is not, and the two multiply
    // — differently for time than for space, which is why there are two.
    let work = synthesis_work(limit, nlat, nlon);
    if work > MAX_SYNTHESIS_WORK {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis of T={limit} onto {nlon} × {nlat} points costs {work} \
             coefficient evaluations, over the budget of {MAX_SYNTHESIS_WORK}"
        )));
    }
    let cells = synthesis_cells(limit, nlat, nlon);
    if cells > MAX_SYNTHESIS_CELLS {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis of T={limit} onto {nlon} × {nlat} points allocates {cells} \
             values, over the budget of {MAX_SYNTHESIS_CELLS}"
        )));
    }
    if coefficients.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis: got {} coefficient values, expected (T+1)(T+2) = {expected} for T={truncation}",
            coefficients.len()
        )));
    }

    let t = truncation as usize;
    let l = limit as usize;
    // Every length below is a term of `cells`, which is now bounded by
    // `MAX_SYNTHESIS_CELLS` — 907,562 — so each product fits a 32-bit `usize`
    // and none of these multiplies can overflow. That bound is what makes them
    // exact, not an assumption about how large a caller's slices could be.
    let mut out = vec![0.0; nlat * nlon];
    let lon_rad: Vec<f64> = longitudes_deg.iter().map(|l| l.to_radians()).collect();

    // ── Latitude-invariant tables, hoisted out of the latitude loop ──────────
    // The longitude phases and the Legendre-recurrence coefficients depend only
    // on the column `m` (and the longitude, resp. `n`), never on `μ` — so
    // recomputing them per latitude row wasted ~nlat× the trig and sqrt work
    // (hundreds of millions of calls at high truncation). Each table entry is
    // built with the identical expression it replaces, so the synthesis stays
    // bit-for-bit unchanged; only the arithmetic count drops. Both are sized by
    // the band limit, not the file's truncation.

    // cos(mλ), sin(mλ) for every column `m` and longitude, laid out `m`-major.
    let mut cos_tab = vec![0.0f64; (l + 1) * nlon];
    let mut sin_tab = vec![0.0f64; (l + 1) * nlon];
    for m in 0..=l {
        let mf = m as f64;
        let base = m * nlon;
        for (lo, &lr) in lon_rad.iter().enumerate() {
            let ang = mf * lr;
            cos_tab[base + lo] = ang.cos();
            sin_tab[base + lo] = ang.sin();
        }
    }

    // The two-term normalised recurrence coefficients `a`, `b` for every
    // `(n, m)` with `m + 2 ≤ n ≤ L`, pushed in the same `m`-major /
    // `n`-ascending order the latitude loop reads them back (via a running
    // cursor), so addressing them needs no `(n, m)` arithmetic. `L(L−1)/2`
    // `(f64, f64)`.
    let mut ab = Vec::with_capacity(l.saturating_sub(1) * l / 2);
    for m in 0..=l {
        for n in (m + 2)..=l {
            ab.push(recurrence(n, m));
        }
    }

    for (li, &lat) in latitudes_deg.iter().enumerate() {
        let mu = lat.to_radians().sin();
        let s = (1.0 - mu * mu).max(0.0).sqrt();
        let row = &mut out[li * nlon..(li + 1) * nlon];

        // Walk the columns m = 0..=L. `idx` reads the flat (real, imag) pairs in
        // storage order (n = m..=T within each m), stepping past the ones above
        // `L`. `pmm` carries the sectoral diagonal P̄_m^m from one column to the
        // next. For each column we reduce the coefficients against the Legendre
        // column to a single complex (re_m, im_m), then spread it over longitude.
        let mut idx = 0usize;
        let mut abk = 0usize; // read cursor into `ab`, in lockstep with its build order
        let mut pmm = 1.0f64; // P̄_0^0
        for m in 0..=l {
            let mf = m as f64;
            // n = m (always present).
            let p_m = pmm;
            let mut re_m = p_m * coefficients[idx];
            let mut im_m = p_m * coefficients[idx + 1];
            idx += 2;

            if m < l {
                // n = m + 1: P̄_{m+1}^m = √(2m+3)·μ·P̄_m^m.
                let mut p_prev2 = p_m;
                let mut p_prev1 = (2.0 * mf + 3.0).sqrt() * mu * p_m;
                re_m += p_prev1 * coefficients[idx];
                im_m += p_prev1 * coefficients[idx + 1];
                idx += 2;

                // n = m + 2 ..= L via the two-term normalised recurrence, its
                // latitude-invariant coefficients read from the precomputed table.
                for _ in (m + 2)..=l {
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
            // n = L + 1 ..= T are stored and not summed: step past them to the
            // next column's n = m + 1. Zero when the band limit is the file's.
            idx += 2 * (t - l);

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
            if m < l {
                pmm *= ((2.0 * mf + 3.0) / (2.0 * mf + 2.0)).sqrt() * s;
            }
        }
    }
    Ok(out)
}

/// Evaluate the full spherical-harmonic sum over every `n ≤ T` at one point —
/// the file's own value there, not a map's (#637).
///
/// What a probe reads. A map above [`spectral_render_band_limit`] is the field
/// band-limited to what its raster can carry, so a value read off it is the
/// smoothed field's; this is the one the coefficients state. It is the same
/// sum [`synthesize_spherical_harmonic`] computes at each grid point, term for
/// term in the same order, so at a grid point where the recurrence stays in
/// `f64`'s normal range the two agree bit for bit.
///
/// # Extended exponents, because `f64` runs out
///
/// The sectoral term `P̄_m^m` falls like `cos(φ)^m`, which leaves `f64`'s
/// range long before the truncations that exist: at 60° it is below `1e-308`
/// by `m ≈ 1030`. The column recurrence then starts from a denormal with a few
/// bits left, and since `P̄_n^m` climbs back to order one further up the
/// column, it amplifies that rounding into garbage — measured at 60°, the plain
/// recurrence returns `1.6e155` at T3000 and `NaN` at T4000, and at 37.5° it
/// fails by T6000. So the sectoral term and the start of each column are
/// carried as *X-numbers* — an `f64` and a separate exponent in steps of
/// `2^960` (Fukushima, *J. Geodesy* 86, 2012) — until the column is back in
/// range, and plain `f64` after that. The values that never come back are
/// below `2^-480` of order one and are the zero they round to.
///
/// **No tables and no budget.** The grid transform hoists its phase and
/// recurrence tables out of the latitude loop because it has many latitudes;
/// at one point they would be built to be read once, and the recurrence table
/// alone is 537 MB at [`MAX_TRUNCATION`]. So each recurrence term is computed
/// where it is used, the space is constant, and the time is `(T+1)(T+2)/2`
/// terms — bounded by [`MAX_TRUNCATION`] alone, which is why neither
/// [`MAX_SYNTHESIS_WORK`] nor [`MAX_SYNTHESIS_CELLS`] is charged.
///
/// # Errors
///
/// [`FieldglassError::Parse`] when the truncation is past [`MAX_TRUNCATION`] or
/// `coefficients` is not `(T+1)(T+2)` long.
pub fn evaluate_spherical_harmonic(
    coefficients: &[f64],
    truncation: u32,
    latitude_deg: f64,
    longitude_deg: f64,
) -> Result<f64, FieldglassError> {
    let expected = coefficient_count(truncation)?;
    if coefficients.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "spectral evaluation: got {} coefficient values, expected (T+1)(T+2) = {expected} for T={truncation}",
            coefficients.len()
        )));
    }
    let t = truncation as usize;
    let mu = latitude_deg.to_radians().sin();
    let s = (1.0 - mu * mu).max(0.0).sqrt();
    let lr = longitude_deg.to_radians();

    // The latitude loop of `synthesize_band_limited`, for one latitude and one
    // longitude, with `recurrence` called where that function reads its table
    // and the terms held as X-numbers until they are back in `f64` range. Every
    // X-number operation on a term whose exponent is zero is the plain `f64`
    // expression, which is what keeps the two paths bit-identical there.
    let mut value = 0.0f64;
    let mut idx = 0usize;
    let mut pmm = XNum::ONE;
    for m in 0..=t {
        let mf = m as f64;
        let p_m = pmm;
        let mut re_m = p_m.to_f64() * coefficients[idx];
        let mut im_m = p_m.to_f64() * coefficients[idx + 1];
        idx += 2;
        if m < t {
            let mut p_prev2 = p_m;
            let mut p_prev1 = p_m.scaled((2.0 * mf + 3.0).sqrt() * mu);
            re_m += p_prev1.to_f64() * coefficients[idx];
            im_m += p_prev1.to_f64() * coefficients[idx + 1];
            idx += 2;
            let mut n = m + 2;
            // Extended range until both carried terms are back in `f64`'s.
            while n <= t && (p_prev1.exp != 0 || p_prev2.exp != 0) {
                let (a, b) = recurrence(n, m);
                let p_n = XNum::combine(a * mu, p_prev1, -b, p_prev2);
                re_m += p_n.to_f64() * coefficients[idx];
                im_m += p_n.to_f64() * coefficients[idx + 1];
                idx += 2;
                p_prev2 = p_prev1;
                p_prev1 = p_n;
                n += 1;
            }
            // Plain `f64` for the rest of the column: `P̄_n^m` is bounded by
            // `√(2n + 1)`, so nothing past this point leaves the range again.
            let (mut p1, mut p2) = (p_prev1.x, p_prev2.x);
            while n <= t {
                let (a, b) = recurrence(n, m);
                let p_n = a * mu * p1 - b * p2;
                re_m += p_n * coefficients[idx];
                im_m += p_n * coefficients[idx + 1];
                idx += 2;
                p2 = p1;
                p1 = p_n;
                n += 1;
            }
        }
        if m == 0 {
            value += re_m;
        } else {
            let ang = mf * lr;
            value += 2.0 * (re_m * ang.cos() - im_m * ang.sin());
        }
        if m < t {
            pmm = pmm.scaled(((2.0 * mf + 3.0) / (2.0 * mf + 2.0)).sqrt() * s);
        }
    }
    Ok(value)
}

/// `2^960`: one step of an [`XNum`]'s exponent.
const X_BIG: f64 = f64::from_bits((1023 + 960) << 52);
/// `2^-960`.
const X_BIG_INV: f64 = f64::from_bits((1023 - 960) << 52);
/// `2^480` and `2^-480`, the band an [`XNum`]'s mantissa is kept inside.
const X_HIGH: f64 = f64::from_bits((1023 + 480) << 52);
/// See [`X_HIGH`].
const X_LOW: f64 = f64::from_bits((1023 - 480) << 52);

/// An extended-exponent number, `x · 2^(960·exp)` (Fukushima 2012): the
/// mantissa stays inside `[2^-480, 2^480)` and the exponent carries the rest,
/// so a value can fall far below `f64::MIN_POSITIVE` without losing bits.
///
/// Only what [`evaluate_spherical_harmonic`] needs: scaling by an `f64`, the
/// two-term linear combination the Legendre recurrence is, and conversion back.
/// With `exp == 0` each is the plain `f64` expression.
#[derive(Debug, Clone, Copy)]
struct XNum {
    x: f64,
    exp: i32,
}

impl XNum {
    const ONE: Self = Self { x: 1.0, exp: 0 };

    /// Bring the mantissa back inside the band. One step suffices for every
    /// caller: each multiplies an in-band mantissa by a factor well inside
    /// `2^±480`. Zero stays zero at exponent zero rather than stepping down
    /// forever.
    fn normalized(x: f64, exp: i32) -> Self {
        let w = x.abs();
        if w == 0.0 {
            Self { x: 0.0, exp: 0 }
        } else if w >= X_HIGH {
            Self {
                x: x * X_BIG_INV,
                exp: exp + 1,
            }
        } else if w < X_LOW {
            Self {
                x: x * X_BIG,
                exp: exp - 1,
            }
        } else {
            Self { x, exp }
        }
    }

    fn scaled(self, factor: f64) -> Self {
        Self::normalized(factor * self.x, self.exp)
    }

    /// `f·p + g·q`. Exponents two or more steps apart differ by `2^960`, so
    /// the smaller operand is below the larger's last bit and drops out.
    fn combine(f: f64, p: Self, g: f64, q: Self) -> Self {
        let (x, exp) = match p.exp - q.exp {
            0 => (f * p.x + g * q.x, p.exp),
            1 => (f * p.x + g * (q.x * X_BIG_INV), p.exp),
            -1 => (f * (p.x * X_BIG_INV) + g * q.x, q.exp),
            d if d > 1 => (f * p.x, p.exp),
            _ => (g * q.x, q.exp),
        };
        Self::normalized(x, exp)
    }

    /// The value as an `f64`: exact at exponent zero, and at a negative
    /// exponent the zero (or last denormal) it rounds to. A positive exponent
    /// cannot arise — every value here is a normalised Legendre function,
    /// bounded by `√(2n + 1)` — and would read as infinity.
    fn to_f64(self) -> f64 {
        match self.exp {
            0 => self.x,
            -1 => self.x * X_BIG_INV,
            e if e < 0 => 0.0,
            _ => f64::INFINITY,
        }
    }
}

/// The two-term normalised Legendre recurrence coefficients for `(n, m)`,
/// `n ≥ m + 2`: `P̄_n^m = a·μ·P̄_{n−1}^m − b·P̄_{n−2}^m`.
///
/// One expression for both the grid transform's table and the point
/// evaluation, which is what makes the two agree bit for bit.
#[inline]
fn recurrence(n: usize, m: usize) -> (f64, f64) {
    let (nf, mf) = (n as f64, m as f64);
    let a = ((2.0 * nf + 1.0) * (2.0 * nf - 1.0) / ((nf - mf) * (nf + mf))).sqrt();
    let b = ((2.0 * nf + 1.0) * (nf + mf - 1.0) * (nf - mf - 1.0)
        / ((2.0 * nf - 3.0) * (nf - mf) * (nf + mf)))
        .sqrt();
    (a, b)
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
        let mut out = Vec::with_capacity(coefficient_count(t).expect("test truncation"));
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

    /// The allocation ceiling: exact at the cap, refused one past it. The count
    /// at the cap is what [`MAX_COEFFICIENTS`] documents, and both GRIB
    /// editions' decoders size their coefficient arrays from this function.
    #[test]
    fn the_coefficient_count_is_exact_up_to_the_cap_and_refused_past_it() {
        assert_eq!(coefficient_count(0).expect("T0"), 2);
        assert_eq!(coefficient_count(63).expect("T63"), 64 * 65);
        assert_eq!(
            coefficient_count(MAX_TRUNCATION).expect("at the cap"),
            MAX_COEFFICIENTS
        );
        assert!(coefficient_count(MAX_TRUNCATION + 1).is_err());
        assert!(coefficient_count(u32::MAX).is_err());
    }

    /// The cost budget's own axis: a truncation well inside the cap on a target
    /// grid large enough to blow the budget is refused, and nothing is
    /// allocated for it. Nothing else bounds the caller's slices — the output
    /// `Vec` alone is `nlat · nlon`.
    #[test]
    fn rejects_a_small_truncation_on_an_unaffordable_grid() {
        // 200_000 × 200_000 output points at T=63 is 4·10^10 units, past the
        // budget, and would have allocated 320 GB for the output alone.
        let lats = vec![0.0; 200_000];
        let lons = vec![0.0; 200_000];
        let Err(err) = synthesize_spherical_harmonic(&[0.0; 64 * 65], 63, &lats, &lons) else {
            panic!("a grid past the cost budget must be refused");
        };
        // `costs`, not just `over the budget`: the allocation budget refuses
        // this grid too, and the test name says which one is under test.
        assert!(err.to_string().contains("costs"), "{err}");
    }

    /// Two grids that are affordable by *time* and ruinous by *space*, which is
    /// why `synthesis_cells` exists beside `synthesis_work`. Both pass the work
    /// budget: work charges `nlat` for every column-longitude pair, and each of
    /// these makes one of those two factors one.
    #[test]
    fn rejects_a_grid_the_work_budget_admits_but_the_allocation_does_not() {
        // A degenerate truncation on a raster larger than the pinned one: every
        // column term vanishes, and the output alone is a million values.
        assert!(synthesis_work(0, 1_000, 1_000) <= MAX_SYNTHESIS_WORK);
        assert!(synthesis_cells(0, 1_000, 1_000) > MAX_SYNTHESIS_CELLS);

        // One latitude at the largest truncation: the tables are built once, so
        // `nlat = 1` makes the work cheap while the recurrence table alone is
        // `T(T−1)` values (537 MB).
        assert!(synthesis_work(MAX_TRUNCATION, 1, 1_000) <= MAX_SYNTHESIS_WORK);
        assert!(synthesis_cells(MAX_TRUNCATION, 1, 1_000) > MAX_SYNTHESIS_CELLS);

        // And the transform refuses it rather than allocating for it. `T = 0`
        // takes two coefficient values, so this is a well-formed call in every
        // way except its grid.
        let lats = vec![0.0; 1_000];
        let lons = vec![0.0; 1_000];
        let Err(err) = synthesize_spherical_harmonic(&[1.0, 0.0], 0, &lats, &lons) else {
            panic!("a grid past the allocation budget must be refused");
        };
        // `allocates`, not just `over the budget`: both messages end that way,
        // and this case must be refused by the allocation budget specifically —
        // the whole point is that the work budget admits it.
        assert!(err.to_string().contains("allocates"), "{err}");
    }

    /// The control for the tests above: the map of the largest truncation the
    /// cap admits — band-limited to what the pinned grid carries — is exactly
    /// *both* budgets, so no map is refused by either. Checked as arithmetic
    /// here; `a_band_limited_map_costs_the_limit_not_the_file` runs one.
    #[test]
    fn the_budgets_are_the_map_of_the_largest_truncation_admitted() {
        let limit = spectral_render_band_limit(MAX_TRUNCATION);
        assert_eq!(limit, 359);
        assert_eq!(
            synthesis_work(limit, SYNTHESIS_NJ, SYNTHESIS_NI),
            MAX_SYNTHESIS_WORK
        );
        assert_eq!(
            synthesis_cells(limit, SYNTHESIS_NJ, SYNTHESIS_NI),
            MAX_SYNTHESIS_CELLS
        );
        // The numbers the docs state, so a doc that drifts from them is caught.
        assert_eq!(MAX_SYNTHESIS_WORK, 140_356_800);
        assert_eq!(MAX_SYNTHESIS_CELLS, 907_562);
        // Every truncation's map fits, since each one's band limit is at most
        // the cap's.
        for t in [0, 63, 359, 360, 1279, 7999, MAX_TRUNCATION] {
            let limit = spectral_render_band_limit(t);
            assert!(synthesis_work(limit, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_SYNTHESIS_WORK);
            assert!(synthesis_cells(limit, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_SYNTHESIS_CELLS);
        }
        // What re-setting the budgets gives up, stated rather than discovered:
        // the *full* sum at the cap onto the pinned grid is no longer something
        // the project runs, so the transform refuses it, and so it refuses any
        // grid with more points than the pinned one.
        assert!(synthesis_work(MAX_TRUNCATION, SYNTHESIS_NJ, SYNTHESIS_NI) > MAX_SYNTHESIS_WORK);
        assert!(synthesis_cells(0, SYNTHESIS_NJ * 2, SYNTHESIS_NI * 2) > MAX_SYNTHESIS_CELLS);
        // A coarser caller grid below the map's cost is admitted.
        assert!(synthesis_work(63, 37, 72) < MAX_SYNTHESIS_WORK);
        assert!(synthesis_cells(63, 37, 72) < MAX_SYNTHESIS_CELLS);
    }

    /// The grid transform runs in plain `f64`, which is exact below T1927 at
    /// every latitude (see `synthesize_band_limited`). The allocation budget is
    /// what keeps every call below that, even on an empty grid: the recurrence
    /// table alone is `L(L−1)`, so nothing past T953 is admitted.
    #[test]
    fn the_budgets_keep_the_grid_transform_inside_plain_f64() {
        assert!(synthesis_cells(953, 0, 0) <= MAX_SYNTHESIS_CELLS);
        assert!(synthesis_cells(954, 0, 0) > MAX_SYNTHESIS_CELLS);
    }

    /// A product too large for `u64` must read as "over budget", not wrap to a
    /// small number and let the transform through. Both metrics: either one
    /// wrapping would admit the case the other exists to refuse.
    #[test]
    fn the_metrics_saturate_rather_than_wrapping() {
        assert_eq!(
            synthesis_work(MAX_TRUNCATION, usize::MAX, usize::MAX),
            u64::MAX
        );
        assert_eq!(
            synthesis_cells(MAX_TRUNCATION, usize::MAX, usize::MAX),
            u64::MAX
        );
    }

    /// `MAX_SYNTHESIS_CELLS` bounds every length the transform multiplies out,
    /// which is what makes those `usize` multiplies exact on a 32-bit target.
    /// That claim is the comment beside them; this is it as arithmetic.
    #[test]
    fn the_allocation_budget_keeps_every_length_inside_a_32_bit_usize() {
        assert!(MAX_SYNTHESIS_CELLS < u64::from(u32::MAX));
    }

    // ── Band limit and exact point evaluation (#637) ─────────────────────────

    /// A deterministic coefficient array for truncation `t`: every value set,
    /// so a coefficient read from the wrong slot changes the answer.
    fn pseudo_random(t: u32) -> Vec<f64> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ u64::from(t);
        (0..coefficient_count(t).expect("test truncation"))
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                // The top 53 bits as a uniform value in [-1, 1).
                (state >> 11) as f64 / (1u64 << 52) as f64 - 1.0
            })
            .collect()
    }

    /// `coefficients` (truncation `t`) cut down to the triangle `n ≤ l`, laid
    /// out as a truncation-`l` array would be — the independent way to say what
    /// band-limiting means.
    fn triangle(coefficients: &[f64], t: u32, l: u32) -> Vec<f64> {
        let mut out = Vec::new();
        let mut idx = 0;
        for m in 0..=t {
            for n in m..=t {
                if m <= l && n <= l {
                    out.extend_from_slice(&coefficients[idx..idx + 2]);
                }
                idx += 2;
            }
        }
        out
    }

    /// The limit is derived from the grid, and on the pinned one it is T359 by
    /// both axes — the number the decision on #637 names.
    #[test]
    fn the_pinned_grid_carries_t359_by_both_axes() {
        assert_eq!(grid_band_limit(SYNTHESIS_NI, SYNTHESIS_NJ), 359);
        // Each axis alone: 720 longitudes carry m ≤ 359, 361 latitudes n ≤ 359.
        assert_eq!(grid_band_limit(720, 100_000), 359);
        assert_eq!(grid_band_limit(100_000, 361), 359);
        // The rule `spectral_render_dims`' docs state the other way round: a
        // truncation T needs 2(T+1) longitudes.
        assert_eq!(grid_band_limit(2 * (63 + 1), 1_000), 63);
        // Degenerate grids carry nothing rather than wrapping.
        assert_eq!(grid_band_limit(0, 0), 0);
        assert_eq!(grid_band_limit(1, 1), 0);
    }

    #[test]
    fn a_map_is_band_limited_only_above_the_limit_and_labelled_only_then() {
        for t in [0, 63, 358, 359] {
            assert_eq!(spectral_render_band_limit(t), t, "T{t}");
            assert_eq!(SpectralTruncation::of_render(t), None, "T{t}");
        }
        for t in [360, 1279, 7999, MAX_TRUNCATION] {
            assert_eq!(spectral_render_band_limit(t), 359, "T{t}");
            assert_eq!(
                SpectralTruncation::of_render(t),
                Some(SpectralTruncation {
                    declared: t,
                    truncated_to: 359
                }),
                "T{t}"
            );
        }
    }

    /// Band-limiting skips coefficients in place; cutting the triangle out and
    /// synthesising it in full is the same field, bit for bit. Pins the stride
    /// past `n > L` in every column, including the last few where `m` is close
    /// to `L`.
    #[test]
    fn band_limiting_is_the_full_sum_of_the_cut_triangle() {
        let (t, l) = (24, 9);
        let full = pseudo_random(t);
        let cut = triangle(&full, t, l);
        let lats = [90.0, 61.5, 12.0, 0.0, -33.0, -90.0];
        let lons = [0.0, 17.0, 181.5, 359.5];
        let limited = synthesize_band_limited(&full, t, l, &lats, &lons).expect("limited");
        let reference = synthesize_spherical_harmonic(&cut, l, &lats, &lons).expect("cut");
        assert_eq!(limited, reference);
        // And the band limit really removed something.
        let unlimited = synthesize_spherical_harmonic(&full, t, &lats, &lons).expect("full");
        assert_ne!(limited, unlimited);
    }

    /// A band limit at or past the truncation is the full sum, exactly — which
    /// is why a map below T359 is unchanged by #637.
    #[test]
    fn a_band_limit_at_or_past_the_truncation_changes_nothing() {
        let t = 30;
        let c = pseudo_random(t);
        let full = synthesize_spherical_harmonic(&c, t, &LATS, &LONS).expect("full");
        for l in [t, t + 1, MAX_TRUNCATION] {
            assert_eq!(
                synthesize_band_limited(&c, t, l, &LATS, &LONS).expect("limited"),
                full,
                "band limit {l}"
            );
        }
    }

    /// The point evaluation is the grid transform's sum at one point, term for
    /// term: equal to the last bit at every grid point, poles included.
    #[test]
    fn the_point_evaluation_is_the_grid_sum_bit_for_bit() {
        let t = 40;
        let c = pseudo_random(t);
        let lats = [90.0, 45.5, 1.0, -0.5, -72.0, -90.0];
        let lons = [0.0, 0.5, 97.0, 180.0, 359.5];
        let grid = synthesize_spherical_harmonic(&c, t, &lats, &lons).expect("grid");
        for (i, &lat) in lats.iter().enumerate() {
            for (j, &lon) in lons.iter().enumerate() {
                let point = evaluate_spherical_harmonic(&c, t, lat, lon).expect("point");
                assert_eq!(
                    point.to_bits(),
                    grid[i * lons.len() + j].to_bits(),
                    "({lat}, {lon})"
                );
            }
        }
    }

    /// The analytic cases again, through the point evaluation: they pin the
    /// normalisation independently of the grid transform.
    #[test]
    fn the_point_evaluation_reproduces_the_analytic_harmonics() {
        let (lat, lon) = (30.0_f64, 120.0_f64);
        let (mu, s) = (lat.to_radians().sin(), lat.to_radians().cos());
        let cases = [
            ((0, 0, 1.0, 0.0), 1.0),
            ((1, 0, 1.0, 0.0), 3f64.sqrt() * mu),
            ((1, 1, 1.0, 0.0), 6f64.sqrt() * s * lon.to_radians().cos()),
            (
                (1, 1, 0.0, 1.0),
                -(6f64.sqrt()) * s * lon.to_radians().sin(),
            ),
            ((2, 0, 1.0, 0.0), 5f64.sqrt() * (3.0 * mu * mu - 1.0) / 2.0),
        ];
        for ((n, m, re, im), want) in cases {
            let got =
                evaluate_spherical_harmonic(&single(3, n, m, re, im), 3, lat, lon).expect("point");
            assert!((got - want).abs() < 1e-12, "({n},{m}): {got} vs {want}");
        }
    }

    #[test]
    fn the_point_evaluation_refuses_what_the_transform_refuses() {
        assert!(evaluate_spherical_harmonic(&[0.0; 5], 2, 0.0, 0.0).is_err());
        assert!(evaluate_spherical_harmonic(&[], MAX_TRUNCATION + 1, 0.0, 0.0).is_err());
    }

    /// The whole point of #637, as a cost: a field far above the limit maps at
    /// the limit's cost, and its tables are the limit's size. A grid-sized
    /// synthesis at T2000 in full would cost 13.4× the budget and be refused.
    #[test]
    fn a_band_limited_map_costs_the_limit_not_the_file() {
        let t = 2_000;
        let limit = spectral_render_band_limit(t);
        let (lats, lons) = spectral_render_grid(t).axes();
        assert!(synthesis_work(t, lats.len(), lons.len()) > MAX_SYNTHESIS_WORK);
        let c = pseudo_random(t);
        assert!(synthesize_spherical_harmonic(&c, t, &lats, &lons).is_err());
        // The map runs; two rows are enough to show it is admitted and matches
        // the triangle it claims to be.
        let rows = &lats[100..102];
        let map = synthesize_band_limited(&c, t, limit, rows, &lons).expect("map");
        let cut = triangle(&c, t, limit);
        let reference = synthesize_spherical_harmonic(&cut, limit, rows, &lons).expect("cut");
        assert_eq!(map, reference);
    }
}
