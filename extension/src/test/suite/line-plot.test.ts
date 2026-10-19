// The line plot is a choice (#1037).
//
// A click on a NetCDF or Zarr map used to open a line plot under the readout
// every time, labelled "Plot along [axis]", though a click is usually meant
// only to read a value. Now a Line plot box, unticked in a new panel, decides:
// off, a click only reads the value, as in a GRIB panel; on, it also plots the
// variable through the clicked cell. The picker's label names the variable and
// the axis, the box is kept like any other control (#1027), and the caption's
// range reads with "to" and a true minus sign, so a range from below zero to
// above it is not read as two negative numbers.
//
// Driven through the real panel: the clicks are the page's own click handler,
// and the requests are what the page sent the provider.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { composeLineRange } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type PageState,
  type RealPanel,
  waitUntil,
} from "./real-panel";

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

const lineRequests = (panel: RealPanel): Msg[] => panel.received.filter(isType("lineRequest"));

/** Click the map, and wait until the page has handled the provider's answer. */
async function click(panel: RealPanel): Promise<Msg> {
  const from = panel.sent.length;
  await panel.probe();
  const reply = await panel.waitSent(isType("probeResult"), from, "the probe's answer");
  // The page handles messages in order, so its report follows whatever it did
  // with the answer, a line request included.
  await panel.drive([]);
  return reply;
}

/** A panel on `netcdf4_dimscale.nc`'s `temperature`, time × lat × lon. */
async function temperaturePanel(): Promise<{ panel: RealPanel; close: () => void }> {
  const p = await provider();
  const file = fixture("netcdf4_dimscale.nc");
  const native = loadNative();
  assert.ok(native, "native binding required");
  const temperature = native.open(fs.readFileSync(file))
    .variables()
    .find((v) => v.name === "temperature");
  assert.ok(temperature, "the fixture has temperature");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

/** A Zarr v2 store on a regular lat/lon grid whose variable
 *  `bt(time, lat, lon)` holds `steps[t]` everywhere at time `t`, so the line
 *  through any cell is `steps`. Its variable `one(y, x)` is a single cell,
 *  with no axis to plot along. */
function lineStore(root: string, steps: number[]): void {
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
  array("time", ["time"], [steps.length], steps.map((_, i) => i), "days since 2000-01-01");
  array("bt", ["time", "lat", "lon"], [steps.length, 3, 4], steps.flatMap((v) => Array.from({ length: 12 }, () => v)), "degC");
  array("one", ["y", "x"], [1, 1], [5], "degC");
}

/** A panel on a {@link lineStore} store. */
async function storePanel(steps: number[]): Promise<{ panel: RealPanel; store: string; close: () => void }> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1037-"));
  const store = path.join(dir, "line.zarr");
  lineStore(store, steps);
  const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    store,
    close: () => {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** Tick the Line plot box, click, and read the page once the line is drawn. */
async function plotted(panel: RealPanel): Promise<PageState> {
  await panel.drive([{ id: "line-toggle", checked: true }]);
  const from = panel.sent.length;
  await click(panel);
  await panel.waitSent(isType("lineResult"), from, "the line");
  return panel.drive([]);
}

/** Write the panel again, as an imported colormap does, and wait for the
 *  rebuilt page's own render. */
async function rebuilt(panel: RealPanel): Promise<void> {
  const received = panel.received.length;
  const sent = panel.sent.length;
  panel.rebuild();
  await waitUntil(() => panel.received.slice(received).find(isType("ready")), "the rebuilt page");
  await panel.waitSent(isType("gridReady"), sent, "the rebuilt page's render");
}

suite("The line plot is a choice, off by default (#1037)", function () {
  this.timeout(120_000);

  test("in a new panel the box is unticked, and a click only reads the value", async () => {
    const { panel, close } = await temperaturePanel();
    try {
      const state = await panel.drive([]);
      assert.ok(state.linePlot, "a slice panel has the Line plot box");
      assert.strictEqual(state.linePlot.boxShown, true, "and shows it");
      assert.strictEqual(state.linePlot.checked, false, "unticked");
      const reply = await click(panel);
      assert.ok(reply.result, "the click lands on the grid");
      const after = await panel.drive([]);
      assert.notStrictEqual(after.probe, "", "the readout shows the value");
      assert.deepStrictEqual(lineRequests(panel), [], "no line is asked for");
      assert.strictEqual(after.linePlot?.shown, false, "and no plot is drawn");
    } finally {
      close();
    }
  });

  test("ticked, a click plots the variable through the cell, labelled with the variable and the axis", async () => {
    const { panel, close } = await temperaturePanel();
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.deepStrictEqual(lineRequests(panel), [], "ticking the box before any click asks for nothing");
      const from = panel.sent.length;
      await click(panel);
      assert.strictEqual(lineRequests(panel).length, 1, "the click asks for the cell's line");
      await panel.waitSent(isType("lineResult"), from, "the line");
      const state = await panel.drive([]);
      assert.strictEqual(state.linePlot?.shown, true, "the plot is drawn");
      assert.strictEqual(state.linePlot?.label, "temperature at this point, across time");
      assert.match(state.linePlot?.caption ?? "", /^temperature across time /, "the caption says it the same way");
      assert.ok(!/Plot along/.test(state.text), "the old label is gone");

      // Unticked, the plot goes; ticked again, it comes back for the cell the
      // readout still describes, with no second click.
      const off = await panel.drive([{ id: "line-toggle", checked: false }]);
      assert.strictEqual(off.linePlot?.shown, false, "unticking puts the plot away");
      const again = panel.sent.length;
      await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.strictEqual(lineRequests(panel).length, 2, "ticking it again asks for the same cell");
      await panel.waitSent(isType("lineResult"), again, "the line again");
      assert.strictEqual((await panel.drive([])).linePlot?.shown, true, "and draws it");
    } finally {
      close();
    }
  });

  test("the box survives the panel being written again, ticked or not", async () => {
    const { panel, close } = await temperaturePanel();
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      await rebuilt(panel);
      assert.strictEqual((await panel.controls())["line-toggle"], true, "ticked, it comes back ticked");
      await click(panel);
      assert.strictEqual(lineRequests(panel).length, 1, "and a click plots");

      await panel.drive([{ id: "line-toggle", checked: false }]);
      await rebuilt(panel);
      assert.strictEqual((await panel.controls())["line-toggle"], false, "unticked, it comes back unticked");
      await click(panel);
      assert.strictEqual(lineRequests(panel).length, 1, "and a click does not plot");
    } finally {
      close();
    }
  });

  test("the caption's range shows a negative minimum and a positive maximum as such", async () => {
    const { panel, close } = await storePanel([-72.769, 0, 31.484]);
    try {
      const state = await plotted(panel);
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time");
      assert.strictEqual(
        state.linePlot?.caption,
        "bt across time (days since 2000-01-01) · −72.8 to 31.5 degC · 3 of 3 points",
      );
    } finally {
      close();
    }
  });

  test("a flat line is captioned with its own value, not the span drawn around it", async () => {
    const { panel, close } = await storePanel([250, 250, 250]);
    try {
      const state = await plotted(panel);
      assert.strictEqual(state.linePlot?.caption, "bt across time (days since 2000-01-01) · 250 degC · 3 of 3 points");
    } finally {
      close();
    }
  });

  test("with the box off a click leaves a zonal average on screen; ticked, the cell's line replaces it", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      const zonal = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      await panel.waitSent(isType("zonalResult"), zonal, "the zonal average");
      await click(panel);
      let state = await panel.drive([]);
      assert.deepStrictEqual(lineRequests(panel), [], "the click asks for no line");
      assert.strictEqual((await panel.controls())["zonal-toggle"], true, "the zonal average stays ticked");
      assert.strictEqual(state.linePlot?.shown, true, "and on screen");
      assert.strictEqual(state.linePlot?.label, null, "with no axis picker, as a zonal average has none");

      // The cell the readout describes is plotted once the box is ticked.
      const from = panel.sent.length;
      await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.strictEqual(lineRequests(panel).length, 1, "ticking the box asks for the clicked cell's line");
      await panel.waitSent(isType("lineResult"), from, "the line");
      state = await panel.drive([]);
      assert.strictEqual((await panel.controls())["zonal-toggle"], false, "which replaces the zonal average");
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time");
    } finally {
      close();
    }
  });

  test("a click leaves a hidden Zonal average box as it was (#1036)", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      // time against lat is a cross-section, which takes no zonal average.
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-x", value: 0 }]);
      await panel.waitSent(drawnOrRefused, from, "the cross-section");
      let state = await panel.drive([]);
      assert.strictEqual(state.controlDisplay.zonal, "none", "the Zonal average box is hidden");
      state = await plotted(panel);
      assert.strictEqual(state.linePlot?.shown, true, "the click plots");
      assert.strictEqual((await panel.controls())["zonal-toggle"], true, "and the hidden box keeps its tick");
    } finally {
      close();
    }
  });

  test("the box is hidden for a variable with no axis to plot along, and keeps its tick", async () => {
    const { panel, store, close } = await storePanel([1, 2, 3]);
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const native = loadNative();
      assert.ok(native, "native binding required");
      const one = native.openStore(store).variables().find((v) => v.name === "one");
      assert.ok(one, "the store has the single-cell variable");
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: one.variableIndex }]);
      await panel.waitSent(drawnOrRefused, from, "the single cell");
      const state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "one");
      assert.strictEqual(state.linePlot?.boxShown, false, "a single cell has no Line plot box");
      assert.strictEqual(state.linePlot?.checked, true, "which keeps its tick");
      assert.ok(!/Line plot/.test(state.unavailable?.summary ?? ""), "and is not listed as unavailable: it is a choice");
    } finally {
      close();
    }
  });

  test("the range is written with 'to' and a true minus, at the precision its span needs", () => {
    const m = "−";
    assert.strictEqual(composeLineRange(-72.769, 31.484), `${m}72.8 to 31.5`, "the issue's case");
    assert.strictEqual(composeLineRange(-31.484, -10.2), `${m}31.48 to ${m}10.20`, "both negative");
    assert.strictEqual(composeLineRange(238.35, 300.83), "238.35 to 300.83");
    assert.strictEqual(composeLineRange(101325, 101330), "101325 to 101330", "no zeros both ends share");
    assert.strictEqual(composeLineRange(-0.00001, 9.5), "0.0 to 9.5", "a value rounding to zero is not negative");
    assert.strictEqual(composeLineRange(1.5e-7, 3.25e-7), `1.500e${m}7 to 3.250e${m}7`, "tiny values in exponent form");
    assert.strictEqual(composeLineRange(-2e12, 4e12), `${m}2.000e+12 to 4.000e+12`, "huge ones too");
  });

  test("a flat line reads as its exact value", () => {
    assert.strictEqual(composeLineRange(250, 250), "250");
    assert.strictEqual(composeLineRange(101325, 101325), "101325", "not rounded to 101330");
    assert.strictEqual(composeLineRange(Math.fround(250.1), Math.fround(250.1)), "250.1", "float32 noise is dropped");
    assert.strictEqual(composeLineRange(-3.5, -3.5), "−3.5");
  });

  test("two different ends never print alike", () => {
    const cases: Array<[number, number]> = [
      [280, 280.000001],
      [280.0000001, 280.0000003],
      [1.0e9, 1.0004e9],
      [1.0e9, 1.0000001e9],
      [1.5e-7, 1.5000001e-7],
      [-0.000001, 0.000001],
      [-1e-12, 1e-12],
    ];
    for (const [a, b] of cases) {
      const [lo, hi] = composeLineRange(a, b).split(" to ");
      assert.notStrictEqual(lo, hi, `${a} and ${b} read as ${lo} and ${hi}`);
    }
    assert.strictEqual(composeLineRange(1.0e9, 1.0004e9), "1.0000e+9 to 1.0004e+9");
    assert.strictEqual(composeLineRange(280, 280.000001), "280.000000 to 280.000001");
  });
});
