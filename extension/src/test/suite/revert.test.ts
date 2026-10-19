// File: Revert File on an open editor (#1217).
//
// Revert reads the file from disk again. The editor page used to be left as it
// was: the provider posted an `update` the read-only page ignores, so the table
// kept the old file's rows, and a row clicked afterwards sent its number, which
// was served against the new file. A NetCDF document was opened into the GRIB
// handle map instead, and its own handle was never replaced, so its dump and
// every slice panel stayed on the old bytes.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { type FieldglassEditorProvider } from "../../provider";
import { extensionPath, isType, type Msg, openEditor, openReal, pageNumberOf, provider } from "./real-panel";

function gribFixture(name: string): Buffer {
  return fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", name));
}

function netcdfFixture(name: string): Buffer {
  return fs.readFileSync(
    path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", name),
  );
}

/** The `#` and Parameter cells of each row in an editor page's message table. */
function tableRows(html: string): string[] {
  return Array.from(
    html.matchAll(/<tr class="msg-row" data-message-index="\d+">\s*<td>(\d+)<\/td>\s*<td>([^<]*)<\/td>/g),
    (m) => `${m[1]} ${m[2]}`,
  );
}

/** What the table would list for `bytes`. */
function rowsOf(bytes: Buffer): string[] {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const h = native.open(bytes);
  return Array.from({ length: h.count() }, (_, i) => `${i} ${h.message(i).parameter}`);
}

/** Run `send` and report how many panels it opened, opening none for real. */
function panelsOpenedBy(send: () => void): number {
  let opened = 0;
  const origCreate = vscode.window.createWebviewPanel;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).createWebviewPanel = () => {
    opened += 1;
    throw new Error("a panel was opened");
  };
  try {
    send();
  } catch {
    // Counted above.
  } finally {
    vscode.window.createWebviewPanel = origCreate;
  }
  return opened;
}

async function revert(p: FieldglassEditorProvider, doc: Parameters<FieldglassEditorProvider["revertCustomDocument"]>[0]) {
  await p.revertCustomDocument(doc, new vscode.CancellationTokenSource().token);
}

suite("File: Revert File", function () {
  this.timeout(60_000);

  test("a GRIB editor's table is written from the reverted file, and the old table's rows open nothing", async () => {
    // Temperature then MSLP, overwritten by MSLP then temperature: every row
    // number names another field after the revert.
    const before = Buffer.concat([gribFixture("regular_latlon_surface.grib2"), gribFixture("eta_lambert_msg0.grib2")]);
    const after = Buffer.concat([gribFixture("eta_lambert_msg0.grib2"), gribFixture("regular_latlon_surface.grib2")]);
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1217-"));
    const file = path.join(dir, "reverted.grib2");
    fs.writeFileSync(file, before);
    const editor = await openEditor(p, file);
    try {
      assert.deepStrictEqual(tableRows(editor.html()), rowsOf(before));
      const oldPage = pageNumberOf(editor.html());

      fs.writeFileSync(file, after);
      await revert(p, editor.document);
      assert.deepStrictEqual(tableRows(editor.html()), rowsOf(after), "the table lists the new file's messages");
      assert.notStrictEqual(pageNumberOf(editor.html()), oldPage, "the page is numbered afresh");

      // A click on the old table's first row, temperature, sent before the
      // page was written again: the new file's first message is MSLP.
      assert.strictEqual(
        panelsOpenedBy(() => editor.send({ type: "decodeGrid", messageIndex: 0, page: oldPage })),
        0,
        "a row of the old table opens nothing",
      );

      // The new table's first row draws the new file's message 0 under its own
      // heading.
      const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
      try {
        const drawn = await panel.waitSent(isType("gridReady"), 0, "the render");
        const native = loadNative();
        assert.ok(native);
        const m = native.open(after).message(0);
        assert.strictEqual(drawn.titleLine, `Message 0 — ${m.parameter}` + (m.units ? ` (${m.units})` : ""));
      } finally {
        panel.dispose();
      }
    } finally {
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF editor and its slice panel are written from the reverted file", async () => {
    // `sst` is the first of ERSST's two variables and the second of OISST's.
    const before = netcdfFixture("ersst_v5_187001_cdf1.nc");
    const after = netcdfFixture("oisst_avhrr_v2.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const indexIn = (bytes: Buffer) => native.open(bytes).variables().find((v) => v.name === "sst")!.index;
    assert.notStrictEqual(indexIn(before), indexIn(after), "sst moves");

    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1217-"));
    const file = path.join(dir, "reverted.nc");
    const key = vscode.Uri.file(file).toString();
    fs.writeFileSync(file, before);
    const editor = await openEditor(p, file);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const gribHandles = (p as any)._handlesByDoc as Map<string, unknown>;
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: indexIn(before) }));
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(opened.messageIndex, indexIn(before));
      assert.ok(!editor.html().includes("Render ice"), "ERSST has no ice");
      const oldPage = pageNumberOf(editor.html());

      fs.writeFileSync(file, after);
      const from = panel.sent.length;
      await revert(p, editor.document);
      assert.ok(editor.html().includes("Render ice"), "the dump lists the new file's variables");
      editor.send({ type: "ready" });
      assert.ok(!gribHandles.has(key), "a NetCDF document has no GRIB handle");

      // The slice panel asks again on its own, and finds sst by name.
      const reply = await panel.waitSent(
        (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError"),
        from,
        "the render after the revert",
      );
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, indexIn(after), "sst at its number in the new file");
      assert.match(reply.titleLine, /^sst\b/);
      const state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "sst");
      assert.deepStrictEqual(state.sliceVariables, ["ice", "sst"]);

      // The old dump's Render sst names sst by its old number, which is ice's
      // in the new file.
      assert.strictEqual(
        panelsOpenedBy(() => editor.send({ type: "renderVariable", variableIndex: indexIn(before), page: oldPage })),
        0,
        "a button on the old dump opens nothing",
      );
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
