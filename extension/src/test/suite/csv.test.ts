// Integration tests for CSV export through the native binding the extension
// talks to. The formatting itself is unit-tested in Rust (`fieldglass-core`);
// these pin the extension-facing `exportCsv` binding and its round-trip
// agreement with `decodeGrid` on a real fixture.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";

const EXT_ID = "fieldglass.fieldglass";

function fixturePath(name: string): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return path.join(ext.extensionPath, "src", "test", "fixtures", name);
}

function grib2Handle() {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const bytes = fs.readFileSync(fixturePath("regular_latlon_surface.grib2"));
  return native.Grib2Handle.fromBytes(bytes);
}

suite("CSV export", () => {
  test("exportCsv matrix round-trips against decodeGrid", () => {
    const handle = grib2Handle();
    const grid = handle.decodeGrid(0);
    const csv = handle.exportCsv(0, "matrix");

    const rows = csv.replace(/\n$/, "").split("\n").map((r) => r.split(","));
    assert.strictEqual(rows.length, grid.height, "one row per grid row");
    assert.strictEqual(rows[0].length, grid.width, "one cell per grid column");

    for (let j = 0; j < grid.height; j++) {
      for (let i = 0; i < grid.width; i++) {
        const k = j * grid.width + i;
        const cell = rows[j][i];
        if (grid.mask[k] === 0) {
          assert.strictEqual(cell, "", `masked cell (${i},${j}) must be empty`);
        } else {
          assert.ok(
            Math.abs(Number(cell) - grid.values[k]) < 1e-9,
            `cell (${i},${j}) = ${cell} should equal ${grid.values[k]}`
          );
        }
      }
    }
  });

  test("exportCsv long has a header and one row per grid point", () => {
    const handle = grib2Handle();
    const grid = handle.decodeGrid(0);
    const csv = handle.exportCsv(0, "long");

    const lines = csv.replace(/\n$/, "").split("\n");
    assert.strictEqual(lines[0], "lat,lon,value", "header row");
    assert.strictEqual(
      lines.length - 1,
      grid.width * grid.height,
      "one data row per grid point"
    );
    // Every data row is `lat,lon,value` (three fields); the value may be empty.
    for (const line of lines.slice(1)) {
      assert.strictEqual(line.split(",").length, 3, `three columns in "${line}"`);
    }
  });

  test("exportCsv rejects an unknown format", () => {
    const handle = grib2Handle();
    assert.throws(() => handle.exportCsv(0, "tsv"), /unknown CSV format/);
  });
});
