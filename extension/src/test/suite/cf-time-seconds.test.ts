// A CF time a few seconds from the next reads with its seconds (#1462).
//
// A CF time was written `YYYY-MM-DD HH:MM`, so on an axis stepping by 20 or 30
// seconds neighboring steps read alike in the slice stepper, the subtitle and
// the line plot's ticks. It now carries its seconds when the axis steps by
// under a minute or the time falls off a whole minute; whole minutes and
// midnights on an ordinary axis read as before.
//
// Driven through the real panel on a classic NetCDF file whose time axis is
// 20 s apart.

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
import {
  classicNetcdf,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
} from "./real-panel";

const SECONDS = "seconds since 1970-01-01T00:00:00Z";
/** 2023-11-14 22:13:10, then 20 s apart: 22:13:30, 22:13:50, 22:14:10. */
const T0 = 1699999990;
const TIMES = [0, 1, 2, 3].map((k) => T0 + 20 * k);

function fileOf(timeUnits: string, times: number[]): Buffer {
  return classicNetcdf(
    [["time", times.length], ["lat", 3], ["lon", 3]],
    [
      { name: "time", dims: ["time"], attrs: { units: timeUnits }, values: times },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20] },
      { name: "v", dims: ["time", "lat", "lon"], attrs: {}, values: Array.from({ length: 9 * times.length }, (_, k) => k) },
    ],
  );
}

async function timePanel(bytes: Buffer): Promise<{ panel: RealPanel; close: () => void }> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1462-"));
  const file = path.join(dir, "t.nc");
  fs.writeFileSync(file, bytes);
  const native = loadNative();
  assert.ok(native, "native binding required");
  const found = native.open(bytes).variables().find((v) => v.name === "v");
  assert.ok(found, "the file has v");
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: found.index }));
  await panel.waitSent(isType("gridReady"), 0, "v drawn");
  await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const drawn = (m: Msg) => m && m.type === "gridReady";

async function stepTime(panel: RealPanel, index: number, fieldB = false): Promise<PageState> {
  const from = panel.sent.length;
  const container = fieldB ? "#compare-dims" : "#slice-dims";
  await panel.drive([{ selector: `${container} .slice-number[data-dim="0"]`, value: index }]);
  await panel.waitSent(drawn, from, `the render at time ${index}`);
  return panel.drive([]);
}

suite("A CF time reads with its seconds where they tell steps apart (#1462)", function () {
  this.timeout(120_000);

  test("one formatter: seconds on a fine axis or off a whole minute, nothing else changes", () => {
    const fine = axisDecimals(TIMES, SECONDS);
    assert.strictEqual(fine, 1, "an axis 20 s apart");
    assert.strictEqual(formatAxisValue(TIMES[1], SECONDS, fine), "2023-11-14 22:13:30");
    // A midnight on a fine axis reads like its neighbors.
    assert.strictEqual(formatAxisValue(0, SECONDS, fine), "1970-01-01 00:00:00");
    // Off a whole minute on a coarse axis.
    assert.strictEqual(formatAxisValue(1700000000, SECONDS), "2023-11-14 22:13:20");
    // Whole minutes and midnights read as they did.
    assert.strictEqual(formatAxisValue(1700000040, SECONDS), "2023-11-14 22:14");
    assert.strictEqual(formatAxisValue(1699920000, SECONDS), "2023-11-14");
    assert.strictEqual(formatAxisValue(6, "hours since 2020-01-01 00:00:00"), "2020-01-01 06:00");
    // A value a rounding error short of a minute is that minute.
    assert.strictEqual(formatAxisValue(1700000040 - 1e-7, SECONDS), "2023-11-14 22:14");
    // A minute apart is not under a minute; a quarter hour is 15 minutes.
    assert.strictEqual(axisDecimals([0, 60, 120], SECONDS), 0);
    assert.strictEqual(axisDecimals([0, 30, 60], "minutes since 2000-01-01"), 0);
    assert.strictEqual(axisDecimals([0, 0.25], "hours since 2000-01-01"), 0);
    assert.strictEqual(axisDecimals([0, 0.5], "minutes since 2000-01-01"), 1, "half a minute is 30 s");
    // A minute apart with float noise in the values is still a minute.
    const days = Array.from({ length: 5 }, (_, k) => 60000 + k / 1440);
    assert.strictEqual(axisDecimals(days, "days since 1850-01-01 00:00:00"), 0);
    const hours = Array.from({ length: 5 }, (_, k) => 1e6 + k / 60);
    assert.strictEqual(axisDecimals(hours, "hours since 1900-01-01 00:00:00"), 0);
    for (const [values, units] of [[days, "days since 1850-01-01 00:00:00"], [hours, "hours since 1900-01-01 00:00:00"]] as const) {
      const texts = values.map((v) => formatAxisValue(v, units, axisDecimals(values, units)));
      assert.ok(texts.every((t) => /^\d{4}-\d\d-\d\d( \d\d:\d\d)?$/.test(t)), `no seconds: ${texts.join(", ")}`);
    }
    // 90 s apart: one form on the whole axis, midnight included.
    const ninety = [0, 90, 180, 270];
    const flag = axisDecimals(ninety, SECONDS);
    assert.strictEqual(flag, 1);
    assert.deepStrictEqual(
      ninety.map((v) => formatAxisValue(v, SECONDS, flag)),
      ["1970-01-01 00:00:00", "1970-01-01 00:01:30", "1970-01-01 00:03:00", "1970-01-01 00:04:30"],
    );
    assert.strictEqual(formatCoordinate(TIMES[0], SECONDS, fine), "2023-11-14 22:13:10", "a time carries no units after it");
  });

  test("the export name and the slice parts carry the seconds", () => {
    const held = (index: number) =>
      heldSliceParts(
        ["time", "lat", "lon"],
        { yDim: 1, xDim: 2, sliceIndices: [index, 0, 0] },
        (dim) => (dim === 0 ? { coordinates: TIMES, units: SECONDS } : null),
        memoAxisDecimals,
      );
    const one = held(1);
    assert.ok(one);
    assert.strictEqual(one[0].coordinate, "2023-11-14 22:13:30");
    assert.strictEqual(
      composeExportName({ index: 0, parameter: "v" }, { kind: "variable", name: "t.nc" }, "png", one),
      "t_v_time-2023-11-14-221330.png",
    );
    // The next step is not the same name again.
    const two = held(2);
    assert.ok(two);
    assert.notStrictEqual(two[0].coordinate, one[0].coordinate);
    // A daily axis keeps its name.
    const daily = heldSliceParts(
      ["time", "lat", "lon"],
      { yDim: 1, xDim: 2, sliceIndices: [1, 0, 0] },
      () => ({ coordinates: [0, 24, 48], units: "hours since 2020-01-01 00:00:00" }),
      memoAxisDecimals,
    );
    assert.strictEqual(daily?.[0].coordinate, "2020-01-02");
  });

  test("a cross-section rail's labels on a fine time axis each read their own second", () => {
    const axis = { coordinates: TIMES, units: SECONDS };
    const texts = railTicks(axis, TIMES.length, 4).map((t) => t.text);
    assert.strictEqual(new Set(texts).size, texts.length, `no two alike: ${texts.join(", ")}`);
    assert.ok(texts.every((t) => /:\d\d:\d\d$/.test(t)), `each with its seconds: ${texts.join(", ")}`);
  });

  test("the stepper, Animate label, subtitle and Field B stepper read the second", async () => {
    const { panel, close } = await timePanel(fileOf(SECONDS, TIMES));
    try {
      const first = await panel.drive([]);
      assert.strictEqual(first.steppers?.main[0].coordinate, "2023-11-14 22:13:10");
      let state = await stepTime(panel, 1);
      const time = state.steppers?.main.find((s) => s.dim === 0);
      assert.strictEqual(time?.coordinate, "2023-11-14 22:13:30");
      assert.strictEqual(time?.text, "time 2023-11-14 22:13:30 (0–3)");
      assert.ok(state.subtitle.endsWith(" · time 2023-11-14 22:13:30"), state.subtitle);
      state = await panel.drive([{ id: "animate-dim", value: 0 }]);
      assert.strictEqual(state.animateLabel, "2023-11-14 22:13:30 (step 1 of 0–3)");
      state = await stepTime(panel, 2);
      assert.ok(state.subtitle.endsWith(" · time 2023-11-14 22:13:50"), "the next step reads differently: " + state.subtitle);

      const from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(drawn, from, "the difference map");
      state = await stepTime(panel, 3, true);
      const b = state.steppers?.fieldB.find((s) => s.dim === 0);
      assert.strictEqual(b?.text, "Field B · time 2023-11-14 22:14:10 (0–3)");
      assert.ok(state.subtitle.endsWith(" · Field B at time 2023-11-14 22:14:10"), state.subtitle);
    } finally {
      close();
    }
  });

  test("a line plot along a fine time axis ticks its points with their seconds", async () => {
    const { panel, close } = await timePanel(fileOf(SECONDS, TIMES));
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      let from = panel.sent.length;
      await panel.probe();
      await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      await panel.waitSent(isType("lineResult"), from, "the line");
      from = panel.sent.length;
      await panel.drive([{ id: "line-axis", value: "0" }]);
      await panel.waitSent(isType("lineResult"), from, "the line along time");
      let labels: string[] = [];
      // The axis's values come after the first line.
      for (let i = 0; i < 100; i++) {
        labels = ((await panel.drive([])).linePlot?.plot.x ?? []).map((l) => l.text);
        if (labels.length >= 2 && labels.every((t) => /^\d{4}-/.test(t))) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(labels.length >= 2, `time ticks drawn: ${labels.join(", ")}`);
      assert.strictEqual(new Set(labels).size, labels.length, `no two alike: ${labels.join(", ")}`);
      assert.ok(labels.every((t) => /\d\d:\d\d:\d\d$/.test(t)), `each with its seconds: ${labels.join(", ")}`);
    } finally {
      close();
    }
  });

  test("a daily axis still reads dates", async () => {
    const days = [0, 1, 2].map((k) => 24 * k);
    const { panel, close } = await timePanel(fileOf("hours since 2020-01-01 00:00:00", days));
    try {
      const state = await stepTime(panel, 1);
      assert.strictEqual(state.steppers?.main.find((s) => s.dim === 0)?.coordinate, "2020-01-02");
      assert.ok(state.subtitle.endsWith(" · time 2020-01-02"), state.subtitle);
    } finally {
      close();
    }
  });
});
