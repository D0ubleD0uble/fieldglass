// Type declarations + runtime loader for the napi-rs native module.
//
// The Rust crate `fieldglass-napi` exports these types in
// `extension/bin/index.d.ts` after `napi build`; we mirror them here so
// the TypeScript checker has stable shapes regardless of whether the
// generated `.d.ts` exists in the workspace (CI generates it before
// `tsc` runs; locally during development it may lag).
// `tools/check_native_declarations.py` compares the two in CI.
//
// The API's own wire types (`MessageInfo`, `Georef`, `AxisValues`, …) are not
// written here: they are generated from the Rust schema into `api.generated.ts`
// by `tools/gen_api_declarations.py` (#574).
//
// Every object the addon returns carries every key, and a field with nothing to
// report is `null`, never missing (#574). So an optional field below is
// `field: T | null`, and the one `?` left is on `RenderOptions`, which the
// extension sends rather than receives. Guard with `!= null` or `??`, which see
// both `null` and `undefined`.

import * as path from "path";

import * as vscode from "vscode";

import type { AxisValues, MessageInfo, Placement } from "./api.generated";

export type { AxisValues, Georef, Identification, MessageInfo, Placement } from "./api.generated";

// ---------------------------------------------------------------------------
// NetCDF dataset types (returned from the native module)
// ---------------------------------------------------------------------------

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
  /** The variable's CF `units`, typeset by the native side (ADR-0007); `null`
   *  when the variable declares none (#775). Lifted out of `attributes` so the
   *  metadata table can show units without them being lost to the
   *  three-attribute preview. */
  units: string | null;
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
  /** Why the metadata could not be fully resolved, when it could not; `null`
   *  otherwise. */
  note: string | null;
  dimensions: DimensionMeta[];
  globalAttributes: AttributeMeta[];
  variables: VariableMeta[];
  /** HDF5 superblock version; `null` for a classic file. */
  hdf5SuperblockVersion: number | null;
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

/** Projected vector arrows plus the speed a full-length arrow stands for
 *  (#241). The runs are the overlay's shape — five vertices per arrow. */
export interface ProjectedVectors {
  xy: Float64Array;
  segLengths: Uint32Array;
  referenceSpeed: number;
}

export interface RenderedGrid {
  rgba: Buffer;
  width: number;
  height: number;
  usedMin: number;
  usedMax: number;
  /** Geographic extent actually rendered (degrees), echoed back so the
   *  panel can pre-fill the manual-bounds inputs. Present for the warped
   *  lat/lon targets (equirectangular, web_mercator); `null` for the
   *  source-projection target (no geographic extent). */
  usedLatMin: number | null;
  usedLatMax: number | null;
  usedLonMin: number | null;
  usedLonMax: number | null;
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

/** The field under a rendered pixel (#172). `lat`/`lon` are `null` when the
 *  grid can't be geolocated (a source view of a grid whose forward map isn't
 *  wired); `value` is `null` off-grid or on a masked cell. */
export interface ProbeResult {
  lat: number | null;
  lon: number | null;
  value: number | null;
  gridI: number | null;
  gridJ: number | null;
  /** For a spectral map drawn band-limited below what its message declares
   *  (#637): the full sum over every wavenumber the file holds at the same
   *  cell. `value` is the map's own, matching the colour under the cursor;
   *  the readout shows this one beside it. `null` for every other field. */
  fullDetailValue: number | null;
  /** The truncation `fullDetailValue` carries (the message's declared T; for a
   *  combined map, the larger operand's). Set whenever the probe read a
   *  full-detail value; `fullDetailValue` is then `null` only on a combined
   *  cell the operation leaves empty. */
  fullDetailTruncation: number | null;
}

/** One line through a variable — a vertical profile or a time series at a cell
 *  (#172). `values` holds `NaN` where `mask` is 0, so read `mask` first. A field
 *  with nothing to report is `null` (#574). */
export interface LineResult {
  values: number[];
  mask: number[];
  min: number | null;
  max: number | null;
  variable: string;
  /** `null` when the variable states no units (#775). */
  units: string | null;
  dimension: string;
  /** The axis's coordinate values, in index order; `null` when the axis has no
   *  coordinate array, or when one of its values is missing or not finite. Fall
   *  back to indices. */
  coordinates: number[] | null;
  coordinateUnits: string | null;
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
  /** How many messages the file holds; `message(i)` answers for each `i` below
   *  it. The browser package's `count()`. */
  count(): number;
  /** One message's metadata as the fieldglass API states it, the same
   *  `MessageInfo` the browser package returns. Every key is present; a field
   *  with nothing to report is `null` (#574). */
  message(messageIndex: number): MessageInfo;
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
  /** Arrows for a vector field built from two messages (#241): `u` eastward and
   *  `v` northward, or along the grid's own axes, as each message's
   *  `MessageInfo.uvRelativeToGrid` reports. One arrow is one run of five
   *  vertices, in the same pixel space the coastlines come back in. Throws when
   *  the two messages are not on the same grid, as a combine would (#793), or
   *  state different component frames (#805). `gridRelative` overrides the
   *  frame the pair states; left out, the pair's own is used. */
  projectVectors(
    messageIndexU: number,
    messageIndexV: number,
    options: RenderOptions,
    spacing?: number,
    gridRelative?: boolean,
  ): ProjectedVectors;
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
  /** Sibling to {@link Grib1Handle.count}. */
  count(): number;
  /** Sibling to {@link Grib1Handle.message}. */
  message(messageIndex: number): MessageInfo;
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
  /** Arrows for a vector field built from two messages (#241): `u` eastward and
   *  `v` northward, or along the grid's own axes, as each message's
   *  `MessageInfo.uvRelativeToGrid` reports. One arrow is one run of five
   *  vertices, in the same pixel space the coastlines come back in. Throws when
   *  the two messages are not on the same grid, as a combine would (#793), or
   *  state different component frames (#805). `gridRelative` overrides the
   *  frame the pair states; left out, the pair's own is used. */
  projectVectors(
    messageIndexU: number,
    messageIndexV: number,
    options: RenderOptions,
    spacing?: number,
    gridRelative?: boolean,
  ): ProjectedVectors;
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
 *  `null` means detection found no coordinate variable and the user assigns
 *  that axis by hand. */
export interface NetcdfVariableMeta {
  variableIndex: number;
  name: string;
  ncType: string;
  dims: NetcdfAxis[];
  detectedYDim: number | null;
  detectedXDim: number | null;
  /** The axis index of the time dimension, which the panel animates along
   *  (#170); `null` when the variable has none. Never an image axis. */
  detectedTimeDim: number | null;
  /** The variable's CF `units`, typeset for display the way a GRIB unit is
   *  (ADR-0007); `null` when the variable declares none (#775). */
  units: string | null;
}

/** Where one slice sits, as the render panel asks it (#574): the family to
 *  caption, whether it can be placed, and whether the reprojection targets may
 *  be offered. The same placement the handle renders from, so the picker cannot
 *  offer a target the render then refuses. */
export interface SliceGrid {
  /** `"latlon"`, `"lambert"`, `"curvilinear"`, … or `"source"` for a slice with
   *  no coordinates to place it by. */
  label: string;
  placement: Placement;
  reprojectable: boolean;
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
  /** The coordinate values along one axis, for labelling a cross-section
   *  (#171). Reads the coordinate array only, never the field. */
  axisValues(variableIndex: number, dim: number): AxisValues;
  /** Where the slice on these image axes sits (#574). Decodes nothing. */
  sliceGrid(variableIndex: number, yDim: number, xDim: number): SliceGrid;
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
  /** The coordinate values along one axis, for labelling a cross-section
   *  (#171). Reads the coordinate array only, never the field. */
  axisValues(variableIndex: number, dim: number): AxisValues;
  /** Where the slice on these image axes sits (#574). Decodes nothing. */
  sliceGrid(variableIndex: number, yDim: number, xDim: number): SliceGrid;
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
  /** The message table's label for a `MessageInfo.packing` identifier, e.g.
   *  `"Second-order (SPD-2)"`; an identifier with no label comes back as
   *  itself. */
  packingLabel(packing: string): string;
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
