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
import { axisDecimals, calendarTicks, formatAxisValue, formatCoordinate, heldSliceParts, memoAxisDecimals } from "../../render-panel";
import { classicNetcdf, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

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
    // Whole-month and whole-year ticks are not counted on the Julian
    // calendar, so a span that reaches before the switch keeps its points.
    assert.strictEqual(calendarTicks(0, 17067072, NCEP, 5, 24, "standard"), null);
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
