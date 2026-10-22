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
import { HandleLeases, HeldHandles } from "./handle-leases";
import { escapeHtml, nonce } from "./html";
import { couldNotOpen, errorText, nothingToDraw, notAStore, reportProblem } from "./problems";
import {
  loadNative,
  nativeBinaryName,
  type AxisValues,
  type CombineOp,
  type ColourRange,
  type CombineOpInfo,
  type DatasetMeta,
  type FieldglassNative,
  type GridSupport,
  type GridTarget,
  type Handle,
  type Identification,
  type MessageInfo,
  type VariableInfo,
  type RangeRefusal,
  type SliceAxes,
  type SliceGrid,
  type SliceRequest,
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
  cleanTitlePart,
  composeDefaultPngName,
  composeExportName,
  composeTabTitle,
  composeTitleLine,
  composeTruncationNote,
  CSV_FIELD_A_NOTE,
  heldSliceParts,
  NO_LONG_CSV_NOTE,
  NO_SUPPORT,
  UNPLACED_LONG_CSV_NOTE,
  offersOverlays,
  renderImagePanelHtml,
  sanitizeExportName,
  sanitizePngName,
  unavailableControls,
  type CompareFieldOption,
  type ExportExtension,
  type PanelField,
  type PanelSource,
  type SliceNotePart,
  type SlicePanelData,
  type SliceSpec,
  type UnavailableGroup,
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
export function listMessages(handle: Handle): MessageInfo[] {
  return Array.from({ length: handle.count() }, (_, i) => handle.message(i));
}

/** A handle's variables, as the panel shows them: the browser package's
 *  listing (#1073) with each `units` typeset for display (ADR-0007), the way a
 *  GRIB message's units arrive. */
export function listVariables(native: FieldglassNative, handle: Handle): VariableInfo[] {
  return handle.variables().map((v) => ({
    ...v,
    units: v.units == null ? null : native.typesetUnits(v.units) || null,
  }));
}

/** {@link listVariables} with the addon this extension loaded. */
function listVariablesOf(handle: Handle): VariableInfo[] {
  const native = loadNative();
  return native ? listVariables(native, handle) : handle.variables();
}

/** A panel's slice as the handle's `SliceRequest` (#1073): the panel names the
 *  variable `variableIndex`, the handle `variable`. */
export function sliceOf(spec: SliceSpec): SliceRequest {
  return {
    variable: spec.variableIndex,
    yDim: spec.yDim,
    xDim: spec.xDim,
    sliceIndices: spec.sliceIndices,
  };
}

/** A panel's slice without its held positions: what placing it takes. */
function axesOf(spec: SliceSpec): SliceAxes {
  return { variable: spec.variableIndex, yDim: spec.yDim, xDim: spec.xDim };
}

/** What a slice panel draws: its slice, or that slice combined with Field B,
 *  a slice of `variableIndexB` on the same axes at `sliceIndicesB` (#239). */
export function sliceTarget(
  spec: SliceSpec,
  compare: { variableIndexB: number; sliceIndicesB: number[]; op: CombineOp } | null | undefined,
): GridTarget {
  if (!compare) return sliceOf(spec);
  return {
    a: sliceOf(spec),
    b: sliceOf({ ...spec, variableIndex: compare.variableIndexB, sliceIndices: compare.sliceIndicesB }),
    op: compare.op,
  };
}

/** What a message panel draws: its message, or that message combined with
 *  message B (#239). */
export function messageTarget(index: number, compare: GribCompare | null | undefined): GridTarget {
  return compare ? { a: index, b: compare.messageIndexB, op: compare.op } : index;
}

/** The name a render panel calls its file or store by (#1038): the last part
 *  of its path, as the explorer shows it. A store is a directory, and a
 *  trailing separator does not hide its name. */
export function sourceName(uri: vscode.Uri): string {
  return path.posix.basename(uri.path);
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

/** A message from the editor page, with the number the page was written
 *  under (#1217). Untrusted, as everything from a webview is. */
type WebviewMessage = (
  | EditP1Message
  | ReadyMessage
  | DecodeGridMessage
  | RenderVariableMessage
  | ExportCsvMessage
) & { page?: unknown };

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
 * re-fetch the handle (a revert or a reopen replaces it while the panel lives),
 * where an export should start, what to caption the geometry, and what to say
 * when the panel has no handle.
 */
interface SlicePanelSubject {
  /** The container's handle now, which a panel's gate compares with the one
   *  it holds to notice a revert or a reopen. A request is served from the
   *  gate's handle (#1338); an export is given that one. */
  handle(): Handle | undefined;
  gone: string;
  exportDir: vscode.Uri;
  /** The container's name, which the caption opens with: `"NetCDF"`, `"Zarr"`. */
  container: string;
  /** What the container is, for a message about it: `"file"`, `"store"`. */
  noun: string;
  /** The file or store's name, which the tab and the line under the heading
   *  carry (#1038). */
  source: string;
  /** The document the panel draws from, which a revert writes the panel
   *  again for (#1217). A Zarr store is not a document. */
  document?: vscode.Uri;
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
    // `loadNative` has already told the user when the module would not load.
    if (!loadNative()) return;
    const picked = await vscode.window.showOpenDialog({
      canSelectFiles: false,
      canSelectFolders: true,
      canSelectMany: false,
      openLabel: "Open Zarr Store",
      title: "Select a Zarr store directory",
    });
    const storeUri = picked?.[0];
    if (!storeUri) return;
    // Awaited, so a throw while opening reaches the command and is reported,
    // rather than being dropped as an unhandled rejection.
    await this.openZarrStore(storeUri);
  }

  /** Open a store the caller has already chosen.
   *
   * Separate from the picker so a test can drive it without a modal dialog, which
   * `@vscode/test-electron` cannot dismiss.
   *
   * A store that opens gets its panel before this returns. When it will not
   * open, the user is told why, and the promise settles once they have
   * dismissed that (#1040).
   */
  public async openZarrStore(storeUri: vscode.Uri): Promise<void> {
    const native = loadNative();
    if (!native) return;
    const name = sourceName(storeUri);
    if (!native.isStore(storeUri.fsPath)) {
      // A folder of stores is the likely mistake, so name the stores in it.
      const inside = await childStores(storeUri, (p) => native.isStore(p));
      await reportProblem("error", notAStore(storeUri.fsPath, name, inside.stores, inside.partial, MAX_FOLDERS_EXAMINED));
      return;
    }
    let handle: Handle;
    try {
      handle = native.openStore(storeUri.fsPath);
    } catch (err) {
      await reportProblem("error", couldNotOpen("Zarr store", name, storeUri.fsPath, err));
      return;
    }
    let problem: ReturnType<typeof nothingToDraw> | undefined;
    try {
      const variables = listVariables(native, handle);
      if (variables.length > 0) {
        this._zarrHandlesByStore.set(storeUri.toString(), handle);
        this.openZarrRenderPanel(storeUri, variables[0].index);
        return;
      }
      // A store that opened but holds nothing drawable. `leftOut` and
      // `undrawable` say why for each of its arrays (#1129), and
      // saying so beats a panel with an empty picker.
      problem = nothingToDraw(storeUri.fsPath, handle.leftOut(), handle.undrawable());
    } finally {
      // Kept only by the map, which a store with nothing to draw is not put
      // in (#1204).
      if (this._zarrHandlesByStore.get(storeUri.toString()) !== handle) handle.close();
    }
    await reportProblem("warning", problem);
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
  /** The CSV export a page last asked for, which a message handler starts and
   *  does not wait on; a test waits on it to read the outcome (#1039). */
  private _csvExport: Promise<void> = Promise.resolve();

  /** Build `panel`'s HTML now, and again whenever the colormaps change. */
  private trackRenderPanel(
    panel: vscode.WebviewPanel,
    build: () => void,
    gate: PanelHandleGate<Handle>,
    document?: vscode.Uri,
  ): void {
    build();
    this._renderPanelBuilders.set(panel, build);
    this._renderPanelGates.set(panel, gate);
    if (document) this._renderPanelDocs.set(panel, document.toString());
    panel.onDidDispose(() => {
      this._renderPanelBuilders.delete(panel);
      this._renderPanelGates.delete(panel);
      this._renderPanelDocs.delete(panel);
    });
  }
  /** Each render panel's gate, which a revert tells that its handle is the
   *  old file's (#1338). */
  private readonly _renderPanelGates = new Map<vscode.WebviewPanel, PanelHandleGate<Handle>>();
  /** The document each render panel draws from, for the panels a revert
   *  writes again (#1217). A Zarr panel has none. */
  private readonly _renderPanelDocs = new Map<vscode.WebviewPanel, string>();

  private readonly _onDidChangeCustomDocument =
    new vscode.EventEmitter<vscode.CustomDocumentEditEvent<FieldglassDocument>>();
  public readonly onDidChangeCustomDocument = this._onDidChangeCustomDocument.event;

  /** Who holds each native handle: the maps below, each render panel's gate,
   *  and a CSV export waiting on its dialogs. The last to let go closes it
   *  (#1204), so its native memory is freed then rather than whenever V8
   *  collects the JS object. */
  private readonly leases = new HandleLeases<Handle>();

  // All panels currently rendering each document, keyed by uri.toString().
  private readonly _panelsByDoc = new Map<string, Set<vscode.WebviewPanel>>();

  // GRIB handles per document. Parsed once; subsequent decode / render calls
  // reuse the same handle rather than re-parsing the buffer on every napi call
  // (was #41 — closed by the handle API).
  private readonly _handlesByDoc = new HeldHandles<Handle>(this.leases);

  // NetCDF handles per document, parallel to `_handlesByDoc`: the same `Handle`
  // type (#1073), kept apart because a NetCDF document is opened through the
  // metadata view and its slice panels rather than the message list. Built
  // lazily when a NetCDF file is opened and dropped with the last panel (see
  // `trackPanel`).
  private readonly _netcdfHandlesByDoc = new HeldHandles<Handle>(this.leases);
  /** Zarr store handles, keyed by the store directory's URI (#659).
   *
   * Not by document, because a store has none: a custom editor cannot bind to a
   * directory, so a store arrives from the "Open Zarr Store…" command. Dropped
   * with the store's last panel. */
  private readonly _zarrHandlesByStore = new HeldHandles<Handle>(this.leases);
  /** The render panels open on each store, by the same key. */
  private readonly _panelsByStore = new Map<string, Set<vscode.WebviewPanel>>();

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
    // Scripts must be enabled so the webview can request and paint a 2-D
    // render of a message's decoded grid. The CSP set in renderHtml is the
    // security boundary — see the comment there for the policy itself.
    // `localResourceRoots: []` makes the no-external-resources boundary
    // explicit: the CSP (`default-src 'none'`) already blocks loads, and this
    // ensures nothing can be served from disk even if the CSP is later relaxed.
    panel.webview.options = { enableScripts: true, localResourceRoots: [] };
    this.writeEditorPage(document, panel);

    panel.webview.onDidReceiveMessage((msg: WebviewMessage) => {
      // From a page this panel has written over since (#1217). Its row and
      // variable numbers are the old file's, so serving it would open whatever
      // the new file holds at that number.
      if (!msg || msg.page !== this._editorPages.get(panel)) return;
      this.handleWebviewMessage(document, panel, msg);
    });
  }

  /** The number each editor page was last written with. The page sends it
   *  back on every message (#1217), as a render panel's page does (#1202). */
  private readonly _editorPages = new WeakMap<vscode.WebviewPanel, number>();

  /** Write `panel`'s editor page from the document's bytes as they are now,
   *  under a number of its own. Reads through the document's cached handles,
   *  opening them when there are none. */
  private writeEditorPage(document: FieldglassDocument, panel: vscode.WebviewPanel): void {
    const native = loadNative();
    const header = document.bytes.slice(0, 32);
    // The whole buffer, not `header`: an HDF5 file may put its signature after
    // a userblock, up to 16 KiB in (#936), and detection reads no further than
    // that. The buffer is passed without a copy.
    const format = native ? native.detectBytes(document.bytes) : "unknown";

    const handle = native ? this.openOrReuseHandle(document, format) : undefined;
    const messages = handle ? listMessages(handle) : undefined;
    let dataset: DatasetMeta | undefined;
    let netcdfVariables: VariableInfo[] | undefined;
    if (native && format === "netcdf") {
      // One reader for the whole open. The handle is retained for rendering
      // anyway, and it answers for the metadata too, so the file is parsed and
      // copied once (#411).
      const ncHandle = this.openOrReuseNetcdfHandle(document);
      try {
        dataset = ncHandle?.metadata();
      } catch (err) {
        console.error("[Fieldglass] Handle.metadata failed:", err);
        // Leave `dataset` undefined; the renderer falls back to its
        // nothing-listed status line with the format badge intact.
      }
      // The renderable-variable list drives the "Render" affordances in the
      // metadata view. A backing without a render path yet (HDF5, #169) returns
      // an empty list, so the dump shows without render buttons.
      try {
        netcdfVariables = ncHandle && native ? listVariables(native, ncHandle) : undefined;
      } catch (err) {
        console.error("[Fieldglass] Handle.variables failed:", err);
      }
    }
    const headerBytes = format === "unknown" ? header : undefined;
    // Editing wiring (set_p1, undo/redo, save, webview script + input) is kept
    // intact for when general PDS field editing lands, but disabled at the
    // entry point so users see a coherent read-only viewer instead of a
    // single editable column.
    const editable = false;

    const page = (this._editorPages.get(panel) ?? -1) + 1;
    this._editorPages.set(panel, page);
    panel.webview.html = renderHtml(
      panel.webview,
      format,
      document.uri.fsPath,
      messages,
      dataset,
      headerBytes,
      editable,
      netcdfVariables,
      page,
    );
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
    this.reloadDocument(document);
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
        this.postCurrentMessages(panel, document, msg.page);
        return;
      case "edit-p1":
        if (!isNonNegativeInt(msg.messageIndex) || !isNonNegativeInt(msg.value)) return;
        this.applyP1Edit(document, msg.messageIndex, msg.value);
        return;
      case "decodeGrid":
        if (!isNonNegativeInt(msg.messageIndex)) return;
        this.handleDecodeGrid(document, panel, msg.messageIndex, msg.page);
        return;
      case "exportCsv":
        if (!isNonNegativeInt(msg.messageIndex)) return;
        this._csvExport = this.handleExportCsv(document, msg.messageIndex);
        return;
      case "renderVariable":
        if (!isNonNegativeInt(msg.variableIndex)) return;
        this.openNetcdfRenderPanel(document, msg.variableIndex);
        void panel.webview.postMessage({ type: "renderOpened", variableIndex: msg.variableIndex, page: msg.page });
        return;
    }
  }

  /** Decode one message's grid in Rust and post values + shape to the webview. */
  private handleDecodeGrid(
    document: FieldglassDocument,
    panel: vscode.WebviewPanel,
    messageIndex: number,
    page: unknown,
  ): void {
    // Every reply names the page that asked (#1219): a revert can write the
    // page again before the reply lands, and the new page drops it.
    const reply = (m: object) => void panel.webview.postMessage({ ...m, page });
    const native = loadNative();
    if (!native) {
      reply({
        type: "gridError",
        messageIndex,
        error: `native module ${nativeBinaryName()} not loaded`,
      });
      return;
    }
    const handle = this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      reply({
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
      reply({
        type: "gridError",
        messageIndex,
        error: `message ${messageIndex} out of range`,
      });
      return;
    }
    if (!messageIsRenderable(meta)) {
      reply({
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

    reply({ type: "renderOpened", messageIndex });
  }

  /** Export one GRIB message's decoded field to a CSV file the user picks.
   *  Asks for the layout (long `lat,lon,value` or a 2-D matrix), guards a
   *  very large export behind a confirm, then writes the CSV to disk. (The
   *  whole field is serialized to bytes in one pass — not streamed.) */
  private async handleExportCsv(
    document: FieldglassDocument,
    messageIndex: number,
    // Asked from a render panel with Compare on: the CSV is Field A's.
    fieldA = false,
    // Asked from a render panel: the handle the panel holds, which outlives
    // the document's editor (#1338). The message table reads the document's.
    panelHandle?: () => Handle | undefined,
  ): Promise<void> {
    const handle = panelHandle ? panelHandle() : this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      // A panel is on a GRIB file whatever happened to its handle, so it says
      // what is missing, as a slice panel does.
      void vscode.window.showErrorMessage(
        panelHandle
          ? "Fieldglass: CSV export is available only for an open message."
          : "Fieldglass: CSV export is available for GRIB files only."
      );
      return;
    }
    await this.whileHeld(handle, () =>
      this.exportMessageCsv(document, handle, messageIndex, fieldA, this.unrevertedSince(document.uri)),
    );
  }

  /** How many times each document has been read from disk again (File: Revert
   *  File, #1219). A handle map entry cannot say so: it is also dropped when
   *  the document's last panel closes, which leaves an export in flight valid. */
  private readonly _reverts = new Map<string, number>();

  /** A check, for an export about to ask the user something, of whether the
   *  document has been reverted since: the field it chose belongs to the old
   *  file. Always true for a store, which cannot be reverted. */
  private unrevertedSince(uri: vscode.Uri | undefined): () => boolean {
    if (!uri) return () => true;
    const key = uri.toString();
    const at = this._reverts.get(key) ?? 0;
    return () => (this._reverts.get(key) ?? 0) === at;
  }

  /** Hold `handle` open until `run` settles (#1204): an export asks for its
   *  layout before it reads, and the file's last panel can close meanwhile. */
  private async whileHeld(handle: Handle, run: () => Promise<void>): Promise<void> {
    const lease = {};
    this.leases.hold(handle, lease);
    try {
      await run();
    } finally {
      this.leases.release(handle, lease);
    }
  }

  private async exportMessageCsv(
    document: FieldglassDocument,
    handle: Handle,
    messageIndex: number,
    fieldA: boolean,
    current: () => boolean,
  ): Promise<void> {
    // The long layout only where Rust says this message's grid takes it: its
    // family places its points (#945). A message it cannot answer for is
    // offered the matrix.
    let longCsv = false;
    let placed = false;
    // Named as its PNG is (#1039), from the file the message is in.
    const source: PanelSource = { kind: "message", name: sourceName(document.uri) };
    let name = composeExportName({ index: messageIndex, parameter: null }, source, "csv");
    try {
      const meta = handle.message(messageIndex);
      longCsv = meta.supports.longCsv;
      placed = offersOverlays(meta.placement);
      name = composeExportName(meta, source, "csv");
    } catch {
      // The export below reports what is wrong with the message.
    }
    const format = await this.pickCsvFormat(longCsv, placed, fieldA);
    if (!format) return;
    if (!current()) return refuseChangedExport();

    let csv: Uint8Array;
    try {
      csv = handle.exportCsv(messageIndex, format);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: CSV export failed: ${errorText(err)}`
      );
      return;
    }

    await this.saveCsv(csv, vscode.Uri.joinPath(document.uri, ".."), name, current);
  }

  /** Ask for a CSV layout (long / matrix). Returns the chosen format, or
   *  `undefined` if the user cancelled. Shared by the GRIB and NetCDF export
   *  commands.
   *
   *  `longCsv` is the field's `GridSupport.longCsv` (#945). Without it the
   *  long layout is not offered, and the prompt says why, rather than being
   *  offered and then refused. `placed` picks the reason: a field nothing
   *  places has no position anywhere, where a placed one is only not yet
   *  supported for its family. */
  private async pickCsvFormat(
    longCsv: boolean,
    placed: boolean,
    fieldA = false,
  ): Promise<"matrix" | "long" | undefined> {
    const matrix = { label: "Matrix — a 2-D grid of values", format: "matrix" as const };
    const pick = await vscode.window.showQuickPick(
      longCsv
        ? [{ label: "Long — one lat,lon,value row per grid point", format: "long" as const }, matrix]
        : [matrix],
      {
        // With Compare on the panel shows the combined map, but the CSV
        // holds Field A (#1039), and the prompt says so before anything is
        // written.
        placeHolder: [
          fieldA ? CSV_FIELD_A_NOTE : "",
          longCsv ? "CSV layout" : `CSV layout. ${placed ? NO_LONG_CSV_NOTE : UNPLACED_LONG_CSV_NOTE}`,
        ].filter((s) => !!s).join(" "),
      }
    );
    return pick?.format;
  }

  /** Confirm a large export, prompt for a destination, and write the CSV.
   *  Shared by the GRIB and NetCDF export commands. The dialog starts in
   *  `exportDir` on `defaultName`, as the PNG export's does (#1039). */
  private async saveCsv(
    csv: Uint8Array,
    exportDir: vscode.Uri,
    defaultName: string,
    // Whether `csv` still comes from the file the panel shows (#1219).
    current: () => boolean,
  ): Promise<void> {
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
      if (!current()) return refuseChangedExport();
    }

    const dest = await saveDestination(
      await vscode.window.showSaveDialog({
        defaultUri: vscode.Uri.joinPath(exportDir, sanitizeExportName(defaultName, "csv")),
        filters: { "CSV files": ["csv"] },
        saveLabel: "Export CSV",
      }),
      "csv",
    );
    if (!dest) return;
    if (!current()) return refuseChangedExport();

    try {
      await vscode.workspace.fs.writeFile(dest, csv);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: could not write CSV: ${errorText(err)}`
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
    spec: { variableIndex: number; yDim: number; xDim: number; sliceIndices: number[] },
    // Asked with Compare on: the CSV is Field A's slice.
    fieldA = false,
  ): Promise<void> {
    const handle = subject.handle();
    if (!handle) {
      void vscode.window.showErrorMessage(
        "Fieldglass: CSV export is available only for an open slice."
      );
      return;
    }
    await this.whileHeld(handle, () =>
      this.exportSliceCsv(subject, handle, spec, fieldA, this.unrevertedSince(subject.document)),
    );
  }

  private async exportSliceCsv(
    subject: SlicePanelSubject,
    handle: Handle,
    spec: { variableIndex: number; yDim: number; xDim: number; sliceIndices: number[] },
    fieldA: boolean,
    current: () => boolean,
  ): Promise<void> {
    // As for a GRIB message: the long layout only where the slice's grid
    // takes it (#945).
    let longCsv = false;
    let placed = false;
    try {
      const grid = handle.sliceGrid(axesOf(spec));
      longCsv = grid.supports.longCsv;
      placed = offersOverlays(grid.placement);
    } catch {
      // The export below reports what is wrong with the slice.
    }
    const format = await this.pickCsvFormat(longCsv, placed, fieldA);
    if (!format) return;
    if (!current()) return refuseChangedExport();

    let csv: Uint8Array;
    try {
      csv = handle.exportCsv(sliceOf(spec), format);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `Fieldglass: CSV export failed: ${errorText(err)}`
      );
      return;
    }

    // Named as the PNG of this slice is (#1039, #1112).
    let variable: VariableInfo | undefined;
    try {
      variable = handle.variables().find((v) => v.index === spec.variableIndex);
    } catch {
      // Named by the variable's index below.
    }
    const name = sliceExportName(variable, spec, subject.source, "csv", axisReader(handle, spec.variableIndex));
    await this.saveCsv(csv, subject.exportDir, name, current);
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
    const dest = await saveDestination(
      await vscode.window.showSaveDialog({
        defaultUri: vscode.Uri.joinPath(exportDir, name),
        filters: { "PNG image": ["png"] },
        saveLabel: "Export PNG",
      }),
      "png",
    );
    if (!dest) return { status: "cancelled" };
    try {
      await vscode.workspace.fs.writeFile(dest, buffer);
    } catch (err) {
      const reason = `could not write PNG: ${errorText(err)}`;
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
   * side and ships paint-ready RGBA bytes over postMessage. Picker
   * changes (projection / resampling / range / flip-y) flow back as
   * `rerenderRequest` and trigger a fresh `renderGrid` call.
   */
  public openRenderPanel(
    document: FieldglassDocument,
    meta: MessageInfo,
  ): void {
    const key = document.uri.toString();
    const docHandle = () => this._handlesByDoc.get(key);
    // The message the panel draws. A file that changes under the panel can
    // hold it at another index, or not at all (#959), so this moves.
    let shown = meta;
    // The tab names the message and the file (#1038). The panel keeps the
    // name of the file it was opened on: it draws from that document.
    const source = sourceName(document.uri);
    // The abbreviation comes from a decoded (untrusted) file. VS Code renders
    // panel titles as plain text, so there's no XSS, but strip control
    // characters and cap the length so a hostile file can't garble the tab.
    const titleOf = (m: MessageInfo): string => {
      const abbr = cleanTitlePart(m.abbreviation);
      return composeTabTitle(`msg ${m.index}` + (abbr ? ` ${abbr}` : ""), source);
    };
    const panel = vscode.window.createWebviewPanel(
      "fieldglass.render",
      titleOf(meta),
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, retainContextWhenHidden: false, localResourceRoots: [] }
    );
    // Every message in the file is a candidate "field B" for a difference map
    // (#239) and a V component (#241); the pickers only appear when there are
    // at least two. Each carries its identity, so a page written again for a
    // changed file finds its Field B by it (#959).
    const fieldsOf = (messages: MessageInfo[]): CompareFieldOption[] =>
      messages.map((m) => ({ index: m.index, label: gribFieldLabel(m), key: messageKey(m) }));
    const opened = docHandle();
    // The messages the page's numbers refer to: the file as it was when the
    // page was written.
    let listed: MessageInfo[] = opened ? listMessages(opened) : [];
    let compareFields: CompareFieldOption[] | undefined = opened ? fieldsOf(listed) : undefined;
    // Set while the panel's message is not exactly once in its changed file:
    // the new file's messages, to pick one from (#959). Declared before the
    // first write, which reads it.
    let picking: CompareFieldOption[] | null = null;
    // A changed file is read through the gate slice panels use (#839). The
    // panel's message is looked for in the new file by its identity, which is
    // Rust's (`MessageInfo.identity`): the parameter, level, reference time,
    // step or range, grid, and ensemble member or product (for GRIB1, the
    // centre's PDS extension as written). Exactly one match is drawn under its
    // own heading. None, or more than one, is refused, and the new file's
    // messages are offered instead (`picking`); nothing is drawn until one is
    // picked.
    // Made before the first write, which numbers the page from it.
    const gate = new PanelHandleGate<Handle>(opened, this.leases);

    const write = () => {
      panel.webview.html = renderImagePanelHtml(
        panel.webview,
        shown,
        describeProjection(shown),
        colormapRegistry(),
        combineOpRegistry(),
        undefined,
        compareFields,
        // Written while no message is chosen (an imported colormap rebuilds
        // every panel): the page opens on the picker, never on the old heading.
        picking ?? undefined,
        source,
        gate.page,
      );
    };
    this.trackRenderPanel(panel, write, gate, document.uri);

    const refuse = (ask: { seq?: number } = {}) => {
      if (!picking) return;
      panel.webview.postMessage({
        type: "gridError",
        ...ask,
        messageIndex: shown.index,
        error: MESSAGE_GONE,
        pickMessages: picking,
      } satisfies GridErrorMessage);
    };
    const adopt = (current: Handle, ask: { seq?: number } = {}): boolean => {
      const next = listMessages(current);
      // The same messages in the same places: the file is the one the page was
      // written from (an editor closed and opened again, say). The page's
      // numbers still mean what they did, so the request is served.
      const again = picking ? undefined : next.find((m) => m.index === shown.index);
      if (again && sameMessages(listed, next)) {
        listed = next;
        shown = again;
        gate.keep(current);
        return true;
      }
      listed = next;
      compareFields = fieldsOf(next);
      const wanted = messageKey(shown);
      const found = picking ? [] : next.filter((m) => messageKey(m) === wanted);
      if (found.length === 1) {
        shown = found[0];
        panel.title = titleOf(shown);
        gate.rewrite(current);
        write();
        return false;
      }
      picking = compareFields;
      // The tab no longer names the message either.
      panel.title = composeTabTitle("pick a message", source);
      gate.keep(current);
      refuse(ask);
      return false;
    };
    // The user picked one of the new file's messages. The index is the one the
    // refusal listed, in the file the gate holds: it has already checked that
    // the file has not changed again since, and the editor may have closed.
    const pick = (index: unknown) => {
      const current = gate.handle;
      if (!picking || !current || !isNonNegativeInt(index) || index >= current.count()) return;
      const m = current.message(index);
      if (!messageIsRenderable(m)) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: index,
          error: "message has no grid dimensions (unsupported GDS)",
          pickMessages: picking,
        } satisfies GridErrorMessage);
        return;
      }
      shown = m;
      picking = null;
      panel.title = titleOf(shown);
      gate.rewrite(current);
      write();
    };

    const paint = (options: RenderOptions, compare?: GribCompare, ask: { seq?: number } = {}) => {
      const reader = gate.handle;
      if (!reader) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: shown.index,
          error: FILE_UNREADABLE,
          ...ask,
          handleGone: true,
        } satisfies GridErrorMessage);
        return;
      }
      try {
        // A difference map combines this message (A) with the chosen message
        // (B); otherwise a plain single-field render. Both return the same
        // paint-ready RGBA, so the panel displays them identically.
        const rendered = reader.renderGrid(
          messageTarget(shown.index, compare),
          options,
        );
        panel.webview.postMessage({
          ...buildGridReadyMessage(rendered, shown, options, { kind: "message", name: source }),
          ...ask,
          // The rider this map combined with, which the probe, the contours
          // and the CSV export follow until the next map lands (#1145).
          compare: compare ?? null,
        } satisfies GridReadyMessage);
      } catch (err) {
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: shown.index,
          error: `render failed: ${errorText(err)}`,
          ...ask,
          ...rangeRefusalOf(err),
        } satisfies GridErrorMessage);
      }
    };

    // Project the requested overlay layers (coastline / graticule) onto the
    // raster for the current options and post the pixel-space runs back. The
    // forward projection runs in Rust (`projectOverlay`); this never decodes
    // values, so toggling the overlay never re-decodes the grid. `seq` is
    // echoed verbatim so the webview can drop a reply for a superseded request.
    const projectOverlay = (req: OverlayRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const layers: OverlayLayerPayload[] = [];
      const project = (name: string, geom: OverlayGeometry) => {
        const projected = reader.projectOverlay(shown.index, options, {
          latlon: geom.latlon,
          ringLengths: geom.ringLengths,
        });
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
          messageIndex: shown.index,
          seq: req.seq,
          layers,
        });
      } catch (err) {
        // Correlate the failure with the in-flight request (`seq`) so the
        // panel can resolve it and re-arm the overlay, rather than dead-ending
        // with an advanced `overlaySeq`/`lastOverlayKey` and a blank overlay.
        panel.webview.postMessage({
          type: "overlayError",
          messageIndex: shown.index,
          seq: req.seq,
          error: errorText(err),
        });
      }
    };

    // Project the field's contour isolines onto the current raster (#238).
    // Unlike the geographic overlays this decodes values, so it rides the same
    // handle as `paint`; `seq` lets the panel drop a superseded reply.
    const projectContours = (req: ContourRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const interval = resolveInterval(req.interval);
      // A difference-map contour traces the combined field, not field A (#329).
      const compare = resolveGribCompare(req);
      try {
        const c = reader.projectContours(
          messageTarget(shown.index, compare),
          options,
          { interval },
        );
        panel.webview.postMessage({
          type: "contourReady",
          messageIndex: shown.index,
          seq: req.seq,
          xy: c.xy,
          segLengths: c.segLengths,
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "contourError",
          messageIndex: shown.index,
          seq: req.seq,
          error: errorText(err),
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
      const reader = gate.handle;
      if (!reader) return;
      if (!isNonNegativeInt(req.messageIndexV)) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const spacing = isNonNegativeInt(req.spacing) && req.spacing > 0 ? req.spacing : undefined;
      try {
        const arrows = reader.projectVectors(
          { u: shown.index, v: req.messageIndexV, spacing, gridRelative: req.gridRelative === true },
          options,
        );
        // Whether the two are a u/v pair is Rust's answer (#1049); the names
        // are what the note says it about.
        const named = (i: number) => {
          const m = reader.message(i);
          return m.abbreviation ?? m.parameter ?? `#${i}`;
        };
        panel.webview.postMessage({
          type: "vectorResult",
          seq: req.seq,
          xy: arrows.xy,
          segLengths: arrows.segLengths,
          referenceSpeed: arrows.referenceSpeed,
          referenceLength: arrows.referenceLength,
          pairing: arrows.pairing,
          pairNames: [named(shown.index), named(req.messageIndexV)],
        });
      } catch (err) {
        panel.webview.postMessage({
          type: "vectorResult",
          seq: req.seq,
          error: errorText(err),
        });
      }
    };

    // The field's zonal mean (#240). A refusal travels with the reply, because
    // the reason — a rotated or projected grid has no latitude circles to average
    // along — is what the panel should say instead of an empty plot. A
    // difference map's is the combined field's, not field A's (#1208).
    const zonal = (req: { seq?: number }) => {
      const seq = req.seq;
      const reader = gate.handle;
      if (!reader) return;
      try {
        const result = reader.zonalMean(messageTarget(shown.index, resolveGribCompare(req)));
        panel.webview.postMessage({ type: "zonalResult", seq, messageIndex: shown.index, result, error: null });
      } catch (err) {
        panel.webview.postMessage({
          type: "zonalResult",
          seq,
          messageIndex: shown.index,
          result: null,
          error: errorText(err),
        });
      }
    };

    // Read the field under a clicked pixel and post the readout back (#172).
    const probe = (req: ProbeRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const options = resolveRerenderOptions(req.options ?? {});
      const px = Math.max(0, Math.floor(req.px));
      const py = Math.max(0, Math.floor(req.py));
      // A click on a difference map reads the combined field, not field A (#329).
      const compare = resolveGribCompare(req);
      try {
        const result = reader.probePixel(
          messageTarget(shown.index, compare),
          options,
          px,
          py,
        );
        panel.webview.postMessage({ type: "probeResult", seq: req.seq, messageIndex: shown.index, result });
      } catch {
        // A probe never blocks the user; swallow and report nothing.
        panel.webview.postMessage({ type: "probeResult", seq: req.seq, messageIndex: shown.index, result: null });
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
        if (!gate.admit(m as { type: string }, docHandle(), (h) => adopt(h, answeredSeq(m)))) return;
        // No message is chosen: a render is answered with the refusal again (a
        // page shown again after it was hidden asks for one), the pick is
        // served, and every other request, which would read a message nobody
        // chose, is dropped.
        if (picking && m.type !== "exportPng") {
          if (m.type === "pickMessage") pick((m as { messageIndex?: unknown }).messageIndex);
          else if (m.type === "ready" || m.type === "rerenderRequest") refuse(seqOf(m));
          return;
        }
        if (m.type === "ready") {
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), resolveGribCompare(m), seqOf(m));
          return;
        }
        if (m.type === "rerenderRequest") {
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), resolveGribCompare(m), seqOf(m));
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
          zonal(m as { seq?: number });
          return;
        }
        // Export CSV beside Export PNG (#1039): the message the panel draws,
        // which until then only the message table could export.
        if (m.type === "exportCsv") {
          // From the message's own file, which the panel holds (#1338).
          this._csvExport = this.handleExportCsv(
            document,
            shown.index,
            (m as { fieldA?: unknown }).fieldA === true,
            () => gate.handle,
          );
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
    panel.onDidDispose(() => {
      sub.dispose();
      gate.dispose();
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
    // Try the cached handle first; fall back to a transient handle so
    // callers that haven't been through `resolveCustomEditor` (e.g.
    // unit tests that drive `applyP1Edit` directly off
    // `openCustomDocument`) still work.
    let handle = this._handlesByDoc.get(document.uri.toString());
    if (!handle) {
      try {
        handle = native.open(document.bytes);
      } catch (err) {
        console.error("[Fieldglass] setP1 lazy handle init failed:", err);
        vscode.window.showErrorMessage(`Fieldglass: failed to parse GRIB1: ${errorText(err)}`);
        return;
      }
    }
    // Held while it is read, so a transient handle is closed after (#1204).
    const lease = {};
    this.leases.hold(handle, lease);
    let newBytes: Uint8Array;
    try {
      if (handle.format() !== "grib1") {
        vscode.window.showErrorMessage(
          "Fieldglass: setP1 only applies to GRIB1 documents",
        );
        return;
      }
      try {
        newBytes = handle.setP1(messageIndex, value);
      } catch (err) {
        console.error("[Fieldglass] setP1 failed:", err);
        vscode.window.showErrorMessage(`Fieldglass: failed to set p1: ${errorText(err)}`);
        // Re-broadcast the old state so the input snaps back.
        this.broadcastUpdate(document);
        return;
      }
    } finally {
      this.leases.release(handle, lease);
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
          // for every file the user has ever opened in this session. It is
          // closed now, or with the last render panel still on it (#1204),
          // which goes on drawing from it (#1338). The handle will be
          // rebuilt on the next `resolveCustomEditor`.
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
      p.webview.postMessage({ type: "update", messages, page: this._editorPages.get(p) });
    }
  }

  /** The document's bytes were read from disk again (File: Revert File,
   *  #1217). Every handle read from the old bytes is dropped, and every editor
   *  page on the document is written again from the new bytes, which opens
   *  the new handles: a GRIB file's messages, or a NetCDF file's dump and
   *  variables. Every render panel on the document is written again too, so it
   *  asks at once, and its {@link PanelHandleGate} finds the new handle and
   *  looks for its message or variable in the new file. An old handle is
   *  closed once every render panel on it has moved to the new one (#1204),
   *  and is never drawn from again: when the new bytes do not open, the
   *  panel says it has no file (#1338). */
  private reloadDocument(document: FieldglassDocument): void {
    const key = document.uri.toString();
    this._reverts.set(key, (this._reverts.get(key) ?? 0) + 1);
    this._handlesByDoc.delete(key);
    this._netcdfHandlesByDoc.delete(key);
    for (const panel of this._panelsByDoc.get(key) ?? []) this.writeEditorPage(document, panel);
    for (const [panel, doc] of this._renderPanelDocs) {
      if (doc !== key) continue;
      // The panel's handle is the old file's. If the new bytes did not open,
      // the panel has nothing to draw, not the old file (#1338).
      this._renderPanelGates.get(panel)?.outdate();
      this._renderPanelBuilders.get(panel)?.();
    }
  }

  /** Send the current document state to a single panel (used by the
   *  `ready` mount handshake). Only a GRIB document has messages to send, and
   *  only from the handle its page was written from: the page's numbers are
   *  that file's. */
  private postCurrentMessages(
    panel: vscode.WebviewPanel,
    document: FieldglassDocument,
    page: unknown,
  ): void {
    const cached = this._handlesByDoc.get(document.uri.toString());
    if (!cached) return;
    panel.webview.postMessage({ type: "update", messages: listMessages(cached), page });
  }

  /** Open the document's bytes again as GRIB, for the edit path's `update`.
   *  Bytes that are not GRIB get no GRIB handle (#1217): a NetCDF document's
   *  handle lives in `_netcdfHandlesByDoc`. */
  private reparseAndCache(document: FieldglassDocument): MessageInfo[] | undefined {
    const native = loadNative();
    if (!native) return undefined;
    const format = native.detectBytes(document.bytes);
    if (format !== "grib1" && format !== "grib2") {
      this._handlesByDoc.delete(document.uri.toString());
      return undefined;
    }
    try {
      const handle = native.open(document.bytes);
      this._handlesByDoc.set(document.uri.toString(), handle);
      return listMessages(handle);
    } catch (err) {
      vscode.window.showErrorMessage(`Fieldglass: failed to re-parse after edit: ${errorText(err)}`);
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
  ): Handle | undefined {
    const key = document.uri.toString();
    const cached = this._handlesByDoc.get(key);
    if (cached) return cached;
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle: Handle | undefined =
        format === "grib1" || format === "grib2" ? native.open(document.bytes) : undefined;
      // Dropped, and closed, with the document's last panel (`trackPanel`).
      if (handle) this._handlesByDoc.set(key, handle);
      return handle;
    } catch (err) {
      void reportProblem(
        "error",
        couldNotOpen("GRIB file", sourceName(document.uri), document.uri.fsPath, err),
      );
      return undefined;
    }
  }

  /** Get-or-build the cached NetCDF reader handle for a document. */
  private openOrReuseNetcdfHandle(
    document: FieldglassDocument,
  ): Handle | undefined {
    const key = document.uri.toString();
    const cached = this._netcdfHandlesByDoc.get(key);
    if (cached) return cached;
    const native = loadNative();
    if (!native) return undefined;
    try {
      const handle = native.open(document.bytes);
      this._netcdfHandlesByDoc.set(key, handle);
      return handle;
    } catch (err) {
      // The NetCDF reader opens HDF5 files too, and either may be the one.
      void reportProblem(
        "error",
        couldNotOpen("NetCDF or HDF5 file", sourceName(document.uri), document.uri.fsPath, err),
      );
      return undefined;
    }
  }

  /**
   * Pop a render tab for one NetCDF variable. Mirrors {@link openRenderPanel}
   * but drives the two-tier slice picker: every paint renders the chosen 2-D
   * plane via `handle.renderGrid(slice, …)`, and the picker (variable / axis / index
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
        // Re-fetched rather than captured: a revert or a reopen gives the
        // document a new handle, which the panel's gate moves to. The panel
        // draws from the handle its gate holds, which stays open after the
        // document's editor closes (#1338).
        handle: () => this._netcdfHandlesByDoc.get(document.uri.toString()),
        gone: FILE_UNREADABLE,
        exportDir: vscode.Uri.joinPath(document.uri, ".."),
        container: "NetCDF",
        noun: "file",
        source: sourceName(document.uri),
        document: document.uri,
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
    // Dropped when no panel comes of this and none is open on the store, so
    // the map does not keep it for good.
    const unused = () => {
      if (!this._panelsByStore.has(key)) this._zarrHandlesByStore.delete(key);
    };
    let panel: vscode.WebviewPanel | undefined;
    try {
      panel = this.openSliceRenderPanel(
        {
          handle: () => this._zarrHandlesByStore.get(key),
          gone: STORE_UNREADABLE,
          // Inside the store, not beside it: a store *is* a directory, so its own
          // folder is where a user expects an export to land.
          exportDir: storeUri,
          container: "Zarr",
          noun: "store",
          source: sourceName(storeUri),
        },
        handle,
        variableIndex,
      );
    } catch (err) {
      unused();
      throw err;
    }
    if (!panel) {
      unused();
      return;
    }
    // The store's handle goes with its last panel (#1204): a store has no
    // editor to keep it, and is opened again from the command.
    let panels = this._panelsByStore.get(key);
    if (!panels) this._panelsByStore.set(key, (panels = new Set()));
    panels.add(panel);
    panel.onDidDispose(() => {
      panels.delete(panel);
      if (panels.size > 0 || this._panelsByStore.get(key) !== panels) return;
      this._panelsByStore.delete(key);
      this._zarrHandlesByStore.delete(key);
    });
  }

  /** The slice panel, over whichever container answered.
   *
   * Both handles satisfy `Handle` with identical signatures, so nothing
   * below this line knows which one it has (#659). What differs is in the
   * `subject`: how to re-fetch the handle, where an export starts, and the
   * container's name.
   */
  private openSliceRenderPanel(
    subject: SlicePanelSubject,
    handle: Handle,
    variableIndex: number,
  ): vscode.WebviewPanel | undefined {
    let variables = listVariablesOf(handle);
    let initialVar =
      variables.find((v) => v.index === variableIndex) ?? variables[0];
    if (!initialVar) return undefined;

    let initial = defaultSliceSpec(initialVar);
    // The slice's own answers — its family, and whether it can be reprojected —
    // asked of the placement the handle renders from (#574), once per variable
    // and axis pair. A slice the handle cannot place at all still opens, on the
    // source view alone. Keyed by handle as well, since a document reopened
    // under the panel brings a new one.
    const grids = new WeakMap<Handle, Map<string, SliceGrid | null>>();
    const gridOf = (h: Handle, spec: SliceSpec): SliceGrid | null => {
      let known = grids.get(h);
      if (!known) grids.set(h, (known = new Map()));
      const key = `${spec.variableIndex}:${spec.yDim}:${spec.xDim}`;
      if (!known.has(key)) {
        let grid: SliceGrid | null = null;
        try {
          grid = h.sliceGrid(axesOf(spec));
        } catch (err) {
          console.error("[Fieldglass] sliceGrid failed:", err);
        }
        known.set(key, grid);
      }
      return known.get(key) ?? null;
    };
    // The axis values the export name reads each held dimension's coordinate
    // from (#1112), per handle and asked for once: every animation frame
    // renders, and a coordinate array does not change under one handle.
    const axes = new WeakMap<Handle, Map<string, AxisValues | null>>();
    const axisOf = (h: Handle, variableIndex: number) => {
      let known = axes.get(h);
      if (!known) axes.set(h, (known = new Map()));
      const cache = known;
      const read = axisReader(h, variableIndex);
      return (dim: number): AxisValues | null => {
        const key = `${variableIndex}:${dim}`;
        if (!cache.has(key)) cache.set(key, read(dim));
        return cache.get(key) ?? null;
      };
    };
    const openedGrid = gridOf(handle, initial);
    // What a rebuilt panel is written from. The slice drawn last rather than the
    // one the panel opened on: a rebuild restores the picker's saved selection,
    // and a map target the opening slice could not take would not survive it.
    let shown = { meta: sliceField(initialVar, openedGrid), grid: openedGrid };
    // The tab names the variable drawn and the file or store (#1038). It
    // follows the variable: set again by every render, from the variable the
    // render drew.
    const titleOf = (v: VariableInfo): string =>
      composeTabTitle(cleanTitlePart(v.name) || "variable", subject.source);
    const panel = vscode.window.createWebviewPanel(
      "fieldglass.render",
      titleOf(initialVar),
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
      { enableScripts: true, retainContextWhenHidden: false, localResourceRoots: [] },
    );
    let slice: SlicePanelData = { variables, initial };
    // The handle the page's variable list was read from. A changed file's
    // variables can be others, or the same ones numbered differently, so the
    // panel is written again from the new file, and the page restores its
    // selection by the variable's name (#839). Made before the first write,
    // which numbers the page from it.
    const gate = new PanelHandleGate<Handle>(handle, this.leases);
    // Said in place of drawing while the variable the page was showing is no
    // longer in the file, or the file has none left, and none has been picked
    // since. Kept until a pick, as a GRIB panel keeps `picking`: a page
    // written again (an imported colour table) or shown again after it was
    // hidden asks with its `ready`, and is answered with this rather than
    // with the new file's first variable (#1237). Declared before the first
    // write, which reads it.
    let refusal: string | null = null;
    // While a pick is awaited, the variable the panel last drew, by name. The
    // page's own slice is then the new file's first variable, which nobody
    // picked, so a further change to the file looks for this one instead, as
    // a GRIB panel goes on refusing until a pick (#1237).
    let lost: string | null = null;
    const write = () => {
      panel.webview.html = renderImagePanelHtml(
        panel.webview,
        shown.meta,
        sliceCaption(subject.container, shown.grid),
        colormapRegistry(),
        combineOpRegistry(),
        // Until a variable is picked, the page is written with no variable's
        // heading: the variable it would otherwise name is one nobody picked
        // (#1230, #1237).
        refusal !== null ? { ...slice, picking: true } : slice,
        undefined,
        undefined,
        subject.source,
        gate.page,
      );
    };
    this.trackRenderPanel(panel, write, gate, subject.document);

    // `picked`: the request is a render, which a page waiting for a pick asks
    // for only once a variable is picked, so its slice names a choice.
    const adopt = (
      current: Handle,
      asked: SliceSpec | undefined,
      ask: { seq?: number } = {},
      picked = false,
    ): void => {
      const name = refusal !== null && lost !== null && !picked
        ? lost
        : (variables.find((v) => v.index === asked?.variableIndex) ?? initialVar).name;
      lost = null;
      const next = listVariablesOf(current);
      const same = next.find((v) => v.name === name);
      const opened = same ?? next[0];
      if (!opened) {
        // Nothing is left under the old heading or tab either: the page clears
        // its picture on this error, and a page written again names no
        // variable (#1237). The gate keeps the old handle, so the next request
        // looks at the file again, but nothing is drawn from it: should the
        // document be read again into bytes that do not open, the panel has
        // no file, not the old one (#1338).
        gate.outdate();
        refusal = `The ${subject.container} ${subject.noun} no longer has a variable to draw.`;
        lost = name;
        panel.title = composeTabTitle("pick a variable", subject.source);
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: asked?.variableIndex ?? initial.variableIndex,
          error: refusal,
          ...ask,
          sliceGrid: sliceAnswer(null),
          noVariables: true,
        } satisfies GridErrorMessage);
        return;
      }
      variables = next;
      initialVar = opened;
      initial = defaultSliceSpec(opened);
      slice = { variables, initial };
      refusal = same ? null : `${name} is no longer in the ${subject.noun}, so nothing was drawn. Pick a variable to draw.`;
      lost = same ? null : name;
      const grid = gridOf(current, initial);
      shown = { meta: sliceField(opened, grid), grid };
      // The tab names no variable either while one is to be picked, as a GRIB
      // panel's names no message (#1230). The render of the picked one
      // renames it.
      panel.title = same ? titleOf(opened) : composeTabTitle("pick a variable", subject.source);
      gate.rewrite(current);
      write();
    };

    // The variable a render is actually drawing. The picker can move off the
    // one the panel opened on, and the heading and probe units have to follow
    // it; labelling every render with `initialVar` reported the opening
    // variable's units against whatever was on screen.
    const renderedVar = (spec: SliceSpec): VariableInfo =>
      variables.find((v) => v.index === spec.variableIndex) ?? initialVar;

    const paint = (requested: RenderOptions, spec: SliceSpec, compare?: NetcdfCompare, ask: { seq?: number } = {}) => {
      const reader = gate.handle;
      if (!reader) {
        // With no handle nothing is placed, so the picker and the Overlay row
        // stop offering what only a placed slice takes (#839).
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: spec.variableIndex,
          error: subject.gone,
          ...ask,
          sliceGrid: sliceAnswer(null),
          handleGone: true,
        } satisfies GridErrorMessage);
        return;
      }
      // The picker offers the map targets for the slice it last drew (#822). A
      // request for one that this slice cannot take — the picker has just moved
      // onto it — is drawn in the source view, and the answer goes back with
      // the render so the picker follows.
      const grid = gridOf(reader, spec);
      const options: RenderOptions =
        grid?.reprojectable || requested.projection === "source"
          ? requested
          : { ...requested, projection: "source" };
      try {
        // A difference map combines this slice (A) with a second slice (B) —
        // the same or another variable at its own indices; otherwise a plain
        // single-slice render. Both return the same paint-ready RGBA.
        const rendered = reader.renderGrid(sliceTarget(spec, compare), options);
        shown = { meta: sliceField(renderedVar(spec), grid), grid };
        // Every animation frame renders, and the tab is renamed only when the
        // variable changed.
        const title = titleOf(renderedVar(spec));
        if (panel.title !== title) panel.title = title;
        // The slice drawn, in the PNG name as in the subtitle (#1112).
        const held = heldSliceParts(
          renderedVar(spec).dims.map((d) => d.name),
          spec,
          axisOf(reader, spec.variableIndex),
        );
        panel.webview.postMessage({
          ...ask,
          ...buildGridReadyMessage(
            rendered,
            sliceTitle(renderedVar(spec), spec.variableIndex),
            options,
            { kind: "variable", name: subject.source },
            held ?? [],
          ),
          sliceGrid: sliceAnswer(grid),
          // The slice drawn, which the subtitle names (#1041), and Field B's
          // indices when it is a difference of two slices of this variable.
          slice: spec,
          compareSliceIndicesB:
            compare && compare.variableIndexB === spec.variableIndex ? compare.sliceIndicesB : null,
          // The rider this map combined with, as for a GRIB panel (#1145).
          compare: compare ?? null,
        } satisfies GridReadyMessage);
      } catch (err) {
        // The answers go with the error too. The picker has moved onto this
        // slice, and a failed render must not leave it, or the Overlay row, on
        // the previous slice's answer (#839).
        panel.webview.postMessage({
          type: "gridError",
          messageIndex: spec.variableIndex,
          error: `render failed: ${errorText(err)}`,
          ...ask,
          sliceGrid: sliceAnswer(grid),
          ...rangeRefusalOf(err),
        } satisfies GridErrorMessage);
      }
    };

    const projectOverlay = (req: OverlayRequest & { slice?: SliceSpec }) => {
      const reader = gate.handle;
      if (!reader) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const layers: OverlayLayerPayload[] = [];
      const project = (name: string, geom: OverlayGeometry) => {
        const projected = reader.projectOverlay(sliceOf(spec), options, {
          latlon: geom.latlon,
          ringLengths: geom.ringLengths,
        });
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
          error: errorText(err),
        });
      }
    };

    // Contour isolines for the current slice, projected onto its raster (#238).
    const projectContours = (req: ContourRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const interval = resolveInterval(req.interval);
      // A difference-map contour traces the combined field, not slice A (#329).
      const compare = resolveNetcdfCompare(req);
      try {
        const c = reader.projectContours(sliceTarget(spec, compare), options, { interval });
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
          error: errorText(err),
        });
      }
    };

    // The slice's zonal mean (#240), with the refusal passed through as on the
    // GRIB panel. A difference map's is the combined field's (#1208).
    const zonal = (req: { slice?: SliceSpec; seq?: number }) => {
      const reader = gate.handle;
      if (!reader) return;
      const spec = req.slice ?? initial;
      try {
        const result = reader.zonalMean(sliceTarget(spec, resolveNetcdfCompare(req)));
        panel.webview.postMessage({ type: "zonalResult", seq: req.seq, messageIndex: spec.variableIndex, result, error: null });
      } catch (err) {
        panel.webview.postMessage({
          type: "zonalResult",
          seq: req.seq,
          messageIndex: spec.variableIndex,
          result: null,
          error: errorText(err),
        });
      }
    };

    // The line through the probed cell, along the axis the panel picked (#172).
    // A failure answers with no line rather than an error toast: the plot is
    // a companion to the readout, and a variable with no axis to read along is
    // an ordinary state, not something the user did wrong.
    const line = (req: LineRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const spec = req.slice ?? initial;
      const indices = lineIndices(spec, req.gridI, req.gridJ);
      if (!indices || !isNonNegativeInt(req.alongDim)) {
        panel.webview.postMessage({ type: "lineResult", seq: req.seq, messageIndex: spec.variableIndex, result: null });
        return;
      }
      const a = { variable: spec.variableIndex, alongDim: req.alongDim, sliceIndices: indices };
      // On a difference map, the line through the combined field (#1208):
      // Field B's slice with the same cell written in. Along an axis of the
      // map the line is a row or column of it; along a held axis Field B stays
      // where it is drawn, as it does while Field A steps (the owner's
      // decision on #1208).
      const compare = resolveNetcdfCompare(req);
      const indicesB = compare
        ? lineIndices({ ...spec, variableIndex: compare.variableIndexB, sliceIndices: compare.sliceIndicesB }, req.gridI, req.gridJ)
        : null;
      if (compare && !indicesB) {
        panel.webview.postMessage({ type: "lineResult", seq: req.seq, messageIndex: spec.variableIndex, result: null });
        return;
      }
      try {
        const result = compare && indicesB
          ? reader.decodeLineCombined({
            a,
            b: { variable: compare.variableIndexB, alongDim: req.alongDim, sliceIndices: indicesB },
            op: compare.op,
            holdB: req.alongDim !== spec.yDim && req.alongDim !== spec.xDim,
          })
          : reader.decodeLine(a);
        panel.webview.postMessage({ type: "lineResult", seq: req.seq, messageIndex: spec.variableIndex, result });
      } catch {
        panel.webview.postMessage({ type: "lineResult", seq: req.seq, messageIndex: spec.variableIndex, result: null });
      }
    };

    // The coordinate values a cross-section labels an axis from (#171). Like
    // the line above, a failure answers with no values rather than an error:
    // an axis with no coordinate array is an ordinary state, and the panel
    // falls back to drawing indices.
    //
    // With the file closed (#839) it still answers, saying so: a panel holds a
    // cross-section's render for its Y axis (#1043), and the render is what
    // tells the user the file is gone.
    const axis = (req: { slice?: SliceSpec; dim?: unknown }) => {
      const reader = gate.handle;
      const spec = req.slice ?? initial;
      // The answer names the variable it is for, so a page that has moved to
      // another variable meanwhile files it under the right one (#1041).
      const variableIndex = spec.variableIndex;
      if (!reader) {
        panel.webview.postMessage({ type: "axisResult", dim: req.dim, variableIndex, result: null, handleGone: true });
        return;
      }
      const reply = (result: AxisValues | null) =>
        panel.webview.postMessage({ type: "axisResult", dim: req.dim, variableIndex, result });
      if (!isNonNegativeInt(req.dim)) {
        reply(null);
        return;
      }
      try {
        reply(reader.axisValues(spec.variableIndex, req.dim));
      } catch {
        reply(null);
      }
    };

    // Point-probe readout for the current slice (#172).
    const probe = (req: ProbeRequest) => {
      const reader = gate.handle;
      if (!reader) return;
      const spec = req.slice ?? initial;
      const options = resolveRerenderOptions(req.options ?? {});
      const px = Math.max(0, Math.floor(req.px));
      const py = Math.max(0, Math.floor(req.py));
      // A click on a difference map reads the combined field, not slice A (#329).
      const compare = resolveNetcdfCompare(req);
      try {
        const result = reader.probePixel(sliceTarget(spec, compare), options, px, py);
        panel.webview.postMessage({ type: "probeResult", seq: req.seq, messageIndex: spec.variableIndex, result });
      } catch {
        panel.webview.postMessage({ type: "probeResult", seq: req.seq, messageIndex: spec.variableIndex, result: null });
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
        const served = gate.admit(m as { type: string }, current, (h) => {
          adopt(h, (m as { slice?: SliceSpec }).slice, answeredSeq(m), m.type === "rerenderRequest");
          return false;
        });
        if (!served) return;
        if (m.type === "ready" && refusal !== null) {
          const spec = (m as { slice?: SliceSpec }).slice ?? initial;
          // Read from the file the panel holds, which stays open after its
          // editor closes (#1338).
          const reader = gate.handle;
          panel.webview.postMessage({
            type: "gridError",
            messageIndex: spec.variableIndex,
            error: reader ? refusal : subject.gone,
            ...seqOf(m),
            sliceGrid: sliceAnswer(reader ? gridOf(reader, spec) : null),
            pickVariable: true,
            ...(reader ? {} : { handleGone: true }),
          } satisfies GridErrorMessage);
          return;
        }
        // The page asks for no render while it waits for a pick, so the next
        // one it asks for draws the variable picked (#1237).
        if (m.type === "rerenderRequest") {
          refusal = null;
          lost = null;
        }
        // `ready` carries the webview's (state-restored) selections and slice,
        // exactly like a rerenderRequest, so a remount repaints what the user
        // had; a fresh panel sends its defaults.
        if (m.type === "ready" || m.type === "rerenderRequest") {
          const spec = (m as { slice?: SliceSpec }).slice ?? initial;
          paint(resolveRerenderOptions(m as Partial<RenderOptions>), spec, resolveNetcdfCompare(m), seqOf(m));
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
          zonal(m as { slice?: SliceSpec; seq?: number });
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
            // From the slice's own file, which the panel holds (#1338).
            this._csvExport = this.handleExportSliceCsv(
              { ...subject, handle: () => gate.handle },
              spec,
              (m as { fieldA?: unknown }).fieldA === true,
            );
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
    panel.onDidDispose(() => {
      sub.dispose();
      gate.dispose();
    });
    return panel;
  }
}

/** Which reader handle a render panel's page was written from, and whether a
 *  request from that page may be served (#839, #959).
 *
 *  A render panel outlives the editor it was opened from. A document closed
 *  and opened again brings a new handle, on a file that may have changed, and
 *  every request the page makes names its field by a number in the file the
 *  page was written from. So the first request after the handle changes is not
 *  served; the panel decides what the new file holds for it (`adopt`). When it
 *  writes the page again (`rewrite`), anything the old page sent before it was
 *  replaced is still on its way with the old numbers, its `ready` included, so
 *  every page is written with a number of its own ({@link PanelHandleGate.page})
 *  and echoes it on every message. A message carrying another page's number is
 *  dropped (#1202), and the new page's other requests are dropped until its
 *  `ready`. Slice panels and GRIB panels both go through this, so the two
 *  cannot drift apart. */
export class PanelHandleGate<H> {
  private awaitingReady = false;
  private written = 0;
  // Set once the page's handle no longer holds what the document shows: the
  // document was read again (File: Revert File), or its new file has nothing
  // the page can draw. Cleared when the panel adopts a handle.
  private outdated = false;

  /** `leases`, when given, is held on the page's handle until the panel adopts
   *  another or is disposed (#1204): a revert or a reopen replaces the
   *  document's handle, and the old one stays open until the panel moves off
   *  it. */
  constructor(
    private pageHandle: H | undefined,
    private readonly leases?: { hold(handle: H, holder: object): void; release(handle: H | undefined, holder: object): void },
  ) {
    if (pageHandle) leases?.hold(pageHandle, this);
  }

  /** The handle to serve the page's requests from (#1338). After
   *  {@link admit} has let a request through, it is the document's handle
   *  when the document has one, and the one the panel kept when the
   *  document's editor has closed. It is `undefined` when the panel has no
   *  file it may draw: none was open when it opened, or the document was
   *  read again into bytes that did not open (see {@link outdate}). */
  get handle(): H | undefined {
    return this.outdated ? undefined : this.pageHandle;
  }

  /** The page's handle no longer holds what the document shows: the document
   *  was read again (File: Revert File), or its new file has nothing the page
   *  can draw. The handle stays held, so a panel that moves to the document's
   *  new handle on its next request lets go of it then (#1204). Until it
   *  moves, nothing is drawn from it: should the document have no handle at
   *  that request (the new bytes did not open), the
   *  panel says it has no file and lets go of the old one (#1338). */
  outdate(): void {
    if (this.pageHandle) this.outdated = true;
  }

  /** The number to write the page with, which it sends back on every message.
   *  A page written again for the same file (an imported colormap, say) keeps
   *  it: its numbers mean what the last page's did. */
  get page(): number {
    return this.written;
  }

  /** Whether to serve `m`, a request of `m.type` from the page numbered
   *  `m.page`, with `current` the document's handle now. When the handle has
   *  changed, `adopt` runs instead and says whether the request is served
   *  after all. */
  admit(m: { type: string; page?: unknown }, current: H | undefined, adopt: (current: H) => boolean): boolean {
    // A PNG export reads the page's own pixels, not the handle.
    if (m.type === "exportPng") return true;
    // From a page that has been replaced since. Its `ready` is not the new
    // page's: the old page can post one after the panel started writing the
    // new one, and it names the field by the old file's numbers.
    if (m.page !== this.written) return false;
    if (this.awaitingReady) {
      if (m.type !== "ready") return false;
      this.awaitingReady = false;
    }
    // Checked for the new page's ready as well: the handle can have changed
    // again while that page loaded, and its numbers are then the previous
    // file's.
    if (current && current !== this.pageHandle) return adopt(current);
    // Outdated with nothing to move to: the old file is not drawn as the
    // document's, and is no longer kept for it.
    if (this.outdated && !current) this.dispose();
    return true;
  }

  /** The page is being written again from `handle`'s file. */
  rewrite(handle: H): void {
    this.adopt(handle);
    this.written += 1;
    this.awaitingReady = true;
  }

  /** The page stays as it is, and its numbers are read from `handle`'s file
   *  from now on. */
  keep(handle: H): void {
    this.adopt(handle);
  }

  /** The panel closed: let go of its handle. */
  dispose(): void {
    const old = this.pageHandle;
    this.pageHandle = undefined;
    this.outdated = false;
    this.leases?.release(old, this);
  }

  private adopt(handle: H): void {
    const old = this.pageHandle;
    this.pageHandle = handle;
    this.outdated = false;
    if (old === handle) return;
    this.leases?.hold(handle, this);
    this.leases?.release(old, this);
  }
}

/** A GRIB message's identity as one string, for comparing two (#959). The
 *  identity is Rust's (`MessageInfo.identity`), so no code table is rebuilt
 *  here; both sides of a comparison come from the same addon, which writes the
 *  keys in one order. */
export function messageKey(m: Pick<MessageInfo, "identity">): string {
  return JSON.stringify(m.identity);
}

/** Whether two listings of a GRIB file hold the same messages in the same
 *  places, so an index into one means the same message in the other. */
function sameMessages(
  a: ReadonlyArray<Pick<MessageInfo, "identity" | "offsetBytes" | "totalLengthBytes">>,
  b: ReadonlyArray<Pick<MessageInfo, "identity" | "offsetBytes" | "totalLengthBytes">>,
): boolean {
  const others = b.values();
  return a.length === b.length && a.every((m) => {
    const n = others.next().value;
    return n !== undefined &&
      m.offsetBytes === n.offsetBytes &&
      m.totalLengthBytes === n.totalLengthBytes &&
      messageKey(m) === messageKey(n);
  });
}

/** What a panel says when it has no file to draw from: its document was read
 *  again (File: Revert File) into bytes that did not open, and the old file is
 *  not drawn as the document's (#1338, #1350). */
export const FILE_UNREADABLE = "This file can no longer be read.";
/** The same for a Zarr store. */
export const STORE_UNREADABLE = "This store can no longer be read.";

/** Said in place of drawing when a GRIB panel's message is not exactly once in
 *  its changed file (#959). */
export const MESSAGE_GONE = "This message is no longer in the file (or is ambiguous); pick one.";

// ---------------------------------------------------------------------------
// Render-panel wire payload
// ---------------------------------------------------------------------------

export interface GridReadyMessage {
  type: "gridReady";
  /** The `seq` of the render request this answers, when it had one. */
  seq?: number;
  messageIndex: number;
  rgba: Uint8Array;
  width: number;
  height: number;
  /** `null` when no cell had a value to take a range from (#871). */
  usedMin: number | null;
  usedMax: number | null;
  /** The whole range result (#1155): which ends were given and how many
   *  cells fell outside the range. */
  range: ColourRange;
  /** Equirectangular extent actually rendered, echoed so the panel can
   *  pre-fill the manual-bounds inputs. `null` for source projection. */
  usedLatMin: number | null;
  usedLatMax: number | null;
  usedLonMin: number | null;
  usedLonMax: number | null;
  projectionSummary: string;
  /** The options this render was drawn with, after the provider's own
   *  fallback to the source view. The panel reads the image's geometry from
   *  it (#1114): `flipY` for which way up the rows are, and `projection`,
   *  `projectionPreset`, `centerLat`, `centerLon` and the `bounds*` edges for
   *  where its pixels fall. The rails, the probe, the overlays and the zoom
   *  and pan gestures follow the image drawn, not the controls, which a
   *  render still on its way will follow. */
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
   *  move onto a variable or axis pair with a different answer.
   *  `placed` is {@link offersOverlays} for it: whether the overlays,
   *  contours and arrows apply (#840). `supports` is which point operations
   *  it takes (#945). `unavailable` is {@link unavailableControls} for it:
   *  the controls the panel hides for it, and why, which the "Not available
   *  for this grid" line says (#1036). Absent for a GRIB panel, which draws
   *  one field. */
  sliceGrid?: SliceAnswer;
  /** A slice panel's slice actually drawn, which the subtitle names by its
   *  coordinates (#1041). The rails, the probe, the line plot, the zonal
   *  average, the overlays and the CSV export read it too (#1145), not the
   *  pickers. Absent for a GRIB panel. */
  slice?: SliceSpec;
  /** Field B's indices when the map drawn is a difference of two slices of
   *  the same variable, `null` when it is not (#1041). Absent for a GRIB
   *  panel. */
  compareSliceIndicesB?: number[] | null;
  /** The Compare rider the map drawn combined with, as the provider took it
   *  (after its own fallback to a single field), or `null` for a single
   *  field (#1145). The probe, the contours and the CSV export follow it, not
   *  the Compare controls, which a render still on its way will follow.
   *  Absent only from a message built outside a render panel. */
  compare?: GribCompare | NetcdfCompare | null;
}

/** A slice's answers as the render panel takes them; see
 *  {@link GridReadyMessage.sliceGrid}. */
export interface SliceAnswer {
  label: string | null;
  reprojectable: boolean;
  placed: boolean;
  supports: GridSupport;
  unavailable: UnavailableGroup[];
}

/** The answers for a slice from its `SliceGrid`, or for one the handle could
 *  not place (`null`): no family, the source view alone, no overlays, and no
 *  point operation. A slice panel has no Vectors row and does export a CSV. */
export function sliceAnswer(grid: SliceGrid | null): SliceAnswer {
  const label = grid?.label ?? null;
  const reprojectable = grid?.reprojectable ?? false;
  const supports = grid?.supports ?? NO_SUPPORT;
  return {
    label,
    reprojectable,
    placed: offersOverlays(grid?.placement),
    supports,
    unavailable: unavailableControls(
      { reprojectable, placement: grid?.placement, label, supports },
      { arrows: false, csv: true },
    ),
  };
}

/** The `seq` to echo in a render answer to `m`: its own for a `ready` or
 *  `rerenderRequest`, nothing for any other request (an overlay's or a probe's
 *  seq belongs to its own reply, not to a render answer). */
function answeredSeq(m: { type?: string }): { seq?: number } {
  return m.type === "ready" || m.type === "rerenderRequest" ? seqOf(m) : {};
}

/** A request's `seq`, when it sent a number, to echo in the answer. */
function seqOf(m: unknown): { seq?: number } {
  const seq = (m as { seq?: unknown } | null)?.seq;
  return typeof seq === "number" ? { seq } : {};
}

/** `gridError`: a render the provider could not draw. */
export interface GridErrorMessage {
  type: "gridError";
  /** The `seq` of the render request this answers. Absent when the host
   *  refuses on its own. */
  seq?: number;
  messageIndex: number;
  error: string;
  /** A slice panel's answers for the slice it was asked to draw (#839). The
   *  picker and the Overlay row follow the slice the user picked whether or
   *  not it drew, so a failed render does not leave them on the previous
   *  slice's answer. Absent for a GRIB panel. */
  sliceGrid?: SliceAnswer;
  /** The panel has no handle, because its document was read again into bytes
   *  that did not open (File: Revert File). Closing the editor does not set
   *  this: the panel keeps its handle then (#1338). The answer is about the
   *  handle, not the slice, so the panel withdraws the map targets without
   *  saving that as the user's choice (#839). */
  handleGone?: boolean;
  /** The variable the panel was showing is gone from a file that changed, and
   *  nothing was drawn: the panel asks for a variable to be picked (#839). */
  pickVariable?: boolean;
  /** A changed file has no variable left to draw, and the page is not written
   *  again: a colour range refused of a variable read from the old file no
   *  longer stands (#1194). */
  noVariables?: boolean;
  /** A GRIB panel's message is not exactly once in its changed file, and
   *  nothing was drawn: these are the new file's messages, to pick one from
   *  (#959). */
  pickMessages?: CompareFieldOption[];
  /** The colour range Rust refused, as fields (#1155): the panel shows
   *  `rangeMessage` at the min/max field `rangeRefusal.end` names. Absent for
   *  every other failure. */
  rangeRefusal?: RangeRefusal;
  /** Rust's own words for `rangeRefusal`, without the "render failed" prefix. */
  rangeMessage?: string;
}

/** The refused colour range a render threw, as the addon puts it on the error
 *  (`code` and `rangeRefusal`, #1155), ready to spread into a `gridError`.
 *  Empty for any other failure, so the panel never matches on the wording. */
export function rangeRefusalOf(
  err: unknown,
): Pick<GridErrorMessage, "rangeRefusal" | "rangeMessage"> {
  if (typeof err !== "object" || err === null) return {};
  const { rangeRefusal, message } = err as { rangeRefusal?: unknown; message?: unknown };
  if (typeof rangeRefusal !== "object" || rangeRefusal === null) return {};
  return {
    rangeRefusal: rangeRefusal as RangeRefusal,
    rangeMessage: typeof message === "string" ? message : undefined,
  };
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
  /** Echoed in `probeResult`, so the page can drop a reply for an older click or render. */
  seq?: number;
  px: number;
  py: number;
  options?: Partial<RenderOptions>;
  slice?: SliceSpec;
}

/** `lineRequest` posted by the render panel once a probe has resolved a cell
 *  (#172): plot the variable through that cell along `alongDim`. A Compare
 *  map's carries its `compare` rider, read by {@link resolveNetcdfCompare}
 *  (#1208). */
export interface LineRequest {
  type: "lineRequest";
  /** Echoed in `lineResult` (and `zonalResult`), as `ProbeRequest.seq`. */
  seq?: number;
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
 * napi hands back `rendered.rgba` as a `Uint8Array` since #1073; it was a
 * Node `Buffer`, whose `constructor.name === "Buffer"`. VS Code's webview
 * serializer (`extHostWebviewMessaging.ts::getTypedArrayType`) only
 * recognises the standard TypedArray constructor names (`Uint8Array`,
 * `Float64Array`, …) and silently falls back to default JSON for anything
 * else — `Buffer.prototype.toJSON` emits `{type:"Buffer", data:[…]}`, which
 * the webview script then fails to blit (`new ImageData` throws on length 0).
 *
 * The payload is still wrapped as a plain `Uint8Array` view, so a caller that
 * hands in a `Buffer` (a test, a future host) cannot reintroduce that. Pinned
 * by `render.test.ts`.
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
  // What the panel draws from: a GRIB message or a variable, and the file or
  // store's name, for the heading and the PNG name (#1038).
  source: PanelSource,
  // The dimensions a slice holds, which the PNG name states (#1112); none
  // for a GRIB message.
  slice: ReadonlyArray<SliceNotePart> = [],
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
    range: rendered.range,
    usedLatMin: rendered.usedLatMin,
    usedLatMax: rendered.usedLatMax,
    usedLonMin: rendered.usedLonMin,
    usedLonMax: rendered.usedLonMax,
    projectionSummary: rendered.projectionSummary,
    options,
    titleLine: composeTitleLine(meta, source.kind),
    parameterUnits: meta.units ?? "",
    defaultPngName: composeDefaultPngName(meta, source, slice),
    truncationNote: composeTruncationNote(rendered),
  };
}

/** Reads one variable's axis values for {@link heldSliceParts}: `null` for an
 *  axis the handle cannot give values for, which is then named by index. */
function axisReader(handle: Handle, variableIndex: number): (dim: number) => AxisValues | null {
  return (dim) => {
    try {
      return handle.axisValues(variableIndex, dim);
    } catch {
      return null;
    }
  };
}

/** The default export name of a slice, PNG or CSV (#1039, #1112): the
 *  variable, then each dimension the slice holds by its coordinate, as
 *  {@link composeExportName} writes it. `variable` is `undefined` when the
 *  handle no longer lists it, and the name then carries the index. */
function sliceExportName(
  variable: Pick<VariableInfo, "name" | "dims"> | undefined,
  spec: SliceSpec,
  source: string,
  ext: ExportExtension,
  axisOf: (dim: number) => AxisValues | null,
): string {
  const parts = variable ? heldSliceParts(variable.dims.map((d) => d.name), spec, axisOf) : null;
  return composeExportName(
    { index: spec.variableIndex, parameter: variable?.name ?? null },
    { kind: "variable", name: source },
    ext,
    parts ?? [],
  );
}

/** An export that was waiting on a dialog when File: Revert File replaced
 *  the file it was asked of (#1219). The number it was asked at, and the
 *  layout it was offered, belong to the old file, so nothing is written. */
function refuseChangedExport(): void {
  void vscode.window.showErrorMessage(
    "Fieldglass: the file changed while the export was waiting. Export again."
  );
}

/** The file a save dialog's answer names, with `.<ext>` added when the name
 *  typed lacks it (#1039). On Linux the dialog saves a name as typed, so
 *  `out` was written with no extension. The name it gains can be a file the
 *  dialog never asked about replacing, so that is asked here. `undefined`
 *  when the dialog was dismissed, or the user keeps the file. */
async function saveDestination(
  dest: vscode.Uri | undefined,
  ext: ExportExtension,
): Promise<vscode.Uri | undefined> {
  if (!dest) return undefined;
  if (dest.path.toLowerCase().endsWith(`.${ext}`)) return dest;
  const named = dest.with({ path: `${dest.path}.${ext}` });
  try {
    await vscode.workspace.fs.stat(named);
  } catch {
    return named;
  }
  const replace = await vscode.window.showWarningMessage(
    `${path.posix.basename(named.path)} already exists. Replace it?`,
    { modal: true },
    "Replace",
  );
  return replace === "Replace" ? named : undefined;
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
function defaultSliceSpec(v: VariableInfo): SliceSpec {
  const yDim = v.detectedYDim ?? 0;
  let xDim = v.detectedXDim ?? (yDim === 0 ? 1 : 0);
  if (xDim === yDim) xDim = yDim === 0 ? 1 : 0;
  return {
    variableIndex: v.index,
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
  v: VariableInfo,
  grid: SliceGrid | null,
  index = v.index,
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
    supports: grid?.supports ?? NO_SUPPORT,
    // A slice is never a band-limited spectral field (#637).
    truncation: null,
    grid: grid ? { label: grid.label } : null,
  };
}

/** The heading a render of this variable is labelled with. */
function sliceTitle(
  v: VariableInfo,
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

/** The render panel's projection caption: the grid label and size, and the
 *  corner pair where the message has one. Exported for the tests, which hold
 *  it to Rust's answer for a grid nothing places (#1011). */
export function describeProjection(meta: MessageInfo): string {
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
  netcdfVariables?: VariableInfo[],
): string {
  // Long attribute strings are common in CF-Convention NetCDF files, and a
  // `history` can run to thousands of characters. A row shows the first
  // ATTR_PREVIEW_LIMIT of one, and the value opens to its whole text (#1232).
  // It used to be in the cell's title only, and the native tooltip stops near
  // 1,900 characters. A <details> needs no script, opens on a click or on
  // Enter or Space at its focused summary, and leaves the text selectable to
  // copy. A short value is plain text, as it always was.
  const ATTR_PREVIEW_LIMIT = 120;
  const isLong = (s: string): boolean => s.length > ATTR_PREVIEW_LIMIT;
  const previewAttr = (s: string): string => {
    if (!isLong(s)) return escapeHtml(s);
    return `<details class="long-value"><summary>`
      + `<span class="long-value-preview">${escapeHtml(s.slice(0, ATTR_PREVIEW_LIMIT))}…</span>`
      + `<span class="long-value-less">Show less</span></summary>`
      + `<div class="long-value-full">${escapeHtml(s)}</div></details>`;
  };
  // The title a value's cell carries: the value itself, as before, for a short
  // one. A long one has none, which would only repeat a cut-off copy of what
  // opening the cell shows.
  const titleAttr = (s: string): string => (isLong(s) ? "" : ` title="${escapeHtml(s)}"`);
  // How many of a variable's attributes its row lists before the rest go
  // behind "+N more", which opens to them.
  const ATTRS_PREVIEWED = 3;

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
          `<button type="button" class="netcdf-render-btn" data-variable-index="${v.index}">Render ${escapeHtml(v.name)}</button>`,
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
        <td${titleAttr(a.value)}>${previewAttr(a.value)}</td>
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
      const attr = (a: { name: string; value: string }) => `${escapeHtml(a.name)}=${previewAttr(a.value)}`;
      const rest = v.attributes.slice(ATTRS_PREVIEWED);
      const attrPreview = v.attributes.length === 0
        ? "—"
        : v.attributes.slice(0, ATTRS_PREVIEWED).map(attr).join("; ") + (rest.length > 0
          ? `; <details class="more-attrs"><summary>+${rest.length} more</summary>`
            + rest.map((a) => `<div>${attr(a)}</div>`).join("") + `</details>`
          : "");
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
        <td${titleAttr(v.reason)}>${previewAttr(v.reason)}</td>
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

/** The viewer's line when a file lists nothing: in GRIB's words for a GRIB
 *  file, and in no format's for any other (#1053). */
export function nothingListed(format: string): string {
  return format === "grib1" || format === "grib2" ? "No messages found." : "Nothing to list in this file.";
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
  netcdfVariables?: VariableInfo[],
  // The number the provider wrote this page under, which the page sends back
  // on every message (#1217).
  page = 0,
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
             Opens the rendered grid in a new editor tab, in its own grid
             coordinates or on a map projection where the grid allows one;
             bitmap-masked points render as transparent. Export CSV writes the
             decoded field to a file.
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
    // "Messages" is GRIB's word. A NetCDF file whose metadata could not be
    // read, or an empty file, has none of them to be missing (#1053).
    bodyContent = `<div class="status">${escapeHtml(nothingListed(format))}</div>`;
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
        // Every message carries the page's number. A revert writes the page
        // again, and what the old page sent names rows of the old file, so the
        // provider drops it (#1217).
        const PAGE = ${JSON.stringify(page)};
        function post(m) { vscode.postMessage(Object.assign({}, m, { page: PAGE })); }
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
              post({ type: 'decodeGrid', messageIndex: idx });
            });
          });
          document.querySelectorAll('button.export-csv-btn').forEach((el) => {
            el.addEventListener('click', (ev) => {
              ev.stopPropagation();
              const idx = Number(el.getAttribute('data-message-index'));
              if (!Number.isFinite(idx)) return;
              post({ type: 'exportCsv', messageIndex: idx });
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
              post({ type: 'renderVariable', variableIndex: idx });
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
                post({ type: 'edit-p1', messageIndex: idx, value: v });
              });
            });
          }
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          // An answer to a request an earlier page sent (#1219): its row and
          // variable numbers are the old file's.
          if (msg.page !== PAGE) return;
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
        post({ type: 'ready' });
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
    /* An author display rule beats the user-agent [hidden] rule, so any element
       given a display and toggled with hidden showed anyway (#1035). One rule
       here means no element needs its own override. */
    [hidden] { display: none !important; }
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
    /* A long attribute value, and a variable's attributes past the first
       three, open in place (#1232). Closed, the cell is the one-line preview
       it was; open, the whole value wraps at a readable width, inside the
       table's own sideways scroll. */
    details.long-value, details.more-attrs { display: inline-block; vertical-align: top; }
    details.long-value > summary, details.more-attrs > summary { cursor: pointer; }
    details.long-value > summary:focus-visible, details.more-attrs > summary:focus-visible {
      outline: 1px solid var(--vscode-focusBorder);
    }
    details.long-value[open] .long-value-preview,
    details.long-value:not([open]) .long-value-less { display: none; }
    .long-value-less { color: var(--vscode-textLink-foreground); }
    .long-value-full { white-space: pre-wrap; overflow-wrap: anywhere; max-width: 80ch; margin-top: 0.25rem; }
    details.more-attrs > div { margin-top: 0.15rem; }
    /* An open value makes its row tall; keep the row's name beside its top. */
    tr:has(details[open]) > td { vertical-align: top; }
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
    /* Every button on the dump page takes the theme's secondary button
       colours (#1118): Render, Export CSV and the NetCDF render buttons. */
    button {
      background: var(--vscode-button-secondaryBackground, var(--vscode-button-background));
      color: var(--vscode-button-secondaryForeground, var(--vscode-button-foreground));
      border: 1px solid var(--vscode-button-border, var(--vscode-contrastBorder, transparent));
      padding: 0.15rem 0.6rem;
      cursor: pointer;
      font-family: inherit;
      font-size: inherit;
      border-radius: 2px;
    }
    button:hover:not(:disabled) {
      background: var(--vscode-button-secondaryHoverBackground, var(--vscode-button-hoverBackground));
    }
    button:focus-visible {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: 1px;
    }
    button:disabled { opacity: 0.4; cursor: default; }
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

/** How many folders {@link childStores} looks at. Each look is a few
 *  synchronous file checks, so a folder of thousands (a home directory, a
 *  data archive) is cut off here rather than stalling the window. */
export const MAX_FOLDERS_EXAMINED = 200;

/** The folders directly inside `folder` that are Zarr stores, by name, sorted.
 *
 *  One level only: a folder of stores is the mistake this answers, and a deeper
 *  walk would be slow on a large tree for little more help. Only the first
 *  {@link MAX_FOLDERS_EXAMINED} folders by name are looked at, and `partial`
 *  says when there were more. A folder that cannot be listed has none to offer.
 *  Hidden folders are skipped. */
async function childStores(
  folder: vscode.Uri,
  isStore: (fsPath: string) => boolean,
): Promise<{ stores: string[]; partial: boolean }> {
  let entries: [string, vscode.FileType][];
  try {
    entries = await vscode.workspace.fs.readDirectory(folder);
  } catch {
    return { stores: [], partial: false };
  }
  const folders = entries
    .filter(([n, type]) => (type & vscode.FileType.Directory) !== 0 && !n.startsWith("."))
    .map(([n]) => n)
    .sort();
  return {
    stores: folders
      .slice(0, MAX_FOLDERS_EXAMINED)
      .filter((n) => isStore(vscode.Uri.joinPath(folder, n).fsPath)),
    partial: folders.length > MAX_FOLDERS_EXAMINED,
  };
}
