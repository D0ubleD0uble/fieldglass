import * as vscode from "vscode";
import * as path from "path";

import { VIRIDIS_LUT } from "./render-helpers";

interface MessageMeta {
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
}

let fieldglass: {
  detectBytes: (bytes: Uint8Array) => string;
  openGrib1: (bytes: Uint8Array) => MessageMeta[];
  decodeGrid: (bytes: Uint8Array, messageIndex: number) => Array<number | null>;
  setP1: (bytes: Uint8Array, messageIndex: number, value: number) => Buffer;
} | undefined;

function nativeBinaryName(): string {
  const platform = process.platform;
  const arch = process.arch;
  const abi = platform === "linux" ? "-gnu"
            : platform === "win32" ? "-msvc"
            : "";
  return `fieldglass.${platform}-${arch}${abi}.node`;
}

function loadNative(): typeof fieldglass {
  if (fieldglass) {
    return fieldglass;
  }
  const nodePath = path.join(__dirname, "..", "bin", nativeBinaryName());
  try {
    // The native module path is computed at runtime from process.platform /
    // arch, so we must use require() rather than a static import. The path
    // is built from a closed set of platform/arch tokens — never user input.
    // eslint-disable-next-line @typescript-eslint/no-require-imports, security/detect-non-literal-require
    fieldglass = require(nodePath);
  } catch (err) {
    console.error(`[Fieldglass] failed to load ${nodePath}:`, err);
    vscode.window.showErrorMessage(
      `Fieldglass: failed to load native module (${nativeBinaryName()}): ${err}`
    );
  }
  return fieldglass;
}

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

    const messages = (native && format === "grib1")
      ? native.openGrib1(document.bytes)
      : undefined;
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
        this.postUpdate(panel, document);
        return;
      case "edit-p1":
        this.applyP1Edit(document, msg.messageIndex, msg.value);
        return;
      case "decodeGrid":
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
    let messages: MessageMeta[];
    try {
      messages = native.openGrib1(document.bytes);
    } catch (err) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: `re-parse failed: ${err}`,
      });
      return;
    }
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
    let raw: Array<number | null>;
    try {
      raw = native.decodeGrid(document.bytes, messageIndex);
    } catch (err) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: `decode failed: ${err}`,
      });
      return;
    }

    // Convert the heterogeneous Array<number | null> coming back from napi
    // into a Float64Array (NaN sentinel for masked) plus a parallel Uint8Array
    // mask. This keeps the postMessage transfer compact (typed arrays
    // structured-clone fast) and lets the webview paint without per-cell
    // null checks. NaN is the sentinel because Float64Array can't hold null.
    const total = raw.length;
    const values = new Float64Array(total);
    const bitmapMask = new Uint8Array(total);
    let anyMasked = false;
    // i is a strictly bounded counter; values/bitmapMask/raw are length
    // `total`. The security plugin can't see the loop bound, so silence the
    // generic-injection warning here.
    /* eslint-disable security/detect-object-injection */
    for (let i = 0; i < total; i++) {
      const v = raw[i];
      if (v === null) {
        values[i] = Number.NaN;
        bitmapMask[i] = 0;
        anyMasked = true;
      } else {
        values[i] = v;
        bitmapMask[i] = 1;
      }
    }
    /* eslint-enable security/detect-object-injection */

    const projectionSummary = describeProjection(meta);

    panel.webview.postMessage({
      type: "gridReady",
      messageIndex,
      values,
      nx: meta.gridNi,
      ny: meta.gridNj,
      projectionSummary,
      bitmapMask: anyMasked ? bitmapMask : undefined,
    });
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
    let newBytes: Uint8Array;
    try {
      newBytes = native.setP1(oldBytes, messageIndex, value);
    } catch (err) {
      console.error("[Fieldglass] setP1 failed:", err);
      vscode.window.showErrorMessage(`Fieldglass: failed to set p1: ${err}`);
      // Re-broadcast the old state so the input snaps back.
      this.broadcastUpdate(document);
      return;
    }

    document.setBytes(newBytes);
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
        if (s.size === 0) this._panelsByDoc.delete(key);
      }
    });
  }

  private broadcastUpdate(document: FieldglassDocument): void {
    const panels = this._panelsByDoc.get(document.uri.toString());
    if (!panels) return;
    for (const p of panels) {
      this.postUpdate(p, document);
    }
  }

  private postUpdate(panel: vscode.WebviewPanel, document: FieldglassDocument): void {
    const native = loadNative();
    if (!native) return;
    try {
      const messages = native.openGrib1(document.bytes);
      panel.webview.postMessage({ type: "update", messages });
    } catch (err) {
      vscode.window.showErrorMessage(`Fieldglass: failed to re-parse after edit: ${err}`);
    }
  }
}

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

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

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function nonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += chars.charAt(Math.floor(Math.random() * chars.length));
  return s;
}

function renderHtml(
  webview: vscode.Webview,
  format: string,
  filePath: string,
  messages: MessageMeta[] | undefined,
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
    const rows = messages.map((m) => {
      const gridDims = (m.gridNi !== null && m.gridNj !== null)
        ? `${m.gridNi}×${m.gridNj}` : "—";
      const gridBounds = (m.latFirst !== null && m.lonFirst !== null)
        ? `${fmt1(m.latFirst)},${fmt1(m.lonFirst)} → ${fmt1(m.latLast)},${fmt1(m.lonLast)}` : "—";
      const fcstCell = editable
        ? `<input type="number" class="p1-input" data-message-index="${m.messageIndex}" min="0" max="255" step="1" value="${m.forecastHours}" />`
        : escapeHtml(m.forecastDisplay);
      const canRender = m.gridNi !== null && m.gridNj !== null;
      const renderCell = canRender
        ? `<button type="button" class="render-btn" data-message-index="${m.messageIndex}">Render</button>`
        : `<span class="render-na" title="Grid dimensions unknown">—</span>`;
      return `
      <tr>
        <td>${m.messageIndex}</td>
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
        <td>${escapeHtml(m.originatingCentre)}</td>
        <td>${renderCell}</td>
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
          <th>Render</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>
    <section id="render-pane" class="render-pane" hidden>
      <h2 class="render-title">Grid render</h2>
      <div class="render-meta" id="render-meta"></div>
      <div class="render-status" id="render-status"></div>
      <div class="render-area">
        <canvas id="render-canvas" class="render-canvas"></canvas>
        <div class="colorbar-wrap">
          <canvas id="colorbar-canvas" class="colorbar-canvas" width="20" height="200"></canvas>
          <div class="colorbar-labels">
            <div class="cb-max" id="cb-max">—</div>
            <div class="cb-min" id="cb-min">—</div>
          </div>
        </div>
      </div>
      <div class="render-legend">
        Painted in grid coordinates (no map reprojection). Bitmap-masked
        points render as transparent.
      </div>
    </section>`;
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

  // Embed the viridis LUT as a JSON literal once per page so the webview
  // script can paint without reaching for any colormap library at runtime.
  const lutJson = JSON.stringify(Array.from(VIRIDIS_LUT));

  const script = `
    <script nonce="${cspNonce}">
      (function () {
        const vscode = acquireVsCodeApi();
        const VIRIDIS = new Uint8ClampedArray(${lutJson});
        const editable = ${editable ? "true" : "false"};

        function paintGrid(values, bitmapMask, nx, ny, min, max) {
          const total = nx * ny;
          const span = max - min;
          const denom = span > 0 ? span : 1;
          const buf = new Uint8ClampedArray(total * 4);
          for (let i = 0; i < total; i++) {
            const v = values[i];
            const masked = bitmapMask && bitmapMask[i] === 0;
            const o = i * 4;
            if (masked || !Number.isFinite(v)) {
              buf[o] = 0; buf[o + 1] = 0; buf[o + 2] = 0; buf[o + 3] = 0;
              continue;
            }
            let t = span > 0 ? (v - min) / denom : 0;
            if (t < 0) t = 0; else if (t > 1) t = 1;
            const idx = Math.round(t * 255) * 3;
            buf[o] = VIRIDIS[idx];
            buf[o + 1] = VIRIDIS[idx + 1];
            buf[o + 2] = VIRIDIS[idx + 2];
            buf[o + 3] = 255;
          }
          return new ImageData(buf, nx, ny);
        }

        function paintColorbar(canvas) {
          const ctx = canvas.getContext('2d');
          if (!ctx) return;
          const w = canvas.width, h = canvas.height;
          const buf = new Uint8ClampedArray(w * h * 4);
          for (let y = 0; y < h; y++) {
            // top of bar = max, bottom = min
            const t = 1 - y / Math.max(1, h - 1);
            const idx = Math.round(t * 255) * 3;
            for (let x = 0; x < w; x++) {
              const o = (y * w + x) * 4;
              buf[o] = VIRIDIS[idx];
              buf[o + 1] = VIRIDIS[idx + 1];
              buf[o + 2] = VIRIDIS[idx + 2];
              buf[o + 3] = 255;
            }
          }
          ctx.putImageData(new ImageData(buf, w, h), 0, 0);
        }

        function minMaxIgnoringMask(values, bitmapMask) {
          let min = Infinity, max = -Infinity, seen = false;
          for (let i = 0; i < values.length; i++) {
            if (bitmapMask && bitmapMask[i] === 0) continue;
            const v = values[i];
            if (!Number.isFinite(v)) continue;
            if (v < min) min = v;
            if (v > max) max = v;
            seen = true;
          }
          return seen ? { min, max } : null;
        }

        function setStatus(text) {
          const el = document.getElementById('render-status');
          if (el) el.textContent = text;
        }

        function showPane() {
          const pane = document.getElementById('render-pane');
          if (pane) pane.removeAttribute('hidden');
        }

        function handleGridReady(msg) {
          showPane();
          const canvas = document.getElementById('render-canvas');
          const cb = document.getElementById('colorbar-canvas');
          const meta = document.getElementById('render-meta');
          const cbMin = document.getElementById('cb-min');
          const cbMax = document.getElementById('cb-max');
          if (!canvas || !cb) return;
          const ctx = canvas.getContext('2d');
          if (!ctx) return;

          const nx = msg.nx, ny = msg.ny;
          canvas.width = nx;
          canvas.height = ny;

          const range = minMaxIgnoringMask(msg.values, msg.bitmapMask);
          if (!range) {
            setStatus('Message ' + msg.messageIndex + ' has no usable grid points (all masked or non-finite).');
            ctx.clearRect(0, 0, nx, ny);
            paintColorbar(cb);
            cbMin.textContent = '—';
            cbMax.textContent = '—';
            if (meta) meta.textContent = msg.projectionSummary || '';
            return;
          }
          const img = paintGrid(msg.values, msg.bitmapMask, nx, ny, range.min, range.max);
          ctx.putImageData(img, 0, 0);
          paintColorbar(cb);
          cbMin.textContent = range.min.toPrecision(4);
          cbMax.textContent = range.max.toPrecision(4);
          const masked = msg.bitmapMask ? ' (transparent = bitmap-masked)' : '';
          setStatus('Message ' + msg.messageIndex + ': ' + nx + '×' + ny
                    + ', range ' + range.min.toPrecision(4) + ' … ' + range.max.toPrecision(4)
                    + masked + '.');
          if (meta) meta.textContent = msg.projectionSummary || '';
        }

        function handleGridError(msg) {
          showPane();
          setStatus('Render failed for message ' + msg.messageIndex + ': ' + msg.error);
        }

        function attach() {
          document.querySelectorAll('button.render-btn').forEach((el) => {
            el.addEventListener('click', () => {
              const idx = Number(el.getAttribute('data-message-index'));
              if (!Number.isFinite(idx)) return;
              showPane();
              setStatus('Decoding message ' + idx + '…');
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
          if (msg.type === 'gridReady') {
            handleGridReady(msg);
            return;
          }
          if (msg.type === 'gridError') {
            handleGridError(msg);
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
    tr:hover td { background: var(--vscode-list-hoverBackground); }
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
    .render-na { color: var(--vscode-descriptionForeground); }
    .render-pane {
      margin-top: 2rem;
      padding-top: 1rem;
      border-top: 1px solid var(--vscode-panel-border);
    }
    .render-title { font-size: 1.05rem; margin: 0 0 0.4rem 0; }
    .render-meta { font-size: 0.85rem; color: var(--vscode-descriptionForeground); margin-bottom: 0.4rem; }
    .render-status { font-size: 0.85rem; margin-bottom: 0.5rem; }
    .render-area {
      display: flex;
      align-items: flex-start;
      gap: 0.75rem;
    }
    .render-canvas {
      max-width: 100%;
      height: auto;
      image-rendering: pixelated;
      background: var(--vscode-editor-background);
      border: 1px solid var(--vscode-panel-border);
    }
    .colorbar-wrap {
      display: flex;
      align-items: stretch;
      gap: 0.4rem;
      height: 200px;
    }
    .colorbar-canvas {
      width: 20px;
      height: 200px;
      border: 1px solid var(--vscode-panel-border);
    }
    .colorbar-labels {
      display: flex;
      flex-direction: column;
      justify-content: space-between;
      font-size: 0.75rem;
      color: var(--vscode-descriptionForeground);
    }
    .render-legend {
      margin-top: 0.5rem;
      font-size: 0.75rem;
      color: var(--vscode-descriptionForeground);
    }
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
