// Integration tests for CSV export through the native binding the extension
// talks to. The formatting itself is unit-tested in Rust (`fieldglass-core`);
// these pin the extension-facing `exportCsv` binding and its round-trip
// agreement with `decodeGrid` on a real fixture.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import type { FieldglassApi } from "../../extension";

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
    const csv = handle.exportCsv(0, "matrix").toString("utf8");

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
    const csv = handle.exportCsv(0, "long").toString("utf8");

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

suite("CSV export (Lambert)", () => {
  function lambertHandle() {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const bytes = fs.readFileSync(fixturePath("eta_lambert_msg0.grib2"));
    return native.Grib2Handle.fromBytes(bytes);
  }

  // A projected grid used to refuse the long format for want of per-point
  // coordinates (#470). Its projector has reported them since #422/#423, so the
  // export now carries a latitude and longitude on every row.
  test("exportCsv long geolocates a Lambert grid (#470)", () => {
    const handle = lambertHandle();
    const meta = handle.messages()[0];
    assert.strictEqual(meta.gridType, "lambert");

    const csv = handle.exportCsv(0, "long").toString("utf8");
    const lines = csv.replace(/\n$/, "").split("\n");
    assert.strictEqual(lines[0], "lat,lon,value");
    assert.strictEqual(
      lines.length - 1,
      93 * 65,
      "one data row per grid point, none dropped for want of a coordinate"
    );

    // Corners against eccodes 2.34.1 (`grib_get_data`), longitudes folded into
    // the ±180 convention this binding reports.
    for (const [i, j, lat, lon] of [
      [0, 0, 12.19, -133.459],
      [92, 0, 14.33464247, -65.091275135],
      [0, 64, 54.535803478, -152.855459065],
      [92, 64, 57.289403949, -49.38509725],
    ] as const) {
      const [gotLat, gotLon] = lines[1 + j * 93 + i].split(",").map(Number);
      assert.ok(
        Math.abs(gotLat - lat) < 1e-6 && Math.abs(gotLon - lon) < 1e-6,
        `(${i},${j}) exported (${gotLat}, ${gotLon}); eccodes says (${lat}, ${lon})`
      );
    }
  });
});

suite("CSV export (NetCDF)", () => {
  function netcdfSst() {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const bytes = fs.readFileSync(fixturePath("ersst_v5_187001_cdf1.nc"));
    const handle = native.NetcdfHandle.fromBytes(bytes);
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst, "sst must be renderable");
    const y = sst.detectedYDim ?? 2;
    const x = sst.detectedXDim ?? 3;
    const indices = sst.dims.map(() => 0);
    return { handle, sst, y, x, indices };
  }

  test("exportCsv matrix is a rectangular grid of the slice", () => {
    const { handle, sst, y, x, indices } = netcdfSst();
    const csv = handle.exportCsv(sst.variableIndex, y, x, indices, "matrix").toString("utf8");
    const rows = csv.replace(/\n$/, "").split("\n").map((r) => r.split(","));
    assert.ok(rows.length > 1, "more than one row");
    const cols = rows[0].length;
    assert.ok(cols > 1, "more than one column");
    for (const r of rows) {
      assert.strictEqual(r.length, cols, "every row has the same column count");
    }
  });

  test("exportCsv long has a header and one row per grid point", () => {
    const { handle, sst, y, x, indices } = netcdfSst();
    const matrix = handle
      .exportCsv(sst.variableIndex, y, x, indices, "matrix")
      .toString("utf8")
      .replace(/\n$/, "")
      .split("\n");
    const cells = matrix.length * matrix[0].split(",").length;

    const long = handle
      .exportCsv(sst.variableIndex, y, x, indices, "long")
      .toString("utf8")
      .replace(/\n$/, "")
      .split("\n");
    assert.strictEqual(long[0], "lat,lon,value", "header row");
    assert.strictEqual(long.length - 1, cells, "one data row per grid point");
    for (const line of long.slice(1)) {
      assert.strictEqual(line.split(",").length, 3, `three columns in "${line}"`);
    }
  });

  test("exportCsv rejects an unknown format", () => {
    const { handle, sst, y, x, indices } = netcdfSst();
    assert.throws(
      () => handle.exportCsv(sst.variableIndex, y, x, indices, "tsv"),
      /unknown CSV format/,
    );
  });
});

suite("Export CSV command (NetCDF slice)", () => {
  test("handleExportSliceCsv reports a clear error when no NetCDF slice is open", async () => {
    const ext = vscode.extensions.getExtension<FieldglassApi>("fieldglass.fieldglass");
    assert.ok(ext, "extension is installed");
    const provider = (await ext.activate()).provider;

    // openCustomDocument does not register a NetCDF reader handle (that happens
    // when the editor resolves), so the slice-export guard must fire cleanly.
    const uri = vscode.Uri.file(fixturePath("regular_latlon_surface.grib2"));
    const doc = await provider.openCustomDocument(
      uri,
      {} as vscode.CustomDocumentOpenContext,
      new vscode.CancellationTokenSource().token,
    );

    const original = vscode.window.showErrorMessage;
    let shown = "";
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showErrorMessage = (msg: string) => {
      shown = msg;
      return Promise.resolve(undefined);
    };
    try {
      await provider.handleExportSliceCsv(doc, {
        variableIndex: 0,
        yDim: 0,
        xDim: 1,
        sliceIndices: [0],
      });
    } finally {
      vscode.window.showErrorMessage = original;
    }
    assert.match(shown, /NetCDF/);
  });
});
