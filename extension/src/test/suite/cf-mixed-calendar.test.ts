// A CF time on the `standard` calendar is dated on the mixed Julian/Gregorian
// calendar CF 4.4.1 defines (#1574): Julian before 1582-10-15, Gregorian from
// it, with the time elapsed across the switch counted as it is.
//
// The panel dated every standard-calendar time on the proleptic Gregorian
// calendar, so NCEP/NCAR reanalysis files, whose time is "hours since 1-1-1
// 00:00:0.0", read two days late: 1948-01-03 for 1948-01-01. Every expected
// date here is cftime 1.6.5's `num2date` for the same value, units and
// calendar.
//
// Driven through the pure formatters, the real handle and the real panel on a
// classic NetCDF file built here.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  axisDecimals,
  calendarTicks,
  coordinateTicks,
  datedAxis,
  formatAxisValue,
  formatCoordinate,
  heldSliceParts,
  memoAxisDecimals,
  railTicks,
} from "../../render-panel";
import { classicNetcdf, isType, type Msg, openEditor, openReal, provider, type RailsState, type RealPanel } from "./real-panel";

// The NCEP/NCAR reanalysis spelling, fractional seconds and all.
const NCEP = "hours since 1-1-1 00:00:0.0";
// 1948-01-01 00:00, 06:00 and 12:00 on the mixed calendar.
const TIMES = [17067072, 17067078, 17067084];

function fileOf(calendar: string | null): Buffer {
  const attrs: Record<string, string> = { units: NCEP };
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
  const found = handle.variables().find((v) => v.name.endsWith("v"));
  assert.ok(found, "the file has its variable");
  return { handle, index: found.index };
}

suite("A standard-calendar CF time is dated on the mixed Julian/Gregorian calendar (#1574)", function () {
  this.timeout(120_000);

  test("the acceptance case: NCEP's hours since 1-1-1 read 1948-01-01, not 1948-01-03", () => {
    for (const calendar of [null, undefined, "", "standard", "gregorian", " Standard "]) {
      const label = JSON.stringify(calendar);
      assert.strictEqual(formatAxisValue(17067072, NCEP, 0, calendar), "1948-01-01", label);
      assert.strictEqual(formatAxisValue(17067078, NCEP, 0, calendar), "1948-01-01 06:00", label);
      assert.strictEqual(formatAxisValue(0, NCEP, 0, calendar), "0001-01-01", `${label}: the reference itself`);
    }
    assert.strictEqual(formatCoordinate(17067072, NCEP, 0, "standard"), "1948-01-01");
  });

  test("proleptic_gregorian keeps proleptic dates", () => {
    assert.strictEqual(formatAxisValue(17067072, NCEP, 0, "proleptic_gregorian"), "1948-01-03");
    assert.strictEqual(formatAxisValue(13865664, NCEP, 0, "proleptic_gregorian"), "1582-10-16");
    assert.strictEqual(formatAxisValue(13865640, NCEP, 0, "proleptic_gregorian"), "1582-10-15");
    assert.strictEqual(formatAxisValue(0, NCEP, 0, "proleptic_gregorian"), "0001-01-01");
    assert.strictEqual(formatAxisValue(-40000, "days since 1600-01-01", 0, "proleptic_gregorian"), "1490-06-26");
  });

  test("a time before the switch reads its Julian date, and the switch skips ten days", () => {
    // Both sides of 1582-10-15 00:00, an hour apart.
    assert.strictEqual(formatAxisValue(13865687, NCEP, 0, "standard"), "1582-10-04 23:00");
    assert.strictEqual(formatAxisValue(13865688, NCEP, 0, "standard"), "1582-10-15");
    assert.strictEqual(formatAxisValue(13865689, NCEP, 0, "standard"), "1582-10-15 01:00");
    assert.strictEqual(formatAxisValue(13865676, NCEP, 0, "standard"), "1582-10-04 12:00");
    assert.strictEqual(formatAxisValue(13865664, NCEP, 0, "standard"), "1582-10-04");
    assert.strictEqual(formatAxisValue(13865640, NCEP, 0, "standard"), "1582-10-03");
    // 1500 is a leap year on the Julian calendar, not on the Gregorian.
    assert.strictEqual(formatAxisValue(13141632, NCEP, 0, "standard"), "1500-02-29");
    // A reference after the switch, counted back across it.
    assert.strictEqual(formatAxisValue(-40000, "days since 1600-01-01", 0, "standard"), "1490-06-17");
    assert.strictEqual(formatAxisValue(-6000, "days since 1600-01-01", 0, "standard"), "1583-07-29");
  });

  test("a reference either side of the switch: Julian on 1582-10-04, Gregorian from 1582-10-15", () => {
    const julian = "days since 1582-10-04";
    assert.deepStrictEqual([0, 1, 2].map((v) => formatAxisValue(v, julian, 0, "standard")), ["1582-10-04", "1582-10-15", "1582-10-16"]);
    const gregorian = "days since 1582-10-15";
    assert.deepStrictEqual([-1, 0, 1].map((v) => formatAxisValue(v, gregorian, 0, "standard")), ["1582-10-04", "1582-10-15", "1582-10-16"]);
    const lastHour = "hours since 1582-10-04 23:00";
    assert.deepStrictEqual([0, 1].map((v) => formatAxisValue(v, lastHour, 0, "standard")), ["1582-10-04 23:00", "1582-10-15"]);
  });

  test("a time the mixed calendar has no date for reads as its value and units", () => {
    // 1582-10-05 to 1582-10-14 do not exist on it, and cftime refuses them.
    for (const reference of ["1582-10-05", "1582-10-10", "1582-10-14"]) {
      const units = "days since " + reference;
      assert.strictEqual(formatAxisValue(0, units, 0, "standard"), "0", reference);
      assert.strictEqual(formatCoordinate(0, units, 0, "standard"), "0 " + units, reference);
      assert.strictEqual(formatAxisValue(0, units, 0, "proleptic_gregorian"), reference, "a proleptic date exists");
    }
    // Nor does year 0, as a reference or a time.
    assert.strictEqual(formatCoordinate(0, "days since 0-1-1", 0, "standard"), "0 days since 0-1-1");
    assert.strictEqual(formatCoordinate(400, "days since 0-1-1", 0, "standard"), "400 days since 0-1-1", "a time in year 1 too");
    assert.strictEqual(formatAxisValue(0, "days since 0-1-1", 0, "proleptic_gregorian"), "0000-01-01");
    assert.strictEqual(formatAxisValue(-1, "days since 1-1-1", 0, "standard"), "−1", "cftime's -0001-12-31");
  });

  test("julian and the other calendars still read as numbers (#1033)", () => {
    for (const calendar of ["julian", "noleap", "360_day"]) {
      assert.strictEqual(formatAxisValue(17067072, NCEP, 0, calendar), "17067072", calendar);
    }
  });

  test("ticks across the switch are Julian before it and Gregorian after", () => {
    const ticks = calendarTicks(13865664, 13865664 + 48, NCEP, 3, 24, "standard");
    assert.deepStrictEqual(ticks, [
      { value: 13865664, text: "1582-10-04" },
      { value: 13865688, text: "1582-10-15" },
      { value: 13865712, text: "1582-10-16" },
    ]);
    // Whole years on the Julian calendar before the switch (#1593), as
    // cftime counts them: 0500-01-01, 1000-01-01, 1500-01-01.
    assert.deepStrictEqual(calendarTicks(0, 17067072, NCEP, 5, 24, "standard"), [
      { value: 4374216, text: "0500-01-01" },
      { value: 8757216, text: "1000-01-01" },
      { value: 13140216, text: "1500-01-01" },
    ]);
    // After the switch, the same round dates as ever.
    const modern = calendarTicks(17067072, 17067072 + 24 * 366, NCEP, 5, 24, "standard");
    assert.deepStrictEqual(modern?.map((t) => t.text), ["1948-01-01", "1948-04-01", "1948-07-01", "1948-10-01", "1949-01-01"]);
    assert.strictEqual(modern?.[0].value, 17067072);
    assert.strictEqual(axisDecimals(TIMES, NCEP, "standard"), 0, "six hours apart is whole minutes");
  });

  test("the real handle's NCEP axis reads 1948-01-01 on the mixed calendar", () => {
    for (const calendar of [null, "standard"]) {
      const { handle, index } = handleOf(fileOf(calendar));
      const held = heldSliceParts(
        ["time", "lat", "lon"],
        { yDim: 1, xDim: 2, sliceIndices: [1, 0, 0] },
        (dim) => handle.axisValues(index, dim),
        memoAxisDecimals,
      );
      assert.strictEqual(held?.[0].coordinate, "1948-01-01 06:00", String(calendar));
    }
    const { handle, index } = handleOf(fileOf("proleptic_gregorian"));
    const time = handle.axisValues(index, 0);
    assert.strictEqual(formatCoordinate(time.coordinates?.[0] ?? NaN, time.units, 0, time.calendar), "1948-01-03");
  });

  test("the panel's stepper and subtitle read the mixed-calendar date", async () => {
    const bytes = fileOf(null);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1574-"));
    const file = path.join(dir, "ncep.nc");
    fs.writeFileSync(file, bytes);
    const { index } = handleOf(bytes);
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "v drawn");
      await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
      const from = panel.sent.length;
      await panel.drive([{ selector: `#slice-dims .slice-number[data-dim="0"]`, value: 2 }]);
      await panel.waitSent(isType("gridReady"), from, "the render at 12:00");
      const state = await panel.drive([]);
      assert.strictEqual(state.steppers?.main.find((s) => s.dim === 0)?.coordinate, "1948-01-01 12:00");
      assert.ok(state.subtitle.endsWith(" · time 1948-01-01 12:00"), state.subtitle);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

// Round months and years on the mixed calendar, and number ticks on an axis
// with no date at an end (#1593). Every expected date and value is cftime
// 1.6.5's `num2date` or `date2num` on the `standard` calendar.

// 1500-01-01 to 2000-01-01 every 50 years, in days since 1500-01-01.
const CENTURIES = [0, 18263, 36515, 54778, 73040, 91302, 109564, 127826, 146088, 164350, 182612];
// cftime: -0002-11-28 to 0002-08-24, so the first four have no date.
const AROUND_YEAR_1 = [-400, -300, -200, -100, 0, 100, 200, 300, 400, 500, 600];
const DATE = /^\d{4}-\d\d-\d\d/;
const NUMBER = /^−?\d+$/;

/** A time × lat × lon file whose time is `times` in `units`, standard calendar. */
function timeFile(units: string, times: number[]): Buffer {
  return classicNetcdf(
    [["time", times.length], ["lat", 3], ["lon", 3]],
    [
      { name: "time", dims: ["time"], attrs: { units, calendar: "standard" }, values: times },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20] },
      { name: "v", dims: ["time", "lat", "lon"], attrs: {}, values: Array.from({ length: 9 * times.length }, (_, k) => k) },
    ],
  );
}

/** The real panel on `bytes`, its variable drawn, and how to close it. */
async function panelOn(bytes: Buffer): Promise<{ panel: RealPanel; close: () => void }> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1593-"));
  const file = path.join(dir, "t.nc");
  fs.writeFileSync(file, bytes);
  const { index } = handleOf(bytes);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "v drawn");
    await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
  } catch (err) {
    close();
    throw err;
  }
  return { panel, close };
}

/** The panel's rails once it is drawn as a time × lat section of `times`
 *  columns and the time rail has labels. */
async function sectionRails(panel: RealPanel, times: number): Promise<RailsState> {
  const from = panel.sent.length;
  await panel.drive([{ id: "slice-y", value: 1 }, { id: "slice-x", value: 0 }]);
  await panel.waitSent((m: Msg) => m && m.type === "gridReady" && m.width === times, from, "the time × lat section");
  let rails = (await panel.drive([])).rails;
  for (let i = 0; i < 100 && !(rails?.shown && rails.x.length >= 2 && rails.caption !== ""); i++) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    rails = (await panel.drive([])).rails;
  }
  assert.ok(rails?.shown, "the rails are shown");
  return rails;
}

suite("Round months and years on the mixed Julian/Gregorian calendar, and number ticks with no date at an end (#1593)", function () {
  this.timeout(120_000);

  test("a standard-calendar span from 1500 to 2000 ticks round years, Julian before the switch", () => {
    const units = "days since 1500-01-01";
    assert.deepStrictEqual(calendarTicks(0, 182612, units, 5, 1, "standard"), [
      { value: 0, text: "1500-01-01" },
      { value: 36515, text: "1600-01-01" },
      { value: 73040, text: "1700-01-01" },
      { value: 109564, text: "1800-01-01" },
      { value: 146088, text: "1900-01-01" },
      { value: 182612, text: "2000-01-01" },
    ]);
    // Through the axis: the same round years, each where its value falls.
    const ticks = coordinateTicks({ coordinates: CENTURIES, units, calendar: "standard" }, 5);
    assert.deepStrictEqual(ticks?.map((t) => [t.text, t.index]), [
      ["1500-01-01", 0], ["1600-01-01", 2], ["1700-01-01", 4], ["1800-01-01", 6], ["1900-01-01", 8], ["2000-01-01", 10],
    ]);
  });

  test("a one-year span in 1500 ticks months on the Julian calendar, its leap day included", () => {
    const ticks = calendarTicks(0, 366, "days since 1500-01-01", 13, 1, "standard");
    assert.deepStrictEqual(ticks?.map((t) => t.value), [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366]);
    assert.deepStrictEqual(ticks?.map((t) => t.text), [
      "1500-01-01", "1500-02-01", "1500-03-01", "1500-04-01", "1500-05-01", "1500-06-01", "1500-07-01",
      "1500-08-01", "1500-09-01", "1500-10-01", "1500-11-01", "1500-12-01", "1501-01-01",
    ]);
  });

  test("half months across the switch: the Julian 1st of October, then the Gregorian 16th", () => {
    assert.deepStrictEqual(calendarTicks(243, 324, "days since 1582-01-01", 6, 1, "standard"), [
      { value: 243, text: "1582-09-01" },
      { value: 258, text: "1582-09-16" },
      { value: 273, text: "1582-10-01" },
      { value: 278, text: "1582-10-16" },
      { value: 294, text: "1582-11-01" },
      { value: 309, text: "1582-11-16" },
      { value: 324, text: "1582-12-01" },
    ]);
  });

  test("an axis from -400 to 600 days since 1-1-1 ticks numbers only", () => {
    const axis = { coordinates: AROUND_YEAR_1, units: "days since 1-1-1", calendar: "standard" };
    assert.strictEqual(datedAxis(axis), false, "year 0 has no date");
    assert.strictEqual(datedAxis({ ...axis, coordinates: [0, 600] }), true, "from 0001-01-01 every value has one");
    assert.strictEqual(datedAxis({ ...axis, coordinates: [-1, 600] }), false, "cftime's -0001-12-31 has none");
    assert.strictEqual(datedAxis({ ...axis, coordinates: [600, -1] }), false, "whichever end it is");
    assert.strictEqual(datedAxis({ ...axis, coordinates: [0, 4000000] }), false, "nor has year 10952");
    const numbers = ["−400", "−200", "0", "200", "400", "600"];
    assert.deepStrictEqual(coordinateTicks(axis, 5)?.map((t) => t.text), numbers);
    // Down the axis too: the earliest value last.
    const down = coordinateTicks({ ...axis, coordinates: [...AROUND_YEAR_1].reverse() }, 5);
    assert.deepStrictEqual(down?.map((t) => t.text), numbers);
    // The rail, on these even points and on uneven ones it labels itself.
    assert.deepStrictEqual(railTicks(axis, AROUND_YEAR_1.length, 5).map((t) => t.text), numbers);
    const uneven = { ...axis, coordinates: [-400, -399, -398, 0, 600] };
    assert.deepStrictEqual(railTicks(uneven, 5, 5).map((t) => t.text), ["−400", "−399", "−398", "0", "600"]);
    // Each to the decimals that keep it apart, as a number's are (#1459).
    const quarters = { ...axis, coordinates: [-100000.5, -100000.25, -100000, 0, 600] };
    assert.deepStrictEqual(railTicks(quarters, 5, 5).map((t) => t.text), ["−100000.5", "−100000.25", "−100000", "0", "600"]);
    // The control: an axis from year 1 on is dated.
    const dated = coordinateTicks({ ...axis, coordinates: [0, 100, 200, 300, 400, 500, 600] }, 2);
    assert.ok(dated?.every((t) => DATE.test(t.text)), JSON.stringify(dated));
  });

  test("the real panel's section rail ticks round years from 1500 to 2000, with no units in its caption", async () => {
    const { panel, close } = await panelOn(timeFile("days since 1500-01-01", CENTURIES));
    try {
      const rails = await sectionRails(panel, CENTURIES.length);
      const texts = rails.x.map((l) => l.text);
      const years = ["1500-01-01", "1600-01-01", "1700-01-01", "1800-01-01", "1900-01-01", "2000-01-01"];
      assert.ok(texts.length >= 2 && texts.every((t) => years.includes(t)), `round years: ${texts.join(", ")}`);
      assert.strictEqual(texts[0], "1500-01-01");
      assert.strictEqual(rails.caption, "x: time · y: lat (degrees_north)");
    } finally {
      close();
    }
  });

  test("the real panel's section rail from -400 to 600 days since 1-1-1 reads numbers, with the units in its caption", async () => {
    const { panel, close } = await panelOn(timeFile("days since 1-1-1", AROUND_YEAR_1));
    try {
      const rails = await sectionRails(panel, AROUND_YEAR_1.length);
      const texts = rails.x.map((l) => l.text);
      assert.ok(texts.length >= 2, `time labels: ${texts.join(", ")}`);
      assert.ok(texts.every((t) => NUMBER.test(t)), `numbers only: ${texts.join(", ")}`);
      assert.strictEqual(rails.caption, "x: time (days since 1-1-1) · y: lat (degrees_north)");
    } finally {
      close();
    }
  });

  test("a line plot along a time out of order, from before year 1, labels its points as numbers", async () => {
    // Out of order, so the plot has no round ticks and labels its own points.
    const times = [600, -400, 200];
    const { panel, close } = await panelOn(timeFile("days since 1-1-1", times));
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      let from = panel.sent.length;
      await panel.probe();
      await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      await panel.waitSent(isType("lineResult"), from, "the line");
      from = panel.sent.length;
      await panel.drive([{ id: "line-axis", value: "0" }]);
      const line = await panel.waitSent(isType("lineResult"), from, "the line along time");
      assert.deepStrictEqual(line.result?.coordinates, times, "the line carries the time's values");
      let labels: string[] = [];
      for (let i = 0; i < 100; i++) {
        labels = ((await panel.drive([])).linePlot?.plot.x ?? []).map((l) => l.text);
        if (labels.length >= 2) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(labels.length >= 2, `time labels drawn: ${labels.join(", ")}`);
      assert.ok(labels.every((t) => NUMBER.test(t)), `numbers only: ${labels.join(", ")}`);
    } finally {
      close();
    }
  });
});
