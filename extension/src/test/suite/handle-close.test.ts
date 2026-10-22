// A native handle is closed when the last editor and render panel on it close
// (#1204).
//
// A handle keeps its file's bytes, its decoded-value memos and a warp plan of
// up to 256 MiB (#1233) in native memory, which V8 does not count. The
// extension used to drop a closed file's handle and leave the rest to the
// garbage collector, which can keep that memory for as long as it likes. Each
// file test works on a copy of its fixture, so no other test's editor shares
// the handle. The Zarr test opens the committed store, and no other suite
// leaves a panel open on it.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { type Handle } from "../../native";
import { FILE_UNREADABLE, type FieldglassEditorProvider } from "../../provider";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  pageNumberOf,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

type Maps = Record<"_handlesByDoc" | "_netcdfHandlesByDoc" | "_zarrHandlesByStore", Map<string, Handle>>;

function maps(p: FieldglassEditorProvider): Maps {
  return p as unknown as Maps;
}

/** Whether `handle` has been closed: every call on a closed handle throws. */
function isClosed(handle: Handle): boolean {
  try {
    handle.format();
    return false;
  } catch (err) {
    assert.match(String(err), /closed/, "a closed handle says so");
    return true;
  }
}

/** Wait for `handle` to close; a panel's dispose event can land a tick later. */
async function closes(handle: Handle, what: string): Promise<void> {
  await waitUntil(() => (isClosed(handle) ? true : undefined), what, 5_000);
}

/** A copy of `source` in a directory of its own, removed by `done`. */
function copyOf(source: string): { file: string; key: string; done: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1204-"));
  const file = path.join(dir, path.basename(source));
  fs.copyFileSync(source, file);
  return { file, key: vscode.Uri.file(file).toString(), done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

const GRIB = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const NETCDF = () =>
  path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "ersst_v5_187001_cdf1.nc");
const STORE = () => path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores", "cf_v3");

const isReply = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** Ask `panel` for a render, a probe and contours, as a user would, and check
 *  each is answered (#1338): a panel that holds a handle open draws from it. */
async function serves(panel: RealPanel, what: string): Promise<void> {
  let from = panel.sent.length;
  await panel.drive([{ id: "flip-y", checked: true }]);
  const drawn = await panel.waitSent(isReply, from, `the render ${what}`);
  assert.strictEqual(drawn.type, "gridReady", `the render ${what}: ${drawn.error ?? ""}`);
  from = panel.sent.length;
  await panel.probe();
  const probed = await panel.waitSent(isType("probeResult"), from, `the probe ${what}`);
  assert.ok(probed.result, `the probe ${what} reads the cell`);
  from = panel.sent.length;
  await panel.drive([{ id: "overlay-contours", checked: true }]);
  const contours = await panel.waitSent(
    (m) => m.type === "contourReady" || m.type === "contourError",
    from,
    `the contours ${what}`,
  );
  assert.strictEqual(contours.type, "contourReady", `the contours ${what}: ${contours.error ?? ""}`);
  assert.ok(contours.segLengths.length > 0, `the contours ${what} trace a line`);
}

/** Stand in for the CSV export's dialogs while `run` runs: the layout prompt
 *  runs `onPick`, then picks the matrix, and the save dialog is cancelled.
 *  Hands back every error shown and whether the save dialog was asked for. */
async function withCsvDialogs(
  run: () => Promise<void>,
  onPick: () => void = () => undefined,
): Promise<{ errors: string[]; saveAsked: boolean }> {
  const errors: string[] = [];
  let saveAsked = false;
  const w = vscode.window as unknown as Record<string, unknown>;
  const saved = { showQuickPick: w.showQuickPick, showSaveDialog: w.showSaveDialog, showErrorMessage: w.showErrorMessage };
  w.showQuickPick = (items: Array<{ format?: string }>) => {
    onPick();
    return Promise.resolve(items.find((i) => i.format === "matrix"));
  };
  w.showSaveDialog = () => {
    saveAsked = true;
    return Promise.resolve(undefined);
  };
  w.showErrorMessage = (message: string) => {
    errors.push(message);
    return Promise.resolve(undefined);
  };
  try {
    await run();
  } finally {
    Object.assign(w, saved);
  }
  return { errors, saveAsked };
}

/** Press Export CSV on `panel`, as its page does, and wait for the export to
 *  finish. A slice panel's request names the slice it last drew. */
async function exportCsv(p: FieldglassEditorProvider, panel: RealPanel, type: string): Promise<void> {
  const exports = p as unknown as { _csvExport?: Promise<void> };
  const before = exports._csvExport;
  const slice = panel.received.filter((m: Msg) => m && m.slice).pop()?.slice;
  await panel.sendAsPage({ type, page: pageNumberOf(panel.html()), ...(slice ? { slice } : {}) });
  const started = await waitUntil(
    () => (exports._csvExport && exports._csvExport !== before ? exports._csvExport : undefined),
    "the export to start",
  );
  await started;
}

suite("Native handles close with their last panel (#1204)", function () {
  this.timeout(60_000);

  for (const [kind, source, map, open, csv] of [
    ["GRIB", GRIB, "_handlesByDoc", { type: "decodeGrid", messageIndex: 0 }, "exportCsv"],
    ["NetCDF", NETCDF, "_netcdfHandlesByDoc", { type: "renderVariable", variableIndex: 0 }, "exportSliceCsv"],
  ] as const) {
    test(`a ${kind} file's handle closes with the last of its editor and render panel, in either order`, async () => {
      const p = await provider();
      const copy = copyOf(source());
      try {
        // The editor closes first: the render panel still holds the handle.
        let editor = await openEditor(p, copy.file);
        const first = maps(p)[map].get(copy.key);
        assert.ok(first, "the editor opened a handle");
        let panel = await openReal(p, () => editor.send(open));
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        editor.close();
        assert.ok(!maps(p)[map].has(copy.key), "the document's handle is dropped with its editor");
        assert.ok(!isClosed(first), "a handle a render panel still holds stays open");
        // And the panel draws from it (#1338). It used to read the document's
        // handle, which had gone, and answer "disposed".
        await serves(panel, "after the editor closed");
        const exported = await withCsvDialogs(() => exportCsv(p, panel, csv));
        assert.deepStrictEqual(exported.errors, [], "Export CSV reads the field after the editor closed");
        assert.ok(exported.saveAsked, "and goes on to ask where to save it");
        assert.ok(!isClosed(first), "the handle stays open while the panel is");
        panel.dispose();
        await closes(first, "the handle to close with the render panel");

        // The render panel closes first: the editor still holds the handle.
        editor = await openEditor(p, copy.file);
        const second = maps(p)[map].get(copy.key);
        assert.ok(second && second !== first, "opening the file again opens a new handle");
        panel = await openReal(p, () => editor.send(open));
        await panel.waitSent(isType("gridReady"), 0, "the render on the reopened file");
        panel.dispose();
        await new Promise((r) => setTimeout(r, 200));
        assert.ok(!isClosed(second), "a handle the editor still holds stays open");
        editor.close();
        assert.ok(isClosed(second), "the handle closes with the editor");
      } finally {
        copy.done();
      }
    });
  }

  test("two editors on one file share its handle, and the second to close closes it", async () => {
    const p = await provider();
    const copy = copyOf(GRIB());
    try {
      const one = await openEditor(p, copy.file);
      const handle = maps(p)._handlesByDoc.get(copy.key);
      assert.ok(handle, "the editor opened a handle");
      const two = await openEditor(p, copy.file);
      assert.strictEqual(maps(p)._handlesByDoc.get(copy.key), handle, "the second editor reuses it");
      one.close();
      assert.ok(!isClosed(handle), "a handle another editor still shows stays open");
      two.close();
      assert.ok(isClosed(handle), "the last editor closes it");
    } finally {
      copy.done();
    }
  });

  test("a Zarr store's handle closes with its last panel", async () => {
    const p = await provider();
    const key = vscode.Uri.file(STORE()).toString();
    const store = vscode.Uri.file(STORE());
    const one = await openReal(p, () => void p.openZarrStore(store));
    await one.waitSent(isType("gridReady"), 0, "the store's first render");
    const handle = maps(p)._zarrHandlesByStore.get(key);
    assert.ok(handle, "the store's handle is kept for its panel");
    const variable = handle.variables()[0].index;
    const two = await openReal(p, () => p.openZarrRenderPanel(store, variable));
    await two.waitSent(isType("gridReady"), 0, "the second panel's render");
    one.dispose();
    await new Promise((r) => setTimeout(r, 200));
    assert.ok(!isClosed(handle), "a store handle another panel still draws from stays open");
    two.dispose();
    await closes(handle, "the store's handle to close with its last panel");
    assert.ok(!maps(p)._zarrHandlesByStore.has(key), "and to leave the map");
  });

  test("a Zarr store opened again while its panel is open closes the first handle once that panel moves off it", async () => {
    const p = await provider();
    const key = vscode.Uri.file(STORE()).toString();
    const store = vscode.Uri.file(STORE());
    const one = await openReal(p, () => void p.openZarrStore(store));
    await one.waitSent(isType("gridReady"), 0, "the store's first render");
    const first = maps(p)._zarrHandlesByStore.get(key);
    assert.ok(first, "the store's handle is kept for its panel");
    const two = await openReal(p, () => void p.openZarrStore(store));
    let second: Handle | undefined;
    try {
      await two.waitSent(isType("gridReady"), 0, "the second open's render");
      second = maps(p)._zarrHandlesByStore.get(key);
      assert.ok(second && second !== first, "opening the store again opens a new handle");
      assert.ok(!isClosed(first), "the first panel still holds the first handle");
      // The first panel's next request moves it to the new handle.
      const from = one.sent.length;
      one.rebuild();
      await one.waitSent(isReply, from, "the first panel's render on the new handle");
      assert.ok(isClosed(first), "the first handle closes once no panel holds it");
      one.dispose();
      await new Promise((r) => setTimeout(r, 200));
      assert.ok(!isClosed(second), "the second panel still draws from the new handle");
    } finally {
      one.dispose();
      two.dispose();
    }
    assert.ok(second);
    await closes(second, "the second handle to close with the last panel");
    assert.ok(!maps(p)._zarrHandlesByStore.has(key), "and to leave the map");
  });

  test("File: Revert File closes the old handle once the render panel has moved to the new one", async () => {
    const p = await provider();
    const copy = copyOf(GRIB());
    const editor = await openEditor(p, copy.file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const old = maps(p)._handlesByDoc.get(copy.key);
      assert.ok(old, "the editor opened a handle");
      // The panel moves to the new handle on its page's next request, which
      // takes a round trip through the webview: until then the old handle
      // stays open.
      const from = panel.sent.length;
      await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
      const renewed = maps(p)._handlesByDoc.get(copy.key);
      assert.ok(renewed && renewed !== old, "the revert opens a new handle");
      assert.ok(!isClosed(old), "the old handle stays open while the panel is on it");
      const reply = await panel.waitSent(isReply, from, "the render after the revert");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.ok(isClosed(old), "the old handle closes once the panel has moved off it");
      assert.ok(!isClosed(renewed), "the new one stays open");
    } finally {
      panel.dispose();
      editor.close();
      copy.done();
    }
  });

  // A revert to bytes that do not open leaves the document with no handle,
  // as a closed editor does, but the panel's handle is now the old file's
  // (#1338): it is not drawn from, read or exported, and it is let go.
  for (const [kind, source, map, open, csv, missing] of [
    [
      "GRIB",
      GRIB,
      "_handlesByDoc",
      { type: "decodeGrid", messageIndex: 0 },
      "exportCsv",
      "Fieldglass: CSV export is available only for an open message.",
    ],
    [
      "NetCDF",
      NETCDF,
      "_netcdfHandlesByDoc",
      { type: "renderVariable", variableIndex: 0 },
      "exportSliceCsv",
      "Fieldglass: CSV export is available only for an open slice.",
    ],
  ] as const) {
    test(`a ${kind} render panel draws nothing from its file once it is reverted to bytes that do not open (#1338)`, async () => {
      const p = await provider();
      const copy = copyOf(source());
      const editor = await openEditor(p, copy.file);
      const panel = await openReal(p, () => editor.send(open));
      try {
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        const old = maps(p)[map].get(copy.key);
        assert.ok(old, "the editor opened a handle");
        fs.writeFileSync(copy.file, Buffer.from("not a GRIB or NetCDF file"));
        let from = panel.sent.length;
        await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
        assert.ok(!maps(p)[map].has(copy.key), "the new bytes open no handle");
        const reply = await panel.waitSent(isReply, from, "the answer after the revert");
        assert.strictEqual(reply.type, "gridError", `drew "${reply.titleLine}" from the file before the revert`);
        assert.strictEqual(reply.error, FILE_UNREADABLE);
        assert.strictEqual(reply.handleGone, true);
        await closes(old, "the old file's handle to be let go");

        // A render, a probe and contours asked for now read nothing.
        from = panel.sent.length;
        const page = pageNumberOf(panel.html());
        await panel.sendAsPage({ type: "rerenderRequest", page, seq: 900 });
        await panel.sendAsPage({ type: "probeRequest", page, seq: 901, px: 1, py: 1 });
        await panel.sendAsPage({ type: "contourRequest", page, seq: 902 });
        const render = await panel.waitSent(isReply, from, "the render after the revert");
        assert.strictEqual(render.type, "gridError", `drew "${render.titleLine}" from the file before the revert`);
        await new Promise((r) => setTimeout(r, 200));
        assert.deepStrictEqual(
          panel.sent.slice(from).filter((m: Msg) => m.type === "probeResult" || m.type === "contourReady").map((m: Msg) => m.type),
          [],
          "no value or contour is read from the old file",
        );
        // A slice panel's cross-section asks for its axis values, and is told
        // the panel has no file rather than that the axis has none.
        if (kind === "NetCDF") {
          from = panel.sent.length;
          await panel.sendAsPage({ type: "axisRequest", page, dim: 0 });
          const axis = await panel.waitSent(isType("axisResult"), from, "the axis answer after the revert");
          assert.strictEqual(axis.handleGone, true);
          assert.strictEqual(axis.result, null);
        }
        const exported = await withCsvDialogs(() => exportCsv(p, panel, csv));
        assert.deepStrictEqual(exported.errors, [missing], "Export CSV names what is missing");
        assert.ok(!exported.saveAsked, "nothing is saved");
      } finally {
        panel.dispose();
        editor.close();
        copy.done();
      }
    });
  }

  test("a CSV export keeps the handle open while its layout prompt is up, then lets go", async () => {
    // The file's last panel closes while the user picks a layout. The export
    // still reads the field, and the handle closes after it.
    const p = await provider();
    const copy = copyOf(GRIB());
    const editor = await openEditor(p, copy.file);
    const handle = maps(p)._handlesByDoc.get(copy.key);
    assert.ok(handle, "the editor opened a handle");
    let exported: { errors: string[]; saveAsked: boolean };
    try {
      exported = await withCsvDialogs(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        () => (p as any).handleExportCsv(editor.document, 0),
        () => editor.close(),
      );
    } finally {
      copy.done();
    }
    assert.deepStrictEqual(exported.errors, [], "the export read the field");
    assert.ok(exported.saveAsked, "and went on to ask where to save it");
    assert.ok(isClosed(handle), "the handle closed once the export let go");
  });
});
