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
  projection: "source" | "equirectangular" | "web_mercator";
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

export interface Grib1Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  setP1(messageIndex: number, value: number): Buffer;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
}

export interface Grib2Handle {
  messages(): MessageMeta[];
  decodeGrid(messageIndex: number): DecodedGrid;
  renderGrid(messageIndex: number, options: RenderOptions): RenderedGrid;
}

export interface Grib1HandleCtor {
  fromBytes(bytes: Uint8Array): Grib1Handle;
}

export interface Grib2HandleCtor {
  fromBytes(bytes: Uint8Array): Grib2Handle;
}

// ---------------------------------------------------------------------------
// Native module loader
// ---------------------------------------------------------------------------

export interface FieldglassNative {
  detectBytes(bytes: Uint8Array): string;
  openNetcdf(bytes: Uint8Array): DatasetMeta;
  Grib1Handle: Grib1HandleCtor;
  Grib2Handle: Grib2HandleCtor;
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
