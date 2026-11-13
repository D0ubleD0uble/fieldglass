// The messages between a render panel's page and the provider (#1808).
//
// A GRIB panel draws one message and a slice panel (NetCDF or Zarr) one slice
// of a variable, but their pages are one script and send the same requests.
// Both panels answer them through one dispatch (`servePanel` in provider.ts),
// and what differs between them is a `PanelSubject`: how a request names the
// field it reads, and what the panel does to draw, adopt a changed file, or
// export a CSV.
//
// Every request is answered exactly once: with its result, or with an error or
// empty result saying why there is none. The page counts what it has asked for
// and not yet heard back, and an Export PNG waits on the overlay, contour and
// arrow answers in flight (#1379), so a request left unanswered held the export
// until its timeout. Three exceptions, each answered some other way:
//
// - A request from a page the panel has since replaced is dropped (#1202). The
//   page that asked is gone, and a reply would reach the new one, whose
//   numbers are another file's.
// - The request that finds the file changed under the panel is answered by
//   the page being written again, or by a `gridError` that says nothing is
//   drawn and to pick a field (#839, #959). The page asks for nothing more
//   until one is picked.
// - Export CSV (`exportCsv`, `exportSliceCsv`) says how it ended in VS Code's
//   own dialogs and notifications, and a pick (`pickMessage`) by the page
//   written again for the picked message, or by a `gridError`. A pick made
//   while none is awaited is ignored: the page offers the picker only while
//   one is.

import type * as vscode from "vscode";

import type { AxisValues, GridField, GridTarget, Handle, LineResult, ProbeResult, RenderOptions, VectorMismatch, VectorPairing } from "./native";
import type { GridErrorMessage, GridReadyMessage, OverlayLayerPayload, PngExportOutcome } from "./provider";
import type { SliceSpec } from "./render-panel";

// ---------------------------------------------------------------------------
// Requests: page to provider
// ---------------------------------------------------------------------------

/** Every request carries the number of the page that sent it (#1202), which
 *  {@link PanelHandleGate} checks. */
interface FromPage {
  page?: unknown;
}

/** `ready` (a page loaded, with its saved selections) or `rerenderRequest`
 *  (a control changed): draw the field with these options. A slice panel's
 *  carries its slice, and either may carry a Compare rider. */
export interface RenderRequest extends FromPage, Partial<RenderOptions> {
  type: "ready" | "rerenderRequest";
  seq?: number;
  slice?: SliceSpec;
  compare?: unknown;
}

/** `overlayRequest` posted by the render panel when an overlay layer is
 *  toggled on or the underlying raster changes. Carries the same render
 *  options the image was painted with so the projection matches pixel-for-
 *  pixel, plus which layers to project. */
export interface OverlayRequest extends FromPage {
  type: "overlayRequest";
  /** Monotonic request id, echoed back in `overlayReady` so the webview can
   *  discard a reply that a newer request has superseded. */
  seq?: number;
  options?: Partial<RenderOptions>;
  /** The bundled Natural Earth vector layers, each independently toggleable. */
  coastlines?: boolean;
  borders?: boolean;
  lakes?: boolean;
  rivers?: boolean;
  graticule?: boolean;
  graticuleSpacing?: number;
  /** The NetCDF slice, when this panel renders a variable (#122). */
  slice?: SliceSpec;
}

/** `vectorRequest` posted by the render panel when the arrows are on: which
 *  message is the v component, how far apart to draw them, and whether the
 *  file states its components along the grid's axes (#241). */
export interface VectorRequest extends FromPage {
  type: "vectorRequest";
  seq?: number;
  messageIndexV?: unknown;
  spacing?: unknown;
  gridRelative?: unknown;
  options?: Partial<RenderOptions>;
  slice?: SliceSpec;
}

/** `contourRequest` posted by the render panel when contours are on and the
 *  field, range, projection, or interval changes (#238). Carries the render
 *  options (for the projection + used range) and an optional manual level
 *  interval; a NetCDF panel also carries its slice. */
export interface ContourRequest extends FromPage {
  type: "contourRequest";
  seq?: number;
  options?: Partial<RenderOptions>;
  /** Manual level spacing; omitted / non-positive → automatic nice levels. */
  interval?: number;
  slice?: SliceSpec;
  compare?: unknown;
}

/** `probeRequest` posted by the render panel when the user clicks the image to
 *  read the field there (#172). Carries the clicked output-raster pixel, the
 *  render options (for the projection), and the NetCDF slice when applicable. */
export interface ProbeRequest extends FromPage {
  type: "probeRequest";
  /** Echoed in `probeResult`, so the page can drop a reply for an older click or render. */
  seq?: number;
  px: number;
  py: number;
  options?: Partial<RenderOptions>;
  slice?: SliceSpec;
  compare?: unknown;
}

/** `zonalRequest`: the field's zonal mean (#240), a Compare map's of the
 *  combined field (#1208). */
export interface ZonalRequest extends FromPage {
  type: "zonalRequest";
  seq?: number;
  slice?: SliceSpec;
  compare?: unknown;
}

/** `lineRequest` posted by the render panel once a probe has resolved a cell
 *  (#172): plot the variable through that cell along `alongDim`. A Compare
 *  map's carries its `compare` rider, read by `resolveNetcdfCompare`
 *  (#1208). */
export interface LineRequest extends FromPage {
  type: "lineRequest";
  /** Echoed in `lineResult` (and `zonalResult`), as `ProbeRequest.seq`. */
  seq?: number;
  alongDim: number;
  gridI: number;
  gridJ: number;
  slice?: SliceSpec;
  compare?: unknown;
}

/** `axisRequest`: the coordinate values a cross-section labels axis `dim`
 *  of the slice's variable from (#171). */
export interface AxisRequest extends FromPage {
  type: "axisRequest";
  dim?: unknown;
  slice?: SliceSpec;
}

/** `pickMessage`: the message a GRIB panel waiting for a pick should draw,
 *  by its index in the changed file (#959). */
export interface PickMessageRequest extends FromPage {
  type: "pickMessage";
  messageIndex?: unknown;
}

/** `exportCsv` (a GRIB panel) or `exportSliceCsv` (a slice panel, naming the
 *  slice drawn): write the field to a CSV file (#1039). `fieldA` when the map
 *  drawn is a Compare map, whose CSV is Field A's. */
export interface ExportCsvRequest extends FromPage {
  type: "exportCsv" | "exportSliceCsv";
  fieldA?: unknown;
  slice?: SliceSpec;
}

/** `exportPng`: save the image the page composited. */
export interface ExportPngRequest extends FromPage {
  type: "exportPng";
  dataUrl?: unknown;
  defaultName?: unknown;
}

/** A request that reads the panel's field and is answered from its handle. */
export type FieldRequest =
  | OverlayRequest
  | ContourRequest
  | VectorRequest
  | ProbeRequest
  | ZonalRequest
  | LineRequest
  | AxisRequest;

/** Everything a render panel's page sends the provider. */
export type PanelRequest =
  | RenderRequest
  | FieldRequest
  | PickMessageRequest
  | ExportCsvRequest
  | ExportPngRequest;

const REQUEST_TYPES: ReadonlySet<string> = new Set<PanelRequest["type"]>([
  "ready",
  "rerenderRequest",
  "overlayRequest",
  "contourRequest",
  "vectorRequest",
  "probeRequest",
  "zonalRequest",
  "lineRequest",
  "axisRequest",
  "pickMessage",
  "exportCsv",
  "exportSliceCsv",
  "exportPng",
]);

/** Whether `m` is a request a render panel's page sends. Only its `type` is
 *  checked; each handler validates the fields it reads. */
export function isPanelRequest(m: unknown): m is PanelRequest {
  const type = (m as { type?: unknown } | null)?.type;
  return typeof type === "string" && REQUEST_TYPES.has(type);
}

// ---------------------------------------------------------------------------
// Replies: provider to page
// ---------------------------------------------------------------------------

/** The panel's file can no longer be read (#1338): set on an answer that
 *  carries nothing for that reason. */
interface MaybeHandleGone {
  handleGone?: true;
}

/** `overlayReady`: the requested layers, projected onto the raster. */
export interface OverlayReadyReply {
  type: "overlayReady";
  messageIndex: number;
  seq?: number;
  layers: OverlayLayerPayload[];
}

/** `overlayError`: the overlay could not be projected, and why. */
export interface OverlayErrorReply extends MaybeHandleGone {
  type: "overlayError";
  messageIndex: number;
  seq?: number;
  error: string;
}

/** `contourReady`: the field's isolines, projected onto the raster (#238). */
export interface ContourReadyReply {
  type: "contourReady";
  messageIndex: number;
  seq?: number;
  xy: Float64Array;
  segLengths: Uint32Array;
}

/** `contourError`: no contours, and why. */
export interface ContourErrorReply extends MaybeHandleGone {
  type: "contourError";
  messageIndex: number;
  seq?: number;
  error: string;
}

/** `vectorResult`: the arrows of a u/v pair (#241), or why there are none.
 *  `messageIndex` is the u component's. */
export type VectorResultReply =
  | {
    type: "vectorResult";
    seq?: number;
    messageIndex: number;
    xy: Float64Array;
    segLengths: Uint32Array;
    referenceSpeed: number | null;
    referenceLength: number | null;
    pairing: VectorPairing;
    pairNames: [string, string];
    mismatch: VectorMismatch | null;
  }
  | ({ type: "vectorResult"; seq?: number; messageIndex: number; error: string } & MaybeHandleGone);

/** `probeResult`: the field under a clicked pixel (#172), or `null`. */
export interface ProbeResultReply extends MaybeHandleGone {
  type: "probeResult";
  seq?: number;
  messageIndex: number;
  result: ProbeResult | null;
}

/** `zonalResult`: the field's zonal mean (#240), or why there is none. */
export interface ZonalResultReply extends MaybeHandleGone {
  type: "zonalResult";
  seq?: number;
  messageIndex: number;
  result: LineResult | null;
  error: string | null;
}

/** `lineResult`: the line through a probed cell (#172), or `null`. */
export interface LineResultReply extends MaybeHandleGone {
  type: "lineResult";
  seq?: number;
  messageIndex: number;
  result: LineResult | null;
}

/** `axisResult`: axis `dim`'s coordinate values (#171), or `null`. Named by
 *  the variable it is for (#1041), and echoing `dim` as the page sent it. */
export interface AxisResultReply extends MaybeHandleGone {
  type: "axisResult";
  dim: unknown;
  variableIndex: number;
  result: AxisValues | null;
}

/** `exportPngDone`: how a PNG export ended. */
export interface ExportPngDoneReply {
  type: "exportPngDone";
  outcome: PngExportOutcome;
}

/** Everything the provider posts to a render panel's page. */
export type PanelReply =
  | GridReadyMessage
  | GridErrorMessage
  | OverlayReadyReply
  | OverlayErrorReply
  | ContourReadyReply
  | ContourErrorReply
  | VectorResultReply
  | ProbeResultReply
  | ZonalResultReply
  | LineResultReply
  | AxisResultReply
  | ExportPngDoneReply;

/** The answer to `req` when its field cannot be read: `reason` says why, as
 *  an error or, for the kinds whose empty answer is an ordinary state (a
 *  probe, a line, an axis), an empty result. `index` is the field the request
 *  named. Every {@link FieldRequest} kind has one, so none goes unanswered. */
export function refusalFor(req: FieldRequest, index: number, reason: string, handleGone: boolean): PanelReply {
  const gone: MaybeHandleGone = handleGone ? { handleGone: true } : {};
  switch (req.type) {
    case "overlayRequest":
      return { type: "overlayError", messageIndex: index, seq: req.seq, error: reason, ...gone };
    case "contourRequest":
      return { type: "contourError", messageIndex: index, seq: req.seq, error: reason, ...gone };
    case "vectorRequest":
      return { type: "vectorResult", seq: req.seq, messageIndex: index, error: reason, ...gone };
    case "probeRequest":
      return { type: "probeResult", seq: req.seq, messageIndex: index, result: null, ...gone };
    case "zonalRequest":
      return { type: "zonalResult", seq: req.seq, messageIndex: index, result: null, error: reason, ...gone };
    case "lineRequest":
      return { type: "lineResult", seq: req.seq, messageIndex: index, result: null, ...gone };
    case "axisRequest":
      return { type: "axisResult", dim: req.dim, variableIndex: index, result: null, ...gone };
    default:
      return unreachable(req);
  }
}

function unreachable(req: never): never {
  throw new Error(`no answer for ${(req as { type?: unknown }).type}`);
}

// ---------------------------------------------------------------------------
// What differs between a GRIB panel and a slice panel
// ---------------------------------------------------------------------------

/** The field a request reads, as the panel names it. */
export interface RequestedField {
  /** The index replies carry: a GRIB panel's message, a slice panel's
   *  variable. */
  index: number;
  /** The field alone, which the overlays are projected for. */
  field: GridField;
  /** The field with the request's Compare rider: what a contour, a probe
   *  and a zonal mean read (#329, #1208). */
  target: GridTarget;
}

/** One render panel, as {@link PanelRequest}s are served for it. A GRIB
 *  panel and a slice panel each implement it; the dispatch, the overlay,
 *  contour, probe and zonal handlers and the PNG export are shared. */
export interface PanelSubject {
  /** The handle the panel's document or store has now, which the panel's
   *  gate compares with the one its page was written from. */
  current(): Handle | undefined;
  /** The handle changed under the page: decide what the new file holds for
   *  the panel. Whether `req` is served after all. */
  adopt(current: Handle, req: PanelRequest): boolean;
  /** Draw what a `ready` or `rerenderRequest` asks for, or say why not. */
  render(req: RenderRequest): GridReadyMessage | GridErrorMessage;
  /** Why a request that reads the field is not served while the panel has a
   *  file: a GRIB panel waits for a message to be picked (#959). `null` when
   *  it is served. */
  withheld(): string | null;
  /** The field `req` names. */
  field(req: FieldRequest): RequestedField;
  /** The arrows of a u/v pair. A GRIB panel's only. */
  vectors?(reader: Handle, req: VectorRequest): VectorResultReply;
  /** The line through a probed cell. A slice panel's only. */
  line?(reader: Handle, req: LineRequest): LineResultReply;
  /** An axis's coordinate values. A slice panel's only. */
  axis?(reader: Handle, req: AxisRequest): AxisResultReply;
  /** Draw the picked message, or say why not; `null` when the page is written
   *  again for it or no pick is awaited. A GRIB panel's only. */
  pick?(req: PickMessageRequest): GridErrorMessage | null;
  /** Export the field drawn to a CSV file. */
  exportCsv(req: ExportCsvRequest): void;
  /** Where a PNG export's dialog starts, and the document whose revert makes
   *  the image stale (#1315); none for a store. */
  exportPng: { dir: vscode.Uri; document?: vscode.Uri };
}
