// A Zarr array opened from its own folder is named by that folder (#1468).
//
// From the 0.6.0 manual pass (§2): `Fieldglass: Open Zarr Store…` on an array's
// folder (rather than the store above it) drew the array but named nothing. The
// tab read `variable · k`, the heading `Variable 0`, and the Variable picker was
// empty, because an array at a store's root has no name inside the store.
//
// Driven through the real panel over committed fixtures: a v3 array folder
// (`v3_v2keys/k`, holding `zarr.json`) and a v2 one (`v2_nested/temp`, holding
// `.zarray`). Only VS Code's dialogs are stubbed, as in `export-names.test.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { extensionPath, isType, openReal, provider, type RealPanel, waitUntil } from "./real-panel";

const STORES = () => path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores");

function fixture(p: string): string {
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** Click Export CSV with the dialogs stubbed, and return the name it suggests. */
async function suggestedCsv(panel: RealPanel, csvId: string): Promise<string> {
  const w = vscode.window as unknown as Record<string, unknown>;
  const saved = { showSaveDialog: w.showSaveDialog, showQuickPick: w.showQuickPick };
  const asked: vscode.Uri[] = [];
  w.showSaveDialog = (opts: { defaultUri?: vscode.Uri }) => {
    if (opts.defaultUri) asked.push(opts.defaultUri);
    return Promise.resolve(undefined);
  };
  w.showQuickPick = (items: Array<{ format?: string }>) =>
    Promise.resolve(Array.isArray(items) ? items.find((i) => i.format === "matrix") : undefined);
  try {
    await panel.drive([{ id: csvId, click: true }]);
    await waitUntil(() => (asked.length > 0 ? true : undefined), "the CSV save dialog");
  } finally {
    Object.assign(w, saved);
  }
  return path.basename(asked[0].fsPath);
}

suite("A Zarr array opened from its own folder is named by it (#1468)", function () {
  this.timeout(120_000);

  for (const [edition, folder, name] of [
    ["v3", path.join("v3_v2keys", "k"), "k"],
    ["v2", path.join("v2_nested", "temp"), "temp"],
    // The codec corpus: real stores whose root is an array (#1468).
    ["v3 codec-corpus", path.join("..", "v3_crc32c_zstd"), "v3_crc32c_zstd"],
    ["v3 codec-corpus", path.join("..", "v3_zstd_gzip"), "v3_zstd_gzip"],
  ] as const) {
    test(`a ${edition} root array (${name}) names the tab, heading, picker and exports`, async () => {
      const dir = fixture(path.join(STORES(), folder));
      const native = loadNative();
      assert.ok(native, "native binding required");
      const handle = native.openStore(dir);
      try {
        assert.deepStrictEqual(
          handle.variables().map((v) => v.name),
          [name],
          "the listing names the array by its folder",
        );
      } finally {
        handle.close();
      }

      const p = await provider();
      const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(dir)));
      try {
        const ready = await panel.waitSent(isType("gridReady"), 0, "the first render");
        assert.strictEqual(panel.title(), `${name} · ${name}`, "the tab");
        const state = await panel.drive([]);
        assert.strictEqual(state.title, ready.titleLine);
        assert.ok(state.title === name || state.title.startsWith(`${name} (`), `the heading: "${state.title}"`);
        assert.strictEqual(state.subtitle, name, "the line under the heading");
        assert.deepStrictEqual(state.sliceVariables, [name], "the Variable picker lists the array");
        assert.strictEqual(state.sliceVariable, name);
        assert.ok(!/Variable 0/.test(state.text), "nothing falls back to a placeholder");
        assert.strictEqual(ready.defaultPngName, `${name}_${name}.png`, "the PNG name");
        const csv = state.exports.find((b) => b.text === "Export CSV…");
        assert.ok(csv, "Export CSV is offered");
        assert.strictEqual(await suggestedCsv(panel, csv.id), `${name}_${name}.csv`, "the CSV name");
      } finally {
        panel.dispose();
      }
    });
  }
});
