// Overlay layer data sources for the render panel (#72, extended in #237).
//
// This module produces geographic `(lat, lon)` polylines for the bundled vector
// layers (coastline, borders, lakes, rivers) and the graticule, and nothing more
// — the *projection* into pixel space happens in Rust
// (`Grib{1,2}Handle.projectOverlay`), which owns every bit of the forward map.
// Keeping the math on the Rust side is the contract: the webview never
// reprojects, so it can never drift from the warp.
//
// Every layer hands back the same flat shape the napi `projectOverlay` call
// expects — `latlon` as `[lat, lon, lat, lon, …]` plus a `ringLengths` count
// per polyline — so a new layer is just another producer of this shape, and a
// future user-defined-shape layer needs zero Rust change.

import * as fs from "fs";
import * as path from "path";

/** Geographic polylines as the flat pair `projectOverlay` consumes:
 *  `latlon` is `[lat, lon, …]`; `ringLengths[k]` is the vertex count of
 *  polyline `k` (so `sum(ringLengths) * 2 === latlon.length`). */
export interface OverlayGeometry {
  latlon: Float64Array;
  ringLengths: Uint32Array;
}

/** Shape of a bundled vector asset: each line is flat `[lon, lat, …]`
 *  (GeoJSON coordinate order). Converted to `[lat, lon, …]` on load. */
interface VectorAsset {
  lines: number[][];
}

/** The bundled Natural Earth 1:110m vector layers, all public domain. The
 *  coastline predates the others; the rest are built by
 *  `tools/build_overlay_layers.py`. Lakes are stroked as their boundary rings —
 *  the overlay pipeline draws lines and fills nothing. */
export const VECTOR_LAYERS = ["coastline", "borders", "lakes", "rivers"] as const;

export type VectorLayer = (typeof VECTOR_LAYERS)[number];

/** Parsed geometry per layer, kept for the extension-host session. Each asset
 *  is read and flattened at most once however many panels ask for it. */
const layerCache = new Map<VectorLayer, OverlayGeometry>();

/** Load + cache a bundled Natural Earth 1:110m vector layer as
 *  `OverlayGeometry`. The assets sit beside the compiled output under
 *  `media/`, located the same way `native.ts` finds the native binary
 *  (relative to `__dirname`).
 *
 *  `layer` is a member of the closed {@link VECTOR_LAYERS} set — never a
 *  caller-supplied string — so the filename can't be steered off `media/`. */
export function loadVectorLayer(layer: VectorLayer): OverlayGeometry {
  const cached = layerCache.get(layer);
  if (cached) {
    return cached;
  }
  const file = path.join(__dirname, "..", "media", `${layer}-110m.json`);
  // Path is built from `__dirname` + a name from the closed layer set above,
  // never user input.
  // eslint-disable-next-line security/detect-non-literal-fs-filename
  const asset = JSON.parse(fs.readFileSync(file, "utf8")) as VectorAsset;
  const geometry = flattenLonLatLines(asset.lines);
  layerCache.set(layer, geometry);
  return geometry;
}

/** Flatten lines into the `OverlayGeometry` contract shape. When `swapPairs`
 *  is set, each line is read as `(lon, lat)` pairs and emitted swapped to
 *  `(lat, lon)` (GeoJSON → contract order); otherwise it is copied verbatim
 *  (already `[lat, lon, …]`). */
function flattenLines(lines: number[][], swapPairs: boolean): OverlayGeometry {
  const flat: number[] = [];
  const ringLengths: number[] = [];
  for (const line of lines) {
    ringLengths.push(line.length / 2);
    if (swapPairs) {
      let lon: number | null = null;
      for (const value of line) {
        if (lon === null) {
          lon = value;
        } else {
          flat.push(value, lon); // value is lat → emit (lat, lon)
          lon = null;
        }
      }
    } else {
      for (const value of line) {
        flat.push(value);
      }
    }
  }
  return { latlon: Float64Array.from(flat), ringLengths: Uint32Array.from(ringLengths) };
}

/** Flatten GeoJSON-order `[lon, lat, …]` lines into the `[lat, lon, …]`
 *  contract shape. */
export function flattenLonLatLines(lines: number[][]): OverlayGeometry {
  return flattenLines(lines, true);
}

const GRATICULE_DEFAULT_SPACING = 30;
/** Degrees between successive vertices along a graticule line — fine enough
 *  that a meridian/parallel renders as a smooth curve under the azimuthal and
 *  Mercator targets, not a chord. */
const GRATICULE_SAMPLE_STEP = 2;

/**
 * Build a lat/lon graticule (meridians + parallels) at `spacingDeg` degrees.
 * Meridians span the full pole-to-pole latitude range; parallels span the
 * full longitude range and skip the poles (where a parallel degenerates to a
 * point). Each line is densely sampled so curved projections render smoothly.
 */
export function buildGraticule(spacingDeg: number): OverlayGeometry {
  const spacing =
    Number.isFinite(spacingDeg) && spacingDeg > 0
      ? Math.min(spacingDeg, 90)
      : GRATICULE_DEFAULT_SPACING;
  const lines: number[][] = [];

  // Meridians: a vertical line at each longitude, sampled in latitude. The
  // upper bound excludes +180 so the antimeridian (already drawn at -180)
  // isn't stroked twice when `spacing` divides 360.
  for (let lon = -180; lon < 180 - 1e-9; lon += spacing) {
    const line: number[] = [];
    for (let lat = -90; lat <= 90 + 1e-9; lat += GRATICULE_SAMPLE_STEP) {
      line.push(lat, lon);
    }
    lines.push(line);
  }
  // Parallels: a horizontal line at each latitude, sampled in longitude. Built
  // symmetrically about the equator (0 and ±k·spacing) so the spacing reads the
  // same north and south regardless of whether it divides 90; the poles are
  // skipped since a parallel there degenerates to a point.
  for (let lat = 0; lat < 90 - 1e-9; lat += spacing) {
    const lats = lat === 0 ? [0] : [lat, -lat];
    for (const parallelLat of lats) {
      const line: number[] = [];
      for (let lon = -180; lon <= 180 + 1e-9; lon += GRATICULE_SAMPLE_STEP) {
        line.push(parallelLat, lon);
      }
      lines.push(line);
    }
  }

  return flattenLines(lines, false);
}
