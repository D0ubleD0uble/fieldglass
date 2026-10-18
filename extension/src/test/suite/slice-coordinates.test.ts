// Slice steppers name their coordinate, not just their index (#1041).
//
// A NetCDF or Zarr panel's steppers showed `level 5 … 0–16`, and the Animate
// row `step 5 of 0–16`: nothing said that level 5 is 500 millibar, or that
// time 14 is 2024-01-15. Now each stepper shows the coordinate at its index,
// formatted as the line plot's axis formats it (a CF time as a date, anything
// else as the number with its units), the Animate label leads with it, and the
// subtitle, which the PNG export draws, names the slice drawn. An axis with no
// coordinate variable keeps its index.
//
// From the same pass (§4.3): the Compare row's Field B steppers looked like
// the main ones and were taken for them, and two of them ran together as
// `0–11level`. They are now named as Field B's, set apart, and spaced.
//
// The stores mirror the two 0.6.0 sample files (`ncep_air_levels_ltm.nc`,
// `ncep_air_daily_2024.nc`, NCEP/NCAR reanalysis), which are not committed:
// the same axes, values and units, on a 3 × 4 grid. The committed
// `netcdf4_dimscale.nc` takes the NetCDF reader's path.
//
// Driven through the real panel: the steppers are the page's own, and what is
// read back is what the page shows.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { composeProbeValue, composeSliceNote, composeTitleLine, composeVectorScale, displayUnits, formatCoordinate } from "../../render-panel";
import {
  classicNetcdf,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  type StepperState,
  waitUntil,
} from "./real-panel";

/** The 17 pressure levels of the NCEP/NCAR reanalysis, in millibar. */
const LEVELS = [1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 70, 50, 30, 20, 10];
/** `ncep_air_levels_ltm.nc`'s 12 climatological months. */
const LTM_HOURS = [
  -15769752, -15769008, -15768336, -15767592, -15766872, -15766128, -15765408, -15764664, -15763920, -15763200,
  -15762456, -15761736,
];
/** The time units both NCEP files state. */
const NCEP_TIME = "hours since 1800-01-01 00:00:0.0";
/** How the shared formatter dates the climatology's first month. The file
 *  means 0001-01-01 on the CF standard calendar, which is Julian before
 *  1582; the formatter counts on the proleptic Gregorian one and says
 *  0000-12-30, as the line plot's axis does. The calendar is #1033's. */
const LTM_JANUARY = formatCoordinate(LTM_HOURS[0], NCEP_TIME);

interface ArraySpec {
  dims: string[] | null;
  shape: number[];
  values: number[];
  units?: string;
}

/** A Zarr v2 store of float64 arrays. `dims: null` writes no
 *  `_ARRAY_DIMENSIONS`, so the reader names the axes `phony_dim_N`. */
function writeStore(root: string, arrays: Record<string, ArraySpec>): void {
  fs.mkdirSync(root);
  fs.writeFileSync(path.join(root, ".zgroup"), JSON.stringify({ zarr_format: 2 }));
  for (const [name, a] of Object.entries(arrays)) {
    const dir = path.join(root, name);
    fs.mkdirSync(dir);
    fs.writeFileSync(
      path.join(dir, ".zarray"),
      JSON.stringify({
        zarr_format: 2,
        shape: a.shape,
        chunks: a.shape,
        dtype: "<f8",
        compressor: null,
        fill_value: null,
        order: "C",
        filters: null,
      }),
    );
    const attrs: Record<string, unknown> = {};
    if (a.dims) attrs._ARRAY_DIMENSIONS = a.dims;
    if (a.units) attrs.units = a.units;
    fs.writeFileSync(path.join(dir, ".zattrs"), JSON.stringify(attrs));
    const bytes = Buffer.alloc(a.values.length * 8);
    a.values.forEach((v, i) => bytes.writeDoubleLE(v, i * 8));
    fs.writeFileSync(path.join(dir, a.shape.map(() => "0").join(".")), bytes);
  }
}

const GRID = {
  lat: { dims: ["lat"], shape: [3], values: [-10, 0, 10], units: "degrees_north" },
  lon: { dims: ["lon"], shape: [4], values: [0, 90, 180, 270], units: "degrees_east" },
};

/** A field of `count` planes of 3 × 4, plane `p` holding `p` everywhere. */
const planes = (count: number) => Array.from({ length: count * 12 }, (_, i) => Math.floor(i / 12));

/** `ncep_air_levels_ltm.nc`'s axes: `air(time, level, lat, lon)`. */
function levelsStore(root: string): void {
  writeStore(root, {
    ...GRID,
    level: { dims: ["level"], shape: [17], values: LEVELS, units: "millibar" },
    time: { dims: ["time"], shape: [12], values: LTM_HOURS, units: NCEP_TIME },
    air: { dims: ["time", "level", "lat", "lon"], shape: [12, 17, 3, 4], values: planes(12 * 17), units: "degC" },
  });
}

/** `ncep_air_daily_2024.nc`'s axes: `air(time, lat, lon)`, 366 days of 2024. */
function dailyStore(root: string): void {
  const hours = Array.from({ length: 366 }, (_, d) => 1963536 + 24 * d);
  writeStore(root, {
    ...GRID,
    time: { dims: ["time"], shape: [366], values: hours, units: NCEP_TIME },
    air: { dims: ["time", "lat", "lon"], shape: [366, 3, 4], values: planes(366), units: "degK" },
  });
}

/** Three unnamed axes, which the reader names `phony_dim_0..2`. */
function phonyStore(root: string): void {
  writeStore(root, { k: { dims: null, shape: [5, 3, 4], values: planes(5) } });
}

type Opened = { panel: RealPanel; source: string; close: () => void };

/** Open a store written by `write` and wait for its first render. */
async function openStore(name: string, write: (root: string) => void): Promise<Opened> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1041-"));
  const store = path.join(dir, name);
  write(store);
  const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    source: name,
    close: () => {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const answered = (dim: number) => (m: Msg) => m && m.type === "axisResult" && m.dim === dim;
const drawn = (m: Msg) => m && m.type === "gridReady";

/** Wait until the page has every stepper's axis values, and read it. */
async function withAxes(panel: RealPanel, dims: number[]): Promise<PageState> {
  for (const dim of dims) await panel.waitSent(answered(dim), 0, `axis ${dim}'s values`);
  return panel.drive([]);
}

/** Set Field A's (or Field B's) stepper for `dim` to `index`, as a user types
 *  it, and read the page once the render it asks for has landed. */
async function step(panel: RealPanel, dim: number, index: number, fieldB = false): Promise<PageState> {
  const from = panel.sent.length;
  const container = fieldB ? "#compare-dims" : "#slice-dims";
  await panel.drive([{ selector: `${container} .slice-number[data-dim="${dim}"]`, value: index }]);
  await panel.waitSent(drawn, from, `the render at ${dim} = ${index}`);
  return panel.drive([]);
}

function stepper(list: StepperState[], dim: number): StepperState {
  const s = list.find((x) => x.dim === dim);
  assert.ok(s, `a stepper for dimension ${dim}, got ${JSON.stringify(list.map((x) => x.text))}`);
  return s;
}

function steppers(state: PageState) {
  assert.ok(state.steppers, "a slice panel has steppers");
  return state.steppers;
}

suite("Slice coordinates (#1041)", () => {
  test("a coordinate reads as the line plot's axis reads it, with its units", () => {
    assert.strictEqual(formatCoordinate(500, "millibar"), "500 millibar");
    assert.strictEqual(formatCoordinate(1963872, NCEP_TIME), "2024-01-15");
    assert.strictEqual(formatCoordinate(6, "hours since 2020-01-01 00:00:00"), "2020-01-01 06:00");
    assert.strictEqual(formatCoordinate(-5.5, "degC"), "−5.5 degC");
    assert.strictEqual(formatCoordinate(3, null), "3");
    assert.strictEqual(formatCoordinate(3, ""), "3");
    // A time whose reference cannot be read stays a number, and keeps the
    // units that say what the number counts.
    assert.strictEqual(formatCoordinate(12, "hours since the flood"), "12 hours since the flood");
    assert.strictEqual(formatCoordinate(NaN, "millibar"), "");
    // Year 1 is a date, not a count of hours (see LTM_JANUARY).
    assert.match(LTM_JANUARY, /^000[01]-\d\d-\d\d$/);
  });

  test("CF's dimensionless unit \"1\" is left out, and only exactly that", () => {
    assert.strictEqual(formatCoordinate(0.995, "1"), "0.995");
    assert.strictEqual(formatCoordinate(0.995, " 1 "), "0.995");
    assert.strictEqual(formatCoordinate(3, "   "), "3");
    assert.strictEqual(formatCoordinate(3, "1e-3"), "3 1e-3");
    assert.strictEqual(formatCoordinate(3, "100 1"), "3 100 1");
    assert.strictEqual(formatCoordinate(500, "millibar"), "500 millibar");
    assert.strictEqual(displayUnits("1"), "");
    assert.strictEqual(displayUnits(null), "");
    assert.strictEqual(displayUnits(" K "), "K");
    // The same rule wherever a value is printed with its units.
    assert.strictEqual(composeProbeValue({ value: 0.5, fullDetailValue: null, fullDetailTruncation: null }, "1"), "0.50000");
    assert.strictEqual(composeVectorScale(2, "1"), "2.00");
    assert.strictEqual(composeTitleLine({ index: 0, parameter: "sigma", units: "1" }, "variable"), "sigma");
    assert.strictEqual(composeTitleLine({ index: 0, parameter: "air", units: "K" }, "variable"), "air (K)");
  });

  test("the slice note names each held axis by coordinate, or by index where it has none", () => {
    const level = { name: "level", index: 5, coordinate: "500 millibar" };
    const time = { name: "time", index: 0, coordinate: "0001-01-01" };
    const phony = { name: "phony_dim_0", index: 3, coordinate: null };
    assert.strictEqual(composeSliceNote([time, level], null), "time 0001-01-01 · level 500 millibar");
    assert.strictEqual(composeSliceNote([phony], null), "phony_dim_0 index 3");
    assert.strictEqual(composeSliceNote([], null), "");
    // A Compare map says where Field B differs, and only there.
    const levelB = { name: "level", index: 2, coordinate: "850 millibar" };
    assert.strictEqual(
      composeSliceNote([time, level], [time, levelB]),
      "time 0001-01-01 · level 500 millibar · Field B at level 850 millibar",
    );
    assert.strictEqual(composeSliceNote([time, level], [time, level]), "time 0001-01-01 · level 500 millibar");
  });

  test("ncep_air_levels_ltm's level 5 reads 500 millibar beside the stepper, in the Animate label and the subtitle", async () => {
    const { panel, source, close } = await openStore("ncep_air_levels_ltm.zarr", levelsStore);
    try {
      let state = await withAxes(panel, [0, 1]);
      const opened = steppers(state);
      assert.strictEqual(stepper(opened.main, 0).coordinate, LTM_JANUARY, "time 0 by the shared formatter");
      assert.strictEqual(stepper(opened.main, 1).coordinate, "1000 millibar", "level 0 is the surface level");

      state = await step(panel, 1, 5);
      const level = stepper(steppers(state).main, 1);
      assert.strictEqual(level.index, "5");
      assert.strictEqual(level.coordinate, "500 millibar");
      assert.strictEqual(level.text, "level 500 millibar (0–16)");
      assert.strictEqual(state.subtitle, `${source} · time ${LTM_JANUARY} · level 500 millibar`);

      state = await panel.drive([{ id: "animate-dim", value: 1 }]);
      assert.strictEqual(state.animateLabel, "500 millibar (step 5 of 0–16)");
      // The label follows the stepper, and so does the subtitle.
      state = await step(panel, 1, 6);
      assert.strictEqual(state.animateLabel, "400 millibar (step 6 of 0–16)");
      assert.strictEqual(state.subtitle, `${source} · time ${LTM_JANUARY} · level 400 millibar`);
    } finally {
      close();
    }
  });

  test("a sigma level with units \"1\" shows no stray 1 in the stepper, the Animate label or the subtitle", async () => {
    const sigma = [0.995, 0.982, 0.964];
    const { panel, source, close } = await openStore("sigma.zarr", (root) =>
      writeStore(root, {
        ...GRID,
        sigma: { dims: ["sigma"], shape: [3], values: sigma, units: "1" },
        air: { dims: ["sigma", "lat", "lon"], shape: [3, 3, 4], values: planes(3), units: "1" },
      }),
    );
    try {
      await withAxes(panel, [0]);
      const state = await step(panel, 0, 1);
      const level = stepper(steppers(state).main, 0);
      assert.strictEqual(level.coordinate, "0.982");
      assert.strictEqual(level.text, "sigma 0.982 (0–2)");
      assert.strictEqual(state.animateLabel, "0.982 (step 1 of 0–2)");
      assert.strictEqual(state.subtitle, `${source} · sigma 0.982`);
    } finally {
      close();
    }
  });

  test("the Compare row's Field B steppers are named as Field B's, set apart, spaced, and read their own coordinate", async () => {
    const { panel, source, close } = await openStore("ncep_air_levels_ltm.zarr", levelsStore);
    try {
      await withAxes(panel, [0, 1]);
      let state = await step(panel, 1, 5);
      const from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(drawn, from, "the difference map");
      state = await step(panel, 1, 2, true);

      const s = steppers(state);
      const levelB = stepper(s.fieldB, 1);
      assert.strictEqual(levelB.text, "Field B · level 850 millibar (0–16)");
      assert.strictEqual(stepper(s.fieldB, 0).text, `Field B · time ${LTM_JANUARY} (0–11)`);
      // Field A is where it was.
      assert.strictEqual(stepper(s.main, 1).coordinate, "500 millibar");
      assert.notStrictEqual(s.fieldBStyle, s.mainStyle, "Field B's steppers are set apart from Field A's");
      assert.match(s.fieldBStyle, /^solid /, `Field B's steppers carry a rule: ${s.fieldBStyle}`);

      // Two steppers side by side are spaced and separated, not `0–11level`.
      assert.match(s.fieldBText, /\(0–11\)·Field B · level/, `a separator between the steppers: ${s.fieldBText}`);
      const [first, second] = [stepper(s.fieldB, 0), stepper(s.fieldB, 1)];
      if (Math.abs(first.box.top - second.box.top) < 1) {
        assert.ok(second.box.left - first.box.right >= 8, `room between the steppers: ${second.box.left - first.box.right} px`);
      }
      assert.strictEqual(state.subtitle, `${source} · time ${LTM_JANUARY} · level 500 millibar · Field B at level 850 millibar`);
    } finally {
      close();
    }
  });

  test("ncep_air_daily_2024's time 14 reads 2024-01-15", async () => {
    const { panel, source, close } = await openStore("ncep_air_daily_2024.zarr", dailyStore);
    try {
      await withAxes(panel, [0]);
      const state = await step(panel, 0, 14);
      const time = stepper(steppers(state).main, 0);
      assert.strictEqual(time.coordinate, "2024-01-15");
      assert.strictEqual(time.text, "time 2024-01-15 (0–365)");
      assert.strictEqual(state.animateLabel, "2024-01-15 (step 14 of 0–365)", "the Animate row starts on time");
      assert.strictEqual(state.subtitle, `${source} · time 2024-01-15`);
    } finally {
      close();
    }
  });

  test("an axis with no coordinate variable shows its index", async () => {
    const { panel, source, close } = await openStore("phony.zarr", phonyStore);
    try {
      const opened = await panel.drive([]);
      const [only] = steppers(opened).main;
      assert.ok(only, "one stepper");
      await panel.waitSent(answered(only.dim), 0, "the stepper's axis");
      const state = await step(panel, only.dim, 3);
      const s = stepper(steppers(state).main, only.dim);
      assert.match(s.text, /^phony_dim_\d \(0–\d\)$/, `the name and the range, no coordinate: ${s.text}`);
      assert.strictEqual(s.coordinate, "");
      assert.strictEqual(s.index, "3");
      const name = s.text.split(" ")[0];
      assert.strictEqual(state.subtitle, `${source} · ${name} index 3`);
      assert.strictEqual(state.animateLabel, `step 3 of 0–${/\(0–(\d)\)/.exec(s.text)?.[1]}`);
    } finally {
      close();
    }
  });

  test("a NetCDF file's time axis reads as dates too", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
    try {
      await panel.waitSent(drawn, 0, "the first render");
      await withAxes(panel, [0]);
      const state = await step(panel, 0, 1);
      assert.strictEqual(stepper(steppers(state).main, 0).coordinate, "2020-01-01 06:00");
      assert.strictEqual(state.subtitle, "netcdf4_dimscale.nc · time 2020-01-01 06:00");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("an axis answer for another variable does not label this one", async () => {
    const { panel, close } = await openStore("ncep_air_levels_ltm.zarr", levelsStore);
    try {
      await withAxes(panel, [0, 1]);
      // An answer the page asked for before it moved to another variable, as
      // it would arrive after the move.
      await panel.post({
        type: "axisResult",
        dim: 1,
        variableIndex: 999,
        result: { dimension: "level", length: 17, coordinates: LEVELS.map(() => 1), units: "bogus", vertical: null },
      });
      const state = await panel.drive([]);
      assert.strictEqual(stepper(steppers(state).main, 1).coordinate, "1000 millibar");
    } finally {
      close();
    }
  });

  test("a page written again over a reopened file still names its slice", async () => {
    // A page written again (#839, #959) has every request before its ready
    // dropped, the stepper axes' too, so they are asked for again after it.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1041-"));
    const file = path.join(dir, "levels.nc");
    fs.writeFileSync(
      file,
      classicNetcdf(
        [["level", LEVELS.length], ["lat", 3], ["lon", 4]],
        [
          { name: "level", dims: ["level"], attrs: { units: "millibar" }, values: LEVELS },
          { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: GRID.lat.values },
          { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: GRID.lon.values },
          { name: "air", dims: ["level", "lat", "lon"], attrs: { units: "degC" }, values: planes(LEVELS.length) },
        ],
      ),
    );
    const native = loadNative();
    assert.ok(native, "native binding required");
    const air = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name === "air");
    assert.ok(air, "air is renderable");
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: air.variableIndex }));
    try {
      await panel.waitSent(drawn, 0, "the first render");
      await withAxes(panel, [0]);
      let state = await step(panel, 0, 5);
      assert.strictEqual(stepper(steppers(state).main, 0).coordinate, "500 millibar");

      editor.close();
      editor = await openEditor(p, file);
      const sent = panel.sent.length;
      const received = panel.received.length;
      const rewritten = new Promise<void>((resolve) => panel.onNextWrite(resolve));
      // A click is a request too, which finds the file reopened.
      await panel.probe();
      await rewritten;
      await waitUntil(() => panel.received.slice(received).find(isType("ready")), "the new page's ready");
      await panel.waitSent(drawn, sent, "the rewritten page's render");
      await panel.waitSent(answered(0), sent, "the level axis, asked for again");
      state = await panel.drive([]);
      const s = steppers(state);
      assert.strictEqual(stepper(s.main, 0).index, "5", "the page is restored at level 5");
      assert.strictEqual(stepper(s.main, 0).coordinate, "500 millibar");
      assert.strictEqual(stepper(s.fieldB, 0).coordinate, "1000 millibar", "Field B's stepper too");
      assert.strictEqual(state.animateLabel, "500 millibar (step 5 of 0–16)");
      assert.strictEqual(state.subtitle, "levels.nc · level 500 millibar");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
