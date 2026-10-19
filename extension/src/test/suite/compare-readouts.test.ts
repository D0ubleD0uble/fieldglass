// The line plot and the zonal average on a Compare map read the map (#1208).
//
// With a difference map on screen, the probe and the contours read the
// combined field, but the line through a cell and the zonal average were
// asked of Field A alone: neither request carried the compare rider, and the
// provider took one only for the probe and the contours. So the plot under a
// difference map showed Field A's values, which breaks the rule #1145 set:
// every readout follows the image on screen.
//
// Driven through the real panel and the real provider, on a NetCDF file and a
// GRIB file. A GRIB message has no axes to plot along, so it has no line plot,
// only the zonal average.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  classicNetcdf,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const STEPS = [0, 7, 30, 100];
const LAT = [-10, 0, 10];
const LON = [0, 90, 180, 270];
// Every value names its own time, row and column, and the column's weight
// grows with time, so a difference of two times is not flat along a row.
const v = (t: number, j: number, i: number) => STEPS[t] + (t + 1) * (10 * j + i);
// Field A at the last time, Field B two steps before it.
const TA = 3;
const TB = 1;
const aMinusB = (j: number, i: number) => v(TA, j, i) - v(TB, j, i);

/** `v(time, lat, lon)` on a regular lat/lon grid. */
function stepsFile(): Buffer {
  const values: number[] = [];
  for (let t = 0; t < STEPS.length; t++) {
    for (let j = 0; j < LAT.length; j++) {
      for (let i = 0; i < LON.length; i++) values.push(v(t, j, i));
    }
  }
  return classicNetcdf(
    [["time", STEPS.length], ["lat", LAT.length], ["lon", LON.length]],
    [
      { name: "time", dims: ["time"], attrs: { units: "days since 2000-01-01" }, values: STEPS.map((_, t) => t) },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: LAT },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: LON },
      { name: "v", dims: ["time", "lat", "lon"], attrs: { units: "K" }, values },
    ],
  );
}

/** The last render the page drew. */
function lastDrawn(panel: RealPanel): Msg {
  const drew = panel.sent.filter(isType("gridReady"));
  return drew[drew.length - 1];
}

/** A line's points, absent ones as `null`. */
function points(line: { values: number[]; mask: number[] }): Array<number | null> {
  return line.values.map((x, k) => (line.mask[k] === 1 ? x : null));
}

function assertClose(got: Array<number | null>, want: Array<number | null>, what: string): void {
  assert.strictEqual(got.length, want.length, `${what}: ${JSON.stringify(got)} against ${JSON.stringify(want)}`);
  got.forEach((x, k) => {
    const w = want[k];
    if (w === null || x === null) {
      assert.strictEqual(x, w, `${what}, point ${k}: ${JSON.stringify(got)} against ${JSON.stringify(want)}`);
    } else {
      assert.ok(Math.abs(x - w) < 1e-9, `${what}, point ${k}: ${JSON.stringify(got)} against ${JSON.stringify(want)}`);
    }
  });
}

suite("The line plot and the zonal average on a Compare map read the map (#1208)", function () {
  this.timeout(120_000);

  test("NetCDF: the line along each axis and the zonal average are of A − B", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1208-"));
    const file = path.join(dir, "steps.nc");
    fs.writeFileSync(file, stepsFile());
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variable = native.open(fs.readFileSync(file)).variables().find((x) => x.name === "v");
    assert.ok(variable, "v is renderable");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variable.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Field A at the last time; then A − B with Field B two steps before.
      let from = panel.sent.length;
      await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: TA }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.slice?.sliceIndices?.[0] === TA, from, "Field A at the last time");
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { selector: '#compare-dims .slice-number[data-dim="0"]', value: TB },
      ]);
      await waitUntil(() => {
        const last = lastDrawn(panel);
        return last?.compare?.op === "a_minus_b" && last.compareSliceIndicesB?.[0] === TB ? true : undefined;
      }, "the difference map drawn");

      // The probe reads the difference: the image on screen.
      await panel.drive([{ id: "line-toggle", checked: true }, { id: "line-axis", value: 0 }]);
      from = panel.sent.length;
      await panel.probe(0.5);
      const probe = (await panel.waitSent(isType("probeResult"), from, "the probe")).result;
      assert.ok(probe && probe.gridI != null && probe.gridJ != null, "the click lands on a cell");
      const { gridI: i, gridJ: j } = probe;
      assert.strictEqual(probe.value, aMinusB(j, i), "the probe reads A − B");

      // Along time: Field A stepped along the axis against Field B where it
      // is drawn, as the map shows while Field A's stepper moves (the owner's
      // decision on #1208). Every step has a value, and the point where the
      // map sits is the value the probe read.
      const alongTime = (await panel.waitSent(isType("lineResult"), from, "the line along time")).result;
      assert.ok(alongTime, "a line along time");
      assertClose(
        points(alongTime),
        STEPS.map((_, t) => v(t, j, i) - v(TB, j, i)),
        "the line along time",
      );
      assert.strictEqual(points(alongTime)[TA], probe.value, "where the map sits, the line reads the probe's value");

      // Along longitude: a row of the map on screen.
      from = panel.sent.length;
      await panel.drive([{ id: "line-axis", value: 2 }]);
      const alongLon = (await panel.waitSent(isType("lineResult"), from, "the line along longitude")).result;
      assert.ok(alongLon, "a line along longitude");
      assertClose(points(alongLon), LON.map((_, x) => aMinusB(j, x)), "the line along longitude");
      const plot = (await panel.drive([])).linePlot;
      assert.match(plot?.caption ?? "", /^A − B · v across lon /, `the caption says it is the difference: ${plot?.caption}`);

      // The zonal average: each row's mean of A − B.
      from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      const zonal = await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      assert.strictEqual(zonal.error, null, `the zonal average is drawn: ${zonal.error}`);
      const lats: number[] = zonal.result.coordinates;
      assert.ok(Array.isArray(lats) && lats.length === LAT.length, "one point per latitude");
      assertClose(
        points(zonal.result),
        lats.map((lat) => {
          const row = LAT.indexOf(lat);
          return LON.reduce((s, _, x) => s + aMinusB(row, x), 0) / LON.length;
        }),
        "the zonal average",
      );
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("GRIB: the zonal average is of A + B", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1208-"));
    const file = path.join(dir, "pair.grib2");
    const bytes = fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"));
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const native = loadNative();
    assert.ok(native, "native binding required");
    // A + B of a message and itself is twice it, so twice each row's mean.
    const single = native.open(fs.readFileSync(file)).zonalMean(0);
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await panel.drive([
        { id: "compare-op", value: "a_plus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      await waitUntil(() => {
        const last = lastDrawn(panel);
        return last?.compare?.op === "a_plus_b" && last.compare.messageIndexB === 1 ? true : undefined;
      }, "the sum drawn");
      const received = panel.received.length;
      const from = panel.sent.length;
      await panel.drive([{ id: "zonal-toggle", checked: true }]);
      const asked = await waitUntil(() => panel.received.slice(received).find(isType("zonalRequest")), "the zonal request");
      assert.deepStrictEqual(asked.compare, { op: "a_plus_b", messageIndexB: 1 }, "the request carries the map drawn");
      const zonal = await panel.waitSent(isType("zonalResult"), from, "the zonal average");
      assert.strictEqual(zonal.error, null, `the zonal average is drawn: ${zonal.error}`);
      assertClose(
        points(zonal.result),
        points(single).map((x) => (x === null ? null : 2 * x)),
        "the zonal average of A + B",
      );
      const plot = (await panel.drive([])).linePlot;
      assert.match(plot?.caption ?? "", /^A \+ B · /, `the caption says it is the sum: ${plot?.caption}`);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
