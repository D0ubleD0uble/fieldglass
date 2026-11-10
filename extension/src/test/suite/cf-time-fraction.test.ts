// A CF time on an axis stepping by under a second reads with the fraction of
// a second it needs (#1481).
//
// #1462 wrote seconds when an axis steps by under a minute, but rounded every
// time to the whole second, so `seconds since …` values 0.5 apart read
// `00:00:01`, `00:00:01`, `00:00:02`: neighboring steps alike, and two exports
// with one file name. The axis now takes the fraction its steps need, the same
// on every label.
//
// Driven through the real panel on a classic NetCDF file written by the
// `classicNetcdf` test helper (built in memory, no committed fixture): a time
// axis of [0, 0.5, 1, 1.5] in `seconds since 1970-01-01`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  axisDecimals,
  composeExportName,
  formatAxisValue,
  formatCoordinate,
  heldSliceParts,
  memoAxisDecimals,
  railTicks,
} from "../../render-panel";
import { classicNetcdf, isType, type Msg, openEditor, openReal, type PageState, provider, type RealPanel } from "./real-panel";

const SECONDS = "seconds since 1970-01-01";
const HALVES = [0, 0.5, 1, 1.5];

function fileOf(times: number[]): Buffer {
  return classicNetcdf(
    [["time", times.length], ["lat", 3], ["lon", 3]],
    [
      { name: "time", dims: ["time"], attrs: { units: SECONDS }, values: times },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20] },
      { name: "v", dims: ["time", "lat", "lon"], attrs: {}, values: Array.from({ length: 9 * times.length }, (_, k) => k) },
    ],
  );
}

const labels = (values: number[], units = SECONDS) => {
  const d = axisDecimals(values, units);
  return values.map((v) => formatAxisValue(v, units, d));
};

suite("A CF time under a second apart reads with its fraction (#1481)", function () {
  this.timeout(120_000);

  test("one formatter: the fraction the steps need, the same on every label", () => {
    assert.deepStrictEqual(labels(HALVES), [
      "1970-01-01 00:00:00.0",
      "1970-01-01 00:00:00.5",
      "1970-01-01 00:00:01.0",
      "1970-01-01 00:00:01.5",
    ]);
    assert.deepStrictEqual(labels([0, 0.25, 0.5, 0.75]), [
      "1970-01-01 00:00:00.00", "1970-01-01 00:00:00.25", "1970-01-01 00:00:00.50", "1970-01-01 00:00:00.75",
    ]);
    // 10 Hz with float noise in the values (0.1 * 3 is 0.30000000000000004).
    const tenth = [0, 1, 2, 3, 4].map((k) => 1700000000 + k * 0.1);
    assert.deepStrictEqual(labels(tenth), [
      "2023-11-14 22:13:20.0", "2023-11-14 22:13:20.1", "2023-11-14 22:13:20.2", "2023-11-14 22:13:20.3", "2023-11-14 22:13:20.4",
    ]);
    // Milliseconds, and no finer: a Date has no more.
    assert.deepStrictEqual(labels([0, 0.001, 0.002]), ["1970-01-01 00:00:00.000", "1970-01-01 00:00:00.001", "1970-01-01 00:00:00.002"]);
    assert.strictEqual(axisDecimals([0, 0.0006, 0.0012], SECONDS), 4, "capped at milliseconds");
    // Another unit: 0.5 s in minutes; instants before the epoch.
    const minutes = [0, 1, 2].map((k) => k / 120);
    assert.deepStrictEqual(labels(minutes, "minutes since 2000-01-01"), ["2000-01-01 00:00:00.0", "2000-01-01 00:00:00.5", "2000-01-01 00:00:01.0"]);
    assert.deepStrictEqual(labels([-1, -0.5, 0]), ["1969-12-31 23:59:59.0", "1969-12-31 23:59:59.5", "1970-01-01 00:00:00.0"]);
  });

  test("an axis a second or more apart reads as it did", () => {
    assert.strictEqual(axisDecimals([0, 1, 2], SECONDS), 1);
    assert.strictEqual(axisDecimals([0, 20, 40], SECONDS), 1);
    assert.strictEqual(axisDecimals([0, 60, 120], SECONDS), 0);
    assert.strictEqual(formatAxisValue(1.5, SECONDS, 1), "1970-01-01 00:00:02");
    // Float noise between near-equal values is not a fine axis.
    assert.strictEqual(axisDecimals([60000, 60000 + 1e-12, 60001], "days since 1850-01-01"), 0);
    assert.strictEqual(formatAxisValue(0, SECONDS), "1970-01-01");
  });

  test("a coordinate carries the fraction and no units", () => {
    assert.strictEqual(formatCoordinate(0.5, SECONDS, axisDecimals(HALVES, SECONDS)), "1970-01-01 00:00:00.5");
  });

  test("the export name keeps the fraction, and two steps do not share a name", () => {
    const held = (index: number) =>
      heldSliceParts(
        ["time", "lat", "lon"],
        { yDim: 1, xDim: 2, sliceIndices: [index, 0, 0] },
        (dim) => (dim === 0 ? { coordinates: HALVES, units: SECONDS } : null),
        memoAxisDecimals,
      );
    const name = (index: number) => {
      const parts = held(index);
      assert.ok(parts);
      return composeExportName({ index: 0, parameter: "v" }, { kind: "variable", name: "t.nc" }, "png", parts);
    };
    assert.strictEqual(name(1), "t_v_time-1970-01-01-000000.5.png");
    assert.strictEqual(name(2), "t_v_time-1970-01-01-000001.0.png");
    assert.notStrictEqual(name(2), name(3));
  });

  test("a cross-section rail's labels each read their own fraction", () => {
    const texts = railTicks({ coordinates: HALVES, units: SECONDS }, HALVES.length, 4).map((t) => t.text);
    assert.strictEqual(new Set(texts).size, texts.length, `no two alike: ${texts.join(", ")}`);
    assert.ok(texts.every((t) => /:\d\d\.\d$/.test(t)), `each with its fraction: ${texts.join(", ")}`);
  });

  test("the stepper, Animate label and subtitle tell the half seconds apart", async () => {
    const bytes = fileOf(HALVES);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1481-"));
    const file = path.join(dir, "t.nc");
    fs.writeFileSync(file, bytes);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const found = native.open(bytes).variables().find((v) => v.name === "v");
    assert.ok(found, "the file has v");
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel: RealPanel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: found.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "v drawn");
      await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
      const step = async (index: number): Promise<PageState> => {
        const from = panel.sent.length;
        await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: index }]);
        await panel.waitSent((m: Msg) => m && m.type === "gridReady", from, `the render at time ${index}`);
        return panel.drive([]);
      };
      assert.strictEqual((await panel.drive([])).steppers?.main[0].coordinate, "1970-01-01 00:00:00.0");
      const one = await step(1);
      assert.strictEqual(one.steppers?.main.find((s) => s.dim === 0)?.coordinate, "1970-01-01 00:00:00.5");
      assert.ok(one.subtitle.endsWith(" · time 1970-01-01 00:00:00.5"), one.subtitle);
      const two = await step(2);
      assert.strictEqual(two.steppers?.main.find((s) => s.dim === 0)?.coordinate, "1970-01-01 00:00:01.0");
      assert.notStrictEqual(two.subtitle, one.subtitle, "stepping from 1 to 2 changes the label");
      const three = await step(3);
      assert.ok(three.subtitle.endsWith(" · time 1970-01-01 00:00:01.5"), three.subtitle);
      const animate = await panel.drive([{ id: "animate-dim", value: 0 }]);
      assert.strictEqual(animate.animateLabel, "1970-01-01 00:00:01.5 (step 3 of 0–3)");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
