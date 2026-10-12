// A render panel keeps every control across a rebuild (#1027).
//
// VS Code tears a hidden panel's page down (`retainContextWhenHidden: false`)
// and an imported colour table writes every open panel again, so the page is
// rebuilt from its saved state far more often than it is opened. Arrows, the
// zonal average, the animation and the line plot's axis were left out of that
// state and came back at their defaults.
//
// The test does not list the controls. It moves every enabled control on a real
// page off its value, rebuilds the page, and requires every control to read the
// same as before. A control added later and not saved fails it with no edit
// here. What it cannot move is named per panel below, with the reason, so a
// control that silently stops being exercised fails too.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** Not moved, though still compared after the rebuild. Each reshapes the panel
 *  rather than setting a value, and is saved through `sliceState` and held by
 *  its own tests (#822, #839). */
const STRUCTURAL = ["slice-variable", "slice-y", "slice-x"];

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

/** Wait until the page and the provider have stopped talking: every render,
 *  overlay and line the moved controls asked for has been answered. */
async function settle(panel: RealPanel): Promise<void> {
  let count = -1;
  let since = Date.now();
  await waitUntil(
    () => {
      const now = panel.sent.length + panel.received.length;
      if (now !== count) {
        count = now;
        since = Date.now();
        return undefined;
      }
      return Date.now() - since > 1500 ? true : undefined;
    },
    "the panel to settle",
    45_000,
  );
}

async function rebuilt(panel: RealPanel): Promise<void> {
  const from = panel.received.length;
  panel.rebuild();
  await waitUntil(() => panel.received.slice(from).find(isType("ready")), "the rebuilt page");
  await settle(panel);
}

/** Move every control, rebuild, and require every control back as it was.
 *  `stuck` is what the page cannot move, which must be exactly the set the
 *  caller names. Returns the ids moved. */
async function roundTrip(
  panel: RealPanel,
  values: Record<string, string | number>,
  stuck: string[],
): Promise<string[]> {
  await panel.waitSent(drawnOrRefused, 0, "the first render");
  await settle(panel);
  const original = await panel.controls();
  const done = await panel.perturb(STRUCTURAL, values);
  assert.deepStrictEqual(done.unknown, [], "the driver knows how to move every kind of control");
  assert.deepStrictEqual(done.stuck.sort(), stuck.slice().sort(), "only the named controls cannot move");
  await settle(panel);
  const before = await panel.controls();
  // A control the page put back while it settled was never really moved, and
  // would pass the comparison below having tested nothing.
  const reverted = done.moved.filter((id) => before[id] === original[id]);
  assert.deepStrictEqual(reverted, [], "every moved control stays moved");

  await rebuilt(panel);
  const after = await panel.controls();
  const lost = Object.keys(before)
    .filter((id) => after[id] !== before[id])
    .map((id) => `${id}: ${JSON.stringify(before[id])} came back as ${JSON.stringify(after[id])}`);
  assert.deepStrictEqual(lost, [], "every control survives the rebuild");
  return done.moved;
}

/** A Zarr v2 store with a time × level × lat × lon variable `t`: two axes to
 *  animate along besides the map's, which no committed fixture has. */
function fourDimensionStore(root: string): void {
  fs.mkdirSync(root);
  fs.writeFileSync(path.join(root, ".zgroup"), JSON.stringify({ zarr_format: 2 }));
  const array = (name: string, dims: string[], shape: number[], values: number[], units: string) => {
    const dir = path.join(root, name);
    fs.mkdirSync(dir);
    fs.writeFileSync(
      path.join(dir, ".zarray"),
      JSON.stringify({
        zarr_format: 2,
        shape,
        chunks: shape,
        dtype: "<f4",
        compressor: null,
        fill_value: null,
        order: "C",
        filters: null,
      }),
    );
    fs.writeFileSync(path.join(dir, ".zattrs"), JSON.stringify({ _ARRAY_DIMENSIONS: dims, units }));
    const bytes = Buffer.alloc(values.length * 4);
    values.forEach((v, i) => bytes.writeFloatLE(v, i * 4));
    fs.writeFileSync(path.join(dir, shape.map(() => "0").join(".")), bytes);
  };
  array("lat", ["lat"], [3], [-10, 0, 10], "degrees_north");
  array("lon", ["lon"], [4], [0, 90, 180, 270], "degrees_east");
  array("time", ["time"], [2], [0, 1], "days since 2000-01-01");
  array("level", ["level"], [2], [1000, 500], "hPa");
  array("t", ["time", "level", "lat", "lon"], [2, 2, 3, 4], Array.from({ length: 48 }, (_, i) => 250 + i), "K");
  // The same axes with time second: a default kept by position would animate
  // `level` here.
  array("u", ["level", "time", "lat", "lon"], [2, 2, 3, 4], Array.from({ length: 48 }, (_, i) => i), "m s-1");
}

suite("A render panel keeps its controls across a rebuild (#1027)", function () {
  this.timeout(120_000);

  test("a GRIB panel keeps every control, arrows and zonal average included", async () => {
    const p = await provider();
    // Two messages on a placed lat/lon grid, so the Vectors and Compare rows
    // are written and arrows are offered.
    const bytes = fs.readFileSync(fixture("regular_latlon_surface.grib2"));
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1027-"));
    const file = path.join(dir, "pair.grib2");
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const moved = await roundTrip(
        panel,
        {
          "range-min": 280,
          "range-max": 300,
          "bounds-lat-min": 5,
          "bounds-lat-max": 25,
          "bounds-lon-min": 10,
          "bounds-lon-max": 50,
        },
        // A GRIB panel has no slice, so no line plot to pick an axis for, and
        // its message picker is filled only when its message is gone (#959).
        ["line-axis", "message-pick"],
      );
      for (const id of ["overlay-vectors", "vector-field-v", "vector-spacing", "color-vectors", "width-vectors", "zonal-toggle"]) {
        assert.ok(moved.includes(id), `the test moved ${id}`);
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF slice panel keeps every control, and a click that replaces the zonal average is kept too", async () => {
    const p = await provider();
    const file = fixture("netcdf4_dimscale.nc");
    // `temperature` is time × lat × lon: three axes to plot a line along.
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
    try {
      const moved = await roundTrip(
        panel,
        {
          "range-min": 1,
          "range-max": 2,
          "bounds-lat-min": -5,
          "bounds-lat-max": 5,
          "bounds-lon-min": 10,
          "bounds-lon-max": 200,
        },
        // `time` is the only axis besides the map's, so there is one to animate.
        ["animate-dim"],
      );
      for (const id of ["anim-speed", "anim-loop", "line-axis", "zonal-toggle"]) {
        assert.ok(moved.includes(id), `the test moved ${id}`);
      }

      // The round trip left the zonal average on. A click on the map asks for
      // that cell's line instead and unticks it, outside the toolbar.
      assert.strictEqual((await panel.controls())["zonal-toggle"], true, "the zonal average is on");
      await panel.probe();
      await settle(panel);
      assert.strictEqual((await panel.controls())["zonal-toggle"], false, "the click replaced the zonal average");
      await rebuilt(panel);
      assert.strictEqual((await panel.controls())["zonal-toggle"], false, "and the panel shown again keeps it off");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a Zarr panel keeps the axis it animates along", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1027-"));
    const store = path.join(dir, "four.zarr");
    fourDimensionStore(store);
    const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
    try {
      const moved = await roundTrip(
        panel,
        {
          "range-min": 250,
          "range-max": 290,
          "bounds-lat-min": -5,
          "bounds-lat-max": 5,
          "bounds-lon-min": 10,
          "bounds-lon-max": 200,
        },
        [],
      );
      for (const id of ["animate-dim", "anim-speed", "anim-loop", "line-axis"]) {
        assert.ok(moved.includes(id), `the test moved ${id}`);
      }
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("an axis nobody picked is not kept by its position for the next variable", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1027-"));
    const store = path.join(dir, "four.zarr");
    fourDimensionStore(store);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.ZarrHandle.fromDirectory(store).variables();
    const t = variables.find((v) => v.name === "t");
    const u = variables.find((v) => v.name === "u");
    assert.ok(t && u, "the store has t and u");
    assert.deepStrictEqual([t.detectedTimeDim, u.detectedTimeDim], [0, 1], "time sits first in t and second in u");
    const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
    try {
      await panel.waitSent(drawnOrRefused, 0, "the first render");
      await settle(panel);
      const first = (await panel.controls())["slice-variable"];
      const other = first === String(t.variableIndex) ? u : t;
      // Another control changed, which saves the panel with Animate on its
      // default; shown again; then moved to the variable whose time sits
      // elsewhere.
      await panel.drive([{ id: "anim-speed", value: 5 }]);
      await rebuilt(panel);
      await panel.drive([{ id: "slice-variable", value: other.variableIndex }]);
      await settle(panel);
      assert.strictEqual(
        (await panel.controls())["animate-dim"],
        String(other.detectedTimeDim),
        `${other.name} animates along time`,
      );
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
