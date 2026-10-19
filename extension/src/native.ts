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
// `field: T | null`, and a `?` is left only on an object the extension sends
// rather than receives (`RenderOptions`, `ContourSpacing`, `VectorPair`). Guard
// with `!= null` or `??`, which see both `null` and `undefined`.
//
// The addon exports one `Handle` (#1073), from `open(bytes)` or
// `openStore(path)`, whose opening, listing and decoding calls are the browser
// package's, name for name.

import * as path from "path";

import { nativeNotLoaded, reportProblem } from "./problems";

import type { Addressing, AxisValues, ColourRange, DecodeOptions, DisplayDecodeOptions, Georef, GridSupport, LeftOutArray, LineRequest, MessageInfo, Placement, SliceRequest, SourceFormat, Stats, VariableInfo, VectorPairing, Vertical } from "./api.generated";

export type {
  Addressing,
  AxisValues,
  ColourRange,
  Georef,
  GridSupport,
  Identification,
  LeftOutArray,
  LeftOutCause,
  LineRequest,
  MessageInfo,
  Placement,
  RangeRefusal,
  SliceRequest,
  SourceFormat,
  DimensionInfo,
  VariableInfo,
  VectorPairing,
  Vertical,
} from "./api.generated";

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
  /** A manual colour range. Either end may be sent alone; the other comes
   *  from the field. A range that does not hold is refused with an error
   *  carrying `rangeRefusal` (#1155). */
  rangeMin?: number;
  rangeMax?: number;
  /** A value to centre the ramp on, for a diverging map (#1155). */
  rangeCenter?: number;
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
   *  Read by renderGrid, projectOverlay, projectContours and probePixel alike
   *  (combined targets too), so a caller that sets it for one must set it
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
 *  (#241). The runs are the overlay's shape — five vertices per arrow.
 *  `referenceSpeed` is `null` when no arrow was drawn (#871).
 *  `referenceLength` is how long a full-length arrow is drawn, in the same
 *  raster pixels as `xy`, for the key (#1046): every arrow at the reference
 *  speed, since arrows are one on-screen length per speed (#1159); `null`
 *  when no arrow is drawn. `pairing` says whether the two messages are a u/v pair at all, by
 *  the parameter each names (#1049), or `"unknown"` when a parameter has no
 *  name to read (#1142); the arrows are drawn either way. */
export interface ProjectedVectors {
  xy: Float64Array;
  segLengths: Uint32Array;
  referenceSpeed: number | null;
  referenceLength: number | null;
  pairing: VectorPairing;
}

export interface RenderedGrid {
  rgba: Uint8Array;
  width: number;
  height: number;
  /** The range the field was painted over; `null` when the range was
   *  automatic and no cell had a value, so there is none to show (#871). */
  usedMin: number | null;
  usedMax: number | null;
  /** The whole range result (#1155): which ends were given, the center, and
   *  how many present cells were below and above the range. */
  range: ColourRange;
  /** Geographic extent actually rendered (degrees), echoed back so the
   *  panel can pre-fill the manual-bounds inputs. Present for the warped
   *  lat/lon targets (equirectangular, web_mercator); `null` for the
   *  source-projection target (no geographic extent). */
  usedLatMin: number | null;
  usedLatMax: number | null;
  usedLonMin: number | null;
  usedLonMax: number | null;
  projectionSummary: string;
  /** The band limit of a spectral map drawn below what its message declares
   *  (#637); for a combined map, the label either operand carries (#814).
   *  `null` for every other map. */
  truncation: SpectralTruncation | null;
}

/** A rendered spectral map's band limit (#637): the truncation its message
 *  declares and the one the map was synthesised at. The same shape as the
 *  API's `SpectralTruncation` on `MessageInfo`. */
export interface SpectralTruncation {
  declared: number;
  truncatedTo: number;
}

/** One decoded field, from {@link Handle.decode} or {@link Handle.decodeSlice}:
 *  `values` at the width `dtype` asked for, a masked cell's value `0`, so read
 *  `mask` first. `stats` is what {@link Handle.renderDisplayGrid} takes back. */
export interface DecodedGrid {
  values: Float32Array | Float64Array;
  dtype: "f32" | "f64";
  mask: Uint8Array;
  ni: number;
  nj: number;
  stats: Stats;
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
  /** `null` for the zonal mean of a GRIB2 message that carries no parameter
   *  codes (#775). */
  variable: string | null;
  /** `null` when the variable states no units (#775). */
  units: string | null;
  dimension: string;
  /** The axis's coordinate values, in index order; `null` when the axis has no
   *  coordinate array, or when one of its values is missing or not finite. Fall
   *  back to indices. */
  coordinates: number[] | null;
  coordinateUnits: string | null;
  /** Whether the axis is vertical, and which way its values increase (CF
   *  §4.3), when there are coordinates; `null` otherwise (#1043). */
  coordinateVertical: Vertical | null;
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

// ---------------------------------------------------------------------------
// NetCDF 2-D slice rendering (#122)
// ---------------------------------------------------------------------------

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
  /** Which of contours, arrows, the long CSV and the zonal mean the slice
   *  takes (#945): the umbrella's answer, so the panel keeps no family list. */
  supports: GridSupport;
}

/** One field of a file, as the map pipeline addresses it: a message index for
 *  a GRIB file, or one slice of a variable for a NetCDF file or a Zarr store. */
export type GridField = number | SliceRequest;

/** Two fields combined cell by cell under `op` (#239, #329): the difference
 *  map. `op` is a {@link CombineOp}, a `value` from `combineOps()`; Rust refuses
 *  any other. Both must sit on the same grid, and two slices on the same axes;
 *  the result is drawn against `a`'s grid. */
export interface CombinedFields {
  a: GridField;
  b: GridField;
  op: string;
}

/** What the map pipeline draws: one field, or two combined. */
export type GridTarget = GridField | CombinedFields;

/** Two lines combined point by point under `op` (#1208): the line plot
 *  through a Compare map. Both run along the same `alongDim`, and `b` is read
 *  as far along it from `a` as their two entries for it are apart. */
export interface CombinedLines {
  a: LineRequest;
  b: LineRequest;
  op: string;
}

/** The polylines an overlay projects (#72): flat `[lat, lon, …]` and how many
 *  points each ring has. */
export interface Polylines {
  latlon: Float64Array;
  ringLengths: Uint32Array;
}

/** How contours are spaced (#238). `interval` left out picks about eight round
 *  levels over the range painted. */
export interface ContourSpacing {
  interval?: number;
}

/** Two messages drawn as arrows (#241). `u` eastward and `v` northward, or
 *  along the grid's own axes, as each message's `MessageInfo.uvRelativeToGrid`
 *  reports; `gridRelative` overrides the frame the pair states, and left out
 *  the pair's own is used. */
export interface VectorPair {
  u: number;
  v: number;
  spacing?: number;
  gridRelative?: boolean;
}

/** A variable's two image axes, without a position on the rest: all that
 *  placing a slice needs. */
export interface SliceAxes {
  variable: number;
  yDim: number;
  xDim: number;
}

/** An open container: a GRIB file, a NetCDF file or a Zarr store (#1073).
 *
 *  The browser package's `Handle` has the same shape for opening, listing and
 *  decoding: `count`, `message`, `variables`, `leftOut`, `undrawable`,
 *  `decode`, `decodeSlice`, `decodeLine`, `decodeDisplay`, `format`,
 *  `addressing` and `close` are spelt and called the same in both. The rest is
 *  the panel's map pipeline, which draws a {@link GridTarget}. A message
 *  question asked of a variable container, or the reverse, throws with
 *  `code: "wrong_addressing"`.
 *
 *  `close()` releases the file now rather than when the garbage collector runs;
 *  `[Symbol.dispose]` is the same method, so `using` works. */
export interface Handle {
  close(): void;
  [Symbol.dispose](): void;
  format(): SourceFormat;
  addressing(): Addressing;
  /** How many messages the file holds; `message(i)` answers for each `i` below
   *  it. Zero for a variable container. */
  count(): number;
  /** One message's metadata as the fieldglass API states it, the same
   *  `MessageInfo` the browser package returns. Every key is present; a field
   *  with nothing to report is `null` (#574). */
  message(index: number): MessageInfo;
  /** The variables a slice can be drawn from, as the browser package lists
   *  them (#1073): `index` is the position every slice request names, and
   *  `units` is the file's own spelling, which {@link FieldglassNative.typesetUnits}
   *  typesets for display. Empty for a message stream. */
  variables(): VariableInfo[];
  /** The arrays the container holds and this build will not read (#709), each
   *  with the reader's reason and a `cause` code (#1129). */
  leftOut(): LeftOutArray[];
  /** The arrays the container reads that no map can be drawn from, each with
   *  why (#1129). With {@link leftOut} and the variable list, every array once. */
  undrawable(): LeftOutArray[];
  /** A NetCDF file's dimensions, attributes and variables, for the metadata
   *  view. Throws for any other container. */
  metadata(): DatasetMeta;
  /** One message's field, resolved: a spectral or HEALPix message comes back on
   *  the global lat/lon grid `renderGrid` paints (#580). */
  decode(index: number, options?: DecodeOptions | null): DecodedGrid;
  /** One slice of a variable, as {@link decode} returns a message. */
  decodeSlice(slice: SliceRequest, options?: DecodeOptions | null): DecodedGrid;
  /** One line along `line.alongDim`, every other axis held at
   *  `line.sliceIndices` — the entry for `alongDim` is ignored (#172). */
  decodeLine(line: LineRequest, options?: DecodeOptions | null): LineResult;
  /** One line through a combined map (#1208): `a` and `b` along the same
   *  axis, combined point by point. Here each request's entry for `alongDim`
   *  counts: `b` is read as far along the axis from `a` as the two entries
   *  are apart, so the point at `a`'s entry is the map's value, and a step
   *  with no `b` is absent. Named as `a`'s line. */
  decodeLineCombined(line: CombinedLines, options?: DecodeOptions | null): LineResult;
  /** Decode a message to draw, at a reduced resolution (#1082): `reduce`
   *  discards that many of a JPEG 2000 message's finest wavelet levels. The
   *  values are not the file's, so the result is a {@link DisplayGrid}, which
   *  only `renderDisplayGrid` takes. Throws `unsupported` above `reduce: 0` for
   *  a message with no coarse field. */
  decodeDisplay(index: number, options?: DisplayDecodeOptions | null): DisplayGrid;
  /** Decode, project and paint `target` under `options`. */
  renderGrid(target: GridTarget, options: RenderOptions): RenderedGrid;
  /** Paint a {@link DisplayGrid} through the `renderGrid` pipeline. The
   *  automatic colour range is the full field's, from `fullStats`; `null` is
   *  allowed only when both range ends are set. */
  renderDisplayGrid(field: DisplayGrid, fullStats: Stats | null, options: RenderOptions): RenderedGrid;
  /** Geographic polylines projected onto the raster `renderGrid` paints for
   *  this target and these options (#72). Geometry only. */
  projectOverlay(target: GridTarget, options: RenderOptions, lines: Polylines): ProjectedOverlay;
  /** Contour isolines of the target, projected onto its render raster (#238).
   *  A combined target is contoured as the combined map (#329). */
  projectContours(target: GridTarget, options: RenderOptions, spacing?: ContourSpacing | null): ProjectedOverlay;
  /** Arrows for a vector field built from two messages (#241). One arrow is one
   *  run of five vertices. Throws when the two are not on the same grid (#793)
   *  or state different component frames (#805). */
  projectVectors(pair: VectorPair, options: RenderOptions): ProjectedVectors;
  /** The field under a rendered pixel (#172): `px`/`py` are output-raster
   *  pixels (post-flip). `null` off the raster or off the globe. A combined
   *  target reads the combined map (#329). */
  probePixel(target: GridTarget, options: RenderOptions, px: number, py: number): ProbeResult | null;
  /** Each row's mean over longitude, against latitude (#240). Throws for a grid
   *  whose rows are not circles of latitude (rotated, projected, curvilinear).
   *  A combined target is averaged as the combined map (#1208). */
  zonalMean(target: GridTarget): LineResult;
  /** One field as CSV, as its UTF-8 bytes to write straight to disk (#341).
   *  `format` is `"matrix"` (a 2-D grid of values) or `"long"` (a
   *  `lat,lon,value` table); missing points are empty value cells. */
  exportCsv(field: GridField, format: "matrix" | "long"): Uint8Array;
  /** Where the slice on these image axes sits (#574). Decodes nothing. */
  sliceGrid(axes: SliceAxes): SliceGrid;
  /** The coordinate values along one axis, for labelling a cross-section
   *  (#171). Reads the coordinate array only, never the field. */
  axisValues(variable: number, dim: number): AxisValues;
  /** Patch one GRIB1 message's PDS `P1` octet and return the whole edited
   *  file. Throws for any other container. */
  setP1(index: number, value: number): Uint8Array;
}

/** A message decoded to draw, from {@link Handle.decodeDisplay} (#1082).
 *  It carries no values or stats to measure with, and the handle's measuring
 *  calls take a {@link GridTarget}, which this is not. */
export interface DisplayGrid {
  /** Wavelet levels discarded; `0` is the message's own field. */
  reduction(): number;
  ni(): number;
  nj(): number;
  /** The derived grid above reduction zero, never the message's own. */
  grid(): Georef;
  close(): void;
  [Symbol.dispose](): void;
}

// ---------------------------------------------------------------------------
// Native module loader
// ---------------------------------------------------------------------------

export interface FieldglassNative {
  /** `"grib1"`, `"grib2"`, `"netcdf"` or `"unknown"`, from the bytes alone. */
  detectBytes(bytes: Uint8Array): string;
  /** Open a GRIB1, GRIB2 or NetCDF file from its bytes; the format is detected.
   *  Throws with `code: "unsupported_format"` for anything else. */
  open(bytes: Uint8Array): Handle;
  /** Open the Zarr store rooted at the directory `path` (#659). Throws when the
   *  directory holds none of `zarr.json`, `.zmetadata`, `.zgroup` or
   *  `.zarray`. */
  openStore(path: string): Handle;
  /** Whether a directory looks like a store, without opening it — what the
   *  folder picker asks so it can refuse in its own words. The `.zarr` suffix
   *  is a convention and is **not** what this checks. */
  isStore(path: string): boolean;
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
  /** A unit string typeset for display (ADR-0007), as the panel shows a
   *  variable's units; blank comes back empty. */
  typesetUnits(units: string): string;
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
    // The whole error, stack included, for a bug report; the toast's
    // Details has only its message.
    console.error(`[Fieldglass] failed to load ${nodePath}:`, err);
    void reportProblem("error", nativeNotLoaded(nodePath, err));
  }
  return cached;
}
