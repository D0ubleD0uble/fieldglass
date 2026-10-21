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

import type { Handle } from "../../native";
import { type FieldglassEditorProvider } from "../../provider";
import { extensionPath, isType, type Msg, openEditor, openReal, provider, waitUntil } from "./real-panel";

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

suite("Native handles close with their last panel (#1204)", function () {
  this.timeout(60_000);

  for (const [kind, source, map, open] of [
    ["GRIB", GRIB, "_handlesByDoc", { type: "decodeGrid", messageIndex: 0 }],
    ["NetCDF", NETCDF, "_netcdfHandlesByDoc", { type: "renderVariable", variableIndex: 0 }],
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

  test("a CSV export keeps the handle open while its layout prompt is up, then lets go", async () => {
    // The file's last panel closes while the user picks a layout. The export
    // still reads the field, and the handle closes after it.
    const p = await provider();
    const copy = copyOf(GRIB());
    const editor = await openEditor(p, copy.file);
    const handle = maps(p)._handlesByDoc.get(copy.key);
    assert.ok(handle, "the editor opened a handle");
    const errors: string[] = [];
    let saveAsked = false;
    const w = vscode.window as unknown as Record<string, unknown>;
    const saved = { showQuickPick: w.showQuickPick, showSaveDialog: w.showSaveDialog, showErrorMessage: w.showErrorMessage };
    w.showQuickPick = (items: Array<{ format?: string }>) => {
      editor.close();
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
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await (p as any).handleExportCsv(editor.document, 0);
    } finally {
      Object.assign(w, saved);
      copy.done();
    }
    assert.deepStrictEqual(errors, [], "the export read the field");
    assert.ok(saveAsked, "and went on to ask where to save it");
    assert.ok(isClosed(handle), "the handle closed once the export let go");
  });
});
