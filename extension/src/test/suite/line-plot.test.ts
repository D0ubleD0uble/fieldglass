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
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
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

  test("ticking Zonal average over a cell's line unticks Line plot, and a rebuild keeps it so (#1234)", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      let state = await plotted(panel);
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time", "the cell's line is on screen");

      const from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      state = await panel.drive([]);
      assert.strictEqual(state.linePlot?.shown, true, "the zonal average is drawn");
      assert.strictEqual(state.linePlot?.label, null, "in place of the cell's line");
      let controls = await panel.controls();
      assert.strictEqual(controls["zonal-toggle"], true, "Zonal average is ticked");
      assert.strictEqual(controls["line-toggle"], false, "and Line plot, whose line it replaced, is not");

      // The untick is outside the toolbar, so it saves itself (#1027).
      await rebuilt(panel);
      controls = await panel.controls();
      assert.strictEqual(controls["line-toggle"], false, "the panel shown again keeps Line plot unticked");
      assert.strictEqual(controls["zonal-toggle"], true, "and Zonal average ticked");
      const requests = lineRequests(panel).length;
      await click(panel);
      assert.strictEqual(lineRequests(panel).length, requests, "so a click only reads the value");
      assert.strictEqual((await panel.controls())["zonal-toggle"], true, "and the zonal average stays");
    } finally {
      close();
    }
  });

  test("a zonal average that comes back with its field replaces the line and unticks Line plot (#1234)", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      const mapX = String((await panel.controls())["slice-x"]);
      // time against lat is a cross-section, which takes no zonal average: the
      // box is hidden and keeps its tick (#1036), and a click plots a line.
      let from = panel.sent.length;
      await panel.drive([{ id: "slice-x", value: 0 }]);
      await panel.waitSent(drawnOrRefused, from, "the cross-section");
      let state = await plotted(panel);
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time", "the cell's line is on screen");

      // Back on the lat/lon map the zonal average is drawn again.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-x", value: mapX }]);
      await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      state = await panel.drive([]);
      assert.notStrictEqual(state.controlDisplay.zonal, "none", "the Zonal average box is shown");
      assert.strictEqual(state.linePlot?.label, null, "the zonal average replaced the line");
      assert.strictEqual((await panel.controls())["line-toggle"], false, "and Line plot is unticked");

      // The replaced line is gone for good. Ticking Line plot with no cell to
      // plot still takes the plot from the zonal average (#1389), and unticking
      // it leaves nothing drawn.
      state = await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.strictEqual(state.linePlot?.shown, false, "the zonal average goes");
      assert.strictEqual((await panel.controls())["zonal-toggle"], false, "and its box unticks");
      state = await panel.drive([{ id: "line-toggle", checked: false }]);
      assert.strictEqual(state.linePlot?.shown, false, "nothing is drawn");
    } finally {
      close();
    }
  });

  test("ticking Zonal average after a step cleared the line unticks Line plot (#1389)", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      const state = await plotted(panel);
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time", "the cell's line is on screen");
      // Stepping time clears the line and leaves the box ticked.
      let from = panel.sent.length;
      await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: 2 }]);
      await panel.waitSent(isType("gridReady"), from, "time step 2");
      let after = await panel.drive([]);
      assert.strictEqual(after.linePlot?.shown, false, "the step cleared the line");
      assert.strictEqual((await panel.controls())["line-toggle"], true, "Line plot is still ticked");

      from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      after = await panel.drive([]);
      assert.strictEqual(after.linePlot?.shown, true, "the zonal average is drawn");
      const controls = await panel.controls();
      assert.strictEqual(controls["zonal-toggle"], true, "Zonal average is ticked");
      assert.strictEqual(controls["line-toggle"], false, "and Line plot is not");
    } finally {
      close();
    }
  });

  test("ticking Line plot unticks Zonal average whether or not a cell is probed (#1389)", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      let from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      // No cell has been clicked: the box still takes the plot.
      let state = await panel.drive([{ id: "line-toggle", checked: true }]);
      let controls = await panel.controls();
      assert.strictEqual(controls["line-toggle"], true, "Line plot is ticked");
      assert.strictEqual(controls["zonal-toggle"], false, "so Zonal average is not");
      assert.strictEqual(state.linePlot?.shown, false, "and the zonal average is gone");

      // With a cell probed, the same box draws its line and keeps the rule.
      await panel.drive([{ id: "line-toggle", checked: false }]);
      await click(panel);
      from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      from = panel.sent.length;
      await panel.drive([{ id: "line-toggle", checked: true }]);
      await panel.waitSent(isType("lineResult"), from, "the line");
      state = await panel.drive([]);
      controls = await panel.controls();
      assert.strictEqual(state.linePlot?.label, "bt at this point, across time", "the cell's line is drawn");
      assert.strictEqual(controls["zonal-toggle"], false, "and Zonal average is unticked");
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
      await panel.drive([{ id: "slice-variable", value: one.index }]);
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

// The dashed rule marks where the clicked cell sits along the plotted axis
// (#1386). Along a held axis that is the index the stepper holds; along one of
// the image's own axes, a transect through the cell, it is the cell's row or
// column. It used to take the hidden stepper's index for every axis, so a line
// along lon was marked at lon 0 wherever the click landed.

/** The index of the dot the rule passes through, or `null` for no rule. */
function markedIndex(state: PageState): number | null {
  const plot = state.linePlot?.plot;
  if (!plot?.rule) return null;
  const rule = plot.rule;
  const on = plot.points.filter((p) => Math.abs(p.x - rule.x1) < 0.5);
  assert.strictEqual(on.length, 1, "the rule passes through one dot");
  return on[0].index;
}

/** Click the point `fy` down and `fx` across the image, and return the cell
 *  the provider resolved, once the line for it is drawn. */
async function clickCell(panel: RealPanel, fy: number, fx: number): Promise<{ gridI: number; gridJ: number }> {
  const from = panel.sent.length;
  await panel.probe(fy, fx);
  const reply = await panel.waitSent(isType("probeResult"), from, "the probe's answer");
  await panel.waitSent(isType("lineResult"), from, "the line");
  await panel.drive([]);
  assert.ok(reply.result, "the click lands on the grid");
  return { gridI: reply.result.gridI, gridJ: reply.result.gridJ };
}

/** Plot along dimension `dim` and read the page once that line is drawn. */
async function along(panel: RealPanel, dim: number): Promise<PageState> {
  const from = panel.sent.length;
  await panel.drive([{ id: "line-axis", value: String(dim) }]);
  await panel.waitSent(isType("lineResult"), from, `the line along dimension ${dim}`);
  return panel.drive([]);
}

/** Set Flip Y axis, waiting for the render a change asks for. */
async function setFlip(panel: RealPanel, checked: boolean): Promise<void> {
  if ((await panel.controls())["flip-y"] === checked) return;
  const from = panel.sent.length;
  await panel.drive([{ id: "flip-y", checked }]);
  await panel.waitSent(isType("gridReady"), from, "the flipped render");
}

/** The slice the last line was asked for, as the page sent it. */
function lastLineSlice(panel: RealPanel): { yDim: number; xDim: number; sliceIndices: number[] } {
  const requests = lineRequests(panel);
  return requests[requests.length - 1].slice;
}

suite("The line plot marks the clicked cell along the axis plotted (#1386)", function () {
  this.timeout(120_000);

  test("on a lat/lon map, a line along lon or lat marks the cell's column or row, in both flip states", async () => {
    // bt(time, lat, lon): lat is dimension 1, 3 rows; lon dimension 2, 4 columns.
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      for (const flip of [false, true]) {
        await setFlip(panel, flip);
        const rows = new Set<number>();
        // A click near the top and one near the bottom, so in either flip
        // state one lands on a row other than 0, the index the hidden lat
        // stepper holds.
        for (const fy of [0.15, 0.85]) {
          const cell = await clickCell(panel, fy, 0.65);
          rows.add(cell.gridJ);
          assert.notStrictEqual(cell.gridI, 0, "the click is away from column 0");
          const slice = lastLineSlice(panel);
          assert.deepStrictEqual([slice.yDim, slice.xDim], [1, 2], "a lat/lon map");

          const byLon = await along(panel, 2);
          assert.strictEqual(markedIndex(byLon), cell.gridI, `flip ${flip}, fy ${fy}: along lon the rule marks the clicked column`);
          const byLat = await along(panel, 1);
          assert.strictEqual(markedIndex(byLat), cell.gridJ, `flip ${flip}, fy ${fy}: along lat the rule marks the clicked row`);
        }
        assert.ok([...rows].some((j) => j !== 0), `flip ${flip}: a click landed on a row other than 0`);
      }
    } finally {
      close();
    }
  });

  test("a line along a held axis still marks the index held", async () => {
    const { panel, close } = await storePanel([1, 2, 3]);
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const from = panel.sent.length;
      await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: 2 }]);
      await panel.waitSent(isType("gridReady"), from, "time step 2");
      await clickCell(panel, 0.5, 0.65);
      // Plot along lon first, so picking time is a change that asks again.
      await along(panel, 2);
      const byTime = await along(panel, 0);
      assert.strictEqual(lastLineSlice(panel).sliceIndices[0], 2, "time is held at 2");
      assert.strictEqual(markedIndex(byTime), 2, "along time the rule marks the step held");
    } finally {
      close();
    }
  });

  test("on a time × lat cross-section, a line along time marks the clicked row, in both flip states", async () => {
    const { panel, close } = await storePanel([1, 2, 3, 4, 5]);
    try {
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from, "the cross-section");
      await panel.drive([{ id: "line-toggle", checked: true }]);
      for (const flip of [false, true]) {
        await setFlip(panel, flip);
        const rows = new Set<number>();
        for (const fy of [0.15, 0.85]) {
          const cell = await clickCell(panel, fy, 0.5);
          rows.add(cell.gridJ);
          const slice = lastLineSlice(panel);
          assert.strictEqual(slice.yDim, 0, "time runs down the rows");
          // Plot along the other image axis first, so picking time asks again.
          await along(panel, slice.xDim);
          const byTime = await along(panel, 0);
          assert.strictEqual(markedIndex(byTime), cell.gridJ, `flip ${flip}, fy ${fy}: along time the rule marks the clicked row`);
        }
        assert.ok([...rows].some((j) => j !== 0), `flip ${flip}: a click landed on a row other than 0`);
      }
    } finally {
      close();
    }
  });
});
