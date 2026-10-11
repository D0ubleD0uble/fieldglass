import * as vscode from "vscode";
import * as path from "path";

import {
  IMPORT_COLOR_TABLE_COMMAND,
  importColorTableFile,
  importedColorTable,
  importedPickerColormaps,
  useColorTableStore,
  type PickerColormap,
} from "./color-tables";
import { escapeHtml, nonce } from "./html";
import {
  loadNative,
  nativeBinaryName,
  type AxisValues,
  type CombineOp,
  type CombineOpInfo,
  type DatasetMeta,
  type Grib1Handle,
  type Grib2Handle,
  type Identification,
  type MessageInfo,
  type NetcdfHandle,
  type NetcdfVariableMeta,
  type SliceGrid,
  type SlicePanelHandle,
  type ZarrHandle,
  type RenderedGrid,
  type RenderOptions,
} from "./native";
import {
  buildGraticule,
  loadVectorLayer,
  type OverlayGeometry,
  type VectorLayer,
} from "./overlay";
import {
  composeDefaultPngName,
  composeTitleLine,
  composeTruncationNote,
  offersOverlays,
  renderImagePanelHtml,
  reprojectionNote,
  sanitizePngName,
  type PanelField,
  type SlicePanelData,
  type SliceSpec,
} from "./render-panel";

const FORMAT_LABELS: Record<string, string> = {
  grib1: "GRIB Edition 1",
  grib2: "GRIB Edition 2",
  netcdf: "NetCDF",
  unknown: "Unknown",
};

/** Every message in a file, in file order, as the API states each one (#574).
 *
 *  One `message(i)` call per message, the way the browser package lists a file,
 *  rather than a list call of the addon's own: the two hosts answer with the
 *  same `MessageInfo`. Built on each call, so a caller that needs it repeatedly
 *  should hold the result. */
export function listMessages(handle: Grib1Handle | Grib2Handle): MessageInfo[] {
  return Array.from({ length: handle.count() }, (_, i) => handle.message(i));
}

/** Narrow a handle to {@link Grib1Handle} by the `setP1` method only GRIB1
 *  exposes — a real type guard so callers don't need an `as` assertion. */
function isGrib1Handle(handle: Grib1Handle | Grib2Handle): handle is Grib1Handle {
  return "setP1" in handle;
}

/** Sanitize a decoded (untrusted) string for use in a plain-text panel title:
 *  drop control characters and cap the length so it can't garble the tab. */
function sanitizeTitlePart(s: string | undefined | null): string {
  if (!s) {
    return "";
  }
  return s.replace(/[\u0000-\u001F\u007F]/g, "").slice(0, 64);
}

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

interface RenderVariableMessage {
  type: "renderVariable";
  variableIndex: number;
}

interface ExportCsvMessage {
  type: "exportCsv";
  messageIndex: number;
}

type WebviewMessage =
  | EditP1Message
  | ReadyMessage
  | DecodeGridMessage
  | RenderVariableMessage
  | ExportCsvMessage;

/** How a PNG export ended. The panel shows "Exporting PNG…" the moment it
 *  hands the image over, and only this side knows what happened next, so the
 *  outcome travels back as an `exportPngDone` message. */
export type PngExportOutcome =
  | { status: "saved"; path: string }
  | { status: "cancelled" }
  | { status: "failed"; reason: string };

/** What the slice panel needs besides the handle itself (#659).
 *
 * A NetCDF panel is keyed by a document and a Zarr panel by a store path, and
 * neither fact belongs inside the panel. What it actually needs is: how to
 * re-fetch the handle (one can be disposed while the panel lives), where an
 * export should start, what to caption the geometry, and what to say when the
 * handle has gone.
 */
interface SlicePanelSubject {
  handle(): SlicePanelHandle | undefined;
  gone: string;
  exportDir: vscode.Uri;
  /** The container's name, which the caption opens with: `"NetCDF"`, `"Zarr"`. */
  container: string;
  /** What the container is, for a message about it: `"file"`, `"store"`. */
  noun: string;
}

export class FieldglassEditorProvider
  implements vscode.CustomEditorProvider<FieldglassDocument>
{
  public static readonly viewType = "fieldglass.viewer";
  public static readonly viewTypeAny = "fieldglass.viewer.any";

  public static register(context: vscode.ExtensionContext): {
    provider: FieldglassEditorProvider;
    disposables: vscode.Disposable[];
  } {
    useColorTableStore(context.globalState);
    const provider = new FieldglassEditorProvider();
    const opts = { supportsMultipleEditorsPerDocument: true };
    return {
      provider,
      disposables: [
        vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewType, provider, opts),
        vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewTypeAny, provider, opts),
        provider._onDidChangeCustomDocument,
        // The one command this extension contributes (#659). A Zarr store is a
        // *directory*, and `registerCustomEditorProvider` binds to file patterns
        // — so there is no editor path a folder can arrive by, and the entry
        // point has to be a command.
        vscode.commands.registerCommand(FieldglassEditorProvider.openStoreCommand, () =>
          provider.promptOpenZarrStore(),
        ),
        // Import a colour palette table (#236). A command for the same reason
        // as the one above: it acts on no document.
        vscode.commands.registerCommand(IMPORT_COLOR_TABLE_COMMAND, () =>
          provider.promptImportColorTable(),
        ),
      ],
    };
  }

  /** The command id, in one place so `package.json` and the registration cannot
   *  disagree — `extension.test.ts` asserts they match. */
  public static readonly openStoreCommand = "fieldglass.openZarrStore";

  /** Ask for a folder, and open it if it is a Zarr store.
   *
   * Detection is by the presence of a root metadata document, **not** by the
   * `.zarr` suffix, which is a convention and not a guarantee. A folder that is
   * not a store is reported as such rather than opening as an empty editor,
   * which is the acceptance criterion behind this whole entry point.
   */
  public async promptOpenZarrStore(): Promise<void> {
    const native = loadNative();
    if (!native) {
      void vscode.window.showErrorMessage(
        `Fieldglass: native module ${nativeBinaryName()} not loaded.`,
      );
      return;
    }
    const picked = await vscode.window.showOpenDialog({
      canSelectFiles: false,
      canSelectFolders: true,
      canSelectMany: false,
      openLabel: "Open Zarr Store",
      title: "Select a Zarr store directory",
    });
    const storeUri = picked?.[0];
    if (!storeUri) return;
    this.openZarrStore(storeUri);
  }

  /** Open a store the caller has already chosen.
   *
   * Separate from the picker so a test can drive it without a modal dialog, which
   * `@vscode/test-electron` cannot dismiss.
   */
  public openZarrStore(storeUri: vscode.Uri): void {
    const native = loadNative();
    if (!native) return;
    if (!native.ZarrHandle.isStore(storeUri.fsPath)) {
      void vscode.window.showErrorMessage(
        `Fieldglass: ${storeUri.fsPath} is not a Zarr store — it holds no ` +
          "zarr.json, .zmetadata, .zgroup or .zarray. A store is a directory of " +
          "metadata documents and chunks; the .zarr suffix is a convention, not " +
          "what this checks.",
      );
      return;
    }
    let handle: ZarrHandle;
    try {
      handle = native.ZarrHandle.fromDirectory(storeUri.fsPath);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: could not open ${storeUri.fsPath}: ${err instanceof Error ? err.message : err}`,
      );
      return;
    }
    const variables = handle.variables();
    if (variables.length === 0) {
      // A store that opened but holds nothing drawable. `leftOut` is why, and
      // saying so beats a panel with an empty picker.
      const why = handle
        .leftOut()
        .map((l) => `${l.name} (${l.reason})`)
        .join("; ");
      void vscode.window.showWarningMessage(
        why
          ? `Fieldglass: that store holds no variable this build can draw. Left out: ${why}`
          : "Fieldglass: that store holds no variable this build can draw.",
      );
      return;
    }
    this._zarrHandlesByStore.set(storeUri.toString(), handle);
    this.openZarrRenderPanel(storeUri, variables[0].variableIndex);
  }

  /** Ask for a `.cpt` file, import it, and offer it in every open render panel.
   *
   * The file dialog is the only part a test cannot drive, so the rest is
   * {@link importColorTable}. */
  public async promptImportColorTable(): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
      openLabel: "Import Color Table",
      title: "Select a GMT color palette table",
      filters: { "Color palette tables": ["cpt"], "All files": ["*"] },
    });
    const uri = picked?.[0];
    if (uri) await this.importColorTable(uri);
  }

  /** Import the colour table at `uri`, then rebuild every open render panel so
   *  its picker offers it. Resolves to whether the file was imported. */
  public async importColorTable(uri: vscode.Uri): Promise<boolean> {
    const entry = await importColorTableFile(uri);
    if (!entry) return false;
    for (const rebuild of this._renderPanelBuilders.values()) rebuild();
    return true;
  }

  /** How to rebuild each open render panel's HTML, so an import reaches a panel
   *  that is already open.
   *
   * A rebuild rather than a message to the webview. The panels are created
   * with `retainContextWhenHidden: false`, so a hidden one is reloaded from its
   * HTML when it is shown again, and a message would only reach the visible
   * ones; the HTML is what every one of them reloads from. The script restores
   * its selections from `vscode.setState` on load, so a rebuild repaints what
   * was on screen. */
  private readonly _renderPanelBuilders = new Map<vscode.WebviewPanel, () => void>();

  /** Build `panel`'s HTML now, and again whenever the colormaps change. */
  private trackRenderPanel(panel: vscode.WebviewPanel, build: () => void): void {
    build();
    this._renderPanelBuilders.set(panel, build);
    panel.onDidDispose(() => this._renderPanelBuilders.delete(panel));
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

  // NetCDF reader handles per document, parallel to `_handlesByDoc` (the
  // NetCDF surface differs — `variables()` / `renderSlice()` rather than
  // `message(i)` / `renderGrid()`). Built lazily when a NetCDF file is opened
  // and dropped with the last panel (see `trackPanel`).
  private readonly _netcdfHandlesByDoc = new Map<string, NetcdfHandle>();
  /** Zarr store handles, keyed by the store directory's URI (#659).
   *
   * Not by document, because a store has none: a custom editor cannot bind to a
   * directory, so a store arrives from the "Open Zarr Store…" command. */
  private readonly _zarrHandlesByStore = new Map<string, ZarrHandle>();

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
    // The whole buffer, not `header`: an HDF5 file may put its signature after
    // a userblock, up to 16 KiB in (#936), and detection reads no further than
    // that. The buffer is passed without a copy.
    const format = native ? native.detectBytes(document.bytes) : "unknown";

    const handle = native ? this.openOrReuseHandle(document, format) : undefined;
    const messages = handle ? listMessages(handle) : undefined;
    let dataset: DatasetMeta | undefined;
    let netcdfVariables: NetcdfVariableMeta[] | undefined;
    if (native && format === "netcdf") {
      // One reader for the whole open. The handle is retained for rendering
      // anyway, and it can answer for the metadata too — calling `openNetcdf`
      // as well built a second reader over the same bytes, parsing and copying
      // the file twice for a value we already had (#411).
      const ncHandle = this.openOrReuseNetcdfHandle(document);
      try {
        dataset = ncHandle?.metadata();
      } catch (err) {
        console.error("[Fieldglass] NetcdfHandle.metadata failed:", err);
        // Leave `dataset` undefined; the renderer will fall back to the
        // "no messages found" status string with the format badge intact.
      }
      // The renderable-variable list drives the "Render" affordances in the
      // metadata view. A backing without a render path yet (HDF5, #169) returns
      // an empty list, so the dump shows without render buttons.
      try {
        netcdfVariables = ncHandle?.variables();
      } catch (err) {
        console.error("[Fieldglass] NetcdfHandle.variables failed:", err);
      }
    }
    const headerBytes = format === "unknown" ? header : undefined;
    // Editing wiring (set_p1, undo/redo, save, webview script + input) is kept
    // intact for when general PDS field editing lands, but disabled at the
    // entry point so users see a coherent read-only viewer instead of a
    // single editable column.
    const editable = false;

    // Scripts must be enabled so the webview can request and paint a 2-D
    // render of a message's decoded grid. The CSP set in renderHtml is the
    // security boundary — see the comment there for the policy itself.
    // `localResourceRoots: []` makes the no-external-resources boundary
    // explicit: the CSP (`default-src 'none'`) already blocks loads, and this
    // ensures nothing can be served from disk even if the CSP is later relaxed.
    panel.webview.options = { enableScripts: true, localResourceRoots: [] };
    panel.webview.html = renderHtml(
      panel.webview,
      format,
      document.uri.fsPath,
      messages,
      dataset,
      headerBytes,
      editable,
      netcdfVariables
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
      case "exportCsv":
        if (!isNonNegativeInt(msg.messageIndex)) return;
        void this.handleExportCsv(document, msg.messageIndex);
        return;
      case "renderVariable":
        if (!isNonNegativeInt(msg.variableIndex)) return;
        this.openNetcdfRenderPanel(document, msg.variableIndex);
        panel.webview.postMessage({ type: "renderOpened", variableIndex: msg.variableIndex });
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
    // messageIndex originates from a webview-controlled message, so it is
    // bounds-checked against the count before the handle is asked for it.
    const meta = messageIndex < handle.count() ? handle.message(messageIndex) : undefined;
    if (!meta) {
      panel.webview.postMessage({
        type: "gridError",
        messageIndex,
        error: `message ${messageIndex} out of range`,
      });
      return;
    }
    if (!messageIsRenderable(meta)) {
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

  /** Export one GRIB message's decoded field to a CSV file the user picks.
   *  Asks for the layout (long `lat,lon,value` or a 2-D matrix), guards a
   *  very large export behind a confirm, then writes the CSV to disk. (The
   *  whole field is serialized to a `Buffer` in one pass — not streamed.) */
  private async handleExportCsv(
    document: FieldglassDocument,
    messageIndex: number
  ): Promise<void> {
    const handle = this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      void vscode.window.showErrorMessage(
        "Fieldglass: CSV export is available for GRIB files only."
      );
      return;
    }

    const format = await this.pickCsvFormat();
    if (!format) return;

    let csv: Buffer;
    try {
      csv = handle.exportCsv(messageIndex, format);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: CSV export failed: ${err instanceof Error ? err.message : err}`
      );
      return;
    }

    await this.saveCsv(csv);
  }

  /** Ask for a CSV layout (long / matrix). Returns the chosen format, or
   *  `undefined` if the user cancelled. Shared by the GRIB and NetCDF export
   *  commands. */
  private async pickCsvFormat(): Promise<string | undefined> {
    const pick = await vscode.window.showQuickPick(
      [
        { label: "Long — one lat,lon,value row per grid point", format: "long" },
        { label: "Matrix — a 2-D grid of values", format: "matrix" },
      ],
      { placeHolder: "CSV layout" }
    );
    return pick?.format;
  }

  /** Confirm a large export, prompt for a destination, and write the CSV.
   *  Shared by the GRIB and NetCDF export commands. */
  private async saveCsv(csv: Buffer): Promise<void> {
    // Guard a large export: writing tens of megabytes is slow and easy to do
    // by accident on a big grid, so confirm above ~50 MB. `csv` already holds
    // the UTF-8 bytes (from napi), so this is the exact write size — no scan or
    // re-encode of a JS string (#341).
    const bytes = csv.length;
    const SIZE_LIMIT = 50 * 1024 * 1024;
    if (bytes > SIZE_LIMIT) {
      const proceed = await vscode.window.showWarningMessage(
        `The exported CSV is about ${Math.round(bytes / (1024 * 1024))} MB. Export anyway?`,
        { modal: true },
        "Export"
      );
      if (proceed !== "Export") return;
    }

    const dest = await vscode.window.showSaveDialog({
      filters: { "CSV files": ["csv"] },
      saveLabel: "Export CSV",
    });
    if (!dest) return;

    try {
      await vscode.workspace.fs.writeFile(dest, csv);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: could not write CSV: ${err instanceof Error ? err.message : err}`
      );
      return;
    }
    void vscode.window.showInformationMessage(
      `Fieldglass: exported CSV to ${dest.fsPath}`
    );
  }

  /** Export the currently-viewed NetCDF slice as CSV (from the render panel's
   *  "Export CSV…" button). `spec` is the live slice the panel drives. */
  public async handleExportSliceCsv(
    subject: SlicePanelSubject,
    spec: { variableIndex: number; yDim: number; xDim: number; sliceIndices: number[] }
  ): Promise<void> {
    const handle = subject.handle();
    if (!handle) {
      void vscode.window.showErrorMessage(
        "Fieldglass: CSV export is available only for an open slice."
      );
      return;
    }

    const format = await this.pickCsvFormat();
    if (!format) return;

    let csv: Buffer;
    try {
      csv = handle.exportCsv(
        spec.variableIndex,
        spec.yDim,
        spec.xDim,
        spec.sliceIndices,
        format
      );
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: CSV export failed: ${err instanceof Error ? err.message : err}`
      );
      return;
    }

    await this.saveCsv(csv);
  }

  /** Save a PNG the render panel composited (#243). The webview sends a
   *  `data:image/png;base64,…` URL and a suggested name; we validate both,
   *  decode the image, and write it where the user picks.
   *
   *  Returns how it ended so the caller can tell the panel. The panel sets
   *  "Exporting PNG…" when it posts the image and has no other way to learn
   *  the outcome — the save dialog and the result notification both live on
   *  this side — so without a reply that status sits there for good, reading
   *  as a still-running export even after a save or a cancel. */
  public async handleExportPng(
    // The directory a "Save as…" dialog starts in. A document panel passes its
    // file's parent; a Zarr panel passes the store, because a store *is* a
    // directory and that is where a user expects an export to land.
    exportDir: vscode.Uri,
    msg: { dataUrl?: unknown; defaultName?: unknown }
  ): Promise<PngExportOutcome> {
    const prefix = "data:image/png;base64,";
    if (typeof msg.dataUrl !== "string" || !msg.dataUrl.startsWith(prefix)) {
      const reason = "PNG export produced no image (render the field first).";
      void vscode.window.showErrorMessage(`Fieldglass: ${reason}`);
      return { status: "failed", reason };
    }
    let buffer: Buffer;
    try {
      buffer = Buffer.from(msg.dataUrl.slice(prefix.length), "base64");
    } catch {
      const reason = "could not decode the exported image.";
      void vscode.window.showErrorMessage(`Fieldglass: ${reason}`);
      return { status: "failed", reason };
    }
    const name = sanitizePngName(
      typeof msg.defaultName === "string" ? msg.defaultName : "render.png"
    );
    const dest = await vscode.window.showSaveDialog({
      defaultUri: vscode.Uri.joinPath(exportDir, name),
      filters: { "PNG image": ["png"] },
      saveLabel: "Export PNG",
    });
    if (!dest) return { status: "cancelled" };
    try {
      await vscode.workspace.fs.writeFile(dest, buffer);
    } catch (err) {
      const reason = `could not write PNG: ${err instanceof Error ? err.message : err}`;
      void vscode.window.showErrorMessage(`Fieldglass: ${reason}`);
      return { status: "failed", reason };
    }
    void vscode.window.showInformationMessage(`Fieldglass: exported PNG to ${dest.fsPath}`);
    return { status: "saved", path: dest.fsPath };
  }

  /**
   * Pop a separate webview tab beside the table view that paints the
   * decoded grid at full resolution. Each render gets its own tab so
   * users can compare messages side-by-side.
   *
   * The panel script never decodes the values itself — every paint runs
   * via `handle.renderGrid(meta.index, options)` on the provider
   * side and ships a paint-ready RGBA Buffer over postMessage. Picker
   * changes (projection / resampling / range / flip-y) flow back as
   * `rerenderRequest` and trigger a fresh `renderGrid` call.
   */
  public openRenderPanel(
    document: FieldglassDocument,
    meta: MessageInfo,
  ): void {
    // The abbreviation comes from a decoded (untrusted) file. VS Code renders
    // panel titles as plain text, so there's no XSS, but strip control
    // characters and cap the length so a hostile file can't garble the tab.
    const abbr = sanitizeTitlePart(meta.abbreviation);
    const title = `Render: msg ${meta.index}` + (abbr ? ` — ${abbr}` : "");
    const panel = vscode.window.createWebviewPanel(
      "fieldglass.render",
      title,
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, retainContextWhenHidden: false, localResourceRoots: [] }
    );
    // Every message in the file is a candidate "field B" for a difference map
    // (#239); the picker only appears when there are at least two.
    const fileHandle = this._handlesByDoc.get(document.uri.toString());
    const compareFields = fileHandle
      ? listMessages(fileHandle).map((m) => ({ index: m.index, label: gribFieldLabel(m) }))
      : undefined;

    this.trackRenderPanel(panel, () => {
      panel.webview.html = renderImagePanelHtml(
        panel.webview,
        meta,
        describeProjection(meta),
        colormapRegistry(),
        combineOpRegistry(),
        undefined,
        compareFields,
      );
    });

    const paint = (options: RenderOptions, compare?: GribCompare) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: meta.index,
          error: "reader handle was disposed",
        });
        return;
      }
      try {
        // A difference map combines this message (A) with the chosen message
        // (B); otherwise a plain single-field render. Both return the same
        // paint-ready RGBA, so the panel displays them identically.
        const rendered = compare
          ? docHandle.renderGridCombined(
              meta.index,
              compare.messageIndexB,
              compare.op,
              options,
            )
          : docHandle.renderGrid(meta.index, options);
        panel.webview.postMessage(
          buildGridReadyMessage(rendered, meta, options),
        );
      } catch (err) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: meta.index,
          error: `render failed: ${err}`,
        });
      }
    };

    // Project the requested overlay layers (coastline / graticule) onto the
    // raster for the current options and post the pixel-space runs back. The
    // forward projection runs in Rust (`projectOverlay`); this never decodes
    // values, so toggling the overlay never re-decodes the grid. `seq` is
    // echoed verbatim so the webview can drop a reply for a superseded request.
    const projectOverlay = (req: OverlayRequest) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const layers: OverlayLayerPayload[] = [];
      const project = (name: string, geom: OverlayGeometry) => {
        const projected = docHandle.projectOverlay(
          meta.index,
          options,
          geom.latlon,
          geom.ringLengths,
        );
        layers.push({ name, xy: projected.xy, segLengths: projected.segLengths });
      };
      try {
        // Vector layers first, then the graticule on top of them — the fixed
        // draw order the webview also strokes in.
        for (const layer of REQUESTED_VECTOR_LAYERS) {
          if (req[layer.flag]) project(layer.name, loadVectorLayer(layer.asset));
        }
        if (req.graticule) project("graticule", buildGraticule(Number(req.graticuleSpacing)));
        panel.webview.postMessage({
          type: "overlayReady",
          messageIndex: meta.index,
          seq: req.seq,
          layers,
        });
      } catch (err) {
        // Correlate the failure with the in-flight request (`seq`) so the
        // panel can resolve it and re-arm the overlay, rather than dead-ending
        // with an advanced `overlaySeq`/`lastOverlayKey` and a blank overlay.
        panel.webview.postMessage({
          type: "overlayError",
          messageIndex: meta.index,
          seq: req.seq,
          error: `overlay projection failed: ${err}`,
        });
      }
    };

    // Project the field's contour isolines onto the current raster (#238).
    // Unlike the geographic overlays this decodes values, so it rides the same
    // handle as `paint`; `seq` lets the panel drop a superseded reply.
    const projectContours = (req: ContourRequest) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const interval = resolveInterval(req.interval);
      // A difference-map contour traces the combined field, not field A (#329).
      const compare = resolveGribCompare(req);
      try {
        const c = compare
          ? docHandle.projectContoursCombined(
              meta.index, compare.messageIndexB, compare.op, options, interval)
          : docHandle.projectContours(meta.index, options, interval);
        panel.webview.postMessage({
          type: "contourReady",
          messageIndex: meta.index,
          seq: req.seq,
          xy: c.xy,
          segLengths: c.segLengths,
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "contourError",
          messageIndex: meta.index,
          seq: req.seq,
          error: `${err}`.replace(/^Error:\s*/, ""),
        });
      }
    };

    // The arrows of a u/v pair, projected onto the current raster (#241). The
    // panel's own message is the u component; the picker chooses v. A refusal
    // travels with the reply — a grid with no forward geolocation cannot place
    // an arrow, and a v whose cells do not line up with u's is refused as a
    // combine would be (#793) — so the panel can say why rather than drawing
    // nothing.
    const projectVectors = (req: VectorRequest) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) return;
      if (!isNonNegativeInt(req.messageIndexV)) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const spacing = isNonNegativeInt(req.spacing) && req.spacing > 0 ? req.spacing : undefined;
      try {
        const arrows = docHandle.projectVectors(
          meta.index,
          req.messageIndexV,
          options,
          spacing,
          req.gridRelative === true,
        );
        panel.webview.postMessage({
          type: "vectorResult",
          seq: req.seq,
          xy: arrows.xy,
          segLengths: arrows.segLengths,
          referenceSpeed: arrows.referenceSpeed,
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "vectorResult",
          seq: req.seq,
          error: `${err}`.replace(/^Error:\s*/, ""),
        });
      }
    };

    // Read the field under a clicked pixel and post the readout back (#172).
    // The field's zonal mean (#240). A refusal travels with the reply, because
    // the reason — a rotated or projected grid has no latitude circles to average
    // along — is what the panel should say instead of an empty plot.
    const zonal = () => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) return;
      try {
        const result = docHandle.zonalMean(meta.index);
        panel.webview.postMessage({ type: "zonalResult", messageIndex: meta.index, result, error: null });
      } catch (err) {
        panel.webview.postMessage({
          type: "zonalResult",
          messageIndex: meta.index,
          result: null,
          error: `${err}`.replace(/^Error:\s*/, ""),
        });
      }
    };

    const probe = (req: ProbeRequest) => {
      const docHandle = this._handlesByDoc.get(document.uri.toString());
      if (!docHandle) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const px = Math.max(0, Math.floor(req.px));
      const py = Math.max(0, Math.floor(req.py));
      // A click on a difference map reads the combined field, not field A (#329).
      const compare = resolveGribCompare(req);
      try {
        const result = compare
          ? docHandle.probeCombined(
              meta.index, compare.messageIndexB, compare.op, options, px, py)
          : docHandle.probe(meta.index, options, px, py);
        panel.webview.postMessage({ type: "probeResult", messageIndex: meta.index, result });
      } catch {
        // A probe never blocks the user; swallow and report nothing.
        panel.webview.postMessage({ type: "probeResult", messageIndex: meta.index, result: null });
      }
    };

    // Respond for the panel's lifetime: webview is created with
    // retainContextWhenHidden=false so VS Code tears down the DOM/JS
    // context when the tab is hidden; each remount posts a fresh `ready`
    // carrying the webview's (state-restored) selections, so the repaint
    // shows what the user had rather than the defaults.
    const sub = panel.webview.onDidReceiveMessage(
      (
        m:
          | ({ type?: string } & Partial<RenderOptions>)
          | OverlayRequest
          | ContourRequest
          | ProbeRequest
          | { type?: string; dataUrl?: unknown; defaultName?: unknown },
      ) => {
        if (!m || typeof m.type !== "string") return;
        if (m.type === "ready") {
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), resolveGribCompare(m));
          return;
        }
        if (m.type === "rerenderRequest") {
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), resolveGribCompare(m));
          return;
        }
        if (m.type === "overlayRequest") {
          projectOverlay(m as OverlayRequest);
          return;
        }
        if (m.type === "contourRequest") {
          projectContours(m as ContourRequest);
          return;
        }
        if (m.type === "vectorRequest") {
          projectVectors(m as VectorRequest);
          return;
        }
        if (m.type === "probeRequest") {
          probe(m as ProbeRequest);
          return;
        }
        if (m.type === "zonalRequest") {
          zonal();
          return;
        }
        if (m.type === "exportPng") {
          // The Export PNG button lives in the panel HTML both render panels
          // share, so a GRIB render offers it exactly like a NetCDF slice —
          // but only the NetCDF handler listened for the message, so on a GRIB
          // field the composited image went nowhere: no save dialog, and no
          // reply to resolve the panel's "Exporting PNG…" status.
          void this.handleExportPng(
            vscode.Uri.joinPath(document.uri, ".."),
            m as { dataUrl?: unknown; defaultName?: unknown },
          ).then((outcome) => {
            panel.webview.postMessage({ type: "exportPngDone", outcome });
          });
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
    if (!isGrib1Handle(handle)) {
      vscode.window.showErrorMessage(
        "Fieldglass: setP1 only applies to GRIB1 documents",
      );
      return;
    }
    let newBytes: Uint8Array;
    try {
      newBytes = handle.setP1(messageIndex, value);
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
          this._netcdfHandlesByDoc.delete(key);
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
      ? listMessages(cached)
      : this.reparseAndCache(document);
    if (!messages) return;
    panel.webview.postMessage({ type: "update", messages });
  }

  private reparseAndCache(document: FieldglassDocument): MessageInfo[] | undefined {
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle = native.Grib1Handle.fromBytes(document.bytes);
      this._handlesByDoc.set(document.uri.toString(), handle);
      return listMessages(handle);
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

  /** Get-or-build the cached NetCDF reader handle for a document. */
  private openOrReuseNetcdfHandle(
    document: FieldglassDocument,
  ): NetcdfHandle | undefined {
    const key = document.uri.toString();
    const cached = this._netcdfHandlesByDoc.get(key);
    if (cached) return cached;
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle = native.NetcdfHandle.fromBytes(document.bytes);
      this._netcdfHandlesByDoc.set(key, handle);
      return handle;
    } catch (err) {
      console.error("[Fieldglass] NetcdfHandle creation failed:", err);
      vscode.window.showErrorMessage(`Fieldglass: failed to parse NetCDF: ${err}`);
      return undefined;
    }
  }

  /**
   * Pop a render tab for one NetCDF variable. Mirrors {@link openRenderPanel}
   * but drives the two-tier slice picker: every paint renders the chosen 2-D
   * plane via `handle.renderSlice(...)`, and the picker (variable / axis / index
   * controls) flows its {@link SliceSpec} back on each `rerenderRequest`.
   */
  /** Open the slice panel for a NetCDF document. */
  private openNetcdfRenderPanel(
    document: FieldglassDocument,
    variableIndex: number,
  ): void {
    const handle = this.openOrReuseNetcdfHandle(document);
    if (!handle) return;
    this.openSliceRenderPanel(
      {
        // Re-fetched rather than captured: a document can close under a panel
        // that is still open, and the handle goes with it.
        handle: () => this._netcdfHandlesByDoc.get(document.uri.toString()),
        gone: "NetCDF handle was disposed",
        exportDir: vscode.Uri.joinPath(document.uri, ".."),
        container: "NetCDF",
        noun: "file",
      },
      handle,
      variableIndex,
    );
  }

  /** Open the slice panel for a Zarr store directory (#659).
   *
   * A store has no `FieldglassDocument` — a custom editor cannot bind to a
   * directory, which is why this arrives from a command instead — so the subject
   * is keyed by the store's path. */
  public openZarrRenderPanel(storeUri: vscode.Uri, variableIndex: number): void {
    const key = storeUri.toString();
    const handle = this._zarrHandlesByStore.get(key);
    if (!handle) return;
    this.openSliceRenderPanel(
      {
        handle: () => this._zarrHandlesByStore.get(key),
        gone: "Zarr store handle was disposed",
        // Inside the store, not beside it: a store *is* a directory, so its own
        // folder is where a user expects an export to land.
        exportDir: storeUri,
        container: "Zarr",
        noun: "store",
      },
      handle,
      variableIndex,
    );
  }

  /** The slice panel, over whichever container answered.
   *
   * Both handles satisfy `SlicePanelHandle` with identical signatures, so nothing
   * below this line knows which one it has (#659). What differs is in the
   * `subject`: how to re-fetch the handle, where an export starts, and the
   * container's name.
   */
  private openSliceRenderPanel(
    subject: SlicePanelSubject,
    handle: SlicePanelHandle,
    variableIndex: number,
  ): void {
    let variables = handle.variables();
    let initialVar =
      variables.find((v) => v.variableIndex === variableIndex) ?? variables[0];
    if (!initialVar) return;

    let initial = defaultSliceSpec(initialVar);
    // The slice's own answers — its family, and whether it can be reprojected —
    // asked of the placement the handle renders from (#574), once per variable
    // and axis pair. A slice the handle cannot place at all still opens, on the
    // source view alone. Keyed by handle as well, since a document reopened
    // under the panel brings a new one.
    const grids = new WeakMap<SlicePanelHandle, Map<string, SliceGrid | null>>();
    const gridOf = (h: SlicePanelHandle, spec: SliceSpec): SliceGrid | null => {
      let known = grids.get(h);
      if (!known) grids.set(h, (known = new Map()));
      const key = `${spec.variableIndex}:${spec.yDim}:${spec.xDim}`;
      if (!known.has(key)) {
        let grid: SliceGrid | null = null;
        try {
          grid = h.sliceGrid(spec.variableIndex, spec.yDim, spec.xDim);
        } catch (err) {
          console.error("[Fieldglass] sliceGrid failed:", err);
        }
        known.set(key, grid);
      }
      return known.get(key) ?? null;
    };
    const openedGrid = gridOf(handle, initial);
    // What a rebuilt panel is written from. The slice drawn last rather than the
    // one the panel opened on: a rebuild restores the picker's saved selection,
    // and a map target the opening slice could not take would not survive it.
    let shown = { meta: sliceField(initialVar, openedGrid), grid: openedGrid };
    const title = `Render: ${sanitizeTitlePart(initialVar.name) || "variable"}`;
    const panel = vscode.window.createWebviewPanel(
      "fieldglass.render",
      title,
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, retainContextWhenHidden: false, localResourceRoots: [] },
    );
    let slice: SlicePanelData = { variables, initial };
    const write = () => {
      panel.webview.html = renderImagePanelHtml(
        panel.webview,
        shown.meta,
        sliceCaption(subject.container, shown.grid),
        colormapRegistry(),
        combineOpRegistry(),
        slice,
      );
    };
    this.trackRenderPanel(panel, write);

    // The handle the page's variable list was read from. A document closed and
    // opened again brings a new handle, on a file that may have changed: its
    // variables can be others, or the same ones numbered differently, and every
    // request the page makes names a variable by its number in the old list.
    // So the first request after the handle changes is not served. The panel is
    // written again from the new file, and the page restores its selection by
    // the variable's name (#839).
    let pageHandle = handle;
    // Said in place of the first render after that, when the variable the page
    // was showing is no longer in the file.
    let refusal: string | null = null;
    // Set from the rewrite until the new page says it is ready. Anything the old
    // page sent before it was replaced is still on its way, and names variables
    // by the old numbers, so it is dropped rather than read from the new file.
    let awaitingReady = false;
    const adopt = (current: SlicePanelHandle, asked: SliceSpec | undefined): void => {
      const name = (variables.find((v) => v.variableIndex === asked?.variableIndex) ?? initialVar).name;
      const next = current.variables();
      const same = next.find((v) => v.name === name);
      const opened = same ?? next[0];
      if (!opened) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: asked?.variableIndex ?? initial.variableIndex,
          error: `The ${subject.container} ${subject.noun} no longer has a variable to draw.`,
          sliceGrid: sliceAnswer(null),
        } satisfies GridErrorMessage);
        return;
      }
      pageHandle = current;
      variables = next;
      initialVar = opened;
      initial = defaultSliceSpec(opened);
      slice = { variables, initial };
      refusal = same ? null : `${name} is no longer in the ${subject.noun}, so nothing was drawn. Pick a variable to draw.`;
      const grid = gridOf(current, initial);
      shown = { meta: sliceField(opened, grid), grid };
      awaitingReady = true;
      write();
    };

    // The variable a render is actually drawing. The picker can move off the
    // one the panel opened on, and the heading and probe units have to follow
    // it; labelling every render with `initialVar` reported the opening
    // variable's units against whatever was on screen.
    const renderedVar = (spec: SliceSpec): NetcdfVariableMeta =>
      variables.find((v) => v.variableIndex === spec.variableIndex) ?? initialVar;

    const paint = (requested: RenderOptions, spec: SliceSpec, compare?: NetcdfCompare) => {
      const docHandle = subject.handle();
      if (!docHandle) {
        // With no handle nothing is placed, so the picker and the Overlay row
        // stop offering what only a placed slice takes (#839).
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: spec.variableIndex,
          error: subject.gone,
          sliceGrid: sliceAnswer(null),
          handleGone: true,
        } satisfies GridErrorMessage);
        return;
      }
      // The picker offers the map targets for the slice it last drew (#822). A
      // request for one that this slice cannot take — the picker has just moved
      // onto it — is drawn in the source view, and the answer goes back with
      // the render so the picker follows.
      const grid = gridOf(docHandle, spec);
      const options: RenderOptions =
        grid?.reprojectable || requested.projection === "source"
          ? requested
          : { ...requested, projection: "source" };
      try {
        // A difference map combines this slice (A) with a second slice (B) —
        // the same or another variable at its own indices; otherwise a plain
        // single-slice render. Both return the same paint-ready RGBA.
        const rendered = compare
          ? docHandle.renderSliceCombined(
              spec.variableIndex,
              spec.yDim,
              spec.xDim,
              spec.sliceIndices,
              compare.variableIndexB,
              compare.sliceIndicesB,
              compare.op,
              options,
            )
          : docHandle.renderSlice(
              spec.variableIndex,
              spec.yDim,
              spec.xDim,
              spec.sliceIndices,
              options,
            );
        shown = { meta: sliceField(renderedVar(spec), grid), grid };
        panel.webview.postMessage({
          ...buildGridReadyMessage(rendered, sliceTitle(renderedVar(spec), spec.variableIndex), options),
          sliceGrid: sliceAnswer(grid),
        } satisfies GridReadyMessage);
      } catch (err) {
        // The answers go with the error too. The picker has moved onto this
        // slice, and a failed render must not leave it, or the Overlay row, on
        // the previous slice's answer (#839).
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: spec.variableIndex,
          error: `render failed: ${err}`,
          sliceGrid: sliceAnswer(grid),
        } satisfies GridErrorMessage);
      }
    };

    const projectOverlay = (req: OverlayRequest & { slice?: SliceSpec }) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const layers: OverlayLayerPayload[] = [];
      const project = (name: string, geom: OverlayGeometry) => {
        const projected = docHandle.projectOverlay(
          spec.variableIndex,
          spec.yDim,
          spec.xDim,
          options,
          geom.latlon,
          geom.ringLengths,
        );
        layers.push({ name, xy: projected.xy, segLengths: projected.segLengths });
      };
      try {
        // Vector layers first, then the graticule on top of them — the fixed
        // draw order the webview also strokes in.
        for (const layer of REQUESTED_VECTOR_LAYERS) {
          if (req[layer.flag]) project(layer.name, loadVectorLayer(layer.asset));
        }
        if (req.graticule) project("graticule", buildGraticule(Number(req.graticuleSpacing)));
        panel.webview.postMessage({
          type: "overlayReady",
          messageIndex: spec.variableIndex,
          seq: req.seq,
          layers,
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "overlayError",
          messageIndex: spec.variableIndex,
          seq: req.seq,
          error: `overlay projection failed: ${err}`,
        });
      }
    };

    // Contour isolines for the current slice, projected onto its raster (#238).
    const projectContours = (req: ContourRequest) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const interval = resolveInterval(req.interval);
      // A difference-map contour traces the combined field, not slice A (#329).
      const compare = resolveNetcdfCompare(req);
      try {
        const c = compare
          ? docHandle.projectContoursSliceCombined(
              spec.variableIndex, spec.yDim, spec.xDim, spec.sliceIndices,
              compare.variableIndexB, compare.sliceIndicesB, compare.op, options, interval)
          : docHandle.projectContours(
              spec.variableIndex, spec.yDim, spec.xDim, spec.sliceIndices, options, interval);
        panel.webview.postMessage({
          type: "contourReady",
          messageIndex: spec.variableIndex,
          seq: req.seq,
          xy: c.xy,
          segLengths: c.segLengths,
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "contourError",
          messageIndex: spec.variableIndex,
          seq: req.seq,
          error: `${err}`.replace(/^Error:\s*/, ""),
        });
      }
    };

    // The slice's zonal mean (#240), with the refusal passed through as on the
    // GRIB panel.
    const zonal = (req: { slice?: SliceSpec }) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      try {
        const result = docHandle.zonalMean(spec.variableIndex, spec.yDim, spec.xDim, spec.sliceIndices);
        panel.webview.postMessage({ type: "zonalResult", messageIndex: spec.variableIndex, result, error: null });
      } catch (err) {
        panel.webview.postMessage({
          type: "zonalResult",
          messageIndex: spec.variableIndex,
          result: null,
          error: `${err}`.replace(/^Error:\s*/, ""),
        });
      }
    };

    // The line through the probed cell, along the axis the panel picked (#172).
    // A failure answers with no line rather than an error toast: the plot is
    // a companion to the readout, and a variable with no axis to read along is
    // an ordinary state, not something the user did wrong.
    const line = (req: LineRequest) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      const indices = lineIndices(spec, req.gridI, req.gridJ);
      if (!indices || !isNonNegativeInt(req.alongDim)) {
        panel.webview.postMessage({ type: "lineResult", messageIndex: spec.variableIndex, result: null });
        return;
      }
      try {
        const result = docHandle.line(spec.variableIndex, req.alongDim, indices);
        panel.webview.postMessage({ type: "lineResult", messageIndex: spec.variableIndex, result });
      } catch {
        panel.webview.postMessage({ type: "lineResult", messageIndex: spec.variableIndex, result: null });
      }
    };

    // The coordinate values a cross-section labels an axis from (#171). Like
    // the line above, a failure answers with no values rather than an error:
    // an axis with no coordinate array is an ordinary state, and the panel
    // falls back to drawing indices.
    const axis = (req: { slice?: SliceSpec; dim?: unknown }) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      const reply = (result: AxisValues | null) =>
        panel.webview.postMessage({ type: "axisResult", dim: req.dim, result });
      if (!isNonNegativeInt(req.dim)) {
        reply(null);
        return;
      }
      try {
        reply(docHandle.axisValues(spec.variableIndex, req.dim));
      } catch {
        reply(null);
      }
    };

    // Point-probe readout for the current slice (#172).
    const probe = (req: ProbeRequest) => {
      const docHandle = subject.handle();
      if (!docHandle) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const px = Math.max(0, Math.floor(req.px));
      const py = Math.max(0, Math.floor(req.py));
      // A click on a difference map reads the combined field, not slice A (#329).
      const compare = resolveNetcdfCompare(req);
      try {
        const result = compare
          ? docHandle.probeSliceCombined(
              spec.variableIndex, spec.yDim, spec.xDim, spec.sliceIndices,
              compare.variableIndexB, compare.sliceIndicesB, compare.op, options, px, py)
          : docHandle.probe(
              spec.variableIndex, spec.yDim, spec.xDim, spec.sliceIndices, options, px, py);
        panel.webview.postMessage({ type: "probeResult", messageIndex: spec.variableIndex, result });
      } catch {
        panel.webview.postMessage({ type: "probeResult", messageIndex: spec.variableIndex, result: null });
      }
    };

    const sub = panel.webview.onDidReceiveMessage(
      (
        m:
          | ({ type?: string; slice?: SliceSpec } & Partial<RenderOptions>)
          | (OverlayRequest & { slice?: SliceSpec })
          | (ContourRequest & { slice?: SliceSpec })
          | (ProbeRequest & { slice?: SliceSpec }),
      ) => {
        if (!m || typeof m.type !== "string") return;
        const current = subject.handle();
        if (m.type !== "exportPng") {
          if (awaitingReady) {
            if (m.type !== "ready") return;
            awaitingReady = false;
          }
          // Checked for the new page's ready as well: the handle can have
          // changed again while that page loaded, and its numbers are then the
          // previous file's.
          if (current && current !== pageHandle) {
            adopt(current, (m as { slice?: SliceSpec }).slice);
            return;
          }
        }
        if (m.type === "ready" && refusal !== null) {
          const spec = (m as { slice?: SliceSpec }).slice ?? initial;
          panel.webview.postMessage({
            type: "gridError",
            messageIndex: spec.variableIndex,
            error: refusal,
            sliceGrid: sliceAnswer(current ? gridOf(current, spec) : null),
            pickVariable: true,
          } satisfies GridErrorMessage);
          refusal = null;
          return;
        }
        // `ready` carries the webview's (state-restored) selections and slice,
        // exactly like a rerenderRequest, so a remount repaints what the user
        // had; a fresh panel sends its defaults.
        if (m.type === "ready" || m.type === "rerenderRequest") {
          const spec = (m as { slice?: SliceSpec }).slice ?? initial;
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), spec, resolveNetcdfCompare(m));
          return;
        }
        if (m.type === "overlayRequest") {
          projectOverlay(m as OverlayRequest & { slice?: SliceSpec });
          return;
        }
        if (m.type === "contourRequest") {
          projectContours(m as ContourRequest & { slice?: SliceSpec });
          return;
        }
        if (m.type === "probeRequest") {
          probe(m as ProbeRequest & { slice?: SliceSpec });
          return;
        }
        if (m.type === "lineRequest") {
          line(m as LineRequest);
          return;
        }
        if (m.type === "axisRequest") {
          axis(m as { slice?: SliceSpec; dim?: unknown });
          return;
        }
        if (m.type === "zonalRequest") {
          zonal(m as { slice?: SliceSpec });
          return;
        }
        if (m.type === "exportSliceCsv") {
          const spec = (m as { slice?: SliceSpec }).slice;
          if (
            spec &&
            isNonNegativeInt(spec.variableIndex) &&
            isNonNegativeInt(spec.yDim) &&
            isNonNegativeInt(spec.xDim) &&
            Array.isArray(spec.sliceIndices) &&
            spec.sliceIndices.every((n) => isNonNegativeInt(n))
          ) {
            void this.handleExportSliceCsv(subject, spec);
          }
        }
        if (m.type === "exportPng") {
          void this.handleExportPng(
            subject.exportDir,
            m as { dataUrl?: unknown; defaultName?: unknown },
          ).then((outcome) => {
            // Close the loop the panel opened with "Exporting PNG…". Without
            // this the status never resolves, so a saved or cancelled export
            // looks identical to one still in flight.
            panel.webview.postMessage({ type: "exportPngDone", outcome });
          });
        }
      },
    );
    panel.onDidDispose(() => sub.dispose());
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
  /** `null` when no cell had a value to take a range from (#871). */
  usedMin: number | null;
  usedMax: number | null;
  /** Equirectangular extent actually rendered, echoed so the panel can
   *  pre-fill the manual-bounds inputs. `null` for source projection. */
  usedLatMin: number | null;
  usedLatMax: number | null;
  usedLonMin: number | null;
  usedLonMax: number | null;
  projectionSummary: string;
  options: RenderOptions;
  /** The panel heading for the field actually drawn, composed by
   *  {@link composeTitleLine}. Sent with every render because a NetCDF panel
   *  switches variables inside one webview; the heading baked into the initial
   *  HTML describes only the variable it opened on. */
  titleLine: string;
  /** Units of the field actually drawn, for the probe readout. Same reason as
   *  {@link GridReadyMessage.titleLine}: frozen units put one variable's unit
   *  against another variable's numbers. */
  parameterUnits: string;
  /** The PNG export's default filename for the field actually drawn. Same
   *  reason as {@link GridReadyMessage.titleLine}: a slice panel's export was
   *  named after the variable it opened on (#822). */
  defaultPngName: string;
  /** The band-limit note for the map actually drawn, composed by
   *  {@link composeTruncationNote} from `RenderedGrid.truncation`, or `null`
   *  when the map carries every wavenumber its file holds. Sent with every
   *  render, null included, because a Compare map takes the label of either
   *  operand (#814): the note baked into the initial HTML is field A's alone. */
  truncationNote: string | null;
  /** A slice panel's answer for the slice actually drawn (#822): its grid
   *  family (`null` when it could not be placed) and whether it can be
   *  reprojected. The projection picker follows it, because the picker can
   *  move onto a variable or axis pair with a different answer. `note` is
   *  {@link reprojectionNote} for it, the text shown beside the picker.
   *  `placed` is {@link offersOverlays} for it: whether the overlays,
   *  contours and arrows apply, which the Overlay row follows (#840). Absent
   *  for a GRIB panel, which draws one field. */
  sliceGrid?: SliceAnswer;
}

/** A slice's answers as the render panel takes them; see
 *  {@link GridReadyMessage.sliceGrid}. */
export interface SliceAnswer {
  label: string | null;
  reprojectable: boolean;
  note: string;
  placed: boolean;
}

/** The answers for a slice from its `SliceGrid`, or for one the handle could
 *  not place (`null`): no family, the source view alone, no overlays. */
export function sliceAnswer(grid: SliceGrid | null): SliceAnswer {
  return {
    label: grid?.label ?? null,
    reprojectable: grid?.reprojectable ?? false,
    note: reprojectionNote(grid?.reprojectable ?? false, grid?.label ?? null),
    placed: offersOverlays(grid?.placement),
  };
}

/** `gridError`: a render the provider could not draw. */
export interface GridErrorMessage {
  type: "gridError";
  messageIndex: number;
  error: string;
  /** A slice panel's answers for the slice it was asked to draw (#839). The
   *  picker and the Overlay row follow the slice the user picked whether or
   *  not it drew, so a failed render does not leave them on the previous
   *  slice's answer. Absent for a GRIB panel. */
  sliceGrid?: SliceAnswer;
  /** The panel has no handle, because the document's editor closed. The
   *  answer is then about the handle, not the slice, so the panel withdraws
   *  the map targets without saving that as the user's choice (#839). */
  handleGone?: boolean;
  /** The variable the panel was showing is gone from a file that changed, and
   *  nothing was drawn: the panel asks for a variable to be picked (#839). */
  pickVariable?: boolean;
}

/** `overlayRequest` posted by the render panel when an overlay layer is
 *  toggled on or the underlying raster changes. Carries the same render
 *  options the image was painted with so the projection matches pixel-for-
 *  pixel, plus which layers to project. */
export interface OverlayRequest {
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
export interface VectorRequest {
  type: "vectorRequest";
  seq?: number;
  messageIndexV?: unknown;
  spacing?: unknown;
  gridRelative?: unknown;
  options?: Partial<RenderOptions>;
}

/** `contourRequest` posted by the render panel when contours are on and the
 *  field, range, projection, or interval changes (#238). Carries the render
 *  options (for the projection + used range) and an optional manual level
 *  interval; a NetCDF panel also carries its slice. */
export interface ContourRequest {
  type: "contourRequest";
  seq?: number;
  options?: Partial<RenderOptions>;
  /** Manual level spacing; omitted / non-positive → automatic nice levels. */
  interval?: number;
  slice?: SliceSpec;
}

/** `probeRequest` posted by the render panel when the user clicks the image to
 *  read the field there (#172). Carries the clicked output-raster pixel, the
 *  render options (for the projection), and the NetCDF slice when applicable. */
export interface ProbeRequest {
  type: "probeRequest";
  px: number;
  py: number;
  options?: Partial<RenderOptions>;
  slice?: SliceSpec;
}

/** `lineRequest` posted by the render panel once a probe has resolved a cell
 *  (#172): plot the variable through that cell along `alongDim`. */
export interface LineRequest {
  type: "lineRequest";
  alongDim: number;
  gridI: number;
  gridJ: number;
  slice?: SliceSpec;
}

/** The indices a line is read at: the slice on screen, with the probed cell
 *  written into its two horizontal positions (#172).
 *
 *  `null` when the request cannot name a cell on this slice — an axis or cell
 *  that is not a non-negative integer, or indices too short to hold the
 *  horizontal axes — rather than a line through some other cell. The entry for
 *  `alongDim` is left as it is: the native call ignores it. */
export function lineIndices(spec: SliceSpec, gridI: unknown, gridJ: unknown): number[] | null {
  if (!isNonNegativeInt(gridI) || !isNonNegativeInt(gridJ)) return null;
  if (spec.yDim >= spec.sliceIndices.length || spec.xDim >= spec.sliceIndices.length) return null;
  const indices = spec.sliceIndices.slice();
  indices[spec.yDim] = gridJ;
  indices[spec.xDim] = gridI;
  return indices;
}

/** Sanitise the webview's contour interval into a positive number or
 *  `undefined` (automatic levels) — never a zero, negative, or non-finite
 *  value that would confuse the native level picker. */
export function resolveInterval(interval: unknown): number | undefined {
  return typeof interval === "number" && Number.isFinite(interval) && interval > 0
    ? interval
    : undefined;
}

/** The vector layers an `OverlayRequest` can ask for, paired with the asset
 *  each one loads. Keyed by the request flag so the projection loop below is a
 *  filter over this table rather than a chain of `if`s — adding a layer means
 *  adding a row, and the webview's stroke styles key off the same names. */
const REQUESTED_VECTOR_LAYERS: ReadonlyArray<{
  flag: keyof Pick<OverlayRequest, "coastlines" | "borders" | "lakes" | "rivers">;
  name: string;
  asset: VectorLayer;
}> = [
  { flag: "coastlines", name: "coastline", asset: "coastline" },
  { flag: "borders", name: "borders", asset: "borders" },
  { flag: "lakes", name: "lakes", asset: "lakes" },
  { flag: "rivers", name: "rivers", asset: "rivers" },
];

/** One projected overlay layer in the `overlayReady` payload — pixel-space
 *  runs ready for the webview to stroke. */
export interface OverlayLayerPayload {
  name: string;
  xy: Float64Array;
  segLengths: Uint32Array;
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
/** The closed set of projection strings the picker can emit and the Rust
 *  side accepts. Pinned by `resolveRerenderOptions` so adding a target to
 *  the picker without listing it here can't silently snap back to `source`. */
const PROJECTIONS: ReadonlyArray<RenderOptions["projection"]> = [
  "source",
  "equirectangular",
  "web_mercator",
  "orthographic",
  "polar_stereographic",
  "mollweide",
  "robinson",
  "equal_earth",
];

/**
 * Resolve a `rerenderRequest` message from the webview into validated
 * `RenderOptions`. Webview-controlled enum strings are clamped to the closed
 * set the Rust side accepts: an unknown/typo'd `projection` or `resampling`
 * silently snaps to its default (`source` / `nearest`) rather than
 * round-tripping a value `ResolvedOptions::parse` would reject with an error
 * popup. `projectionPreset`, the free-form `centerLat`/`centerLon`, and the
 * manual lat/lon extent pass through untouched — native validates them and
 * falls back to its own defaults on a partial/inverted box or unknown preset.
 *
 * Pinned by `render.test.ts`: every projection the picker offers must survive
 * this clamp. The original two-value clamp here (source/equirectangular) was
 * the #71 regression where the new targets and presets silently did nothing.
 */
/** The colormap registry, straight from Rust, followed by any imported colour
 *  tables (#236). Empty when the native binding is unavailable, which leaves
 *  the panel with no colormap picker rather than a picker offering names the
 *  renderer doesn't have. */
function colormapRegistry(): PickerColormap[] {
  const native = loadNative();
  return native ? [...native.colormaps(), ...importedPickerColormaps()] : [];
}

/** The colormap names the Rust side accepts, read once from the registry.
 *  Derived from the registry rather than hardcoded, so adding a colormap in
 *  Rust needs no matching edit here and the clamp below can't fall behind it. */
let knownColormapNames: ReadonlySet<string> | undefined;
function knownColormaps(): ReadonlySet<string> {
  if (!knownColormapNames) {
    const native = loadNative();
    knownColormapNames = new Set(native ? native.colormaps().map((c) => c.name) : []);
  }
  return knownColormapNames;
}

/** The field-combine op vocabulary, straight from Rust (#342). Empty when the
 *  native binding is unavailable, which leaves the Compare row with no
 *  operations rather than offering ops the renderer can't run. */
function combineOpRegistry(): CombineOpInfo[] {
  const native = loadNative();
  return native ? native.combineOps() : [];
}

/** The combine-op tags the Rust side accepts, read once from the registry.
 *  Derived rather than hardcoded, so adding an op in Rust needs no matching
 *  edit here and the picker can't offer a tag validation would reject. */
let knownCombineOpTags: ReadonlySet<string> | undefined;
function knownCombineOps(): ReadonlySet<string> {
  if (!knownCombineOpTags) {
    knownCombineOpTags = new Set(combineOpRegistry().map((o) => o.value));
  }
  return knownCombineOpTags;
}

/** A validated GRIB difference-map request: combine field A (the panel's
 *  message) with message B under `op`. */
export interface GribCompare {
  op: CombineOp;
  messageIndexB: number;
}

/** Validate a webview `compare` rider into a {@link GribCompare}, or `undefined`
 *  for "no comparison" — an absent rider, an unknown op, or a non-integer index.
 *  Same clamp philosophy as {@link resolveRerenderOptions}: a bad value falls
 *  back to a plain single-field render rather than round-tripping an error. The
 *  accepted ops come from {@link knownCombineOps} (the Rust registry), so the
 *  picker and this gate can't disagree. */
export function resolveGribCompare(m: unknown): GribCompare | undefined {
  const c = (m as { compare?: { op?: unknown; messageIndexB?: unknown } })?.compare;
  if (!c || typeof c.op !== "string" || !knownCombineOps().has(c.op)) return undefined;
  const b = c.messageIndexB;
  if (typeof b !== "number" || !Number.isInteger(b) || b < 0) return undefined;
  return { op: c.op as CombineOp, messageIndexB: b };
}

/** A validated NetCDF difference-map request: combine field A (the panel's
 *  slice) with `variableIndexB` at `sliceIndicesB` under `op`. The common case
 *  is the same variable at a different slice — two time steps or levels. */
export interface NetcdfCompare {
  op: CombineOp;
  variableIndexB: number;
  sliceIndicesB: number[];
}

/** Validate a webview `compare` rider into a {@link NetcdfCompare}, or
 *  `undefined` for "no comparison". Same fall-back-to-single-render philosophy
 *  as {@link resolveGribCompare}; the indices must be a non-negative integer
 *  array (native re-validates them against the variable's shape). */
export function resolveNetcdfCompare(m: unknown): NetcdfCompare | undefined {
  const c = (
    m as { compare?: { op?: unknown; variableIndexB?: unknown; sliceIndicesB?: unknown } }
  )?.compare;
  if (!c || typeof c.op !== "string" || !knownCombineOps().has(c.op)) return undefined;
  const vb = c.variableIndexB;
  if (typeof vb !== "number" || !Number.isInteger(vb) || vb < 0) return undefined;
  const idx = c.sliceIndicesB;
  if (
    !Array.isArray(idx) ||
    !idx.every((n) => typeof n === "number" && Number.isInteger(n) && n >= 0)
  ) {
    return undefined;
  }
  return { op: c.op as CombineOp, variableIndexB: vb, sliceIndicesB: idx as number[] };
}

/** A concise picker label for a GRIB message: index, parameter, level, and
 *  forecast, e.g. `#3 · TMP · 500 (hPa) · +6h`. */
export function gribFieldLabel(
  m: Pick<MessageInfo, "index" | "abbreviation" | "parameter" | "level" | "forecast">,
): string {
  // A field the message does not state is `null` (#775) and is left out.
  const parts = [
    `#${m.index}`,
    m.abbreviation || m.parameter,
    m.level,
    m.forecast,
  ].filter((s) => !!s);
  return parts.join(" · ");
}

export function resolveRerenderOptions(m: Partial<RenderOptions>): RenderOptions {
  const projection: RenderOptions["projection"] =
    m.projection !== undefined && PROJECTIONS.includes(m.projection)
      ? m.projection
      : "source";
  const resampling: RenderOptions["resampling"] =
    m.resampling === "bilinear" ? "bilinear" : "nearest";
  // An unknown colormap drops to `undefined` — the native default (viridis) —
  // rather than round-tripping a name Rust would reject with an error popup.
  // Same clamp as the projection, for the same reason. An imported colour table
  // is not a name Rust knows: it is sent as the table it compiled to (#236).
  const named =
    m.colormap !== undefined && knownColormaps().has(m.colormap) ? m.colormap : undefined;
  const imported =
    named === undefined && m.colormap !== undefined ? importedColorTable(m.colormap) : undefined;
  const colormapTable = imported?.table;
  // Only "log10" turns log scaling on; anything else (including a typo) drops to
  // the native default of linear rather than round-tripping a value Rust would
  // reject. Same clamp shape as the colormap above.
  const scaleMode = m.scaleMode === "log10" ? "log10" : undefined;
  return {
    projection,
    projectionPreset: m.projectionPreset,
    centerLat: m.centerLat,
    centerLon: m.centerLon,
    resampling,
    flipY: !!m.flipY,
    rangeMin: m.rangeMin,
    rangeMax: m.rangeMax,
    boundsLatMin: m.boundsLatMin,
    boundsLatMax: m.boundsLatMax,
    boundsLonMin: m.boundsLonMin,
    boundsLonMax: m.boundsLonMax,
    colormap: named,
    colormapTable,
    reverseColormap: !!m.reverseColormap,
    scaleMode,
  };
}

export function buildGridReadyMessage(
  rendered: RenderedGrid,
  meta: Pick<PanelField, "index" | "parameter" | "units">,
  options: RenderOptions,
): GridReadyMessage {
  const rgbaView = new Uint8Array(
    rendered.rgba.buffer,
    rendered.rgba.byteOffset,
    rendered.rgba.byteLength,
  );
  return {
    type: "gridReady",
    messageIndex: meta.index,
    rgba: rgbaView,
    width: rendered.width,
    height: rendered.height,
    usedMin: rendered.usedMin,
    usedMax: rendered.usedMax,
    usedLatMin: rendered.usedLatMin,
    usedLatMax: rendered.usedLatMax,
    usedLonMin: rendered.usedLonMin,
    usedLonMax: rendered.usedLonMax,
    projectionSummary: rendered.projectionSummary,
    options,
    titleLine: composeTitleLine(meta),
    parameterUnits: meta.units ?? "",
    defaultPngName: composeDefaultPngName(meta),
    truncationNote: composeTruncationNote(rendered),
  };
}

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

function isNonNegativeInt(n: unknown): n is number {
  return typeof n === "number" && Number.isInteger(n) && n >= 0;
}

/// Compose the "Center" table cell: centre name, the sub-centre that produced
/// the field when the file names one, and the GRIB2 production status (Code
/// Table 1.3) so operational vs. research products are visible at a glance
/// without adding another column.
///
/// `subCentre` is `null` when the file names none; the check is nullish, so it
/// holds for `undefined` too (#288). A code no table names arrives as
/// `"Sub-centre 105"` or `"Production status 99"` (#774) and is shown as such;
/// only the table's own `"Missing"` is left out.
function formatCentreCell(m: MessageInfo): string {
  const centre = m.subCentre != null && m.subCentre !== ""
    ? `${m.originatingCentre} (${m.subCentre})`
    : m.originatingCentre;
  const status = productionStatus(m.identification);
  if (status && status !== "Missing") {
    return `${centre} · ${status}`;
  }
  return centre;
}

/** The GRIB2 production status, or `null` for an edition that has none: GRIB1
 *  carries no Code Table 1.3 field at all (#773). */
function productionStatus(id: Identification): string | null {
  switch (id.edition) {
    case "grib1":
      return null;
    case "grib2":
      return id.productionStatus;
    default: {
      const unreachable: never = id;
      void unreachable;
      return null;
    }
  }
}

/** The slice the render panel opens on: the variable's CF-detected horizontal
 *  axes (falling back to the first two dimensions) with every held index at 0. */
function defaultSliceSpec(v: NetcdfVariableMeta): SliceSpec {
  const yDim = v.detectedYDim ?? 0;
  let xDim = v.detectedXDim ?? (yDim === 0 ? 1 : 0);
  if (xDim === yDim) xDim = yDim === 0 ? 1 : 0;
  return {
    variableIndex: v.variableIndex,
    yDim,
    xDim,
    sliceIndices: v.dims.map(() => 0),
  };
}

/** What the render panel reads of a NetCDF or Zarr slice (#574): the variable's
 *  name and units, and the slice's own answers from `sliceGrid`, so the picker
 *  offers the reprojection targets exactly when the handle will draw them.
 *
 *  `grid` is `null` when the handle could not place the slice at all; the panel
 *  then offers the source view alone.
 *
 *  This is what the panel is written from. The picker can then move onto a
 *  variable or axes with a different answer, so every slice render sends its
 *  own in `GridReadyMessage.sliceGrid` and the picker follows that (#822). */
export function sliceField(
  v: NetcdfVariableMeta,
  grid: SliceGrid | null,
  index = v.variableIndex,
): PanelField {
  return {
    index,
    parameter: v.name,
    // Typeset by the native side (ADR-0007), so a NetCDF slice's title line and
    // probe readout carry units the way a GRIB message's do.
    units: v.units,
    level: null,
    levelType: null,
    referenceTime: null,
    forecast: null,
    uvRelativeToGrid: null,
    reprojectable: grid?.reprojectable ?? false,
    // A slice the handle could not answer for at all still draws its source
    // view, and nothing places it.
    placement: grid?.placement ?? "unplaceable",
    // A slice is never a band-limited spectral field (#637).
    truncation: null,
    grid: grid ? { label: grid.label } : null,
  };
}

/** The heading a render of this variable is labelled with. */
function sliceTitle(
  v: NetcdfVariableMeta,
  index: number,
): Pick<PanelField, "index" | "parameter" | "units"> {
  return { index, parameter: v.name, units: v.units };
}

/** The slice panel's projection caption: the container and the slice's family. */
export function sliceCaption(container: string, grid: SliceGrid | null): string {
  return `${container} slice — ${grid?.label ?? "not placed"}`;
}

/** How a message table or a panel caption states a message's size: the size
 *  label the file gives it where it has one (a truncation, an Nside, a reduced
 *  Gaussian's `N32`), else the raster's `ni×nj`, else `fallback`. A grid with
 *  no raster of its own has no `ni`/`nj` (`null`, #775), and a declared `0×0`
 *  is not a size either. */
function messageSize(m: MessageInfo, fallback: string): string {
  const g = m.grid;
  return m.sizeLabel
    ?? (g?.ni != null && g.nj != null && g.ni > 0 && g.nj > 0 ? `${g.ni}×${g.nj}` : fallback);
}

function describeProjection(meta: MessageInfo): string {
  const dims = messageSize(meta, "?");
  const type = meta.grid?.label ?? "unknown grid";
  const corners = meta.grid?.corners;
  if (corners != null) {
    const f = (v: number) => v.toFixed(2);
    const [latFirst, lonFirst, latLast, lonLast] = corners;
    return `${type} ${dims} — ${f(latFirst)},${f(lonFirst)} → `
         + `${f(latLast)},${f(lonLast)} (grid coordinates)`;
  }
  return `${type} ${dims} (grid coordinates)`;
}

function renderDatasetBody(
  d: DatasetMeta,
  netcdfVariables?: NetcdfVariableMeta[],
): string {
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
    const versionLine = d.hdf5SuperblockVersion != null
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

  // Render affordances: one button per renderable variable (numeric, ≥ 2-D).
  // Clicking opens the slice-picker render panel scoped to that variable (#122).
  if (netcdfVariables && netcdfVariables.length > 0) {
    const buttons = netcdfVariables
      .map(
        (v) =>
          `<button type="button" class="netcdf-render-btn" data-variable-index="${v.variableIndex}">Render ${escapeHtml(v.name)}</button>`,
      )
      .join(" ");
    sections.push(`
      <h2>Render</h2>
      <div class="netcdf-render">
        ${buttons}
        <div class="render-legend">
          Opens a 2-D slice of the variable in a new editor tab. Pick the
          image axes and step through the other dimensions in the panel.
        </div>
        <div class="render-status" id="netcdf-render-status"></div>
      </div>`);
  }

  if (d.dimensions.length > 0) {
    const rows = d.dimensions.map((dim) => `
      <tr>
        <td>${escapeHtml(dim.name)}</td>
        <td>${dim.isRecord ? "unlimited" : String(dim.length)}</td>
        <td>${dim.isRecord ? "record" : "fixed"}</td>
      </tr>`).join("");
    sections.push(`
      <h2>Dimensions</h2>
      <div class="table-scroll">
      <table>
        <thead><tr><th>Name</th><th>Length</th><th>Kind</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
      </div>`);
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
      <div class="table-scroll">
      <table>
        <thead><tr><th>Name</th><th>Type</th><th>Value</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
      </div>`);
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
        <td>${escapeHtml(v.units || "—")}</td>
        <td>${dims}</td>
        <td>${attrPreview}</td>
      </tr>`;
    }).join("");
    sections.push(`
      <h2>Variables</h2>
      <div class="table-scroll">
      <table>
        <thead><tr><th>Name</th><th>Type</th><th>Units</th><th>Dimensions</th><th>Attributes</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
      </div>`);
  }

  // Variables the reader had to leave out, listed after the ones it read.
  // Their absence from the table above is otherwise invisible, and a NetCDF-4
  // file mixing a compound or variable-length variable in with ordinary fields
  // is routine — a station-record file, a TROPOMI granule (#550).
  if (d.unsupportedVariables && d.unsupportedVariables.length > 0) {
    const rows = d.unsupportedVariables.map((v) => `
      <tr>
        <td>${escapeHtml(v.name)}</td>
        <td title="${escapeHtml(v.reason)}">${previewAttr(v.reason)}</td>
      </tr>`).join("");
    sections.push(`
      <h2>Variables not decoded</h2>
      <div class="table-scroll">
      <table>
        <thead><tr><th>Name</th><th>Reason</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
      </div>
      <div class="render-legend">
        The rest of this file's metadata is complete; these variables use an
        HDF5 datatype Fieldglass does not decode.
      </div>`);
  }

  if (d.dimensions.length === 0 && d.globalAttributes.length === 0 && d.variables.length === 0) {
    sections.push(`<div class="status">Empty NetCDF dataset.</div>`);
  }

  return sections.join("\n");
}

/** A message is renderable when its values land on a raster: `"placed"`, or
 *  `"unplaceable"`, which still paints in its own grid coordinates.
 *
 *  The answer is Rust's (`MessageInfo.placement`, #776), and it is about the
 *  values rather than the declared grid, so a spectral or HEALPix message —
 *  whose values are synthesised onto a lat/lon grid — is `"placed"`, and a
 *  bi-Fourier one, which has no raster at all, is `"no_raster"`. A template
 *  this build does not model (`"unsupported"`) is not offered either: nothing
 *  decodes it. This used to be a list of family names here. */
/** What the dormant P1 edit box shows after an edit re-lists the file: the
 *  raw octet the edit writes, or `null` to leave the box alone where a
 *  one-octet edit means nothing. Never `forecastHours`, which is normalised:
 *  under a 3-hourly unit it reads 12 for a P1 of 4, and saving the refreshed
 *  box untouched tripled the lead. Serialized into the table's script
 *  (`refreshedP1Value.toString()`), so it must not reference anything outside
 *  itself. */
export function refreshedP1Value(m: Pick<MessageInfo, "identification">): string | null {
  const id = m.identification;
  switch (id.edition) {
    case "grib1":
      return id.p1Octet != null ? String(id.p1Octet) : null;
    case "grib2":
      // Edition 2 has no P1 octet (#773).
      return null;
    default: {
      const unreachable: never = id;
      void unreachable;
      return null;
    }
  }
}

/** The Packing cell: the binding's friendly label for the identifier, the
 *  identifier itself without a binding, and "unknown" — the word the library
 *  used to send in band — where the data section's header could not be read
 *  (`packing` is `null`, #775). */
function packingCell(
  packing: string | null,
  native: { packingLabel(id: string): string } | null | undefined,
): string {
  if (packing == null) return "unknown";
  return native?.packingLabel(packing) ?? packing;
}

function messageIsRenderable(m: Pick<MessageInfo, "placement">): boolean {
  return m.placement === "placed" || m.placement === "unplaceable";
}

/** Exported for tests: the P1 edit path is dormant (`editable` is false at the
 *  call site), so nothing else exercises the markup it would produce. */
export function renderHtml(
  webview: vscode.Webview,
  format: string,
  filePath: string,
  messages: MessageInfo[] | undefined,
  dataset: DatasetMeta | undefined,
  headerBytes: Uint8Array | undefined,
  editable: boolean,
  netcdfVariables?: NetcdfVariableMeta[]
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
    // A field with nothing to report is `null` (#574); the check is nullish
    // anyway, so it holds for `undefined` too — a grid-less message (e.g.
    // GRIB1 spectral) has no bounds.
    const fmt1 = (v: number | null | undefined) => v != null ? v.toFixed(3) : "—";
    const native = loadNative();
    const COLSPAN = 13;
    const rows = messages.map((m) => {
      // A message with no raster shape (spectral, HEALPix) has no Ni×Nj but
      // does state its size, as a truncation or an Nside — showing that is the
      // difference between "we don't know" and "the file says so, in its own
      // units". The label wins where both exist: a reduced Gaussian grid has
      // an Ni×Nj here, but it is the widest row paired with the row count, a
      // raster Fieldglass derives for rendering rather than anything the file
      // says. The file says `N32`, and so does every other tool (#500).
      // Absent values are `null`, and these are nullish checks either way
      // (#288).
      //
      // This cell used to hold only numbers and a dash, so it was interpolated
      // raw; it now carries a string built from a decoded file, and is escaped
      // on the same rule the panel title and the projection caption follow.
      // Rust builds the label from integers alone, so nothing hostile can
      // reach here today — the escape is so that stays true of the *cell*
      // rather than of one `format!` in another crate.
      const gridDims = messageSize(m, "—");
      const corners = m.grid?.corners;
      const gridBounds = corners != null
        ? `${fmt1(corners[0])},${fmt1(corners[1])} → ${fmt1(corners[2])},${fmt1(corners[3])}` : "—";
      // The edit writes the raw P1 octet, so the box has to show that octet —
      // not `forecastHours`, which is normalised (a 3-hourly unit reports 12
      // for a P1 of 4, and saving the untouched box would have tripled the
      // lead). There is no octet wherever a one-octet edit is meaningless (a
      // GRIB2 message, or a GRIB1 16-bit P1), and those messages stay
      // read-only.
      const p1 = refreshedP1Value(m);
      const fcstCell = editable && p1 != null
        ? `<input type="number" class="p1-input" data-message-index="${m.index}" min="0" max="255" step="1" value="${p1}" />`
        : escapeHtml(m.forecast ?? "—");
      const canRender = messageIsRenderable(m);
      const idx = m.index;
      const expansionInner = canRender
        ? `<button type="button" class="render-btn" data-message-index="${idx}">Render</button>
           <button type="button" class="export-csv-btn" data-message-index="${idx}">Export CSV…</button>
           <div class="render-status" id="status-${idx}"></div>
           <div class="render-legend">
             Opens the rendered grid in a new editor tab. Painted in grid
             coordinates (no map reprojection); bitmap-masked points render
             as transparent. Export CSV writes the decoded field to a file.
           </div>`
        : `<div class="render-na">Render not available — grid dimensions unknown for this message.</div>`;
      return `
      <tr class="msg-row" data-message-index="${idx}">
        <td>${idx}</td>
        <td>${escapeHtml(m.parameter ?? "")}</td>
        <td>${escapeHtml(m.abbreviation ?? "")}</td>
        <td>${escapeHtml(m.units ?? "")}</td>
        <td>${escapeHtml(m.level ?? "—")}</td>
        <td>${escapeHtml(m.levelType ?? "—")}</td>
        <td>${escapeHtml(m.referenceTime ?? "")}</td>
        <td>${fcstCell}</td>
        <td>${escapeHtml(m.grid?.label ?? "—")}</td>
        <td>${escapeHtml(gridDims)}</td>
        <td>${gridBounds}</td>
        <td>${escapeHtml(packingCell(m.packing, native))}</td>
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
    <div class="table-scroll">
    <table>
      <thead>
        <tr>
          <th>#</th><th>Parameter</th><th>Abbrev</th><th>Units</th>
          <th>Level</th><th>Level Type</th><th>Reference Time</th><th>${fcstHeader}</th>
          <th>Grid</th><th>Size</th><th>Bounds (lat,lon)</th><th>Packing</th><th>Center</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>
    </div>`;
  } else if (dataset) {
    bodyContent = renderDatasetBody(dataset, netcdfVariables);
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
        ${refreshedP1Value.toString()}

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
          document.querySelectorAll('button.export-csv-btn').forEach((el) => {
            el.addEventListener('click', (ev) => {
              ev.stopPropagation();
              const idx = Number(el.getAttribute('data-message-index'));
              if (!Number.isFinite(idx)) return;
              vscode.postMessage({ type: 'exportCsv', messageIndex: idx });
            });
          });
          // NetCDF: open the slice-picker render panel for a variable.
          document.querySelectorAll('button.netcdf-render-btn').forEach((el) => {
            el.addEventListener('click', (ev) => {
              ev.stopPropagation();
              const idx = Number(el.getAttribute('data-variable-index'));
              if (!Number.isFinite(idx)) return;
              const s = document.getElementById('netcdf-render-status');
              if (s) s.textContent = 'Opening render…';
              vscode.postMessage({ type: 'renderVariable', variableIndex: idx });
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
            if (typeof msg.variableIndex === 'number') {
              const s = document.getElementById('netcdf-render-status');
              if (s) s.textContent = 'Opened render in a new tab.';
            } else {
              setStatus(msg.messageIndex, 'Opened render of message ' + msg.messageIndex + ' in a new tab.');
            }
            return;
          }
          if (msg.type === 'gridError') {
            setStatus(msg.messageIndex, 'Render failed: ' + msg.error);
            return;
          }
          if (editable && msg.type === 'update' && Array.isArray(msg.messages)) {
            for (const m of msg.messages) {
              const el = document.querySelector('input.p1-input[data-message-index="' + m.index + '"]');
              const p1 = refreshedP1Value(m);
              if (el && document.activeElement !== el && p1 != null) {
                el.value = p1;
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
    /* Every table lives in one of these. The cells are no-wrap so a row reads
       as one line, which means a table is as wide as its widest row wants to
       be: thirteen columns for a GRIB message, four of them free text, and a
       centre name that is WMO's own wording at up to 82 characters. Without a
       container that row pushed the whole page sideways and moved the render
       panel and the headings with it. Now it scrolls inside the table and the
       page around it does not move. (No backticks in here - this whole block
       is a template literal.) */
    .table-scroll { overflow-x: auto; max-width: 100%; }
    table { border-collapse: collapse; font-size: 0.85rem; width: 100%; }
    th, td { text-align: left; padding: 0.3rem 0.6rem; border-bottom: 1px solid var(--vscode-panel-border); white-space: nowrap; }
    th { color: var(--vscode-descriptionForeground); font-weight: 600; }
    tr.msg-row { cursor: pointer; }
    tr.msg-row:hover td { background: var(--vscode-list-hoverBackground); }
    tr.msg-row.selected td {
      background: var(--vscode-list-activeSelectionBackground);
      color: var(--vscode-list-activeSelectionForeground);
    }
    /* #448 capped the centre column specifically - white-space: normal and a
       22rem max-width - because it was the one cell wide enough to push the
       page sideways. The .table-scroll rule above is the general form of that
       fix, so the special case is gone rather than kept alongside it: a single
       wrapping column in an otherwise single-line table gave that row a height
       unlike every other, which is what makes a dense table hard to scan.
       The long name is still all there, on one line, a scroll away. */
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
    button.render-btn, button.netcdf-render-btn {
      background: var(--vscode-button-secondaryBackground, var(--vscode-button-background));
      color: var(--vscode-button-secondaryForeground, var(--vscode-button-foreground));
      border: 1px solid var(--vscode-button-border, transparent);
      padding: 0.15rem 0.6rem;
      cursor: pointer;
      font-family: inherit;
      font-size: inherit;
      border-radius: 2px;
    }
    button.render-btn:hover, button.netcdf-render-btn:hover {
      background: var(--vscode-button-secondaryHoverBackground, var(--vscode-button-hoverBackground));
    }
    button.render-btn:focus, button.netcdf-render-btn:focus {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: 1px;
    }
    .netcdf-render { display: flex; flex-wrap: wrap; align-items: center; gap: 0.5rem; }
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
