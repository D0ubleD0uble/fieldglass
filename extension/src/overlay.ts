// Overlay layer data sources for the render panel (#72).
//
// This module produces geographic `(lat, lon)` polylines for the coastline
// and graticule overlays and nothing more — the *projection* into pixel space
// happens in Rust (`Grib{1,2}Handle.projectOverlay`), which owns every bit of
// the forward map. Keeping the math on the Rust side is the contract: the
// webview never reprojects, so it can never drift from the warp.
//
// Both layers hand back the same flat shape the napi `projectOverlay` call
// expects — `latlon` as `[lat, lon, lat, lon, …]` plus a `ringLengths` count
// per polyline — so a future user-defined-shape layer is just another
// producer of this shape with zero Rust change.

import * as fs from "fs";
import * as path from "path";

/** Geographic polylines as the flat pair `projectOverlay` consumes:
 *  `latlon` is `[lat, lon, …]`; `ringLengths[k]` is the vertex count of
 *  polyline `k` (so `sum(ringLengths) * 2 === latlon.length`). */
export interface OverlayGeometry {
  latlon: Float64Array;
  ringLengths: Uint32Array;
}

/** Shape of the bundled coastline asset: each line is flat `[lon, lat, …]`
 *  (GeoJSON coordinate order). Converted to `[lat, lon, …]` on load. */
interface CoastlineAsset {
  lines: number[][];
}

let coastlineCache: OverlayGeometry | undefined;

/** Load + cache the bundled Natural Earth 1:110m coastline as
 *  `OverlayGeometry`. Parsed once per extension-host session. The asset sits
 *  beside the compiled output under `media/`, located the same way
 *  `native.ts` finds the native binary (relative to `__dirname`). */
export function loadCoastline(): OverlayGeometry {
  if (coastlineCache) {
    return coastlineCache;
  }
  const file = path.join(__dirname, "..", "media", "coastline-110m.json");
  // Path is built from `__dirname` + a fixed asset name, never user input.
  // eslint-disable-next-line security/detect-non-literal-fs-filename
  const asset = JSON.parse(fs.readFileSync(file, "utf8")) as CoastlineAsset;
  coastlineCache = flattenLonLatLines(asset.lines);
  return coastlineCache;
}

/** Flatten GeoJSON-order `[lon, lat, …]` lines into the `[lat, lon, …]`
 *  contract shape. */
export function flattenLonLatLines(lines: number[][]): OverlayGeometry {
  const flat: number[] = [];
  const ringLengths: number[] = [];
  for (const line of lines) {
    ringLengths.push(line.length / 2);
    // Walk the line as (lon, lat) pairs and emit them swapped to (lat, lon).
    let lon: number | null = null;
    for (const value of line) {
      if (lon === null) {
        lon = value;
      } else {
        flat.push(value, lon); // value is lat → emit (lat, lon)
        lon = null;
      }
    }
  }
  return { latlon: Float64Array.from(flat), ringLengths: Uint32Array.from(ringLengths) };
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
    Number.isFinite(spacingDeg) && spacingDeg > 0 ? spacingDeg : GRATICULE_DEFAULT_SPACING;
  const lines: number[][] = [];

  // Meridians: a vertical line at each longitude, sampled in latitude.
  for (let lon = -180; lon <= 180 + 1e-9; lon += spacing) {
    const line: number[] = [];
    for (let lat = -90; lat <= 90 + 1e-9; lat += GRATICULE_SAMPLE_STEP) {
      line.push(lat, lon);
    }
    lines.push(line);
  }
  // Parallels: a horizontal line at each latitude (skipping ±90), sampled in
  // longitude.
  for (let lat = -90 + spacing; lat <= 90 - spacing + 1e-9; lat += spacing) {
    const line: number[] = [];
    for (let lon = -180; lon <= 180 + 1e-9; lon += GRATICULE_SAMPLE_STEP) {
      line.push(lat, lon);
    }
    lines.push(line);
  }

  return flattenLatLonLines(lines);
}

/** Flatten already-`[lat, lon, …]`-ordered lines into `OverlayGeometry`. */
function flattenLatLonLines(lines: number[][]): OverlayGeometry {
  const flat: number[] = [];
  const ringLengths: number[] = [];
  for (const line of lines) {
    ringLengths.push(line.length / 2);
    for (const value of line) {
      flat.push(value);
    }
  }
  return { latlon: Float64Array.from(flat), ringLengths: Uint32Array.from(ringLengths) };
}
