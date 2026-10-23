// A field value is read to enough figures to tell large nearby values apart
// (#1432).
//
// The probe read five significant figures, so on `time_bnds` in the 0.6.0
// sample `ncep_air_daily_2024.nc` (hours since 1800) two bounds a day apart,
// 1963512 and 1963536, both read `1.9635e+6`. Now every readout of a field
// value (the probe, the colour bar, the status line's range, the manual
// range's pre-filled ends and the axes' tick labels) keeps the whole part in
// full below a billion, and for a whole number below 2^53 (a time in seconds
// or milliseconds since 1970), through one rule, `readingFigures`. A value that
// rounds below 10^5 (10^4 on the colour bar) reads exactly as before, which
// the representative temperatures and winds below hold it to. The CSV export
// is not a readout and keeps full precision.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import {
  composeLineRange,
  composeProbeValue,
  composeRangeText,
  formatAxisValue,
  formatReading,
  readingFigures,
  typesetMinus,
} from "../../render-panel";
import { classicNetcdf, decodePng, isType, openEditor, openReal, provider, waitUntil } from "./real-panel";

const MINUS = "−";
const one = { fullDetailValue: null, fullDetailTruncation: null };

/** Temperatures (K and °C), winds (m s-1), pressures (hPa), humidities and
 *  precipitation (m), each a value a field of its kind holds. */
const ORDINARY = [
  287.45, 231.84998, 310.25, 273.15, 300, -40.123, -0.5, 0, 12.3456, -31.48, 5, 0.0001234, 1013.25, 985.6,
  0.5, 99.999, 2.5e-7, -1.5e-7,
];

suite("A field value reads to the figures it needs (#1432)", function () {
  this.timeout(120_000);

  test("an ordinary value reads as it did", () => {
    for (const v of ORDINARY) {
      assert.strictEqual(formatReading(v, 5), typesetMinus(v.toPrecision(5)), `probe ${v}`);
      assert.strictEqual(formatReading(v, 4), typesetMinus(v.toPrecision(4)), `colour bar ${v}`);
      assert.strictEqual(composeProbeValue({ value: v, ...one }, "K"), typesetMinus(v.toPrecision(5)) + " K");
    }
    // As the readout reads them, written out.
    assert.strictEqual(composeProbeValue({ value: 287.45, ...one }, "K"), "287.45 K");
    assert.strictEqual(composeProbeValue({ value: -12.3456, ...one }, "m s-1"), `${MINUS}12.346 m s-1`);
    assert.strictEqual(composeProbeValue({ value: 5, ...one }, "m s-1"), "5.0000 m s-1");
    assert.strictEqual(composeRangeText(231.84998, 310.25), "range 231.8 … 310.3");
    assert.strictEqual(composeRangeText(-30, 30), `range ${MINUS}30.00 … 30.00`);
  });

  test("a large value keeps its whole part", () => {
    // The two bounds of one day in the sample, and the issue's pair.
    assert.strictEqual(composeProbeValue({ value: 1963512, ...one }, ""), "1963512");
    assert.strictEqual(composeProbeValue({ value: 1963536, ...one }, ""), "1963536");
    assert.strictEqual(composeProbeValue({ value: 1967832, ...one }, "hours"), "1967832 hours");
    assert.strictEqual(composeProbeValue({ value: 1967856, ...one }, "hours"), "1967856 hours");
    // Five figures alone would read the first two alike.
    assert.strictEqual((1963512).toPrecision(5), (1963536).toPrecision(5));
    // A pressure in pascals reads as itself, not 1.0133e+5.
    assert.strictEqual(formatReading(101325, 5), "101325");
    assert.strictEqual(formatReading(101325.4, 4), "101325");
    // The whole part is counted after rounding: 99999.7 is not 1.0000e+5.
    assert.strictEqual(readingFigures(99999.7, 5), 6);
    assert.strictEqual(formatReading(99999.7, 5), "99999.7");
    // Up to a billion; past it a fraction stands in exponent form, as on
    // the line plot's caption.
    assert.strictEqual(formatReading(999999999.4, 4), "999999999");
    assert.strictEqual(formatReading(1700000000.5, 5), "1.7000e+9");
    assert.strictEqual(formatReading(1e300, 4), "1.000e+300");
    // A whole number keeps its whole part up to 2^53: bounds an hour apart
    // in seconds since 1970, and a time in milliseconds.
    assert.strictEqual(composeProbeValue({ value: 1700000000, ...one }, "s"), "1700000000 s");
    assert.strictEqual(composeProbeValue({ value: 1700003600, ...one }, "s"), "1700003600 s");
    assert.strictEqual(formatReading(1.7e12, 4), "1700000000000");
    assert.strictEqual(formatReading(-1.7e12, 5), `${MINUS}1700000000000`);
    assert.strictEqual(formatReading(9007199254740991, 4), "9007199254740991");
    assert.strictEqual(formatReading(9007199254740992, 4), "9.007e+15");
    assert.strictEqual(formatAxisValue(1700003600, null), "1700003600");
    // Five figures alone read those two seconds alike.
    assert.strictEqual((1700000000).toPrecision(5), (1700003600).toPrecision(5));
    // Not a number reads as JavaScript writes it, as before.
    assert.strictEqual(formatReading(Number.NaN, 5), "NaN");
    assert.strictEqual(formatReading(Number.POSITIVE_INFINITY, 5), "Infinity");
  });

  test("a negative value keeps its true minus and its units", () => {
    assert.strictEqual(composeProbeValue({ value: -1963536, ...one }, "hours"), `${MINUS}1963536 hours`);
    assert.strictEqual(
      composeProbeValue({ value: -1963536.25, fullDetailValue: -1963560.75, fullDetailTruncation: 383 }, "K"),
      `${MINUS}1963536 K shown · ${MINUS}1963561 K at full detail (T383)`,
    );
    assert.strictEqual(composeRangeText(-1972320, -1963536), `range ${MINUS}1972320 … ${MINUS}1963536`);
    // A small negative value is still in exponent form, signed.
    assert.strictEqual(composeProbeValue({ value: -1.5e-7, ...one }, "kg m-2 s-1"), `${MINUS}1.5000e${MINUS}7 kg m-2 s-1`);
    // A dimensionless unit is still dropped.
    assert.strictEqual(composeProbeValue({ value: 1963536, ...one }, "1"), "1963536");
  });

  test("a small value reads to its figures", () => {
    assert.strictEqual(formatReading(0.000123456, 5), "0.00012346");
    assert.strictEqual(formatReading(1.23456e-9, 5), "1.2346e−9");
    assert.strictEqual(formatReading(0, 5), "0.0000");
    assert.strictEqual(formatReading(-0, 4), "0.000");
  });

  test("the line plot's ticks and caption keep the whole part", () => {
    // Six figures would label a tick at 1963535 as 1963540.
    assert.strictEqual(formatAxisValue(1963535, null), "1963535");
    assert.strictEqual(formatAxisValue(-1963535, ""), `${MINUS}1963535`);
    // Float noise is still dropped from an ordinary tick, and a large
    // one's whole part has no fraction left to carry it.
    assert.strictEqual(formatAxisValue(0.1 + 0.2, ""), "0.3");
    assert.strictEqual(formatAxisValue(1963535.0000000002, ""), "1963535");
    assert.strictEqual(formatAxisValue(1013.25, "hPa"), "1013.25");
    // The caption had its own span rule, which already kept the two apart.
    assert.strictEqual(composeLineRange(1963536, 1963560), "1963536 to 1963560");
  });

  test("time_bnds reads its bounds exactly in the probe, the colour bar and the status line", async () => {
    // As the sample has it: 366 days, two bounds each, in hours since 1800.
    const first = 1963536;
    const bound = (t: number, b: number) => first + 24 * (t + b);
    const values: number[] = [];
    for (let t = 0; t < 366; t++) for (let b = 0; b < 2; b++) values.push(bound(t, b));
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1432-"));
    const file = path.join(dir, "bounds.nc");
    fs.writeFileSync(
      file,
      classicNetcdf([["time", 366], ["nbnds", 2]], [{ name: "time_bnds", dims: ["time", "nbnds"], attrs: {}, values }]),
    );
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: 0 }));
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "time_bnds drawn");
      assert.strictEqual(drawn.usedMin, first);
      assert.strictEqual(drawn.usedMax, bound(365, 1));
      const shown = await panel.drive([]);
      assert.deepStrictEqual(shown.colorbarLabels, ["1963536", "1972320"]);
      assert.ok(shown.status.includes("range 1963536 … 1972320"), shown.status);

      // Both columns at the same height: the right bound is a day on.
      const read = async (fx: number) => {
        const from = panel.sent.length;
        await panel.probe(0.3, fx);
        const reply = await panel.waitSent(isType("probeResult"), from, "the probe");
        const state = await panel.drive([]);
        return { value: reply.result.value as number, text: state.probe };
      };
      const left = await read(0.25);
      const right = await read(0.75);
      assert.strictEqual(right.value - left.value, 24, `${left.value} and ${right.value}`);
      for (const r of [left, right]) {
        assert.ok(Number.isInteger(r.value) && r.value >= first, `a bound, got ${r.value}`);
        assert.ok(r.text.includes(String(r.value) + " · grid "), `readout "${r.text}" carries ${r.value}`);
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("milliseconds since 1970 fit the colour bar, on the page and in Export PNG", async () => {
    // A 40 × 200 field of times an hour apart, in milliseconds: thirteen
    // digits. Wide enough that the map, not the heading, sets the PNG's width.
    const first = 1.7e12;
    const values = Array.from({ length: 8000 }, (_, k) => first + 3_600_000 * k);
    const last = values[values.length - 1];
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1432-"));
    const file = path.join(dir, "ms.nc");
    fs.writeFileSync(file, classicNetcdf([["y", 40], ["x", 200]], [{ name: "ms", dims: ["y", "x"], attrs: {}, values }]));
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: 0 }));
    const saved = vscode.window.showSaveDialog;
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "ms drawn");
      const shown = await panel.drive([]);
      assert.deepStrictEqual(shown.colorbarLabels, ["1700000000000", String(last)]);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
      const from = panel.received.length;
      await panel.drive([{ id: "export-png", click: true }]);
      const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
      const png = decodePng(Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
      // Wider than the 64 px label column the PNG had, so the labels are not
      // cut at its edge: the right margin, past the labels, is blank.
      assert.strictEqual(png.width > 14 + drawn.width + 16 + 18 + 64 + 14, true, `the PNG is ${png.width} px wide`);
      for (let y = 0; y < png.height; y++) {
        for (let x = png.width - 10; x < png.width; x++) {
          const at = 4 * (y * png.width + x);
          assert.ok(png.rgba[at] > 200 && png.rgba[at + 1] > 200 && png.rgba[at + 2] > 200, `ink at ${x},${y} in the margin`);
        }
      }
    } finally {
      vscode.window.showSaveDialog = saved;
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
