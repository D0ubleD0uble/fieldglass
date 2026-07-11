// Type declarations + runtime loader for the napi-rs native module.
//
// The Rust crate `fieldglass-napi` exports these types in
// `extension/bin/index.d.ts` after `napi build`; we mirror them here so
// the TypeScript checker has stable shapes regardless of whether the
// generated `.d.ts` exists in the workspace (CI generates it before
// `tsc` runs; locally during development it may lag). When the schema
// changes on the Rust side, update this file in lockstep — there's a
// follow-up item to import from `bin/index.d.ts` directly once we can
// guarantee its presence at typecheck time.

import * as path from "path";

import * as vscode from "vscode";

// ---------------------------------------------------------------------------
// MessageMeta + NetCDF dataset types (returned from the native module)
// ---------------------------------------------------------------------------

export interface MessageMeta {
  messageIndex: number;
  offsetBytes: number;
  parameterName: string;
  parameterUnits: string;
  parameterAbbreviation: string;
  level: string;
  levelType: string;
  referenceTime: string;
  forecastHours: number;
  forecastDisplay: string;
  originatingCentre: string;
  gridType: string | null;
  gridNi: number | null;
  gridNj: number | null;
  latFirst: number | null;
  lonFirst: number | null;
  latLast: number | null;
  lonLast: number | null;
  format: string;
  edition: number | null;
  discipline: string | null;
  totalLengthBytes: number | null;
  productionStatus: string | null;
  dataType: string | null;
  /** Projection parameters surfaced for the render-panel reprojection
   *  warp. Only populated for the matching grid types; null otherwise. */
  lambertLad: number | null;
  lambertLov: number | null;
  lambertDxMetres: number | null;
  lambertDyMetres: number | null;
  lambertLatin1: number | null;
  lambertLatin2: number | null;
  gaussianNParallels: number | null;
  /** Human-readable data-packing method (GRIB1 BDS packing / GRIB2 §5
   *  data-representation template), e.g. "Second-order (SPD-2)". */
  packing: string | null;
  /** Whether this grid supports reprojection (the non-source projection
   *  targets). False for grid types without a warp yet (e.g. an unsupported
   *  GDS template) or with a degenerate Dx/Dy; the panel hides those options
   *  when false. */
  reprojectable: boolean;
}

export interface DimensionMeta {
  name: string;
  length: number;
  isRecord: boolean;
}

export interface AttributeMeta {
  name: string;
  ncType: string;
  value: string;
}

export interface VariableMeta {
  name: string;
  ncType: string;
  dimensions: string[];
  attributes: AttributeMeta[];
}

export interface DatasetMeta {
  backing: string;
  backingLabel: string;
  fullyParsed: boolean;
  note?: string;
  dimensions: DimensionMeta[];
  globalAttributes: AttributeMeta[];
  variables: VariableMeta[];
  hdf5SuperblockVersion?: number;
}

// ---------------------------------------------------------------------------
// Render-pipeline types (handle methods + their return shapes)
// ---------------------------------------------------------------------------

/** Picker state posted from the render panel and forwarded into the
 *  Rust render pipeline. */
export interface RenderOptions {
  projection:
    | "source"
    | "equirectangular"
    | "web_mercator"
    | "orthographic"
    | "polar_stereographic"
    | "mollweide"
    | "robinson"
    | "equal_earth";
  /** Preset for the parameterised targets. "orthographic" reads a centre
   *  preset ("atlantic" (default), "indian", "pacific", "americas",
   *  "north_pole", "south_pole"); "polar_stereographic" reads a hemisphere
   *  preset ("north" (default),
   *  "south"). Ignored by the lat/lon-box and world targets. Superseded
   *  per-component by centerLat/centerLon when those are supplied. */
  projectionPreset?: string;
  /** Free-form projection centre for the azimuthal and world targets
   *  (degrees). "orthographic" reads both (centre lat/lon);
   *  "polar_stereographic" reads only centerLon as the central meridian (its
   *  pole comes from the hemisphere preset), as do the world targets
   *  ("mollweide", "robinson", "equal_earth"). Either omitted falls back to the
   *  preset/default for that component. Ignored by the lat/lon-box targets. */
  centerLat?: number;
  centerLon?: number;
  resampling: "nearest" | "bilinear";
  flipY: boolean;
  rangeMin?: number;
  rangeMax?: number;
  /** Manual lat/lon extent override (degrees). Consulted for the warped
   *  lat/lon targets — "equirectangular" and "web_mercator". Pass all four to
   *  render that window; partial/inverted boxes fall back to the computed
   *  source bounds. lonMin/lonMax may sit outside [-180, 180] to describe an
   *  antimeridian-crossing window — pass back the echoed values verbatim to
   *  reproduce a view. For web_mercator the latitude extent is clamped to the
   *  projection's valid band (~±85.05°). */
  boundsLatMin?: number;
  boundsLatMax?: number;
  boundsLonMin?: number;
  boundsLonMax?: number;
}

export interface RenderedGrid {
  rgba: Buffer;
  width: number;
  height: number;
  usedMin: number;
  usedMax: number;
  /** Geographic extent actually rendered (degrees), echoed back so the
   *  panel can pre-fill the manual-bounds inputs. Present for the warped
   *  lat/lon targets (equirectangular, web_mercator); undefined for the
   *  source-projection target (no geographic extent). */
  usedLatMin?: number;
  usedLatMax?: number;
  usedLonMin?: number;
  usedLonMax?: number;
  projectionSummary: string;
}

export interface DecodedGrid {
  values: Float64Array;
  mask: Buffer;
  width: number;
  height: number;
}

/** Geographic polylines projected into the warped raster's pixel space for
 *  the overlay layer (coastline / graticule / future user shapes). `xy` is
 *  flat `[x0, y0, x1, y1, …]` in output pixel coordinates (post-flipY,
 *  identical to the rendered raster); `segLengths` gives the vertex count of
 *  each visible run, so `sum(segLengths) * 2 === xy.length`. May be empty when
 *  no run survives clipping (every vertex projects off the visible domain). */
export interface ProjectedOverlay {
  xy: Float64Array;
  segLengths: Uint32Array;
}

export interface Grib1Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  setP1(messageIndex: number, value: number): Buffer;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
  projectOverlay(
    messageIndex: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
}

export interface Grib2Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
  projectOverlay(
    messageIndex: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
}

export interface Grib1HandleCtor {
  fromBytes(bytes: Uint8Array): Grib1Handle;
}

export interface Grib2HandleCtor {
  fromBytes(bytes: Uint8Array): Grib2Handle;
}

// ---------------------------------------------------------------------------
// NetCDF 2-D slice rendering (#122)
// ---------------------------------------------------------------------------

/** One axis (dimension) of a renderable NetCDF variable, for the picker's
 *  index controls. */
export interface NetcdfAxis {
  name: string;
  length: number;
}

/** A NetCDF variable the render panel can draw, with its dimensions and the
 *  CF-detected horizontal-axis positions. `detectedYDim` / `detectedXDim` are
 *  axis indices (into `dims`) the picker pre-fills the Y / X selectors with;
 *  undefined means detection found no coordinate variable and the user assigns
 *  that axis by hand. */
export interface NetcdfVariableMeta {
  variableIndex: number;
  name: string;
  ncType: string;
  dims: NetcdfAxis[];
  detectedYDim?: number;
  detectedXDim?: number;
}

export interface NetcdfHandle {
  variables(): NetcdfVariableMeta[];
  renderSlice(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
  ): RenderedGrid;
  projectOverlay(
    variableIndex: number,
    yDim: number,
    xDim: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
}

export interface NetcdfHandleCtor {
  fromBytes(bytes: Uint8Array): NetcdfHandle;
}

// ---------------------------------------------------------------------------
// Native module loader
// ---------------------------------------------------------------------------

export interface FieldglassNative {
  detectBytes(bytes: Uint8Array): string;
  openNetcdf(bytes: Uint8Array): DatasetMeta;
  Grib1Handle: Grib1HandleCtor;
  Grib2Handle: Grib2HandleCtor;
  NetcdfHandle: NetcdfHandleCtor;
}

let cached: FieldglassNative | undefined;

export function nativeBinaryName(): string {
  const platform = process.platform;
  const arch = process.arch;
  const abi = platform === "linux" ? "-gnu" : platform === "win32" ? "-msvc" : "";
  return `fieldglass.${platform}-${arch}${abi}.node`;
}

export function loadNative(): FieldglassNative | undefined {
  if (cached) {
    return cached;
  }
  const nodePath = path.join(__dirname, "..", "bin", nativeBinaryName());
  try {
    // The native module path is computed at runtime from process.platform
    // / arch, so we must use require() rather than a static import. The
    // path is built from a closed set of platform/arch tokens — never
    // user input.
    // eslint-disable-next-line @typescript-eslint/no-require-imports, security/detect-non-literal-require
    cached = require(nodePath) as FieldglassNative;
  } catch (err) {
    console.error(`[Fieldglass] failed to load ${nodePath}:`, err);
    vscode.window.showErrorMessage(
      `Fieldglass: failed to load native module (${nativeBinaryName()}): ${err}`,
    );
  }
  return cached;
}
