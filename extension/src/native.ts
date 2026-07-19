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
  /** Whether the grid's rows scan south→north (GRIB `jScansPositively`). The
   *  source projection orients the raster from this so it isn't upside-down;
   *  null for grids with no scan flag (predefined GRIB1 grids, NetCDF). */
  jScansPositive: boolean | null;
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
  /** Name of the colormap to paint with — one of the names `colormaps()`
   *  reports. Omitted uses the default ("viridis"). An unknown name is an
   *  error on the Rust side rather than a silent fallback. */
  colormap?: string;
  /** Flip the colormap end-for-end. Omitted is false. */
  reverseColormap?: boolean;
  /** Value→colour scaling: "linear" (default) or "log10". Under "log10" the
   *  colour position is log10(value), so quantities spanning orders of
   *  magnitude resolve across their whole range; non-positive values render as
   *  missing. Omitted/unknown is treated as "linear". Log10 needs a positive
   *  minimum: an auto range whose minimum is ≤ 0 is an error on the Rust side,
   *  so the panel keeps the toggle disabled until a positive manual minimum is
   *  set. */
  scaleMode?: "linear" | "log10";
}

/** One entry of the Rust colormap registry — everything the picker and the
 *  legend need. `stops` are sampled from the same lookup table that paints the
 *  grid, so the legend gradient cannot drift from the image. */
export interface ColormapInfo {
  name: string;
  label: string;
  kind: "sequential" | "diverging";
  stops: string[];
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

/** The field under a rendered pixel (#172). `lat`/`lon` are undefined when the
 *  grid can't be geolocated (a source view of a grid whose forward map isn't
 *  wired); `value` is undefined off-grid or on a masked cell. */
export interface ProbeResult {
  lat?: number;
  lon?: number;
  value?: number;
  gridI?: number;
  gridJ?: number;
}

/** Element-wise combine operation on two aligned fields (#239). `aMinusB` is
 *  the difference / anomaly map. */
export type CombineOp = "a_minus_b" | "b_minus_a" | "a_plus_b" | "mean" | "ratio";

export interface Grib1Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  /** Serialize one message's decoded field as CSV. `format` is `"matrix"`
   *  (a 2-D grid of values) or `"long"` (a `lat,lon,value` table); missing
   *  points are empty value cells. The long format is available for the
   *  lat/lon grid family only. */
  exportCsv(messageIndex: number, format: string): string;
  setP1(messageIndex: number, value: number): Buffer;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
  /** Render message A combined element-wise with message B under `op`. Both
   *  messages must sit on the same grid; the result renders through the normal
   *  pipeline against A's geometry. */
  renderGridCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: CombineOp,
    options: RenderOptions,
  ): RenderedGrid;
  projectOverlay(
    messageIndex: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
  /** Contour isolines for this message, projected onto the render raster (#238).
   *  `interval` sets a manual level spacing; omitted picks ~8 nice levels over
   *  the used range. Errors for grid types whose forward geolocation isn't wired
   *  (projected + reduced grids). */
  projectContours(
    messageIndex: number,
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
  /** Read the field under a rendered pixel (#172): the point-probe readout.
   *  `px`/`py` are output-raster pixels (post-flip). Undefined when the pixel is
   *  off the raster or off the globe. */
  probe(
    messageIndex: number,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
}

export interface Grib2Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  /** Sibling to {@link Grib1Handle.exportCsv}. */
  exportCsv(messageIndex: number, format: string): string;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
  /** Sibling to {@link Grib1Handle.renderGridCombined}. */
  renderGridCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: CombineOp,
    options: RenderOptions,
  ): RenderedGrid;
  projectOverlay(
    messageIndex: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
  /** Sibling to {@link Grib1Handle.projectContours}. */
  projectContours(
    messageIndex: number,
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
  /** Read the field under a rendered pixel (#172): the point-probe readout.
   *  `px`/`py` are output-raster pixels (post-flip). Undefined when the pixel is
   *  off the raster or off the globe. */
  probe(
    messageIndex: number,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
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
  /** Render one slice combined element-wise with a second slice under `op`
   *  (#239). Field B is a slice of `variableIndexB` at `sliceIndicesB`, sharing
   *  the same image axes; the common case is two time steps of one variable.
   *  Both slices must resolve to the same grid. */
  renderSliceCombined(
    variableIndexA: number,
    yDim: number,
    xDim: number,
    sliceIndicesA: number[],
    variableIndexB: number,
    sliceIndicesB: number[],
    op: CombineOp,
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
  /** Contour isolines for one slice, projected onto the render raster (#238).
   *  NetCDF grids are always contourable (synthesised lat/lon geometry). */
  projectContours(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
  /** Point-probe readout for one slice (#172). Sibling to {@link Grib1Handle.probe}. */
  probe(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
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
  /** The colormap registry, in picker order; the first entry is the default. */
  colormaps(): ColormapInfo[];
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
