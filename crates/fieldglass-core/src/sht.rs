//! Inverse spherical-harmonic transform — synthesize grid-point values from
//! the triangular spherical-harmonic coefficients stored by ECMWF/IFS spectral
//! GRIB fields (and their GRIB1 equivalents).
//!
//! Definitive reference — WMO FM 92 GRIB, Manual on Codes Vol. I.2, code table
//! 3.6 (code 1) for the basis and code table 3.7 for the storage order; GRIB1's
//! code tables 9 and 10 state the same:
//!
//! ```text
//! F(λ, μ) = Σ_{m=-M}^{M} Σ_{n=|m|}^{N(m)} F_n^m P̄_n^m(μ) e^{i m λ},   μ = sin(lat)
//! ```
//!
//! with `F_n^{-m}` the complex conjugate of `F_n^m`, `P̄_n^{-m} = P̄_n^m` (there is
//! no Condon–Shortley phase), and the normalisation
//! `(1/2) ∫_{-1}^{1} [P̄_n^m(μ)]² dμ = 1` (so `P̄_0^0 = 1`, `P̄_1^0 = √3·μ`).
//! ECMWF's ecCodes FAQ writes the conjugate relation as
//! `X_{n,-m} = conj(X_{n,m}) / (-1)^m`, which is the same field under a
//! `P̄^{-m} = (-1)^m P̄^m` convention; code table 3.6 is the text this follows.
//!
//! Coefficients are stored `m`-major: `Re(F₀₀), Im(F₀₀), Re(F₁₀), Im(F₁₀), …`,
//! `n` increasing from `m` to `T`, first `m = 0`, then `m = 1, …, T`. Collapsing
//! the ±m conjugate pairs for a real field yields the implemented form:
//!
//! ```text
//! F(λ, μ) = Σ_n F_{n,0} P̄_n^0(μ)
//!         + 2 Σ_{m≥1} Σ_n P̄_n^m(μ) [Re(F_{n,m}) cos(mλ) − Im(F_{n,m}) sin(mλ)]
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
//! [`synthesize_map`]: past that, the grid's points alias the rest onto what
//! they can show. The field a file declares above the limit is therefore drawn
//! band-limited, and [`SpectralTruncation`] is the label that says so. A point
//! is evaluated in full by [`evaluate_spherical_harmonic`]. A grid of the
//! caller's own is band-limited the same way by default ([`points_band_limit`]),
//! and in full on request ([`synthesize_spherical_harmonic`]).
//!
//! # Every truncation, in one kernel
//!
//! The plain `f64` recurrence is accurate only to about T1810: the sectoral
//! term `P̄_m^m` falls like `cos(φ)^m` and leaves `f64`'s range at mid and high
//! latitudes, and a column that starts there loses the bits it needs once the
//! recurrence amplifies it back to order one. Measured against 80-bit
//! arithmetic, the worst `P̄` error is at noise level to T1810, `4·10⁻¹⁰` at
//! T1840, `2·10⁻⁴` at T1927 and `0.5` at T2000. So every entry point runs the
//! one kernel SHTns and ducc0 use: each `(latitude, m)` column starts as an
//! extended-exponent number (Fukushima 2012), runs scaled only until it is back in
//! `f64`'s range, and then runs in plain `f64`; a column that never comes back
//! before the band limit contributes nothing and is skipped. That holds at
//! every truncation up to [`MAX_TRUNCATION`], costs nothing where the plain
//! recurrence was already right, and is bit-identical to it there.
//!
//! Oracles for this: a T383 fixture per GRIB edition whose band-limited map and
//! full point sums pyshtools computes, pyshtools again at T2500
//! (`tools/build_spectral_truncation_oracle.py`, both), and a T3000 point sum
//! in 80-bit `long double` from the same script, past where pyshtools' own
//! Legendre routine is documented to hold.

use crate::error::FieldglassError;
use crate::global_grid::{GlobalGrid, SYNTHESIS_NI, SYNTHESIS_NJ};

/// Upper bound on the truncation `T` any spectral decode or synthesis will
/// accept — the ceiling on the coefficient array, which is what a declared
/// truncation turns into (#631).
///
/// `T` is read from attacker-controlled §3 fields and the array it sizes is
/// `(T+1)(T+2)` `f64`, so it is capped before anything is allocated. The
/// transform's own allocation is sized by the grid, not by this (see
/// [`synthesis_cells`]).
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
/// what a map cost until #637 — measured in a release build, one message,
/// with the tabled kernel of the time:
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
/// coefficient array and its recurrence table were 537 MB each and overlapped
/// in time. Past about T1810 it was also wrong, and on a synthetic T7999 field
/// 38% of the map's cells came back non-finite or absurd.
///
/// Since #637 a map synthesises only the T359 the grid carries
/// ([`spectral_render_band_limit`]), so above T359 its transform costs the same
/// at every truncation, and the kernel builds its recurrence one column at a
/// time, so what is left of the cap's cost is the coefficient array itself.
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

/// The inner-loop count the transform runs for a band limit and a target grid,
/// against which the work budgets ([`MAX_MAP_SYNTHESIS_WORK`],
/// [`MAX_SYNTHESIS_WORK`]) are ceilings: one unit per `(latitude, column)` pair
/// times the work that column costs — the `n`-reduction over at most `L+1`
/// coefficients, then the spread over `nlon` longitudes.
///
/// This models the measured cost; `output points × coefficients` does not.
/// Across T63 → T2000 on the pinned grid the time per unit of *this* metric
/// moved 0.54 → 0.99 ns (1.8×) in the tabled kernel before #637, while the time
/// per unit of `output points × coefficients` moved 9.1 → 1.9 ps (4.9×, and in
/// the direction that under-charges the expensive end).
///
/// Saturating rather than wrapping: the slice lengths are caller-supplied and
/// only ever compared against a ceiling, so a product too large to represent
/// must read as "over budget", not wrap to a small number.
#[must_use]
pub const fn synthesis_work(truncation: u32, nlat: usize, nlon: usize) -> u64 {
    let columns = truncation as u64 + 1;
    (nlat as u64).saturating_mul(columns.saturating_mul(columns.saturating_add(nlon as u64)))
}

/// Every `f64`-sized value the transform allocates for a band limit and a
/// target grid, against which the allocation budgets
/// ([`MAX_MAP_SYNTHESIS_CELLS`], [`MAX_SYNTHESIS_CELLS`]) are ceilings.
///
/// Space is a different function of the same two inputs than time is, which is
/// why [`synthesis_work`] does not bound it. Work charges `nlat` for every
/// column-longitude pair; space does not, so a grid with one latitude and a
/// million longitudes is cheap by the work metric and still allocates a phase
/// table of `2(L+1)·nlon`. In the other direction `T = 0` makes every column
/// term vanish while the output raster `nlat·nlon` remains. The terms, in the
/// order the kernel allocates them:
///
/// | table | values |
/// |---|---|
/// | output raster | `nlat·nlon` |
/// | per latitude: `μ`, `cos φ`, and the column seed (two words) | `4·nlat` |
/// | each column's reduction at each latitude, `(re, im)` | `2(L+1)·nlat` |
/// | one column's recurrence `(a, b)` | `2(L+1)` |
/// | longitudes in radians, then `cos(mλ)` and `sin(mλ)` | `(2(L+1)+1)·nlon` |
///
/// The recurrence is built one column at a time and reused across every
/// latitude, so nothing here grows with `L²` (the kernel before #637 held all
/// `L(L−1)/2` recurrence pairs at once, 537 MB at the cap). The coefficient
/// array is the caller's and is bounded separately, by [`MAX_TRUNCATION`] at
/// each decoder.
///
/// Saturating, for the reason [`synthesis_work`] gives.
#[must_use]
pub const fn synthesis_cells(truncation: u32, nlat: usize, nlon: usize) -> u64 {
    let columns = truncation as u64 + 1;
    let (nlat, nlon) = (nlat as u64, nlon as u64);
    let raster = nlat.saturating_mul(nlon);
    let per_latitude = nlat.saturating_mul(columns.saturating_mul(2).saturating_add(4));
    let recurrence = columns.saturating_mul(2);
    let phases = nlon.saturating_mul(columns.saturating_mul(2).saturating_add(1));
    raster
        .saturating_add(per_latitude)
        .saturating_add(recurrence)
        .saturating_add(phases)
}

/// Ceiling on [`synthesis_work`] for a map: the map of the largest truncation
/// that exists, on the grid every host renders onto ([`spectral_render_grid`]),
/// band-limited to what that grid carries ([`spectral_render_band_limit`],
/// T359). 140,356,800 units, about a tenth of a second.
///
/// [`synthesize_map`] is charged against it. Every map fits by construction,
/// since every truncation's band limit is at most the cap's; the budget is
/// what makes that a checked fact rather than a comment.
pub const MAX_MAP_SYNTHESIS_WORK: u64 = {
    let (ni, nj) = spectral_render_dims(MAX_TRUNCATION);
    synthesis_work(spectral_render_band_limit(MAX_TRUNCATION), nj, ni)
};

/// Ceiling on [`synthesis_cells`] for a map, defined as
/// [`MAX_MAP_SYNTHESIS_WORK`] is: 1,041,124 values, 8.3 MB.
pub const MAX_MAP_SYNTHESIS_CELLS: u64 = {
    let (ni, nj) = spectral_render_dims(MAX_TRUNCATION);
    synthesis_cells(spectral_render_band_limit(MAX_TRUNCATION), nj, ni)
};

/// Ceiling on [`synthesis_work`] for a grid of the caller's own
/// ([`synthesize_band_limited`], [`synthesize_spherical_harmonic`], and the
/// readers' `synthesize_spectral_message`) — a budget sized by time, separate
/// from the map's.
///
/// It is the *full* sum at [`MAX_TRUNCATION`] onto the pinned 720×361 grid:
/// 26,361,739,449 units, the transform #631 accepted as the most a single call
/// may cost (27 s in the tabled kernel before #637). So the full sum of every
/// truncation that exists fits onto a half-degree global grid, and a band
/// limit buys a finer grid: at T1279, MIR's linear grid for it (2560×1281) is
/// 6.3·10⁹ units and fits whole.
///
/// Before #637 the map was charged against this budget too; the two are
/// separate because the map's grid is pinned and a caller's is not.
pub const MAX_SYNTHESIS_WORK: u64 = {
    let (ni, nj) = spectral_render_dims(MAX_TRUNCATION);
    synthesis_work(MAX_TRUNCATION, nj, ni)
};

/// Ceiling on [`synthesis_cells`] for a grid of the caller's own:
/// [`MAX_FIELD_POINTS`](crate::MAX_FIELD_POINTS), the most points one decoded
/// field may hold anywhere in the stack (537 MB of `f64`).
///
/// The transform's own tables are the band limit times one side of the grid
/// (see [`synthesis_cells`]), so on any grid near square this is in effect a
/// bound on the output raster, the same bound every reader's decode is held
/// to; the full sum at the cap on the pinned grid allocates 18.0 M values.
pub const MAX_SYNTHESIS_CELLS: u64 = crate::MAX_FIELD_POINTS as u64;

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
/// On the pinned 720 × 361 grid both give 359. That is also the rule ECMWF's
/// MIR applies by default before every spectral-to-grid transform: it takes
/// the Gaussian number of the target grid (`N = 180` for 0.5°) and truncates
/// to the linear `T = 2N − 1` (MIR `key/resol/Resol.cc`,
/// `util/SpectralOrderT.h`), which is T359 here too.
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

/// How many times larger than each gap beside it a gap must be before
/// [`points_band_limit`] reads it as a break between two regions of a grid
/// rather than a step of one.
///
/// Four lets a regular grid lose up to three consecutive rows or columns and
/// still be charged for the hole as one coarse stretch, while the separation
/// between two sampled regions, which is tens or hundreds of steps, is never
/// mistaken for a step.
const REGION_BREAK: f64 = 4.0;

/// The highest total wavenumber a caller's grid resolves — [`grid_band_limit`]
/// for axes that need not be global, regular or ordered — or `None` when
/// neither axis has three distinct points, so neither limits anything.
///
/// Each axis is judged by its coarsest step `Δ` in degrees: a step of `Δ`
/// carries wavenumbers below `180/Δ`, so the limit is `⌊180/Δ⌋ − 1`, and the
/// grid carries the smaller of its two axes' limits. On the pinned global grid
/// that is T359 again, and it is MIR's linear rule for a regular lat/lon
/// target (0.25° gives T719, 1° T179). Longitudes are read round the circle and
/// latitudes as a line.
///
/// **A gap between two regions is not a step.** A grid can sample more than one
/// place: two latitude bands, two longitude sectors, or the one region a
/// regional grid covers, whose outside is the long way round the circle. The
/// gap that separates them is not the spacing either region is sampled at, and
/// charging it would band-limit both regions to the separation — two dense
/// polar caps 140° apart would read as a 140° step and synthesise as the
/// field's global mean (#812). So a gap more than four times each
/// gap beside it is a break between regions, and only the other gaps are
/// steps. A smaller jump is still a step: a regular grid with a row or two
/// missing is charged for the hole, because it is one sampling with a coarse
/// stretch rather than two samplings. The smallest gap on an axis is never a
/// break, so an axis that has steps keeps at least one.
///
/// **A coarse regular sample stays band-limited to what it resolves.** Three
/// longitudes 120° apart carry T0, as [`grid_band_limit`] says of a three-point
/// ring, so `[−60, 0, 60] × [0, 120, 240]` synthesises the field's mean at
/// every point. That is what the grid can show of the field, not a defect; a
/// caller who wants the field's value at a handful of places wants the full
/// sum, which `synthesize_spectral_message_full` and `evaluate_spectral_point`
/// in the format crates give.
///
/// **An axis constrains the limit only when it has at least three distinct
/// points.** One or two points on an axis are a handful of places, not a
/// sampling of it: 361 latitudes at longitudes 0° and 180°, or latitudes ±45°
/// round a whole circle of longitudes, would otherwise read as a 180° or 90°
/// step and collapse the field to its mean. Such an axis leaves the other to
/// decide, and when neither axis constrains anything — a single point, a 2 × 2
/// grid — the grid is evaluated in full, as a point is. Non-finite coordinates
/// are ignored here; the transform evaluates them as it evaluates any other.
#[must_use]
pub fn points_band_limit(latitudes_deg: &[f64], longitudes_deg: &[f64]) -> Option<u32> {
    let limit_of_step = |step: f64| -> u32 {
        // The relative slack absorbs a step that is 0.1 read back as
        // 0.09999999999999787, so a regular grid is not charged a wavenumber
        // for the rounding in its own coordinates.
        let carried = (180.0 / step * (1.0 + 1e-9)).floor();
        if carried >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            // `carried` is finite and non-negative: `step` is a positive gap.
            (carried as u32).saturating_sub(1)
        }
    };
    let sorted = |values: &[f64], wrap: bool| -> Vec<f64> {
        let mut v: Vec<f64> = values
            .iter()
            .filter(|x| x.is_finite())
            .map(|&x| if wrap { x.rem_euclid(360.0) } else { x })
            .collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    // Fewer than this many distinct points on an axis do not sample it.
    const MIN_POINTS: usize = 3;
    // The coarsest of an axis's gaps that are steps rather than breaks between
    // regions. `ring` closes the gaps round the circle, so the first and last
    // gaps are each other's neighbours; a line's end gaps have one neighbour.
    let coarsest_step = |gaps: &[f64], ring: bool| -> Option<f64> {
        let n = gaps.len();
        let neighbours = |i: usize| -> [Option<f64>; 2] {
            let before = if i > 0 {
                Some(gaps[i - 1])
            } else if ring {
                Some(gaps[n - 1])
            } else {
                None
            };
            let after = if i + 1 < n {
                Some(gaps[i + 1])
            } else if ring {
                Some(gaps[0])
            } else {
                None
            };
            [before, after]
        };
        (0..n)
            .filter(|&i| {
                let beside = neighbours(i);
                let is_break = beside.iter().any(Option::is_some)
                    && beside.iter().flatten().all(|&b| gaps[i] > REGION_BREAK * b);
                !is_break
            })
            .map(|i| gaps[i])
            .reduce(f64::max)
    };
    let lats = sorted(latitudes_deg, false);
    let by_latitude = (lats.len() >= MIN_POINTS)
        .then(|| lats.windows(2).map(|w| w[1] - w[0]).collect::<Vec<f64>>())
        .and_then(|gaps| coarsest_step(&gaps, false))
        .map(limit_of_step);
    let lons = sorted(longitudes_deg, true);
    let by_longitude = match (lons.first(), lons.last()) {
        (Some(&first), Some(&last)) if lons.len() >= MIN_POINTS => {
            let mut gaps: Vec<f64> = lons.windows(2).map(|w| w[1] - w[0]).collect();
            gaps.push(first + 360.0 - last);
            coarsest_step(&gaps, true).map(limit_of_step)
        }
        _ => None,
    };
    match (by_latitude, by_longitude) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
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

/// Synthesize the **full** sum over every `n ≤ T` onto a grid of the caller's
/// own: [`synthesize_band_limited`] with the band limit at `T`.
///
/// `coefficients` is the flat `(real, imaginary)` `m`-major sequence (as decoded
/// from §7 by the spectral readers); `truncation` is `T` (`J = K = M`).
/// `latitudes_deg` / `longitudes_deg` give the target grid in degrees.
/// Returns `latitudes_deg.len() · longitudes_deg.len()` values, latitude-major
/// (outer) then longitude (inner) — the usual scan order.
///
/// Exact at every point, and on a grid coarser than the field it aliases the
/// wavenumbers the grid cannot carry onto the ones it can. The band-limited
/// form is what a map is (#637) and what the readers'
/// `synthesize_spectral_message` gives by default; this is the explicit
/// full-detail call.
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

/// Synthesize over only the wavenumbers `n ≤ band_limit` onto a grid of the
/// caller's own — the field triangularly truncated to
/// `L = min(band_limit, truncation)`.
///
/// A grid resolves only so many wavenumbers ([`points_band_limit`]), and
/// evaluating the rest at its points aliases them onto the ones it can show
/// rather than adding detail. The coefficients stay the file's `(T+1)(T+2)`,
/// read in place; the ones past the band limit are skipped, not copied out. A
/// `band_limit` at or past `truncation` is the full sum, bit for bit.
///
/// Correct at every truncation up to [`MAX_TRUNCATION`]: see the module docs
/// for the range-safe kernel every entry point shares.
///
/// # Errors
///
/// [`FieldglassError::Parse`] when the truncation is past [`MAX_TRUNCATION`],
/// when `coefficients` is not `(T+1)(T+2)` long, or when `L` and the target grid
/// together exceed [`MAX_SYNTHESIS_WORK`] or [`MAX_SYNTHESIS_CELLS`].
pub fn synthesize_band_limited(
    coefficients: &[f64],
    truncation: u32,
    band_limit: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
) -> Result<Vec<f64>, FieldglassError> {
    synthesize_within(
        coefficients,
        truncation,
        band_limit,
        latitudes_deg,
        longitudes_deg,
        (MAX_SYNTHESIS_WORK, MAX_SYNTHESIS_CELLS),
    )
}

/// Synthesize a spectral field's **map**: onto [`spectral_render_grid`],
/// band-limited to [`spectral_render_band_limit`], and hand the grid back with
/// the values so a host cannot declare one shape and evaluate at another.
///
/// Charged against the map's own budgets ([`MAX_MAP_SYNTHESIS_WORK`],
/// [`MAX_MAP_SYNTHESIS_CELLS`]), which every truncation up to the cap fits.
/// Both GRIB editions' `synthesize_spectral_global` are this.
///
/// # Errors
///
/// [`FieldglassError::Parse`] when the truncation is past [`MAX_TRUNCATION`] or
/// `coefficients` is not `(T+1)(T+2)` long.
pub fn synthesize_map(
    coefficients: &[f64],
    truncation: u32,
) -> Result<(GlobalGrid, Vec<f64>), FieldglassError> {
    let grid = spectral_render_grid(truncation);
    let (lats, lons) = grid.axes();
    let values = synthesize_within(
        coefficients,
        truncation,
        spectral_render_band_limit(truncation),
        &lats,
        &lons,
        (MAX_MAP_SYNTHESIS_WORK, MAX_MAP_SYNTHESIS_CELLS),
    )?;
    Ok((grid, values))
}

/// Evaluate the full spherical-harmonic sum over every `n ≤ T` at one point —
/// the file's own value there, not a map's (#637).
///
/// What a probe's full-detail line reads. A map above
/// [`spectral_render_band_limit`] is the field band-limited to what its raster
/// can carry, so a value read off it is the smoothed field's; this is the one
/// the coefficients state. It is the grid transform run at one point — the
/// same kernel, term for term — so it agrees with
/// [`synthesize_spherical_harmonic`] bit for bit at every grid point.
///
/// **No budget.** The kernel's tables are one column's recurrence and one
/// point's phases, so the space is `O(T)` and the time `(T+1)(T+2)/2` terms,
/// bounded by [`MAX_TRUNCATION`] alone.
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
    check_coefficients(coefficients, truncation)?;
    let value = synthesize_unchecked(
        coefficients,
        truncation,
        truncation,
        &[latitude_deg],
        &[longitude_deg],
    );
    Ok(value[0])
}

/// The truncation cap and the `(T+1)(T+2)` length, the two things every entry
/// point refuses before any work.
fn check_coefficients(coefficients: &[f64], truncation: u32) -> Result<(), FieldglassError> {
    let expected = coefficient_count(truncation)?;
    if coefficients.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis: got {} coefficient values, expected (T+1)(T+2) = {expected} for T={truncation}",
            coefficients.len()
        )));
    }
    Ok(())
}

/// Refuse, then run the kernel: the budgets `(work, cells)` are charged at the
/// band limit, before the output or any table is allocated. The truncation is
/// bounded above; the target grid is not, and the two multiply — differently
/// for time than for space, which is why there are two.
fn synthesize_within(
    coefficients: &[f64],
    truncation: u32,
    band_limit: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
    (max_work, max_cells): (u64, u64),
) -> Result<Vec<f64>, FieldglassError> {
    coefficient_count(truncation)?;
    let limit = band_limit.min(truncation);
    let (nlat, nlon) = (latitudes_deg.len(), longitudes_deg.len());
    let work = synthesis_work(limit, nlat, nlon);
    if work > max_work {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis of T={limit} onto {nlon} × {nlat} points costs {work} \
             coefficient evaluations, over the budget of {max_work}"
        )));
    }
    let cells = synthesis_cells(limit, nlat, nlon);
    if cells > max_cells {
        return Err(FieldglassError::Parse(format!(
            "spectral synthesis of T={limit} onto {nlon} × {nlat} points allocates {cells} \
             values, over the budget of {max_cells}"
        )));
    }
    check_coefficients(coefficients, truncation)?;
    Ok(synthesize_unchecked(
        coefficients,
        truncation,
        limit,
        latitudes_deg,
        longitudes_deg,
    ))
}

/// How many output cells the spread writes per latitude block: rows are
/// grouped so a block of the output stays in cache while every column's phases
/// stream past it once.
const SPREAD_BLOCK_CELLS: usize = 8_192;

/// The kernel: `Σ_{m ≤ L} Σ_{n=m}^{L}` at every `(latitude, longitude)`, the
/// coefficients read in place from the truncation-`T` array.
///
/// Two stages, each in the order that reuses its tables:
///
/// 1. **Legendre**, column by column: for each order `m` the recurrence
///    coefficients of that column are built once and reused at every
///    latitude, giving each `(m, latitude)` its reduction `(re, im)`. Each
///    latitude carries the sectoral term `P̄_m^m` from one column to the next
///    as an [`XNum`], and [`column_sum`] runs each column scaled until it is
///    back in range.
/// 2. **Spread** over longitude, a block of output rows at a time, every
///    column's `cos(mλ)`, `sin(mλ)` read from one table.
///
/// Per output cell the terms arrive in the same order, by the same
/// expressions, as the row-by-row form this replaced, so the result is
/// bit-identical to it wherever that one stayed in `f64`'s normal range; what
/// changed is that no table grows with `L²`. Callers have checked the
/// coefficient count; nothing here indexes past it.
fn synthesize_unchecked(
    coefficients: &[f64],
    truncation: u32,
    limit: u32,
    latitudes_deg: &[f64],
    longitudes_deg: &[f64],
) -> Vec<f64> {
    let t = truncation as usize;
    let l = limit.min(truncation) as usize;
    let (nlat, nlon) = (latitudes_deg.len(), longitudes_deg.len());
    // Every length below is a term of `synthesis_cells`, which the caller's
    // budget bounds (or, for one point, `O(T)`), so none of these multiplies
    // can overflow.
    let mut out = vec![0.0; nlat * nlon];
    if nlat == 0 || nlon == 0 {
        return out;
    }

    // ── Stage 1: each column's reduction at each latitude ───────────────────
    // Per latitude: μ, cos φ, and the sectoral term of the current column.
    let mut rows: Vec<(f64, f64, XNum)> = latitudes_deg
        .iter()
        .map(|lat| {
            let mu = lat.to_radians().sin();
            (mu, (1.0 - mu * mu).max(0.0).sqrt(), XNum::ONE)
        })
        .collect();
    // `m`-major: `sums[m * nlat + latitude]`, zero for a column skipped there.
    let mut sums: Vec<(f64, f64)> = Vec::with_capacity((l + 1) * nlat);
    let mut ab: Vec<(f64, f64)> = Vec::with_capacity(l.saturating_sub(1));
    // `start` is where column `m` begins in the flat pairs: `n = m..=T` are
    // stored, and `n = m..=L` are read.
    let mut start = 0usize;
    for m in 0..=l {
        let mf = m as f64;
        let column = &coefficients[2 * start..2 * (start + l - m + 1)];
        ab.clear();
        ab.extend(((m + 2)..=l).map(|n| recurrence(n, m)));
        // P̄_{m+1}^{m+1} = √((2m+3)/(2m+2))·cos φ·P̄_m^m.
        let advance = ((2.0 * mf + 3.0) / (2.0 * mf + 2.0)).sqrt();
        for (mu, s, seed) in &mut rows {
            sums.push(column_sum(*seed, *mu, mf, column, &ab).unwrap_or((0.0, 0.0)));
            if m < l {
                *seed = seed.scaled(advance * *s);
            }
        }
        start += t - m + 1;
    }

    // ── Stage 2: spread over longitude ───────────────────────────────────────
    // cos(mλ) and sin(mλ) for every column `m ≥ 1`, `m`-major.
    let lon_rad: Vec<f64> = longitudes_deg.iter().map(|l| l.to_radians()).collect();
    let mut cos_tab = vec![0.0f64; l * nlon];
    let mut sin_tab = vec![0.0f64; l * nlon];
    for ((m, cos_m), sin_m) in (1..=l)
        .zip(cos_tab.chunks_exact_mut(nlon))
        .zip(sin_tab.chunks_exact_mut(nlon))
    {
        let mf = m as f64;
        for ((c, s), &lr) in cos_m.iter_mut().zip(sin_m.iter_mut()).zip(&lon_rad) {
            let ang = mf * lr;
            *c = ang.cos();
            *s = ang.sin();
        }
    }
    let block_rows = (SPREAD_BLOCK_CELLS / nlon).max(1);
    for (b, block) in out.chunks_mut(block_rows * nlon).enumerate() {
        let first = b * block_rows;
        for m in 0..=l {
            let column = &sums[m * nlat + first..];
            for (row, &(re, im)) in block.chunks_exact_mut(nlon).zip(column) {
                // A skipped column, or one that sums to nothing: adding its
                // zero would change no cell but a negative zero.
                if re == 0.0 && im == 0.0 {
                    continue;
                }
                if m == 0 {
                    // The m = 0 term is longitude-independent (its imaginary
                    // part is zero for a real field).
                    for cell in row.iter_mut() {
                        *cell += re;
                    }
                } else {
                    // F += 2·[Re_m·cos(mλ) − Im_m·sin(mλ)].
                    let cos_m = &cos_tab[(m - 1) * nlon..m * nlon];
                    let sin_m = &sin_tab[(m - 1) * nlon..m * nlon];
                    for ((cell, c), s) in row.iter_mut().zip(cos_m).zip(sin_m) {
                        *cell += 2.0 * (re * c - im * s);
                    }
                }
            }
        }
    }
    out
}

/// One column's reduction at one latitude: `Σ_{n=m}^{L} P̄_n^m(μ)·F_{n,m}` as
/// `(re, im)`, from the sectoral term `seed = P̄_m^m`, or `None` when no term of
/// the column is in `f64`'s range before `n = L`.
///
/// `column` is the column's stored pairs for `n = m..=L`, and `ab[k]` the
/// recurrence coefficients for `n = m + 2 + k`.
///
/// While either carried term is below `2^-480` the recurrence runs on
/// [`XNum`]s, and a term below that is not summed: it is below every term an
/// `f64` sum of order-one values can register, and it is the stretch where the
/// plain recurrence loses its bits. `P̄_n^m` grows monotonically through that
/// stretch, so once both carried terms are in range the rest of the column
/// runs in plain `f64` and stays there (`|P̄_n^m| ≤ √(2n + 1)`). When both
/// start in range — every column the plain recurrence got right — this is the
/// plain recurrence, operation for operation.
#[inline]
fn column_sum(
    seed: XNum,
    mu: f64,
    mf: f64,
    column: &[f64],
    ab: &[(f64, f64)],
) -> Option<(f64, f64)> {
    let terms = column.len() / 2;
    // A pole's cos φ is zero, so every column past m = 0 is zero there.
    if seed.x == 0.0 {
        return None;
    }
    if terms == 1 {
        return (seed.exp == 0).then(|| (seed.x * column[0], seed.x * column[1]));
    }
    // n = m + 1: P̄_{m+1}^m = √(2m+3)·μ·P̄_m^m.
    let mut p2 = seed;
    let mut p1 = seed.scaled((2.0 * mf + 3.0).sqrt() * mu);
    // The next term to compute is `n = m + next`.
    let mut next = 2;
    let (mut re, mut im);
    if p2.exp == 0 && p1.exp == 0 {
        re = p2.x * column[0];
        im = p2.x * column[1];
        re += p1.x * column[2];
        im += p1.x * column[3];
    } else {
        // Sum the terms that are in range as they arrive, and treat the rest
        // as the zero they round to.
        let mut sum: Option<(f64, f64)> = None;
        let add = |p: XNum, k: usize, sum: &mut Option<(f64, f64)>| {
            if p.exp == 0 {
                let (c_re, c_im) = (column[2 * k], column[2 * k + 1]);
                match sum {
                    None => *sum = Some((p.x * c_re, p.x * c_im)),
                    Some((re, im)) => {
                        *re += p.x * c_re;
                        *im += p.x * c_im;
                    }
                }
            }
        };
        add(p2, 0, &mut sum);
        add(p1, 1, &mut sum);
        while next < terms && (p1.exp != 0 || p2.exp != 0) {
            let (a, b) = ab[next - 2];
            let p_n = XNum::combine(a * mu, p1, -b, p2);
            add(p_n, next, &mut sum);
            p2 = p1;
            p1 = p_n;
            next += 1;
        }
        if p1.exp != 0 || p2.exp != 0 {
            // The column ended before it was back in range: whatever it summed
            // on the way, if anything, is all it has.
            return sum;
        }
        // Both carried terms are in range, so both were summed.
        (re, im) = sum?;
    }
    let (mut p1, mut p2) = (p1.x, p2.x);
    for (&[c_re, c_im], &(a, b)) in column[2 * next..]
        .as_chunks::<2>()
        .0
        .iter()
        .zip(&ab[next - 2..])
    {
        let p_n = a * mu * p1 - b * p2;
        re += p_n * c_re;
        im += p_n * c_im;
        p2 = p1;
        p1 = p_n;
    }
    Some((re, im))
}

/// `2^960`: one step of an [`XNum`]'s exponent.
const X_BIG: f64 = f64::from_bits((1023 + 960) << 52);
/// `2^-960`.
const X_BIG_INV: f64 = f64::from_bits((1023 - 960) << 52);
/// `2^480` and `2^-480`, the band an [`XNum`]'s mantissa is kept inside.
const X_HIGH: f64 = f64::from_bits((1023 + 480) << 52);
/// See [`X_HIGH`].
const X_LOW: f64 = f64::from_bits((1023 - 480) << 52);

/// An extended-exponent number, `x · 2^(960·exp)` (Fukushima 2012, *J. Geodesy*
/// 86): the mantissa stays inside `[2^-480, 2^480)` and the exponent carries
/// the rest, so a value can fall far below `f64::MIN_POSITIVE` without losing
/// bits. SHTns and ducc0 carry the same thing beside each column's start.
///
/// Only what the kernel needs: scaling by an `f64`, the two-term linear
/// combination the Legendre recurrence is, and the exponent test. With
/// `exp == 0` each is the plain `f64` expression, and scaling by a power of two
/// commutes with rounding, so a value that stays in `f64`'s normal range has
/// the same bits either way.
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
}

/// The two-term normalised Legendre recurrence coefficients for `(n, m)`,
/// `n ≥ m + 2`: `P̄_n^m = a·μ·P̄_{n−1}^m − b·P̄_{n−2}^m`.
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

    /// A grid that is affordable by *time* and ruinous by *space*, which is why
    /// `synthesis_cells` exists beside `synthesis_work`: a degenerate
    /// truncation on a raster past one field's worth of points. Every column
    /// term vanishes, so the work is the raster once, and the output alone is a
    /// hundred million values.
    #[test]
    fn rejects_a_grid_the_work_budget_admits_but_the_allocation_does_not() {
        assert!(synthesis_work(0, 10_000, 10_000) <= MAX_SYNTHESIS_WORK);
        assert!(synthesis_cells(0, 10_000, 10_000) > MAX_SYNTHESIS_CELLS);

        // And the transform refuses it rather than allocating for it. `T = 0`
        // takes two coefficient values, so this is a well-formed call in every
        // way except its grid.
        let lats = vec![0.0; 10_000];
        let lons = vec![0.0; 10_000];
        let Err(err) = synthesize_spherical_harmonic(&[1.0, 0.0], 0, &lats, &lons) else {
            panic!("a grid past the allocation budget must be refused");
        };
        // `allocates`, not just `over the budget`: both messages end that way,
        // and this case must be refused by the allocation budget specifically —
        // the whole point is that the work budget admits it.
        assert!(err.to_string().contains("allocates"), "{err}");

        // What the column-at-a-time kernel no longer allocates: one latitude at
        // the largest truncation used to need a `T(T−1)` recurrence table
        // (537 MB), refused; it now needs one column's, beside its phases.
        assert!(synthesis_cells(MAX_TRUNCATION, 1, 1_000) < MAX_SYNTHESIS_CELLS);
        assert_eq!(
            synthesis_cells(MAX_TRUNCATION, SYNTHESIS_NJ, SYNTHESIS_NI),
            17_991_736
        );
    }

    /// The map's budgets are the map of the largest truncation the cap admits
    /// — band-limited to what the pinned grid carries — so no map is refused
    /// by either. Checked as arithmetic here;
    /// `a_band_limited_map_costs_the_limit_not_the_file` runs one.
    #[test]
    fn the_map_budgets_are_the_map_of_the_largest_truncation_admitted() {
        let limit = spectral_render_band_limit(MAX_TRUNCATION);
        assert_eq!(limit, 359);
        assert_eq!(
            synthesis_work(limit, SYNTHESIS_NJ, SYNTHESIS_NI),
            MAX_MAP_SYNTHESIS_WORK
        );
        assert_eq!(
            synthesis_cells(limit, SYNTHESIS_NJ, SYNTHESIS_NI),
            MAX_MAP_SYNTHESIS_CELLS
        );
        // The numbers the docs state, so a doc that drifts from them is caught.
        assert_eq!(MAX_MAP_SYNTHESIS_WORK, 140_356_800);
        assert_eq!(MAX_MAP_SYNTHESIS_CELLS, 1_041_124);
        // Every truncation's map fits, since each one's band limit is at most
        // the cap's.
        for t in [0, 63, 359, 360, 1279, 7999, MAX_TRUNCATION] {
            let limit = spectral_render_band_limit(t);
            assert!(synthesis_work(limit, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_MAP_SYNTHESIS_WORK);
            assert!(synthesis_cells(limit, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_MAP_SYNTHESIS_CELLS);
        }
    }

    /// A caller's grid has its own budget, sized by time rather than by the
    /// map (#637): the full sum at the cap on the pinned grid, which is what
    /// #631 accepted as the most one call may cost. What it admits and
    /// refuses, stated rather than discovered.
    #[test]
    fn the_caller_budget_admits_every_truncation_in_full_on_the_pinned_grid() {
        assert_eq!(MAX_SYNTHESIS_WORK, 26_361_739_449);
        assert_eq!(MAX_SYNTHESIS_CELLS, 64 * 1024 * 1024);
        // Every truncation there is, in full, on the half-degree grid.
        for t in [0, 63, 1279, 1810, 2500, 7999, MAX_TRUNCATION] {
            assert!(
                synthesis_work(t, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_SYNTHESIS_WORK,
                "T{t}"
            );
            assert!(
                synthesis_cells(t, SYNTHESIS_NJ, SYNTHESIS_NI) <= MAX_SYNTHESIS_CELLS,
                "T{t}"
            );
        }
        // T1279 in full on its own linear grid (MIR: N640, 2560 × 1281).
        assert!(synthesis_work(1279, 1281, 2560) <= MAX_SYNTHESIS_WORK);
        // T7999 in full on the quarter-degree grid is past it; band-limited to
        // what that grid resolves (T719) it is a twentieth of it.
        assert!(synthesis_work(7999, 721, 1440) > MAX_SYNTHESIS_WORK);
        assert!(synthesis_work(719, 721, 1440) < MAX_SYNTHESIS_WORK / 20);
        // The map's budget is far inside the caller's.
        const { assert!(MAX_MAP_SYNTHESIS_WORK < MAX_SYNTHESIS_WORK / 100) };
        const { assert!(MAX_MAP_SYNTHESIS_CELLS < MAX_SYNTHESIS_CELLS) };
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

    /// The allocation budgets bound every length the transform multiplies
    /// out, which is what makes those `usize` multiplies exact on a 32-bit
    /// target. That claim is the comment beside them; this is it as arithmetic.
    #[test]
    fn the_allocation_budget_keeps_every_length_inside_a_32_bit_usize() {
        assert!(MAX_SYNTHESIS_CELLS < u64::from(u32::MAX));
        assert!(MAX_MAP_SYNTHESIS_CELLS < u64::from(u32::MAX));
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
    /// the limit's cost, and the map is the triangle it claims to be.
    #[test]
    fn a_band_limited_map_costs_the_limit_not_the_file() {
        let t = 2_000;
        let limit = spectral_render_band_limit(t);
        let (lats, lons) = spectral_render_grid(t).axes();
        assert!(synthesis_work(t, lats.len(), lons.len()) > 10 * MAX_MAP_SYNTHESIS_WORK);
        let c = pseudo_random(t);
        let (grid, map) = synthesize_map(&c, t).expect("map");
        assert_eq!(grid, spectral_render_grid(t));
        assert_eq!(map.len(), lats.len() * lons.len());
        // Two rows are enough to show the map matches the triangle it claims
        // to be, bit for bit.
        let rows = &lats[100..102];
        let cut = triangle(&c, t, limit);
        let reference = synthesize_spherical_harmonic(&cut, limit, rows, &lons).expect("cut");
        assert_eq!(
            &map[100 * lons.len()..102 * lons.len()],
            reference.as_slice()
        );
    }

    /// What a caller's grid resolves, by its coarsest step on each axis.
    #[test]
    fn a_callers_grid_resolves_what_its_coarsest_step_carries() {
        // The pinned grid, read as points, is T359 as `grid_band_limit` says.
        let (lats, lons) = spectral_render_grid(0).axes();
        assert_eq!(points_band_limit(&lats, &lons), Some(359));
        // MIR's linear rule for regular lat/lon targets: 0.25° is T719, 1° T179.
        let axis = |from: f64, to: f64, step: f64| -> Vec<f64> {
            let n = ((to - from) / step).round() as usize;
            (0..=n).map(|k| from + k as f64 * step).collect()
        };
        assert_eq!(
            points_band_limit(&axis(-90.0, 90.0, 0.25), &axis(0.0, 359.75, 0.25)),
            Some(719)
        );
        assert_eq!(
            points_band_limit(&axis(-90.0, 90.0, 1.0), &axis(0.0, 359.0, 1.0)),
            Some(179)
        );
        // A step that rounds below its value is not charged a wavenumber.
        assert_eq!(
            points_band_limit(&axis(0.0, 10.0, 0.1), &axis(0.0, 10.0, 0.1)),
            Some(1799)
        );
        // The coarser axis decides, and order does not matter.
        assert_eq!(
            points_band_limit(&[10.0, 0.0, 5.0], &axis(0.0, 90.0, 0.5)),
            Some(35)
        );
        // A regional grid across the prime meridian is judged by its steps, not
        // by the gap outside it.
        assert_eq!(
            points_band_limit(&[0.0, 1.0], &[350.0, 355.0, 0.0, 5.0, -10.0]),
            Some(35)
        );
        // One point on an axis says nothing about it; one point says nothing.
        assert_eq!(
            points_band_limit(&[45.0], &axis(0.0, 359.0, 1.0)),
            Some(179)
        );
        assert_eq!(points_band_limit(&[45.0], &[120.0]), None);
        assert_eq!(points_band_limit(&[], &[]), None);
        assert_eq!(points_band_limit(&[f64::NAN, 1.0], &[2.0, 2.0]), None);
        // A step too coarse to carry anything carries T0.
        assert_eq!(points_band_limit(&[], &[0.0, 120.0, 240.0]), Some(0));
        assert_eq!(points_band_limit(&[-90.0, 0.0, 90.0], &[]), Some(1));
        // Two points on an axis do not sample it (#637): they leave the other
        // axis to decide, and with neither sampled the grid is evaluated in
        // full, as a point is.
        assert_eq!(points_band_limit(&[-90.0, 90.0], &[]), None);
        let global_lats = axis(-90.0, 90.0, 0.5);
        let global_lons = axis(0.0, 359.5, 0.5);
        assert_eq!(points_band_limit(&global_lats, &[0.0]), Some(359));
        assert_eq!(points_band_limit(&global_lats, &[0.0, 180.0]), Some(359));
        assert_eq!(points_band_limit(&[-45.0, 45.0], &global_lons), Some(359));
        assert_eq!(points_band_limit(&[30.0], &global_lons), Some(359));
        assert_eq!(points_band_limit(&[-45.0, 45.0], &[0.0, 180.0]), None);
        assert_eq!(points_band_limit(&[30.0], &[120.0]), None);
    }

    /// A grid of two sampled regions is judged by how each region is sampled,
    /// not by the gap between them (#812).
    #[test]
    fn a_gap_between_two_regions_is_not_a_step() {
        let axis = |from: f64, to: f64, step: f64| -> Vec<f64> {
            let n = ((to - from) / step).round() as usize;
            (0..=n).map(|k| from + k as f64 * step).collect()
        };
        let global_lons = axis(0.0, 359.5, 0.5);
        let global_lats = axis(-90.0, 90.0, 0.5);
        // Two polar caps at 0.5°: the 140° between them used to be the step,
        // and T0 is the field's global mean.
        let caps: Vec<f64> = [axis(-80.0, -70.0, 0.5), axis(70.0, 80.0, 0.5)].concat();
        assert_eq!(points_band_limit(&caps, &global_lons), Some(359));
        // Two longitude sectors at 0.5°, 170° apart either way round.
        let sectors: Vec<f64> = [axis(0.0, 10.0, 0.5), axis(180.0, 190.0, 0.5)].concat();
        assert_eq!(points_band_limit(&global_lats, &sectors), Some(359));
        // A sector that crosses the meridian is still one region.
        let across: Vec<f64> = [axis(350.0, 359.5, 0.5), axis(0.0, 10.0, 0.5)].concat();
        assert_eq!(points_band_limit(&global_lats, &across), Some(359));

        // One sampling with a hole is charged for it: up to three missing rows
        // are a coarse stretch, four or more separate two regions.
        let without = |missing: usize| -> Vec<f64> {
            global_lats
                .iter()
                .enumerate()
                .filter(|&(k, _)| !(100..100 + missing).contains(&k))
                .map(|(_, &x)| x)
                .collect()
        };
        assert_eq!(points_band_limit(&without(1), &global_lons), Some(179));
        assert_eq!(points_band_limit(&without(3), &global_lons), Some(89));
        assert_eq!(points_band_limit(&without(4), &global_lons), Some(359));

        // A coarse regular sample is not two regions: it resolves what its step
        // carries, and three longitudes round the circle carry T0, as
        // `grid_band_limit` says of a three-point ring. The full sum is the
        // reader's `_full` call (decided on #812).
        assert_eq!(
            points_band_limit(&[-60.0, 0.0, 60.0], &[0.0, 120.0, 240.0]),
            Some(0)
        );
        assert_eq!(grid_band_limit(3, 3), 0);
        // Six longitudes at 60° carry T2, as do three latitudes 60° apart.
        assert_eq!(
            points_band_limit(&[-60.0, 0.0, 60.0], &axis(0.0, 300.0, 60.0)),
            Some(2)
        );
    }

    /// A column whose sectoral term is in range but whose next term is not —
    /// a latitude a hair off the equator, where `μ` is below `2^-480` — still
    /// sums its first term. The field is continuous, so it is the equator's.
    #[test]
    fn a_column_keeps_its_terms_that_are_in_range() {
        let t = 12;
        let c = pseudo_random(t);
        let lons = [0.0, 33.0, 271.0];
        let at_equator = synthesize_spherical_harmonic(&c, t, &[0.0], &lons).expect("0°");
        let off = synthesize_spherical_harmonic(&c, t, &[1e-200], &lons).expect("1e-200°");
        for (a, b) in at_equator.iter().zip(&off) {
            assert!((a - b).abs() < 1e-12, "{a} vs {b}");
        }
    }

    /// The column skip: at a latitude where the high orders never climb back
    /// into range before the band limit, the transform still answers finite,
    /// agrees with the full triangle it is part of, and the highest columns
    /// contribute nothing.
    #[test]
    fn a_column_that_never_returns_to_range_is_skipped() {
        // cos(89.9°)^m leaves 2^-480 by m ≈ 51; at T200 those columns turn
        // only at n ≈ m / cos φ ≈ 29,000, far past the limit.
        let t = 200;
        let c = pseudo_random(t);
        let lats = [89.9];
        let lons = [0.0, 90.0];
        let full = synthesize_spherical_harmonic(&c, t, &lats, &lons).expect("full");
        assert!(full.iter().all(|v| v.is_finite()));
        let low = synthesize_band_limited(&c, t, 60, &lats, &lons).expect("m ≤ 60");
        let only_low = triangle(&c, t, 60);
        let reference = synthesize_spherical_harmonic(&only_low, 60, &lats, &lons).expect("cut");
        assert_eq!(low, reference);
    }
}
