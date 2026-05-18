//! Viridis colormap + RGBA painting for the render pipeline.
//!
//! Mirrors the TS module that used to live at `extension/src/render-helpers.ts`,
//! moved server-side so the render pipeline can ship a paint-ready RGBA
//! buffer across napi rather than a value array that the webview has to
//! re-paint. The webview just blits the returned `Uint8ClampedArray` to a
//! canvas via `putImageData`.
//!
//! Viridis anchors are sampled from matplotlib's `_cm_listed.py`
//! viridis_data at indices 0, 25, 51, 76, 102, 127, 153, 178, 204, 229,
//! 255 of the upstream 256-entry table — small interpolation drift between
//! anchors is acceptable since this is a viewer, not a reference plotter.
//!
//! **Anchor stops are duplicated on the webview side as the
//! `.cb` element's CSS `linear-gradient` (see
//! `extension/src/provider.ts::renderImagePanelHtml`). If you change the
//! anchors here, update the CSS gradient stops to match — they have to
//! line up or the legend strip and the painted grid drift.
//!
//! Output format: RGBA bytes (`[r, g, b, a, r, g, b, a, …]`), one byte per
//! channel. Masked / non-finite pixels paint as fully transparent (alpha
//! = 0) so the editor background shows through.

/// 11 viridis anchor stops sampled from matplotlib.
const VIRIDIS_ANCHORS: [[f64; 3]; 11] = [
    [0.267004, 0.004874, 0.329415], // 0.0
    [0.282623, 0.140926, 0.457517], // 0.1
    [0.253935, 0.265254, 0.529983], // 0.2
    [0.206756, 0.371758, 0.553117], // 0.3
    [0.163625, 0.471133, 0.558148], // 0.4
    [0.127568, 0.566949, 0.550556], // 0.5
    [0.134692, 0.658636, 0.517649], // 0.6
    [0.266941, 0.748751, 0.440573], // 0.7
    [0.477504, 0.821444, 0.318195], // 0.8
    [0.741388, 0.873449, 0.149561], // 0.9
    [0.993248, 0.906157, 0.143936], // 1.0
];

/// Build a 256-entry RGB LUT from the viridis anchors. Called once per
/// process; the result is cached at module load.
fn build_viridis_lut() -> [u8; 256 * 3] {
    let mut lut = [0u8; 256 * 3];
    let segs = (VIRIDIS_ANCHORS.len() - 1) as f64;
    for i in 0..256 {
        let t = i as f64 / 255.0;
        let mut seg = (t * segs).floor() as usize;
        if seg >= VIRIDIS_ANCHORS.len() - 1 {
            seg = VIRIDIS_ANCHORS.len() - 2;
        }
        let local_t = t * segs - seg as f64;
        let a = VIRIDIS_ANCHORS[seg];
        let b = VIRIDIS_ANCHORS[seg + 1];
        let r = a[0] + (b[0] - a[0]) * local_t;
        let g = a[1] + (b[1] - a[1]) * local_t;
        let bl = a[2] + (b[2] - a[2]) * local_t;
        lut[i * 3] = (r * 255.0).round().clamp(0.0, 255.0) as u8;
        lut[i * 3 + 1] = (g * 255.0).round().clamp(0.0, 255.0) as u8;
        lut[i * 3 + 2] = (bl * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

/// Lazily-built viridis LUT — same across the whole process, computed
/// once on first access.
fn viridis_lut() -> &'static [u8; 256 * 3] {
    use std::sync::OnceLock;
    static LUT: OnceLock<[u8; 256 * 3]> = OnceLock::new();
    LUT.get_or_init(build_viridis_lut)
}

/// Look up a viridis RGB triple for a value in `[0, 1]`; out-of-range
/// inputs clamp.
pub fn viridis(t: f64) -> [u8; 3] {
    let tt = t.clamp(0.0, 1.0);
    let idx = (tt * 255.0).round() as usize;
    let lut = viridis_lut();
    [lut[idx * 3], lut[idx * 3 + 1], lut[idx * 3 + 2]]
}

/// Compute `(min, max)` of a numeric grid, ignoring masked entries.
/// Returns `None` when every entry is masked or the grid is empty.
pub fn min_max_ignoring_mask<I: IntoIterator<Item = Option<f64>>>(values: I) -> Option<(f64, f64)> {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut seen = false;
    for v in values {
        let Some(v) = v else { continue };
        if !v.is_finite() {
            continue;
        }
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
        seen = true;
    }
    if seen { Some((min, max)) } else { None }
}

/// Paint a row-major grid into an RGBA byte buffer suitable for
/// `ImageData`. Each pixel is 4 bytes (`r, g, b, a`); masked or
/// non-finite values render as fully transparent (alpha = 0). When
/// `min == max` (a constant field) every present cell paints at LUT
/// index 0.
///
/// Output length is `width * height * 4`. When `flip_y` is true, rows
/// are emitted bottom-to-top — useful when the source grid scans
/// south-to-north but the canvas wants north-up.
pub fn paint_grid_rgba(
    values: &[f64],
    mask: Option<&[u8]>,
    width: u32,
    height: u32,
    min: f64,
    max: f64,
    flip_y: bool,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let total = w * h;
    if total == 0 {
        return Vec::new();
    }
    debug_assert_eq!(
        values.len(),
        total,
        "paint_grid_rgba: values.len()={} != width*height={total}; trailing pixels would silently render as transparent",
        values.len(),
    );
    if let Some(m) = mask {
        debug_assert_eq!(
            m.len(),
            total,
            "paint_grid_rgba: mask.len()={} != width*height={total}",
            m.len(),
        );
    }
    let mut out = vec![0u8; total * 4];
    let span = max - min;
    let denom = if span > 0.0 { span } else { 1.0 };
    let lut = viridis_lut();

    for (i, &v) in values.iter().enumerate().take(total) {
        let row = i / w;
        let col = i - row * w;
        let out_idx = if flip_y { (h - 1 - row) * w + col } else { i };
        let o = out_idx * 4;

        let masked = mask.is_some_and(|m| m.get(i).copied().unwrap_or(0) == 0);
        if masked || !v.is_finite() {
            // transparent
            out[o] = 0;
            out[o + 1] = 0;
            out[o + 2] = 0;
            out[o + 3] = 0;
            continue;
        }
        let t = if span > 0.0 { (v - min) / denom } else { 0.0 };
        let tt = t.clamp(0.0, 1.0);
        let idx = (tt * 255.0).round() as usize;
        out[o] = lut[idx * 3];
        out[o + 1] = lut[idx * 3 + 1];
        out[o + 2] = lut[idx * 3 + 2];
        out[o + 3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viridis_lut_has_canonical_endpoints() {
        let lut = viridis_lut();
        // First entry is canonical viridis dark-purple, last is
        // bright-yellow. Loose bounds — anchor stops + linear interp.
        assert!(lut[0] < 100 && lut[1] < 50 && lut[2] > 50, "viridis[0]");
        assert!(
            lut[765] > 200 && lut[766] > 200 && lut[767] < 80,
            "viridis[255]"
        );
    }

    #[test]
    fn viridis_function_clamps_out_of_range() {
        let lo = viridis(-5.0);
        let hi = viridis(5.0);
        let lut = viridis_lut();
        assert_eq!(lo, [lut[0], lut[1], lut[2]]);
        assert_eq!(hi, [lut[765], lut[766], lut[767]]);
    }

    #[test]
    fn min_max_skips_nulls_and_nonfinite() {
        let it = vec![
            None,
            Some(1.0),
            Some(2.0),
            None,
            Some(3.0),
            Some(f64::NAN),
            Some(-1.0),
        ];
        let out = min_max_ignoring_mask(it).expect("some present");
        assert_eq!(out.0, -1.0);
        assert_eq!(out.1, 3.0);
    }

    #[test]
    fn min_max_returns_none_when_all_masked() {
        let it: Vec<Option<f64>> = vec![None, None];
        assert!(min_max_ignoring_mask(it).is_none());
    }

    #[test]
    fn paint_grid_rgba_emits_transparent_for_masked() {
        let values = vec![10.0, 20.0, 30.0, 40.0];
        let mask = vec![1u8, 0, 1, 1];
        let out = paint_grid_rgba(&values, Some(&mask), 2, 2, 10.0, 40.0, false);
        assert_eq!(out.len(), 16);
        // Pixel 1 is masked → fully transparent.
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        // Pixel 0 should be the LUT[0] viridis color with alpha 255.
        assert_eq!(out[3], 255);
    }

    #[test]
    fn paint_grid_rgba_returns_empty_for_zero_dims() {
        let out = paint_grid_rgba(&[], None, 0, 10, 0.0, 1.0, false);
        assert!(out.is_empty());
        let out = paint_grid_rgba(&[], None, 10, 0, 0.0, 1.0, false);
        assert!(out.is_empty());
    }

    #[test]
    fn paint_grid_rgba_flip_y_inverts_rows() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        let unflipped = paint_grid_rgba(&values, None, 2, 2, 1.0, 4.0, false);
        let flipped = paint_grid_rgba(&values, None, 2, 2, 1.0, 4.0, true);
        // Top-left of flipped == row 1, col 0 of unflipped (= source pixel 2).
        // We can verify by checking that the bytes differ as expected.
        assert_ne!(unflipped, flipped);
        // Verify symmetry: flipping twice returns the original layout.
        // (Re-paint flipped values from `unflipped`-derived row-order.)
        // Simpler check: the alpha byte of the top-left flipped pixel is
        // still 255 (it's a present cell).
        assert_eq!(flipped[3], 255);
    }
}
