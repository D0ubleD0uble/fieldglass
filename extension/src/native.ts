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
  /** Raw GRIB1 P1 octet, or absent where editing one octet would be wrong
   *  (GRIB2/NetCDF have no P1; time-range 10 spends two octets on one value). */
  p1Octet: number | null;
  forecastDisplay: string;
  originatingCentre: string;
  /** Sub-centre name (WMO C-12), or absent when the field is 0 or unassigned. */
  subCentre?: string;
  gridType: string | null;
  gridNi: number | null;
  gridNj: number | null;
  /** How the file names its own grid, where `gridNi×gridNj` is not how it is
   *  described — a spectral truncation (`T63`), a HEALPix `Nside`, a reduced
   *  Gaussian's `N32`/`O32`. Shown where the dimensions would go, and
   *  preferred over them where both exist. */
  gridSizeLabel: string | null;
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
  /** The variable's CF `units`, typeset by the native side (ADR-0007). Empty
   *  when the variable declares none. Lifted out of `attributes` so the
   *  metadata table can show units without them being lost to the
   *  three-attribute preview. */
  units: string;
  attributes: AttributeMeta[];
}

/** A NetCDF-4 dataset left out of {@link DatasetMeta.variables} because its
 *  HDF5 datatype is outside the decoded subset. */
export interface UnsupportedVariableMeta {
  name: string;
  reason: string;
}

export interface DatasetMeta {
  backing: string;
  backingLabel: string;
  /** Whether the tables below are populated. A file with one undecodable
   *  variable keeps this `true` and lists that variable in
   *  {@link DatasetMeta.unsupportedVariables} — showing the rest is the point,
   *  so this must not gate the tables on it. */
  fullyParsed: boolean;
  note?: string;
  dimensions: DimensionMeta[];
  globalAttributes: AttributeMeta[];
  variables: VariableMeta[];
  hdf5SuperblockVersion?: number;
  /** NetCDF-4 variables whose HDF5 datatype (compound, enum, variable-length,
   *  opaque, array) this build does not decode. Empty for classic files and for
   *  NetCDF-4 files every variable of which decoded. */
  unsupportedVariables: UnsupportedVariableMeta[];
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
  /** Output raster size in pixels for the warped lat/lon targets —
   *  "equirectangular" and "web_mercator" (#465). Pass both or neither: one
   *  alone is an error on the Rust side, unlike the bounds box, which falls
   *  back. An explicit size is taken as given, so it bypasses the 720-pixel
   *  display floor a reprojection otherwise gets. Every other target ignores
   *  it: the azimuthal and world ones keep the aspect their projection fixes,
   *  and "source" paints the array as stored.
   *
   *  Read by renderGrid, projectOverlay, projectContours and probe alike (and
   *  by their *Combined siblings), so a caller that sets it for one must set it
   *  for all of them or the overlays and the probe will be reading a different
   *  raster than the image. The panel does not set it yet (#403). */
  width?: number;
  height?: number;
  /** Name of the colormap to paint with — one of the names `colormaps()`
   *  reports. Omitted uses the default ("viridis"). An unknown name is an
   *  error on the Rust side rather than a silent fallback. */
  colormap?: string;
  /** A colormap as its 768-byte lookup table instead of by name — 256 RGB
   *  entries, low to high — which is how an imported colour table is painted
   *  (#236). Sending `colormap` as well is an error on the Rust side, and so is
   *  any other length. */
  colormapTable?: number[];
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

/** A colour palette table (`.cpt`) read and compiled by `parseColorTable`. */
export interface ParsedColorTable {
  /** Legend stops, sampled from `table` as a registered colormap's are. */
  stops: string[];
  /** The 768-byte lookup table to send as `RenderOptions.colormapTable`. */
  table: number[];
  /** How many slices the file held. */
  slices: number;
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

/** One line through a variable — a vertical profile or a time series at a cell
 *  (#172). `values` holds `NaN` where `mask` is 0, so read `mask` first. Absent
 *  optional fields arrive as `undefined`, never `null` (#288). */
export interface LineResult {
  values: number[];
  mask: number[];
  min?: number;
  max?: number;
  variable: string;
  units: string;
  dimension: string;
  /** The axis's coordinate values, in index order; absent when the axis has no
   *  coordinate array, or when one of its values is. Fall back to indices. */
  coordinates?: number[];
  coordinateUnits?: string;
}

/** Element-wise combine operation on two aligned fields (#239). `aMinusB` is
 *  the difference / anomaly map. The tags mirror `CombineOp` in
 *  `fieldglass-core`; the runtime op list (picker + validation) comes from
 *  {@link FieldglassNative.combineOps}, so a new op added in Rust surfaces here
 *  as a compile error at any call site that hasn't been updated — never a
 *  silent drift (#342). */
export type CombineOp = "a_minus_b" | "b_minus_a" | "a_plus_b" | "mean" | "ratio";

/** One entry of the Rust field-combine op vocabulary — what the Compare picker
 *  and its validation need (#342). */
export interface CombineOpInfo {
  value: string;
  label: string;
}

export interface Grib1Handle {
  messages(): MessageMeta[];
  /** One message's field, resolved: a spectral message has no raster of its
   *  own and comes back synthesized onto a global lat/lon grid (#580), the
   *  same grid `renderGrid` paints. */
  decodeGrid(messageIndex: number): DecodedGrid;
  /** Serialize one message's decoded field as CSV, returned as its UTF-8 bytes
   *  (a `Buffer` written straight to disk — see #341). `format` is `"matrix"`
   *  (a 2-D grid of values) or `"long"` (a `lat,lon,value` table); missing
   *  points are empty value cells. The long format needs per-point
   *  coordinates, so it covers the same grids as `projectContours`. */
  exportCsv(messageIndex: number, format: string): Buffer;
  /** Each row's mean over longitude, against latitude (#240). Throws for a grid
   *  whose rows are not circles of latitude (rotated, projected, curvilinear). */
  zonalMean(messageIndex: number): LineResult;
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
   *  (geostationary scan-angle grids). */
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
  /** Probe a difference/sum/… map (#329): reads the combined field, so the
   *  readout matches the displayed map, not field A. */
  probeCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: string,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
  /** Contour a difference/sum/… map (#329): traces the combined field. */
  projectContoursCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: string,
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
}

export interface Grib2Handle {
  messages(): MessageMeta[];
  /** Sibling to {@link Grib1Handle.decodeGrid}; HEALPix resolves the same way
   *  a spectral message does. */
  decodeGrid(messageIndex: number): DecodedGrid;
  /** Sibling to {@link Grib1Handle.exportCsv}. */
  exportCsv(messageIndex: number, format: string): Buffer;
  /** Each row's mean over longitude, against latitude (#240). Throws for a grid
   *  whose rows are not circles of latitude (rotated, projected, curvilinear). */
  zonalMean(messageIndex: number): LineResult;
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
  /** Sibling to {@link Grib1Handle.probeCombined} (#329). */
  probeCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: string,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
  /** Sibling to {@link Grib1Handle.projectContoursCombined} (#329). */
  projectContoursCombined(
    messageIndexA: number,
    messageIndexB: number,
    op: string,
    options: RenderOptions,
    interval?: number,
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
  /** The variable's CF `units`, typeset for display the way a GRIB unit is
   *  (ADR-0007). Empty string when the variable declares none. */
  units: string;
}

export interface NetcdfHandle {
  /** Dataset metadata from the reader this handle already holds — the same
   *  value {@link FieldglassNative.openNetcdf} returns for the same bytes.
   *  Preferred when a handle is being kept anyway: calling both parses and
   *  copies the whole file twice. */
  metadata(): DatasetMeta;
  variables(): NetcdfVariableMeta[];
  renderSlice(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
  ): RenderedGrid;
  /** Serialize one decoded slice as CSV — `"matrix"` or `"long"`
   *  (`lat,lon,value`), missing points as empty cells. The slice is picked as
   *  in {@link renderSlice}; the long format needs geolocated geometry — a
   *  regular lat/lon grid, or a WRF projection. See the GRIB counterparts. */
  exportCsv(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    format: string,
  ): Buffer;
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
   *  A slice is contourable wherever its synthesised geometry lands on a
   *  geolocated family — every one but a GOES scan-angle grid. */
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
  /** One line along `alongDim`, every other axis held at `sliceIndices` — the
   *  entry for `alongDim` is ignored (#172). */
  line(variableIndex: number, alongDim: number, sliceIndices: number[]): LineResult;
  /** The slice's zonal mean, against latitude (#240). Throws for a slice whose
   *  rows are not circles of latitude. */
  zonalMean(variableIndex: number, yDim: number, xDim: number, sliceIndices: number[]): LineResult;
  /** Probe a NetCDF difference/sum/… map (#329): reads the combined field of
   *  slice A and slice B, so the readout matches the displayed map, not A. */
  probeSliceCombined(
    variableIndexA: number,
    yDim: number,
    xDim: number,
    sliceIndicesA: number[],
    variableIndexB: number,
    sliceIndicesB: number[],
    op: string,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
  /** Contour a NetCDF difference/sum/… map (#329): traces the combined field. */
  projectContoursSliceCombined(
    variableIndexA: number,
    yDim: number,
    xDim: number,
    sliceIndicesA: number[],
    variableIndexB: number,
    sliceIndicesB: number[],
    op: string,
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
}

export interface NetcdfHandleCtor {
  fromBytes(bytes: Uint8Array): NetcdfHandle;
}

/** What the slice render panel needs of a handle.
 *
 *  Both {@link NetcdfHandle} and {@link ZarrHandle} satisfy it structurally, which
 *  is what lets one panel drive either without knowing which container answered
 *  (#659). The signatures are identical on purpose: a store answers the same
 *  questions a NetCDF file does, and two shapes for one question is how the two
 *  hosts drifted apart in the first place (#662).
 *
 *  Compare mode is included. `Session::combine` runs the alignment gate and the
 *  arithmetic, so a combined field arrives like any other and a store gets
 *  difference maps for free. */
export interface SlicePanelHandle {
  variables(): NetcdfVariableMeta[];
  renderSlice(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
  ): RenderedGrid;
  exportCsv(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    format: string,
  ): Buffer;
  projectOverlay(
    variableIndex: number,
    yDim: number,
    xDim: number,
    options: RenderOptions,
    latlon: Float64Array,
    ringLengths: Uint32Array,
  ): ProjectedOverlay;
  projectContours(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
  probe(
    variableIndex: number,
    yDim: number,
    xDim: number,
    sliceIndices: number[],
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
  /** One line along `alongDim`, every other axis held at `sliceIndices` — the
   *  entry for `alongDim` is ignored (#172). */
  line(variableIndex: number, alongDim: number, sliceIndices: number[]): LineResult;
  /** The slice's zonal mean, against latitude (#240). Throws for a slice whose
   *  rows are not circles of latitude. */
  zonalMean(variableIndex: number, yDim: number, xDim: number, sliceIndices: number[]): LineResult;
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
  probeSliceCombined(
    variableIndexA: number,
    yDim: number,
    xDim: number,
    sliceIndicesA: number[],
    variableIndexB: number,
    sliceIndicesB: number[],
    op: string,
    options: RenderOptions,
    px: number,
    py: number,
  ): ProbeResult | null;
  projectContoursSliceCombined(
    variableIndexA: number,
    yDim: number,
    xDim: number,
    sliceIndicesA: number[],
    variableIndexB: number,
    sliceIndicesB: number[],
    op: string,
    options: RenderOptions,
    interval?: number,
  ): ProjectedOverlay;
}

/** One array a store holds that this build will not read, and why (#709). */
export interface ZarrLeftOut {
  name: string;
  reason: string;
}

/** A Zarr store, opened from a directory (#659).
 *
 *  A store is a folder rather than a file, so this is the one handle that takes
 *  a path instead of bytes. It reuses {@link NetcdfVariableMeta} because a store
 *  answers the same question a NetCDF file does, which is what lets the existing
 *  slice picker drive it unchanged. */
export interface ZarrHandle extends SlicePanelHandle {
  /** The arrays the store holds and this build will not read. Shown beside the
   *  variable list so a user sees *why* a name is absent. */
  leftOut(): ZarrLeftOut[];
}

export interface ZarrHandleCtor {
  /** Open the store rooted at `path`. Throws when the directory holds none of
   *  `zarr.json`, `.zmetadata`, `.zgroup` or `.zarray`. */
  fromDirectory(path: string): ZarrHandle;
  /** Whether a directory looks like a store, without opening it — what the
   *  folder picker asks so it can refuse in its own words. The `.zarr` suffix is
   *  a convention and is **not** what this checks. */
  isStore(path: string): boolean;
}

// ---------------------------------------------------------------------------
// Native module loader
// ---------------------------------------------------------------------------

export interface FieldglassNative {
  detectBytes(bytes: Uint8Array): string;
  openNetcdf(bytes: Uint8Array): DatasetMeta;
  /** The colormap registry, in picker order; the first entry is the default. */
  colormaps(): ColormapInfo[];
  /** Read a GMT colour palette table from its text. Throws with the line and
   *  the reason when the file cannot be imported. */
  parseColorTable(text: string): ParsedColorTable;
  /** The field-combine op vocabulary, in menu order (#342). */
  combineOps(): CombineOpInfo[];
  Grib1Handle: Grib1HandleCtor;
  Grib2Handle: Grib2HandleCtor;
  NetcdfHandle: NetcdfHandleCtor;
  ZarrHandle: ZarrHandleCtor;
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
