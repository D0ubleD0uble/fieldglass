import * as vscode from "vscode";
import * as path from "path";

import { escapeHtml, nonce } from "./html";
import {
  loadNative,
  nativeBinaryName,
  type DatasetMeta,
  type Grib1Handle,
  type Grib2Handle,
  type MessageMeta,
  type RenderedGrid,
  type RenderOptions,
} from "./native";
import { renderImagePanelHtml } from "./render-panel";

const FORMAT_LABELS: Record<string, string> = {
  grib1: "GRIB Edition 1",
  grib2: "GRIB Edition 2",
  netcdf: "NetCDF",
  unknown: "Unknown",
};

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

export class FieldglassDocument implements vscode.CustomDocument {
  static async create(uri: vscode.Uri): Promise<FieldglassDocument> {
    const bytes = await vscode.workspace.fs.readFile(uri);
    return new FieldglassDocument(uri, bytes);
  }

  private _bytes: Uint8Array;

  private constructor(public readonly uri: vscode.Uri, bytes: Uint8Array) {
    this._bytes = bytes;
  }

  get bytes(): Uint8Array {
    return this._bytes;
  }

  setBytes(bytes: Uint8Array): void {
    this._bytes = bytes;
  }

  async revertFromDisk(): Promise<void> {
    this._bytes = await vscode.workspace.fs.readFile(this.uri);
  }

  dispose(): void {}
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

interface EditP1Message {
  type: "edit-p1";
  messageIndex: number;
  value: number;
}

interface ReadyMessage {
  type: "ready";
}

interface DecodeGridMessage {
  type: "decodeGrid";
  messageIndex: number;
}

type WebviewMessage = EditP1Message | ReadyMessage | DecodeGridMessage;

export class FieldglassEditorProvider
  implements vscode.CustomEditorProvider<FieldglassDocument>
{
  public static readonly viewType = "fieldglass.viewer";
  public static readonly viewTypeAny = "fieldglass.viewer.any";

  public static register(_context: vscode.ExtensionContext): {
    provider: FieldglassEditorProvider;
    disposables: vscode.Disposable[];
  } {
    const provider = new FieldglassEditorProvider();
    const opts = { supportsMultipleEditorsPerDocument: true };
    return {
      provider,
      disposables: [
        vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewType, provider, opts),
        vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewTypeAny, provider, opts),
        provider._onDidChangeCustomDocument,
      ],
    };
  }

  private readonly _onDidChangeCustomDocument =
    new vscode.EventEmitter<vscode.CustomDocumentEditEvent<FieldglassDocument>>();
  public readonly onDidChangeCustomDocument = this._onDidChangeCustomDocument.event;

  // All panels currently rendering each document, keyed by uri.toString().
  private readonly _panelsByDoc = new Map<string, Set<vscode.WebviewPanel>>();

  // Reader handles per document. Parsed once; subsequent decode / render
  // calls reuse the same `Grib{1,2}Handle` rather than re-parsing the
  // buffer on every napi call (was #41 — closed by the handle API).
  private readonly _handlesByDoc = new Map<string, Grib1Handle | Grib2Handle>();

  // -------------------------------------------------------------------------
  // CustomEditorProvider lifecycle
  // -------------------------------------------------------------------------

  async openCustomDocument(
    uri: vscode.Uri,
    _openContext?: vscode.CustomDocumentOpenContext,
    _token?: vscode.CancellationToken
  ): Promise<FieldglassDocument> {
    return FieldglassDocument.create(uri);
  }

  async resolveCustomEditor(
    document: FieldglassDocument,
    panel: vscode.WebviewPanel
  ): Promise<void> {
    this.trackPanel(document, panel);

    const native = loadNative();
    const header = document.bytes.slice(0, 32);
    const format = native ? native.detectBytes(header) : "unknown";

    const handle = native ? this.openOrReuseHandle(document, format) : undefined;
    const messages = handle?.messages();
    let dataset: DatasetMeta | undefined;
    if (native && format === "netcdf") {
      try {
        dataset = native.openNetcdf(document.bytes);
      } catch (err) {
        console.error("[Fieldglass] openNetcdf failed:", err);
        // Leave `dataset` undefined; the renderer will fall back to the
        // "no messages found" status string with the format badge intact.
      }
    }
    const headerBytes = format === "unknown" ? header : undefined;
    // Editing wiring (set_p1, undo/redo, save, webview script + input) is kept
    // intact for when general PDS field editing lands, but disabled at the
    // entry point so beta users see a coherent read-only viewer instead of a
    // single editable column.
    const editable = false;

    // Scripts must be enabled so the webview can request and paint a 2-D
    // render of a message's decoded grid. The CSP set in renderHtml is the
    // security boundary — see the comment there for the policy itself.
    panel.webview.options = { enableScripts: true };
    panel.webview.html = renderHtml(
      panel.webview,
      format,
      document.uri.fsPath,
      messages,
      dataset,
      headerBytes,
      editable
    );

    panel.webview.onDidReceiveMessage((msg: WebviewMessage) => {
      this.handleWebviewMessage(document, panel, msg);
    });
  }

  async saveCustomDocument(
    document: FieldglassDocument,
    _cancellation: vscode.CancellationToken
  ): Promise<void> {
    await vscode.workspace.fs.writeFile(document.uri, document.bytes);
  }

  async saveCustomDocumentAs(
    document: FieldglassDocument,
    destination: vscode.Uri,
    _cancellation: vscode.CancellationToken
  ): Promise<void> {
    await vscode.workspace.fs.writeFile(destination, document.bytes);
  }

  async revertCustomDocument(
    document: FieldglassDocument,
    _cancellation: vscode.CancellationToken
  ): Promise<void> {
    await document.revertFromDisk();
    this.broadcastUpdate(document);
  }

  async backupCustomDocument(
    document: FieldglassDocument,
    context: vscode.CustomDocumentBackupContext,
    _cancellation: vscode.CancellationToken
  ): Promise<vscode.CustomDocumentBackup> {
    const dest = context.destination;
    await vscode.workspace.fs.writeFile(dest, document.bytes);
    return {
      id: dest.toString(),
      delete: async () => {
        try {
          await vscode.workspace.fs.delete(dest);
        } catch {
          // backup file may already be gone
        }
      },
    };
  }

  // -------------------------------------------------------------------------
  // Edit pipeline
  // -------------------------------------------------------------------------

  private handleWebviewMessage(
    document: FieldglassDocument,
    panel: vscode.WebviewPanel,
    msg: WebviewMessage
  ): void {
    switch (msg.type) {
      case "ready":
        // Webview just finished mounting; push the current state so its
        // inputs are guaranteed to reflect document.bytes.
        this.postCurrentMessages(panel, document);
        return;
      case "edit-p1":
        if (!isNonNegativeInt(msg.messageIndex) || !isNonNegativeInt(msg.value)) return;
        this.applyP1Edit(document, msg.messageIndex, msg.value);
        return;
      case "decodeGrid":
        if (!isNonNegativeInt(msg.messageIndex)) return;
        this.handleDecodeGrid(document, panel, msg.messageIndex);
        return;
    }
  }

  /** Decode one message's grid in Rust and post values + shape to the webview. */
  private handleDecodeGrid(
    document: FieldglassDocument,
    panel: vscode.WebviewPanel,
    messageIndex: number
  ): void {
    const native = loadNative();
    if (!native) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: `native module ${nativeBinaryName()} not loaded`,
      });
      return;
    }
    const handle = this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: "no reader handle for document (not a GRIB file?)",
      });
      return;
    }
    const messages = handle.messages();
    // messageIndex originates from a webview-controlled message but is
    // bounds-checked immediately below; messages is a plain Array.
    // eslint-disable-next-line security/detect-object-injection
    const meta = messages[messageIndex];
    if (!meta) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: `message ${messageIndex} out of range`,
      });
      return;
    }
    if (meta.gridNi === null || meta.gridNj === null) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: "message has no grid dimensions (unsupported GDS)",
      });
      return;
    }

    // The first render uses the picker defaults: source projection +
    // nearest resampling + auto range + no y-flip. Subsequent renders
    // come back via `rerenderRequest` with whatever the user has dialled
    // in.
    this.openRenderPanel(document, meta);

    panel.webview.postMessage({ type: "renderOpened", messageIndex });
  }

  /**
   * Pop a separate webview tab beside the table view that paints the
   * decoded grid at full resolution. Each render gets its own tab so
   * users can compare messages side-by-side.
   *
   * The panel script never decodes the values itself — every paint runs
   * via `handle.renderGrid(meta.messageIndex, options)` on the provider
   * side and ships a paint-ready RGBA Buffer over postMessage. Picker
   * changes (projection / resampling / range / flip-y) flow back as
   * `rerenderRequest` and trigger a fresh `renderGrid` call.
   */
  private openRenderPanel(
    document: FieldglassDocument,
    meta: MessageMeta,
  ): void {
    const title = `Render: msg ${meta.messageIndex}`
      + (meta.parameterAbbreviation ? ` — ${meta.parameterAbbreviation}` : "");
    const panel = vscode.window.createWebviewPanel(
      "fieldglass.render",
      title,
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, retainContextWhenHidden: false }
    );
    panel.webview.html = renderImagePanelHtml(panel.webview, meta, describeProjection(meta));

    const defaultOptions: RenderOptions = {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    };

    const paint = (options: RenderOptions) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: meta.messageIndex,
          error: "reader handle was disposed",
        });
        return;
      }
      try {
        const rendered = docHandle.renderGrid(meta.messageIndex, options);
        panel.webview.postMessage(
          buildGridReadyMessage(rendered, meta, options),
        );
      } catch (err) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: meta.messageIndex,
          error: `render failed: ${err}`,
        });
      }
    };

    // Respond for the panel's lifetime: webview is created with
    // retainContextWhenHidden=false so VS Code tears down the DOM/JS
    // context when the tab is hidden; each remount posts a fresh `ready`.
    const sub = panel.webview.onDidReceiveMessage(
      (m: { type?: string } & Partial<RenderOptions>) => {
        if (!m || typeof m.type !== "string") return;
        if (m.type === "ready") {
          paint(defaultOptions);
          return;
        }
        if (m.type === "rerenderRequest") {
          // Clamp webview-controlled strings to the closed set the Rust
          // side accepts. The native validation in `ResolvedOptions::parse`
          // would reject typos with an explicit error, but we'd rather
          // never round-trip an invalid value at all — a stale webview
          // script with a typo silently snapping to "source" is the
          // expected behaviour, not an error popup.
          const projection: RenderOptions["projection"] =
            m.projection === "equirectangular" ? "equirectangular" : "source";
          const resampling: RenderOptions["resampling"] =
            m.resampling === "bilinear" ? "bilinear" : "nearest";
          const options: RenderOptions = {
            projection,
            resampling,
            flipY: !!m.flipY,
            rangeMin: m.rangeMin,
            rangeMax: m.rangeMax,
          };
          paint(options);
        }
      },
    );
    panel.onDidDispose(() => sub.dispose());
  }

  /** Public for tests; webview message handler also calls into this. */
  public applyP1Edit(
    document: FieldglassDocument,
    messageIndex: number,
    value: number
  ): void {
    const native = loadNative();
    if (!native) {
      throw new Error(
        `Fieldglass: native module ${nativeBinaryName()} could not be loaded`
      );
    }

    const oldBytes = document.bytes;
    // Try the cached handle first; fall back to a transient handle so
    // callers that haven't been through `resolveCustomEditor` (e.g.
    // unit tests that drive `applyP1Edit` directly off
    // `openCustomDocument`) still work.
    let handle = this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      try {
        handle = native.Grib1Handle.fromBytes(document.bytes);
      } catch (err) {
        console.error("[Fieldglass] setP1 lazy handle init failed:", err);
        vscode.window.showErrorMessage(`Fieldglass: failed to parse GRIB1: ${err}`);
        return;
      }
    }
    if (!("setP1" in handle)) {
      vscode.window.showErrorMessage(
        "Fieldglass: setP1 only applies to GRIB1 documents",
      );
      return;
    }
    let newBytes: Uint8Array;
    try {
      newBytes = (handle as Grib1Handle).setP1(messageIndex, value);
    } catch (err) {
      console.error("[Fieldglass] setP1 failed:", err);
      vscode.window.showErrorMessage(`Fieldglass: failed to set p1: ${err}`);
      // Re-broadcast the old state so the input snaps back.
      this.broadcastUpdate(document);
      return;
    }

    document.setBytes(newBytes);
    // Bytes changed → the cached handle is stale. Drop it so the next
    // `openOrReuseHandle` reparses against the new bytes.
    this._handlesByDoc.delete(document.uri.toString());
    this.broadcastUpdate(document);

    this._onDidChangeCustomDocument.fire({
      document,
      label: `Edit forecast period (message ${messageIndex})`,
      undo: () => {
        document.setBytes(oldBytes);
        this.broadcastUpdate(document);
      },
      redo: () => {
        document.setBytes(newBytes);
        this.broadcastUpdate(document);
      },
    });
  }

  // -------------------------------------------------------------------------
  // Panel tracking
  // -------------------------------------------------------------------------

  private trackPanel(document: FieldglassDocument, panel: vscode.WebviewPanel): void {
    const key = document.uri.toString();
    let set = this._panelsByDoc.get(key);
    if (!set) {
      set = new Set();
      this._panelsByDoc.set(key, set);
    }
    set.add(panel);
    panel.onDidDispose(() => {
      const s = this._panelsByDoc.get(key);
      if (s) {
        s.delete(panel);
        if (s.size === 0) {
          // Last panel for this document closed — drop the reader handle
          // so we don't leak the parsed bytes + per-message decode cache
          // for every file the user has ever opened in this session.
          // The handle will be rebuilt on the next `resolveCustomEditor`.
          this._panelsByDoc.delete(key);
          this._handlesByDoc.delete(key);
        }
      }
    });
  }

  /** Re-parse the document and push fresh messages to every panel
   *  bound to it. Rebuilds the cached handle exactly once per broadcast
   *  — earlier shape was O(panels) reparses on every edit. */
  private broadcastUpdate(document: FieldglassDocument): void {
    const panels = this._panelsByDoc.get(document.uri.toString());
    if (!panels || panels.size === 0) return;
    const messages = this.reparseAndCache(document);
    if (!messages) return;
    for (const p of panels) {
      p.webview.postMessage({ type: "update", messages });
    }
  }

  /** Send the current document state to a single panel (used by the
   *  `ready` mount handshake). Same reparse-and-cache shape as
   *  [`broadcastUpdate`]; if the cached handle is still good (no
   *  intervening edits) we reuse it. */
  private postCurrentMessages(
    panel: vscode.WebviewPanel,
    document: FieldglassDocument,
  ): void {
    const cached = this._handlesByDoc.get(document.uri.toString());
    const messages = cached
      ? cached.messages()
      : this.reparseAndCache(document);
    if (!messages) return;
    panel.webview.postMessage({ type: "update", messages });
  }

  private reparseAndCache(document: FieldglassDocument): MessageMeta[] | undefined {
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle = native.Grib1Handle.fromBytes(document.bytes);
      this._handlesByDoc.set(document.uri.toString(), handle);
      return handle.messages();
    } catch (err) {
      vscode.window.showErrorMessage(`Fieldglass: failed to re-parse after edit: ${err}`);
      return undefined;
    }
  }

  /**
   * Get-or-build the cached reader handle for a document. Called from
   * the main `resolveCustomEditor` path; subsequent renders reuse the
   * cached handle to avoid re-parsing the entire file on every call.
   */
  private openOrReuseHandle(
    document: FieldglassDocument,
    format: string,
  ): Grib1Handle | Grib2Handle | undefined {
    const key = document.uri.toString();
    const cached = this._handlesByDoc.get(key);
    if (cached) return cached;
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle: Grib1Handle | Grib2Handle | undefined = format === "grib1"
        ? native.Grib1Handle.fromBytes(document.bytes)
        : format === "grib2"
        ? native.Grib2Handle.fromBytes(document.bytes)
        : undefined;
      if (handle) {
        this._handlesByDoc.set(key, handle);
        // Drop the cached handle when the document is closed.
        // VS Code doesn't expose a per-document close event on
        // CustomEditorProvider, so we rely on bytes changes (handled
        // in applyP1Edit) plus the LRU effect of files being re-opened.
      }
      return handle;
    } catch (err) {
      console.error("[Fieldglass] handle creation failed:", err);
      vscode.window.showErrorMessage(`Fieldglass: failed to parse ${format}: ${err}`);
      return undefined;
    }
  }
}

// ---------------------------------------------------------------------------
// Render-panel wire payload
// ---------------------------------------------------------------------------

export interface GridReadyMessage {
  type: "gridReady";
  messageIndex: number;
  rgba: Uint8Array;
  width: number;
  height: number;
  usedMin: number;
  usedMax: number;
  projectionSummary: string;
  options: RenderOptions;
}

/**
 * Compose the `gridReady` payload posted to the render panel's webview.
 *
 * napi hands back `rendered.rgba` as a Node `Buffer`, whose
 * `constructor.name === "Buffer"`. VS Code's webview serializer
 * (`extHostWebviewMessaging.ts::getTypedArrayType`) only recognises the
 * standard TypedArray constructor names (`Uint8Array`, `Float64Array`,
 * …) and silently falls back to default JSON for anything else —
 * `Buffer.prototype.toJSON` emits `{type:"Buffer", data:[…]}`, which the
 * webview script then fails to blit (`new ImageData` throws on length 0).
 *
 * Wrapping as a plain `Uint8Array` view makes the serializer ship the
 * bytes as a binary reference, and the webview revives it as a real
 * `Uint8Array` on the other side. Pinned by `render.test.ts`.
 */
export function buildGridReadyMessage(
  rendered: RenderedGrid,
  meta: MessageMeta,
  options: RenderOptions,
): GridReadyMessage {
  const rgbaView = new Uint8Array(
    rendered.rgba.buffer,
    rendered.rgba.byteOffset,
    rendered.rgba.byteLength,
  );
  return {
    type: "gridReady",
    messageIndex: meta.messageIndex,
    rgba: rgbaView,
    width: rendered.width,
    height: rendered.height,
    usedMin: rendered.usedMin,
    usedMax: rendered.usedMax,
    projectionSummary: rendered.projectionSummary,
    options,
  };
}

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

function isNonNegativeInt(n: unknown): n is number {
  return typeof n === "number" && Number.isInteger(n) && n >= 0;
}

/// Compose the "Centre" table cell: centre name plus, when available, the
/// GRIB2 production status (Code Table 1.3) so operational vs. research
/// products are visible at a glance without adding another column.
function formatCentreCell(m: MessageMeta): string {
  const status = m.productionStatus;
  if (status && status !== "Missing" && status !== "Unknown") {
    return `${m.originatingCentre} · ${status}`;
  }
  return m.originatingCentre;
}

function describeProjection(meta: MessageMeta): string {
  const dims = (meta.gridNi !== null && meta.gridNj !== null)
    ? `${meta.gridNi}×${meta.gridNj}` : "?";
  const type = meta.gridType ?? "unknown grid";
  if (meta.latFirst !== null && meta.lonFirst !== null
      && meta.latLast !== null && meta.lonLast !== null) {
    const f = (v: number) => v.toFixed(2);
    return `${type} ${dims} — ${f(meta.latFirst)},${f(meta.lonFirst)} → `
         + `${f(meta.latLast)},${f(meta.lonLast)} (grid coordinates)`;
  }
  return `${type} ${dims} (grid coordinates)`;
}

function renderDatasetBody(d: DatasetMeta): string {
  // Long attribute strings are common in CF-Convention NetCDF files; truncate
  // for the row view but keep the full text in the title attribute so users
  // can hover to read it. Numeric attributes never hit this limit.
  const ATTR_PREVIEW_LIMIT = 120;
  const previewAttr = (s: string): string => {
    if (s.length <= ATTR_PREVIEW_LIMIT) return escapeHtml(s);
    return escapeHtml(s.slice(0, ATTR_PREVIEW_LIMIT)) + "…";
  };

  const sections: string[] = [];

  if (!d.fullyParsed && d.note) {
    const versionLine = d.hdf5SuperblockVersion !== undefined
      ? `<div class="status">HDF5 superblock version: ${d.hdf5SuperblockVersion}</div>`
      : "";
    sections.push(`
      <div class="netcdf-notice">
        <div class="dump-label">${escapeHtml(d.backingLabel)}</div>
        <div class="status">${escapeHtml(d.note)}</div>
        ${versionLine}
      </div>`);
    return sections.join("\n");
  }

  sections.push(`<div class="dump-label">${escapeHtml(d.backingLabel)}</div>`);

  if (d.dimensions.length > 0) {
    const rows = d.dimensions.map((dim) => `
      <tr>
        <td>${escapeHtml(dim.name)}</td>
        <td>${dim.isRecord ? "unlimited" : String(dim.length)}</td>
        <td>${dim.isRecord ? "record" : "fixed"}</td>
      </tr>`).join("");
    sections.push(`
      <h2>Dimensions</h2>
      <table>
        <thead><tr><th>Name</th><th>Length</th><th>Kind</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>`);
  }

  if (d.globalAttributes.length > 0) {
    const rows = d.globalAttributes.map((a) => `
      <tr>
        <td>${escapeHtml(a.name)}</td>
        <td>${escapeHtml(a.ncType)}</td>
        <td title="${escapeHtml(a.value)}">${previewAttr(a.value)}</td>
      </tr>`).join("");
    sections.push(`
      <h2>Global attributes</h2>
      <table>
        <thead><tr><th>Name</th><th>Type</th><th>Value</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>`);
  }

  if (d.variables.length > 0) {
    const rows = d.variables.map((v) => {
      const dims = v.dimensions.length > 0
        ? v.dimensions.map(escapeHtml).join(", ")
        : "—";
      const attrPreview = v.attributes.length === 0
        ? "—"
        : v.attributes.slice(0, 3).map((a) =>
            `${escapeHtml(a.name)}=${previewAttr(a.value)}`
          ).join("; ") + (v.attributes.length > 3 ? `; +${v.attributes.length - 3} more` : "");
      return `
      <tr>
        <td>${escapeHtml(v.name)}</td>
        <td>${escapeHtml(v.ncType)}</td>
        <td>${dims}</td>
        <td>${attrPreview}</td>
      </tr>`;
    }).join("");
    sections.push(`
      <h2>Variables</h2>
      <table>
        <thead><tr><th>Name</th><th>Type</th><th>Dimensions</th><th>Attributes</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>`);
  }

  if (d.dimensions.length === 0 && d.globalAttributes.length === 0 && d.variables.length === 0) {
    sections.push(`<div class="status">Empty NetCDF dataset.</div>`);
  }

  return sections.join("\n");
}

function renderHtml(
  webview: vscode.Webview,
  format: string,
  filePath: string,
  messages: MessageMeta[] | undefined,
  dataset: DatasetMeta | undefined,
  headerBytes: Uint8Array | undefined,
  editable: boolean
): string {
  // FORMAT_LABELS is a closed Record<string, string>; `format` originates
  // from native detect_bytes which returns one of a fixed set of tokens.
  // eslint-disable-next-line security/detect-object-injection
  const label = FORMAT_LABELS[format] ?? "Unknown";
  const filename = path.basename(filePath);
  const isKnown = format !== "unknown";
  const cspNonce = nonce();

  let bodyContent = "";

  if (messages && messages.length > 0) {
    const fmt1 = (v: number | null) => v !== null ? v.toFixed(3) : "—";
    const COLSPAN = 12;
    const rows = messages.map((m) => {
      const gridDims = (m.gridNi !== null && m.gridNj !== null)
        ? `${m.gridNi}×${m.gridNj}` : "—";
      const gridBounds = (m.latFirst !== null && m.lonFirst !== null)
        ? `${fmt1(m.latFirst)},${fmt1(m.lonFirst)} → ${fmt1(m.latLast)},${fmt1(m.lonLast)}` : "—";
      const fcstCell = editable
        ? `<input type="number" class="p1-input" data-message-index="${m.messageIndex}" min="0" max="255" step="1" value="${m.forecastHours}" />`
        : escapeHtml(m.forecastDisplay);
      const canRender = m.gridNi !== null && m.gridNj !== null;
      const idx = m.messageIndex;
      const expansionInner = canRender
        ? `<button type="button" class="render-btn" data-message-index="${idx}">Render</button>
           <div class="render-status" id="status-${idx}"></div>
           <div class="render-legend">
             Opens the rendered grid in a new editor tab. Painted in grid
             coordinates (no map reprojection); bitmap-masked points render
             as transparent.
           </div>`
        : `<div class="render-na">Render not available — grid dimensions unknown for this message.</div>`;
      return `
      <tr class="msg-row" data-message-index="${idx}">
        <td>${idx}</td>
        <td>${escapeHtml(m.parameterName)}</td>
        <td>${escapeHtml(m.parameterAbbreviation)}</td>
        <td>${escapeHtml(m.parameterUnits)}</td>
        <td>${escapeHtml(m.level)}</td>
        <td>${escapeHtml(m.levelType)}</td>
        <td>${escapeHtml(m.referenceTime)}</td>
        <td>${fcstCell}</td>
        <td>${escapeHtml(m.gridType ?? "—")}</td>
        <td>${gridDims}</td>
        <td>${gridBounds}</td>
        <td>${escapeHtml(formatCentreCell(m))}</td>
      </tr>
      <tr class="expand-row" id="expand-${idx}" hidden>
        <td class="expand-cell" colspan="${COLSPAN}">
          <div class="expand-content">${expansionInner}</div>
        </td>
      </tr>`;
    }).join("");
    const fcstHeader = editable ? "Fcst (p1)" : "Fcst";
    bodyContent = `
    <table>
      <thead>
        <tr>
          <th>#</th><th>Parameter</th><th>Abbrev</th><th>Units</th>
          <th>Level</th><th>Level Type</th><th>Reference Time</th><th>${fcstHeader}</th>
          <th>Grid</th><th>Size</th><th>Bounds (lat,lon)</th><th>Centre</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>`;
  } else if (dataset) {
    bodyContent = renderDatasetBody(dataset);
  } else if (!isKnown && headerBytes && headerBytes.length > 0) {
    const hex = Array.from(headerBytes)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join(" ");
    const ascii = Array.from(headerBytes)
      .map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."))
      .join("");
    bodyContent = `
    <div class="header-dump">
      <div class="dump-label">First ${headerBytes.length} bytes</div>
      <code class="hex">${hex}</code>
      <code class="ascii">${escapeHtml(ascii)}</code>
    </div>`;
  } else {
    bodyContent = `<div class="status">No messages found.</div>`;
  }

  // Webview Content-Security-Policy. The CSP IS the security boundary that
  // makes enabling scripts safe: it blocks every loader except the webview's
  // own origin and a per-document nonce for our single inline script. No
  // 'unsafe-inline' on script-src, no 'unsafe-eval' anywhere. Image sources
  // include `blob:` and `data:` because the canvas-painted render may be
  // exported via `toDataURL()` for save-image affordances later, and `data:`
  // covers small inline tile previews. `style-src` keeps `'unsafe-inline'`
  // only because VS Code-themed inline styles drive layout colors.
  const csp = [
    `default-src 'none'`,
    `script-src 'nonce-${cspNonce}'`,
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `img-src ${webview.cspSource} blob: data:`,
  ].join("; ");

  const script = `
    <script nonce="${cspNonce}">
      (function () {
        const vscode = acquireVsCodeApi();
        const editable = ${editable ? "true" : "false"};

        function statusElFor(idx) { return document.getElementById('status-' + idx); }
        function expansionFor(idx) { return document.getElementById('expand-' + idx); }
        function rowFor(idx) { return document.querySelector('tr.msg-row[data-message-index="' + idx + '"]'); }

        function setStatus(idx, text) {
          const el = statusElFor(idx);
          if (el) el.textContent = text;
        }

        function collapseAll() {
          document.querySelectorAll('tr.expand-row').forEach((er) => er.setAttribute('hidden', ''));
          document.querySelectorAll('tr.msg-row.selected').forEach((r) => r.classList.remove('selected'));
        }

        function selectRow(idx) {
          const expansion = expansionFor(idx);
          const row = rowFor(idx);
          if (!expansion || !row) return;
          const isOpen = !expansion.hasAttribute('hidden');
          collapseAll();
          if (!isOpen) {
            expansion.removeAttribute('hidden');
            row.classList.add('selected');
          }
        }

        function attach() {
          document.querySelectorAll('tr.msg-row').forEach((row) => {
            row.addEventListener('click', (ev) => {
              // Don't toggle when the click was on an interactive descendant
              // (button, input) inside the expanded row.
              const t = ev.target;
              if (t && (t.closest && t.closest('button, input, a'))) return;
              const idx = Number(row.getAttribute('data-message-index'));
              if (Number.isFinite(idx)) selectRow(idx);
            });
          });
          document.querySelectorAll('button.render-btn').forEach((el) => {
            el.addEventListener('click', (ev) => {
              ev.stopPropagation();
              const idx = Number(el.getAttribute('data-message-index'));
              if (!Number.isFinite(idx)) return;
              setStatus(idx, 'Decoding message ' + idx + '…');
              vscode.postMessage({ type: 'decodeGrid', messageIndex: idx });
            });
          });
          if (editable) {
            // Forecast-period inputs send an edit on commit (Enter / blur).
            document.querySelectorAll('input.p1-input').forEach((el) => {
              el.addEventListener('change', () => {
                const idx = Number(el.getAttribute('data-message-index'));
                const v = Number(el.value);
                if (!Number.isFinite(v) || v < 0 || v > 255 || !Number.isInteger(v)) {
                  return;
                }
                vscode.postMessage({ type: 'edit-p1', messageIndex: idx, value: v });
              });
            });
          }
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          if (msg.type === 'renderOpened') {
            setStatus(msg.messageIndex, 'Opened render of message ' + msg.messageIndex + ' in a new tab.');
            return;
          }
          if (msg.type === 'gridError') {
            setStatus(msg.messageIndex, 'Render failed: ' + msg.error);
            return;
          }
          if (editable && msg.type === 'update' && Array.isArray(msg.messages)) {
            for (const m of msg.messages) {
              const el = document.querySelector('input.p1-input[data-message-index="' + m.messageIndex + '"]');
              if (el && document.activeElement !== el) {
                el.value = String(m.forecastHours);
              }
            }
          }
        });

        attach();
        vscode.postMessage({ type: 'ready' });
      })();
    </script>
  `;

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Fieldglass</title>
  <style>
    body {
      font-family: var(--vscode-font-family);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 2rem;
      margin: 0;
    }
    h1 { font-size: 1.4rem; margin-bottom: 0.25rem; }
    h2 { font-size: 1.05rem; margin-top: 1.5rem; margin-bottom: 0.4rem; color: var(--vscode-descriptionForeground); font-weight: 600; }
    .netcdf-notice { margin-top: 1rem; }
    .subtitle { color: var(--vscode-descriptionForeground); font-size: 0.9rem; margin-bottom: 2rem; }
    .badge {
      display: inline-block;
      padding: 0.2rem 0.6rem;
      border-radius: 3px;
      font-size: 0.8rem;
      font-weight: bold;
      margin-bottom: 1rem;
      background: ${isKnown ? "var(--vscode-badge-background)" : "var(--vscode-inputValidation-warningBackground)"};
      color: ${isKnown ? "var(--vscode-badge-foreground)" : "var(--vscode-inputValidation-warningForeground)"};
    }
    .status { font-size: 0.95rem; color: var(--vscode-descriptionForeground); }
    table { border-collapse: collapse; font-size: 0.85rem; width: 100%; }
    th, td { text-align: left; padding: 0.3rem 0.6rem; border-bottom: 1px solid var(--vscode-panel-border); white-space: nowrap; }
    th { color: var(--vscode-descriptionForeground); font-weight: 600; }
    tr.msg-row { cursor: pointer; }
    tr.msg-row:hover td { background: var(--vscode-list-hoverBackground); }
    tr.msg-row.selected td {
      background: var(--vscode-list-activeSelectionBackground);
      color: var(--vscode-list-activeSelectionForeground);
    }
    tr.expand-row td.expand-cell {
      background: var(--vscode-editorWidget-background, var(--vscode-editor-background));
      padding: 0.75rem 1rem;
      white-space: normal;
    }
    .expand-content {
      display: flex;
      flex-direction: column;
      align-items: flex-start;
      gap: 0.5rem;
    }
    button.render-btn { white-space: nowrap; }
    .header-dump { margin-top: 1rem; }
    .dump-label { font-size: 0.8rem; color: var(--vscode-descriptionForeground); margin-bottom: 0.25rem; }
    code { display: block; font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85rem; }
    .ascii { color: var(--vscode-descriptionForeground); margin-top: 0.2rem; }
    input.p1-input {
      width: 4.5rem;
      background: var(--vscode-input-background);
      color: var(--vscode-input-foreground);
      border: 1px solid var(--vscode-input-border, transparent);
      padding: 0.1rem 0.3rem;
      font-family: inherit;
      font-size: inherit;
    }
    input.p1-input:focus {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: -1px;
    }
    button.render-btn {
      background: var(--vscode-button-secondaryBackground, var(--vscode-button-background));
      color: var(--vscode-button-secondaryForeground, var(--vscode-button-foreground));
      border: 1px solid var(--vscode-button-border, transparent);
      padding: 0.15rem 0.6rem;
      cursor: pointer;
      font-family: inherit;
      font-size: inherit;
      border-radius: 2px;
    }
    button.render-btn:hover {
      background: var(--vscode-button-secondaryHoverBackground, var(--vscode-button-hoverBackground));
    }
    button.render-btn:focus {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: 1px;
    }
    .render-na { color: var(--vscode-descriptionForeground); font-size: 0.85rem; }
    .render-status { font-size: 0.85rem; min-height: 1.1em; }
    .render-legend { font-size: 0.75rem; color: var(--vscode-descriptionForeground); }
  </style>
</head>
<body>
  <h1>Fieldglass</h1>
  <div class="subtitle">${escapeHtml(filename)}</div>
  <div class="badge">${escapeHtml(label)}</div>
  ${bodyContent}
  ${script}
</body>
</html>`;
}
