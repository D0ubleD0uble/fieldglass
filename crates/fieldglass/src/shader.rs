//! The GPU half of the one colour path (ADR-0006 decision 3).
//!
//! A browser host colour-maps on the GPU so restyling never re-decodes. That
//! would normally mean a second colour implementation drifting away from the
//! CPU painter. It does not here: the painter's [`Palette`] leaves Rust as
//! data, this module rebases the field into the form a shader can divide, and
//! [`GLSL`] is the only shader anyone writes. The CPU painter is therefore the
//! *oracle* the GPU path is tested against rather than its rival.
//!
//! # Why the field is rebased in Rust
//!
//! [`Palette::normalise`] is `(transform(v) - t0) / (t1 - t0)`. A shader that
//! does the subtraction in `f32` loses the **domain**, not just the result:
//! once the `f32` gap at `t0` exceeds one lookup step — roughly
//! `t1 - t0 < 255 · |t0| · 2⁻²³` — the error is unbounded within the ramp.
//! Measured at up to 127 lookup entries, half the table, for a manual range of
//! 1.0 over values near 1e7. Geopotential in m²/s², pressure in Pa, and
//! radiances all reach that regime under a tight manual range.
//!
//! So [`shader_values`] uploads `transform(v) - t0`, computed in `f64`, and the
//! shader only ever divides by a span it is handed.

use fieldglass_core::colormap::{Palette, ScaleMode};

use crate::api::Field;

/// The shader a GPU host pastes into its own fragment program.
///
/// Exported as one string rather than described in prose so an app composes it
/// and never rewrites it. It reproduces [`Palette::rgba`] exactly: the mask
/// texture decides whether a cell has a colour at all, the field texture holds
/// the already-transformed, already-rebased value, and the lookup table is
/// sampled `NEAREST` at `round(t · 255)` — the same rounding
/// [`Palette::index`] does.
pub const GLSL: &str = r#"// fieldglass palette shader (GLSL ES 3.00 / WebGL2).
// Consumes a Palette from the fieldglass package. Do not re-derive the colour
// rule here: the CPU painter is the oracle, and this reproduces it exactly.
//
//   u_field      : R32F,  one texel per cell — shaderValues(field, options)
//   u_mask       : R8,    one texel per cell — shaderMask(field, options)
//   u_lut        : RGBA8, 256 x 1, NEAREST/CLAMP — palette.lut
//   u_span       : palette.t1 - palette.t0
//   u_maskedRgba : palette.maskedRgba / 255.0

uniform highp sampler2D u_field;
uniform highp sampler2D u_mask;
uniform highp sampler2D u_lut;
uniform highp float u_span;
uniform highp vec4 u_maskedRgba;

vec4 fieldglassColor(vec2 uv) {
    if (texture(u_mask, uv).r < 0.5) {
        return u_maskedRgba;
    }
    // Already log10'd where the palette says so, and already rebased by t0.
    // A degenerate domain (t1 == t0, a constant field) puts everything on the
    // low end, which is what the CPU painter does.
    float d = texture(u_field, uv).r;
    float t = u_span > 0.0 ? clamp(d / u_span, 0.0, 1.0) : 0.0;
    float idx = floor(t * 255.0 + 0.5);
    return texture(u_lut, vec2((idx + 0.5) / 256.0, 0.5));
}
"#;

/// Cell `k` in the palette's transformed domain, or `None` when it has no place
/// on the ramp.
///
/// Mirrors `core`'s `position_in` up to (but not including) the rebasing and
/// the clamp, which are the shader's two jobs.
fn transformed(field: &Field, k: usize, scale: ScaleMode) -> Option<f64> {
    if field.mask.get(k).copied().unwrap_or(0) == 0 {
        return None;
    }
    let v = field.values.get(k)?;
    if !v.is_finite() {
        return None;
    }
    match scale {
        ScaleMode::Linear => Some(v),
        // No logarithm for a non-positive value — it has no place on a log
        // ramp and paints as missing, which `shader_mask` records.
        ScaleMode::Log10 => (v > 0.0).then(|| v.log10()),
    }
}

/// The field as an `R32F` texture wants it: transformed by the palette's scale
/// and rebased by `t0`, narrowed to `f32` only after that subtraction happened
/// in `f64`.
///
/// A cell with no place on the ramp reads `0.0` and is excluded by
/// [`shader_mask`], so the texture never carries a `NaN` — which is the whole
/// reason the mask is a separate array.
pub fn shader_values(field: &Field, palette: &Palette) -> Vec<f32> {
    (0..field.mask.len())
        .map(|k| match transformed(field, k, palette.scale) {
            Some(tv) => (tv - palette.t0) as f32,
            None => 0.0,
        })
        .collect()
}

/// The mask an `R8` texture wants: the field's own mask with the cells the
/// palette's scale excludes also cleared.
///
/// Under [`ScaleMode::Linear`] this equals `field.mask`. Under
/// [`ScaleMode::Log10`] a non-positive value has no logarithm, and clearing it
/// here is what stops the GPU and the CPU disagreeing about that cell.
pub fn shader_mask(field: &Field, palette: &Palette) -> Vec<u8> {
    (0..field.mask.len())
        .map(|k| u8::from(transformed(field, k, palette.scale).is_some()))
        .collect()
}

/// The lookup-table entry [`GLSL`] selects, evaluated in the shader's own
/// precision.
///
/// This is not a second colour rule — it is the same three lines the shader
/// runs, in `f32`, so a test can compare them against [`Palette::index`]
/// without a GPU. The browser smoke page checks that the real GLSL agrees with
/// this; this checks that the *arithmetic* agrees with the painter.
pub fn shader_index(d: f32, span: f32) -> u8 {
    let t = if span > 0.0 {
        (d / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // `t` is in [0, 1], so the product is in [0, 255] and the saturating cast
    // cannot truncate. GLSL's `floor(x + 0.5)` and Rust's `round` agree for
    // non-negative x.
    (t * 255.0 + 0.5).floor() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::colormap::colormaps;

    /// The rebasing exists for a domain a shader cannot subtract in `f32`.
    /// Without it, a 1 K range over geopotential-sized values collapses to a
    /// handful of distinct table entries; with it, the ramp is intact.
    #[test]
    fn rebasing_keeps_a_tight_range_over_large_values() {
        let base = 1.0e7_f64;
        let palette = Palette::build(&colormaps()[0], false, base, base + 1.0, ScaleMode::Linear);
        let span = (palette.t1 - palette.t0) as f32;

        let mut rebased = std::collections::BTreeSet::new();
        let mut naive = std::collections::BTreeSet::new();
        for k in 0..256 {
            let v = base + f64::from(k) / 255.0;
            rebased.insert(shader_index((v - palette.t0) as f32, span));
            // What a shader that subtracted in `f32` would compute.
            naive.insert(shader_index(v as f32 - palette.t0 as f32, span));
        }
        assert_eq!(
            rebased.len(),
            256,
            "the rebased ramp must resolve every lookup entry"
        );
        assert!(
            naive.len() < 8,
            "an f32 subtraction should visibly collapse this domain, saw {} entries",
            naive.len()
        );
    }

    /// A degenerate domain (a constant field) puts everything on the low end,
    /// matching `position_in`'s `span > 0.0` guard.
    #[test]
    fn a_degenerate_domain_paints_the_low_end() {
        assert_eq!(shader_index(0.0, 0.0), 0);
        assert_eq!(shader_index(5.0, -1.0), 0);
    }

    #[test]
    fn the_index_saturates_rather_than_wrapping() {
        assert_eq!(shader_index(-10.0, 1.0), 0);
        assert_eq!(shader_index(10.0, 1.0), 255);
    }
}
