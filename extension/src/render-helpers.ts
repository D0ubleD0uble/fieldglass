// Pure render helpers used to paint a decoded GRIB grid into an ImageData
// buffer. Kept separate from `provider.ts` so the math is unit-testable
// without a webview/DOM.
//
// All array accesses below are bounds-checked by surrounding loop conditions
// or `Math.min`. The security plugin can't see the structural invariants, so
// we disable detect-object-injection at the file level.
/* eslint-disable security/detect-object-injection */

/**
 * Anchor points for the viridis colormap (matplotlib default), sampled at
 * 11 positions across the [0, 1] domain. Linearly interpolated to build
 * the full 256-entry LUT at module load. We stay close to matplotlib's
 * published values; small interpolation drift is acceptable since this
 * is a viewer, not a reference plotting library.
 *
 * Source: matplotlib `_cm_listed.py` viridis_data, sampled at
 * indices 0, 25, 51, 76, 102, 127, 153, 178, 204, 229, 255 of the
 * matplotlib LUT.
 */
const VIRIDIS_ANCHORS: ReadonlyArray<readonly [number, number, number]> = [
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

/** Build a 256-entry RGB LUT from the viridis anchors. */
function buildViridisLut(): Uint8ClampedArray {
  const lut = new Uint8ClampedArray(256 * 3);
  const segs = VIRIDIS_ANCHORS.length - 1;
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    const seg = Math.min(Math.floor(t * segs), segs - 1);
    const localT = t * segs - seg;
    const a = VIRIDIS_ANCHORS[seg];
    const b = VIRIDIS_ANCHORS[seg + 1];
    const r = a[0] + (b[0] - a[0]) * localT;
    const g = a[1] + (b[1] - a[1]) * localT;
    const bl = a[2] + (b[2] - a[2]) * localT;
    lut[i * 3 + 0] = Math.round(r * 255);
    lut[i * 3 + 1] = Math.round(g * 255);
    lut[i * 3 + 2] = Math.round(bl * 255);
  }
  return lut;
}

/**
 * 256-entry viridis LUT, baked at module load. Format: flat RGB triples,
 * `[r0, g0, b0, r1, g1, b1, …]`, one byte per channel. No alpha.
 */
export const VIRIDIS_LUT: Readonly<Uint8ClampedArray> = buildViridisLut();

/** Look up a viridis color for a normalized value in [0, 1]. */
export function viridis(t: number): [number, number, number] {
  const tt = Math.max(0, Math.min(1, t));
  const idx = Math.round(tt * 255);
  return [
    VIRIDIS_LUT[idx * 3 + 0],
    VIRIDIS_LUT[idx * 3 + 1],
    VIRIDIS_LUT[idx * 3 + 2],
  ];
}

/**
 * Compute (min, max) of a numeric grid, ignoring `null` (BMS-masked) entries.
 * Returns `null` when every entry is masked or the grid is empty.
 */
export function minMaxIgnoringMask(
  values: ReadonlyArray<number | null>
): { min: number; max: number } | null {
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  let seen = false;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (v === null) continue;
    if (!Number.isFinite(v)) continue;
    if (v < min) min = v;
    if (v > max) max = v;
    seen = true;
  }
  if (!seen) return null;
  return { min, max };
}

/**
 * Paint a row-major grid into an RGBA byte buffer suitable for `ImageData`.
 *
 * - Masked points (`null`) and non-finite numbers render as fully transparent
 *   (alpha = 0). This matches the policy documented in the colorbar legend
 *   ("masked: transparent").
 * - When `min == max` (a constant field) every cell paints at LUT index 0.
 * - The output buffer has length `nx * ny * 4`.
 */
export function paintGridRgba(
  values: ReadonlyArray<number | null>,
  nx: number,
  ny: number,
  min: number,
  max: number
): Uint8ClampedArray {
  if (nx <= 0 || ny <= 0) {
    return new Uint8ClampedArray(0);
  }
  const out = new Uint8ClampedArray(nx * ny * 4);
  const span = max - min;
  const denom = span > 0 ? span : 1;

  for (let i = 0; i < nx * ny; i++) {
    const v = i < values.length ? values[i] : null;
    const o = i * 4;
    if (v === null || !Number.isFinite(v)) {
      // transparent for masked / no-data points
      out[o + 0] = 0;
      out[o + 1] = 0;
      out[o + 2] = 0;
      out[o + 3] = 0;
      continue;
    }
    const t = span > 0 ? (v - min) / denom : 0;
    const tt = t < 0 ? 0 : t > 1 ? 1 : t;
    const idx = Math.round(tt * 255) * 3;
    out[o + 0] = VIRIDIS_LUT[idx + 0];
    out[o + 1] = VIRIDIS_LUT[idx + 1];
    out[o + 2] = VIRIDIS_LUT[idx + 2];
    out[o + 3] = 255;
  }
  return out;
}
