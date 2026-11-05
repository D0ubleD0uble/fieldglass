// The render tab's Export CSV writes the file the message table's Export CSV
// button used to write (#1702).
//
// The table's detail row held an Export CSV button, and #1702 took the row
// away because the render tab already has one (#1039). That is only a fair
// swap if the two wrote the same bytes for the same message. The reference
// files here were written by the table's button, on master before the row
// went (commit c8e2ddbd), for message 0 of `regular_latlon_surface.grib2`,
// once in each layout. The render tab's button must write them again, byte for
// byte. Only VS Code's dialogs are stubbed.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { extensionPath, isType, openEditor, openReal, provider } from "./real-panel";

const FIXTURES = () => path.join(extensionPath(), "src", "test", "fixtures");

suite("Export CSV from the render tab matches what the message table wrote (#1702)", function () {
  this.timeout(60_000);

  for (const format of ["matrix", "long"] as const) {
    test(`the ${format} layout is the file the table's button wrote`, async () => {
      const reference = fs.readFileSync(path.join(FIXTURES(), "csv-parity", `regular_latlon_surface.msg0.${format}.csv`));
      assert.ok(reference.length > 100, "the reference holds a field");

      const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1702-"));
      const out = path.join(dir, "out.csv");
      const p = await provider();
      const editor = await openEditor(p, path.join(FIXTURES(), "regular_latlon_surface.grib2"));
      const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
      const w = vscode.window as unknown as Record<string, unknown>;
      const saved = {
        showSaveDialog: w.showSaveDialog,
        showQuickPick: w.showQuickPick,
        showInformationMessage: w.showInformationMessage,
      };
      const offered: string[][] = [];
      w.showQuickPick = (items: Array<{ format?: string }>) => {
        offered.push(items.map((i) => i.format ?? ""));
        return Promise.resolve(items.find((i) => i.format === format));
      };
      w.showSaveDialog = () => Promise.resolve(vscode.Uri.file(out));
      w.showInformationMessage = () => Promise.resolve(undefined);
      try {
        await panel.waitSent(isType("gridReady"), 0, "the render");
        await panel.drive([{ id: "export-csv", click: true }]);
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        await (p as any)._csvExport;
        assert.deepStrictEqual(offered, [["long", "matrix"]], "the layout prompt offers both, as the table's did");
        assert.ok(fs.existsSync(out), "the render tab wrote the file");
        assert.ok(fs.readFileSync(out).equals(reference), `the ${format} CSV is byte for byte the table button's`);
      } finally {
        Object.assign(w, saved);
        panel.dispose();
        editor.close();
        fs.rmSync(dir, { recursive: true, force: true });
      }
    });
  }
});
