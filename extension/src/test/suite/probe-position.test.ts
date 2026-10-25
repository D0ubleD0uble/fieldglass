// The probe writes its position with the decimals the grid needs (#1466).
//
// The readout wrote latitude and longitude with two decimals, so on a grid
// finer than 0.01 degrees two neighbouring cells read the same position. The
// position now takes the spacing between the probed cell and its neighbours
// (Rust's `PixelProbe.cell_step`), counted as axis ticks count their step
// (#1459), and never fewer than two decimals, so a coarse grid reads as it did.
//
// Driven through the real panel and the real addon on files the test writes:
// `samples/` is not in CI.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { formatLatLon } from "../../render-panel";
import { classicNetcdf, extensionPath, isType, openEditor, openReal, provider, type RealPanel } from "./real-panel";

/** `v(lat, lon)` on a regular lat/lon grid `ni` wide, `step` degrees apart. */
function gridFile(ni: number, nj: number, step: number, lat0: number, lon0: number): Buffer {
  const lat = Array.from({ length: nj }, (_, j) => Number((lat0 + j * step).toFixed(6)));
  const lon = Array.from({ length: ni }, (_, i) => Number((lon0 + i * step).toFixed(6)));
  return classicNetcdf(
    [["lat", nj], ["lon", ni]],
    [
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: lat },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: lon },
      {
        name: "v",
        dims: ["lat", "lon"],
        attrs: { units: "K" },
        values: Array.from({ length: ni * nj }, (_, k) => 250 + k),
      },
    ],
  );
}

/** The position in a readout, such as `12.3456°N, 98.7654°E`. */
function position(readout: string): { text: string; decimals: number[]; lon: number } {
  const m = /([\d.]+)°[NS], ([\d.]+)°([EW])/.exec(readout);
  assert.ok(m, `a position in the readout: ${readout}`);
  const decimals = (v: string) => v.length - v.indexOf(".") - 1;
  return { text: m[0], decimals: [decimals(m[1]), decimals(m[2])], lon: Number(m[2]) * (m[3] === "W" ? -1 : 1) };
}

/** The readout once the page has written a position into it. */
async function readoutWithPosition(panel: RealPanel): Promise<string> {
  for (let tries = 0; tries < 50; tries++) {
    const state = await panel.drive([]);
    if (state.probe.includes("°")) return state.probe;
    await panel.pageTimer(100);
  }
  throw new Error("the probe readout never carried a position");
}

/** Click `fx` of the way across and `fy` down; the cell and the readout it wrote. */
async function click(panel: RealPanel, fy: number, fx: number): Promise<{ gridI: number; readout: string }> {
  const from = panel.sent.length;
  await panel.probe(fy, fx);
  const result = (await panel.waitSent(isType("probeResult"), from, "the probe")).result;
  assert.ok(result && result.gridI != null, "the click lands on a cell");
  return { gridI: result.gridI, readout: await readoutWithPosition(panel) };
}

suite("The probe's position reads as finely as the grid (#1466)", function () {
  this.timeout(120_000);

  async function onFile(bytes: Buffer, body: (panel: RealPanel) => Promise<void>): Promise<void> {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1466-"));
    const file = path.join(dir, "grid.nc");
    fs.writeFileSync(file, bytes);
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: 2 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await body(panel);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  test("on a grid finer than 0.01 degrees, neighbouring cells read different positions", async () => {
    await onFile(gridFile(20, 12, 0.001, 40.0, 10.0), async (panel) => {
      const a = await click(panel, 0.5, 0.5);
      const b = await click(panel, 0.5, 0.5 + 1 / 20);
      assert.strictEqual(b.gridI - a.gridI, 1, `neighbouring columns (${a.gridI}, ${b.gridI})`);
      const pa = position(a.readout);
      const pb = position(b.readout);
      assert.notStrictEqual(pa.text, pb.text, `${pa.text} and ${pb.text} must differ`);
      assert.notStrictEqual(pa.lon, pb.lon, "the longitudes differ");
      assert.deepStrictEqual(pa.decimals, [4, 4], a.readout);
    });
  });

  test("on a coarse grid the position reads two decimals", async () => {
    for (const step of [10, 0.25, 0.1, 0.05]) {
      await onFile(gridFile(8, 6, step, -10, 100), async (panel) => {
        const a = await click(panel, 0.5, 0.5);
        assert.deepStrictEqual(position(a.readout).decimals, [2, 2], `${step} degrees apart: ${a.readout}`);
      });
    }
  });

  test("a GRIB grid reads two decimals as before", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const a = await click(panel, 0.4, 0.4);
      assert.deepStrictEqual(position(a.readout).decimals, [2, 2], a.readout);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("on a 0.01 degree grid, such as MRMS, the position reads three decimals", async () => {
    await onFile(gridFile(20, 12, 0.01, 40.0, 10.0), async (panel) => {
      const a = await click(panel, 0.5, 0.5);
      const b = await click(panel, 0.5, 0.5 + 1 / 20);
      assert.strictEqual(b.gridI - a.gridI, 1, `neighbouring columns (${a.gridI}, ${b.gridI})`);
      assert.notStrictEqual(position(a.readout).text, position(b.readout).text);
      assert.deepStrictEqual(position(a.readout).decimals, [3, 3], a.readout);
    });
  });

  test("formatLatLon: two decimals at least, more as the step needs, none without a step", () => {
    const at = (step?: number | null) => formatLatLon(12.3456, -98.7654, step);
    assert.strictEqual(at(), "12.35°N, 98.77°W");
    assert.strictEqual(at(10), "12.35°N, 98.77°W");
    assert.strictEqual(at(0.25), "12.35°N, 98.77°W");
    assert.strictEqual(at(0.1), "12.35°N, 98.77°W");
    assert.strictEqual(at(0.05), "12.35°N, 98.77°W");
    assert.strictEqual(at(0.01), "12.346°N, 98.765°W");
    assert.strictEqual(at(0.0100000000000002), "12.346°N, 98.765°W");
    assert.strictEqual(at(0.0099999999999998), "12.346°N, 98.765°W");
    assert.strictEqual(at(0.001), "12.3456°N, 98.7654°W");
    assert.strictEqual(at(0.0005), "12.3456°N, 98.7654°W");
    assert.strictEqual(formatLatLon(-0.00012345, 0.00045678, 0.00001), "0.000123°S, 0.000457°E");
    // Stops at eight decimals, however fine the step.
    assert.strictEqual(formatLatLon(1, 2, 1e-12), "1.00000000°N, 2.00000000°E");
    assert.strictEqual(formatLatLon(null, 2, 0.001), "");
  });
});
