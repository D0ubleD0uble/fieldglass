// A CF time on a calendar other than the Gregorian one reads as its number,
// not as a Gregorian date (#1033).
//
// The panel counted every CF time on the proleptic Gregorian calendar, so a
// `360_day` axis at `360 days since 2000-01-01` read `2000-12-26` where the
// model means 2001-01-01, and a `noleap` axis drifted a day every four years.
// Until the panel counts on the file's own calendar, a time on `noleap`,
// `360_day`, `julian` or any calendar but `standard`, `gregorian` and
// `proleptic_gregorian` shows its value. That covers every place a CF time is
// shown as a date: the slice steppers, the subtitle and the export name, the
// cross-section rails and their caption, and the line plot's ticks.
//
// Driven through the real handle and the real panel on a classic NetCDF file
// built here, and on ERSST v5, whose time is `360_day`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  axisDecimals,
  calendarTicks,
  composeExportName,
  coordinateTicks,
  formatAxisValue,
  formatCoordinate,
  heldSliceParts,
  memoAxisDecimals,
  railTicks,
} from "../../render-panel";
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
} from "./real-panel";

const UNITS = "days since 2000-01-01";
const TIMES = [0, 180, 360];

function fileOf(calendar: string | null): Buffer {
  const attrs: Record<string, string> = { units: UNITS };
  if (calendar !== null) attrs.calendar = calendar;
  return classicNetcdf(
    [["time", TIMES.length], ["lat", 3], ["lon", 3]],
    [
      { name: "time", dims: ["time"], attrs, values: TIMES },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20] },
      { name: "v", dims: ["time", "lat", "lon"], attrs: {}, values: Array.from({ length: 9 * TIMES.length }, (_, k) => k) },
    ],
  );
}

function handleOf(bytes: Buffer) {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const handle = native.open(bytes);
  const found = handle.variables().find((v) => v.name.endsWith("v") || v.name.endsWith("sst"));
  assert.ok(found, "the file has its variable");
  return { handle, index: found.index };
}

async function timePanel(bytes: Buffer): Promise<{ panel: RealPanel; close: () => void }> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1033-"));
  const file = path.join(dir, "t.nc");
  fs.writeFileSync(file, bytes);
  const { index } = handleOf(bytes);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index }));
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

async function stepTime(panel: RealPanel, index: number): Promise<PageState> {
  const from = panel.sent.length;
  await panel.drive([{ selector: `#slice-dims .slice-number[data-dim="0"]`, value: index }]);
  await panel.waitSent(drawn, from, `the render at time ${index}`);
  return panel.drive([]);
}

const DATE = /^\d{4}-\d\d-\d\d/;
const NUMBER = /^\d+$/;

suite("A CF time on a non-Gregorian calendar reads as its number (#1033)", function () {
  this.timeout(120_000);

  test("the acceptance case: 360 on a 360_day calendar reads 360, never 2000-12-26", () => {
    assert.strictEqual(formatAxisValue(360, UNITS, 0, "360_day"), "360");
    // The control: on the Gregorian calendar the same value is that date.
    assert.strictEqual(formatAxisValue(360, UNITS), "2000-12-26");
    assert.strictEqual(formatAxisValue(360, UNITS, 0, "standard"), "2000-12-26");
    assert.strictEqual(formatCoordinate(360, UNITS, 0, "360_day"), "360 days since 2000-01-01", "never a bare count");
    assert.strictEqual(formatCoordinate(360, UNITS, 0, null), "2000-12-26");
  });

  test("only standard, gregorian and proleptic_gregorian are dated, in any case", () => {
    for (const calendar of [null, undefined, "", "standard", "gregorian", "proleptic_gregorian", "Gregorian", " STANDARD ", "Proleptic_Gregorian"]) {
      assert.strictEqual(formatAxisValue(360, UNITS, 0, calendar), "2000-12-26", `${JSON.stringify(calendar)} is Gregorian`);
    }
    for (const calendar of ["noleap", "365_day", "all_leap", "366_day", "360_day", "julian", "none", "NOLEAP", "utc", "tai", "something else"]) {
      assert.strictEqual(formatAxisValue(360, UNITS, 0, calendar), "360", `${calendar} is not counted as Gregorian`);
    }
  });

  test("ticks, decimals and rails fall back to numbers on a 360_day axis", () => {
    // calendarTicks has no dates to give, so a coordinate axis ticks at round
    // numbers, as any other axis does.
    assert.strictEqual(calendarTicks(0, 720, UNITS, 5, 180, "360_day"), null);
    assert.ok(calendarTicks(0, 720, UNITS, 5, 180), "the control ticks at dates");
    const axis = { coordinates: [0, 180, 360, 540, 720], units: UNITS, calendar: "360_day" };
    const ticks = coordinateTicks(axis, 5);
    assert.ok(ticks, "numeric ticks");
    assert.ok(ticks.every((t) => !DATE.test(t.text)), `no dates: ${ticks.map((t) => t.text).join(", ")}`);
    // Round numbers, each labeled with its own value: `0, 200, 400, 600`.
    assert.ok(ticks.every((t) => t.text === String(t.value)), `numbers: ${ticks.map((t) => t.text).join(", ")}`);
    const gregorian = coordinateTicks({ ...axis, calendar: "standard" }, 5);
    assert.ok(gregorian?.every((t) => DATE.test(t.text)), "the control reads dates");
    const rail = railTicks(axis, 5, 5).map((t) => t.text);
    assert.ok(rail.every((t) => !DATE.test(t)), `rail: ${rail.join(", ")}`);
    // A fine 360_day axis is a number's decimals, not the flag for seconds.
    assert.strictEqual(axisDecimals([0, 0.25, 0.5], UNITS, "360_day"), 2);
    assert.strictEqual(axisDecimals([0, 0.25, 0.5], UNITS, "standard"), 0, "six hours apart is whole minutes");
  });

  test("the real handle carries the calendar, and the export name reads the number", () => {
    const { handle, index } = handleOf(fileOf("360_day"));
    const time = handle.axisValues(index, 0);
    assert.strictEqual(time.calendar, "360_day");
    const line = handle.decodeLine({ variable: index, alongDim: 0, sliceIndices: [0, 1, 1] });
    assert.strictEqual(line.coordinateCalendar, "360_day");
    const held = heldSliceParts(
      ["time", "lat", "lon"],
      { yDim: 1, xDim: 2, sliceIndices: [2, 0, 0] },
      (dim) => handle.axisValues(index, dim),
      memoAxisDecimals,
    );
    assert.strictEqual(held?.[0].coordinate, "360 days since 2000-01-01");
    const name = composeExportName({ index, parameter: "v" }, { kind: "variable", name: "t.nc" }, "png", held ?? []);
    assert.ok(!name.includes("2000-12-26"), name);

    // A file that states no calendar is standard, and its times are dates.
    const plain = handleOf(fileOf(null));
    assert.strictEqual(plain.handle.axisValues(plain.index, 0).calendar, null);
    const dated = heldSliceParts(
      ["time", "lat", "lon"],
      { yDim: 1, xDim: 2, sliceIndices: [2, 0, 0] },
      (dim) => plain.handle.axisValues(plain.index, dim),
      memoAxisDecimals,
    );
    assert.strictEqual(dated?.[0].coordinate, "2000-12-26");

    // ERSST v5 counts minutes since 1870 on 360_day.
    const ersst = handleOf(fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "ersst_v5_187001_cdf1.nc")));
    const ersstTime = ersst.handle.axisValues(ersst.index, 0);
    assert.strictEqual(ersstTime.calendar, "360_day");
    assert.strictEqual(
      formatCoordinate(ersstTime.coordinates?.[0] ?? NaN, ersstTime.units, 0, ersstTime.calendar),
      "0 minutes since 1870-01-01 00:00",
    );
  });

  test("the stepper and the subtitle read the number", async () => {
    const { panel, close } = await timePanel(fileOf("360_day"));
    try {
      const state = await stepTime(panel, 2);
      const time = state.steppers?.main.find((s) => s.dim === 0);
      assert.strictEqual(time?.coordinate, "360 days since 2000-01-01");
      assert.ok(state.subtitle.endsWith(" · time 360 days since 2000-01-01"), state.subtitle);
      assert.ok(!state.subtitle.includes("2000-12-26"), state.subtitle);
    } finally {
      close();
    }
  });

  test("the control: the same file on the standard calendar reads the date", async () => {
    const { panel, close } = await timePanel(fileOf("standard"));
    try {
      const state = await stepTime(panel, 2);
      assert.strictEqual(state.steppers?.main.find((s) => s.dim === 0)?.coordinate, "2000-12-26");
      assert.ok(state.subtitle.endsWith(" · time 2000-12-26"), state.subtitle);
    } finally {
      close();
    }
  });

  test("a time × latitude section's rail reads numbers, with the units in its caption", async () => {
    const { panel, close } = await timePanel(fileOf("360_day"));
    try {
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 1 }, { id: "slice-x", value: 0 }]);
      await panel.waitSent((m: Msg) => drawn(m) && m.width === TIMES.length, from, "the time × lat section");
      let rails = (await panel.drive([])).rails;
      for (let i = 0; i < 100 && !(rails?.shown && rails.x.length >= 2); i++) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        rails = (await panel.drive([])).rails;
      }
      assert.ok(rails?.shown, "the rails are shown");
      const texts = rails.x.map((l) => l.text);
      assert.ok(texts.length >= 2, `time labels: ${texts.join(", ")}`);
      assert.ok(texts.every((t) => !DATE.test(t)), `no dates: ${texts.join(", ")}`);
      assert.ok(rails.caption.includes("(days since 2000-01-01)"), `the caption names the units: ${rails.caption}`);
    } finally {
      close();
    }
  });

  test("a line plot along a 360_day time ticks numbers", async () => {
    const { panel, close } = await timePanel(fileOf("360_day"));
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      let from = panel.sent.length;
      await panel.probe();
      await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      await panel.waitSent(isType("lineResult"), from, "the line");
      from = panel.sent.length;
      await panel.drive([{ id: "line-axis", value: "0" }]);
      const line = await panel.waitSent(isType("lineResult"), from, "the line along time");
      assert.strictEqual(line.result?.coordinateCalendar, "360_day", "the line carries its calendar to the page");
      let labels: string[] = [];
      for (let i = 0; i < 100; i++) {
        labels = ((await panel.drive([])).linePlot?.plot.x ?? []).map((l) => l.text);
        if (labels.length >= 2 && labels.every((t) => NUMBER.test(t))) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(labels.length >= 2, `time ticks drawn: ${labels.join(", ")}`);
      assert.ok(labels.every((t) => !DATE.test(t)), `no dates: ${labels.join(", ")}`);
      assert.ok(labels.every((t) => NUMBER.test(t)), `numbers: ${labels.join(", ")}`);
    } finally {
      close();
    }
  });
});
