// A field value is read to enough figures to tell large nearby values apart
// (#1432).
//
// The probe read five significant figures, so on `time_bnds` in the 0.6.0
// sample `ncep_air_daily_2024.nc` (hours since 1800) two bounds a day apart,
// 1963512 and 1963536, both read `1.9635e+6`. Now every readout of a field
// value (the probe, the colour bar, the status line's range, the manual
// range's pre-filled ends and the axes' tick labels) keeps the whole part in
// full below 2^53 (a time in seconds or milliseconds since 1970), and a
// fraction of a billion or more one decimal besides (#1450), through one
// rule, `readingFigures`. A value that
// rounds below 10^5 (10^4 on the colour bar) reads exactly as before, which
// the representative temperatures and winds below hold it to. The CSV export
// is not a readout and keeps full precision.
//
// Where nearby values need more than that, they get it: a tick label takes
// its step's decimals (#1459), the Export PNG colour bar's middle reads the
// value painted there (#1458), the line plot's caption reads each end at
// least as the probe does (#1461), and the manual range's pre-filled ends
// keep the data's exact range beside the text they show (#1460).

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import {
  composeLineRange,
  composeProbeValue,
  colorbarLabels,
  composeRangeText,
  axisDecimals,
  formatAxisValue,
  formatReading,
  heldSliceParts,
  niceTicks,
  railTicks,
  readingFigures,
  typesetMinus,
} from "../../render-panel";
import { loadNative } from "../../native";
import {
  classicNetcdf,
  decodePng,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

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
    // Past a billion a fraction keeps its whole part and one decimal (#1450).
    assert.strictEqual(formatReading(999999999.4, 4), "999999999");
    assert.strictEqual(formatReading(1700000000.5, 5), "1700000000.5");
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
    // A time with a fraction keeps one decimal besides (#1450): the issue's
    // pair, times half a second apart across a whole second, and a time in
    // seconds since 1993 (TAI93), which passed a billion in September 2024.
    // A whole time keeps none.
    assert.strictEqual(composeProbeValue({ value: 1700000000.25, ...one }, "s"), "1700000000.3 s");
    assert.strictEqual(composeProbeValue({ value: 1700000000.75, ...one }, "s"), "1700000000.8 s");
    assert.strictEqual(formatReading(1700000000.5, 4), "1700000000.5");
    assert.strictEqual(formatReading(1700000001, 4), "1700000001");
    assert.strictEqual(formatReading(1700000000.6, 4), "1700000000.6");
    assert.strictEqual(formatReading(1700000001.1, 4), "1700000001.1");
    assert.strictEqual(formatReading(1066953600.123, 4), "1066953600.1");
    assert.strictEqual(formatReading(-1700000000.5, 4), `${MINUS}1700000000.5`);
    assert.strictEqual(formatAxisValue(1700000000.75, null), "1700000000.8");
    // Five figures alone read the issue's pair alike.
    assert.strictEqual((1700000000.25).toPrecision(5), (1700000000.75).toPrecision(5));
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

  test("the Export PNG colour bar's middle label reads the value painted there (#1458)", () => {
    // Two whole ends a billion or more apart by an odd number: the mean
    // keeps its half, as the probe reads it, and is not rounded onto an end.
    assert.deepStrictEqual(colorbarLabels(1700000000, 1700000001, false), ["1700000001", "1700000000.5", "1700000000"]);
    assert.deepStrictEqual(colorbarLabels(1700000000, 1700003601, false), ["1700003601", "1700001800.5", "1700000000"]);
    assert.deepStrictEqual(colorbarLabels(1.7e12, 1.7e12 + 3_600_001, false), ["1700003600001", "1700001800000.5", "1700000000000"]);
    // An even gap has a whole mean, read with no decimal.
    assert.deepStrictEqual(colorbarLabels(1700000000, 1700003600, false), ["1700003600", "1700001800", "1700000000"]);
    // Below a billion the mean keeps its fraction, as before.
    assert.deepStrictEqual(colorbarLabels(0, 5, false), ["5.000", "2.500", "0.000"]);
    assert.deepStrictEqual(colorbarLabels(250, 261, false), ["261.0", "255.5", "250.0"]);
    assert.deepStrictEqual(colorbarLabels(-30, 30, false), ["30.00", "0.000", `${MINUS}30.00`]);
    // Beside a fractional end the mean keeps one decimal too (#1450). log10
    // takes the geometric mean.
    assert.deepStrictEqual(colorbarLabels(1700000000.5, 1700003601, false), ["1700003601", "1700001800.8", "1700000000.5"]);
    assert.deepStrictEqual(colorbarLabels(1, 100, true), ["100.0", "10.00", "1.000"]);
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

  test("ticks take the decimals of their step (#1459)", () => {
    const labels = (lo: number, hi: number, count = 5) => {
      const ticks = niceTicks(lo, hi, count);
      const decimals = axisDecimals(ticks);
      return ticks.map((v) => formatAxisValue(v, null, decimals));
    };
    // The issue's spans: a pressure in pascals a fifth of a pascal wide, and
    // a time in seconds a fifth of a second wide past a billion. Six
    // figures read every pressure tick 101325; eleven read the times' 0.05
    // steps .0, .0, .1, .2, .2.
    assert.deepStrictEqual(labels(101325, 101325.2), ["101325", "101325.05", "101325.1", "101325.15", "101325.2"]);
    assert.deepStrictEqual(labels(1700000000, 1700000000.2), [
      "1700000000", "1700000000.05", "1700000000.1", "1700000000.15", "1700000000.2",
    ]);
    // A step of 0.25: one decimal would keep the ticks apart but read
    // 1700000000.25 as 1700000000.3.
    assert.deepStrictEqual(labels(1700000000, 1700000001.25), [
      "1700000000", "1700000000.25", "1700000000.5", "1700000000.75", "1700000001", "1700000001.25",
    ]);
    // Twelve figures moved a tick a millionth of a pascal apart off its step.
    assert.deepStrictEqual(niceTicks(101325, 101325.00001, 5), [101325, 101325.0000025, 101325.000005, 101325.0000075, 101325.00001]);
    // Ordinary spans read as before.
    assert.deepStrictEqual(labels(250, 310), ["260", "280", "300"]);
    assert.deepStrictEqual(labels(-90, 90), [`${MINUS}50`, "0", "50"]);
    assert.deepStrictEqual(labels(0.1, 0.3), ["0.1", "0.15", "0.2", "0.25", "0.3"]);
    assert.deepStrictEqual(labels(1000, 1013.25), ["1000", "1005", "1010"]);
    assert.deepStrictEqual(labels(0, 1e-6), ["0", `2e${MINUS}7`, `4e${MINUS}7`, `6e${MINUS}7`, `8e${MINUS}7`, "0.000001"]);
    // The decimals that keep points apart: a step's own, two more at most
    // for a round one, and only what separates points off a round step.
    assert.strictEqual(axisDecimals([0, 0.25, 0.5]), 2);
    assert.strictEqual(axisDecimals([1000, 925, 850]), 0);
    assert.strictEqual(axisDecimals([Math.fround(0.1), Math.fround(0.2), Math.fround(0.3)]), 1);
    assert.strictEqual(axisDecimals([5]), 0);
    assert.strictEqual(axisDecimals([]), 0);
    // A decimal only adds figures: a value the reading already shows whole
    // is unchanged.
    assert.strictEqual(formatAxisValue(1013.25, "hPa", 1), "1013.25");
    assert.strictEqual(formatAxisValue(1700000000.05, null, 2), "1700000000.05");
    assert.strictEqual(formatAxisValue(1700000000.05, null), "1700000000");
    // The cross-section rail labels its points apart, round or not.
    const rail = (values: number[]) => railTicks({ coordinates: values, units: "s" }, values.length, 6).map((t) => t.text);
    assert.deepStrictEqual(rail([1700000000, 1700000000.05, 1700000000.1, 1700000000.15, 1700000000.2]), [
      "1700000000", "1700000000.05", "1700000000.1", "1700000000.15", "1700000000.2",
    ]);
    // Uneven points close together past a billion keep their own values,
    // each to the decimals that keep it apart.
    const uneven = [1700000000, 1700000000.01, 1700000000.03, 1700000000.07, 1700000000.15, 1700000000.31];
    assert.deepStrictEqual(rail(uneven), uneven.map((v) => String(v)));
    // A slice held at a step of such an axis names that step, not its
    // neighbour's value.
    const held = (index: number) =>
      heldSliceParts(["t", "lat", "lon"], { yDim: 1, xDim: 2, sliceIndices: [index, 0, 0] }, (d) =>
        d === 0 ? { coordinates: uneven, units: "s" } : null,
      )?.[0].coordinate;
    assert.deepStrictEqual([held(0), held(1), held(2)], ["1700000000 s", "1700000000.01 s", "1700000000.03 s"]);
  });

  test("the line plot's caption reads a flat or large whole-number line exactly (#1440)", () => {
    assert.strictEqual(composeLineRange(12345678, 12345678), "12345678");
    assert.strictEqual(composeLineRange(1700003600, 1700003600), "1700003600");
    assert.strictEqual(composeLineRange(-1700003600, -1700003600), `${MINUS}1700003600`);
    assert.strictEqual(composeLineRange(101325, 101325), "101325");
    assert.strictEqual(composeLineRange(0.1, 0.1), "0.1");
    assert.strictEqual(composeLineRange(1.5e-7, 1.5e-7), `1.5e${MINUS}7`);
    // A fraction past a billion reads its whole part and one decimal
    // (#1450); an exponent form, past 2^53, stays one and does not turn back
    // into a long whole number.
    assert.strictEqual(composeLineRange(1234567890.5, 1234567890.5), "1234567890.5");
    assert.strictEqual(composeLineRange(1.2345678901234567e17, 1.2345678901234567e17), "1.234568e+17");
    assert.strictEqual(composeLineRange(1700000000, 1700003600), "1700000000 to 1700003600");
    assert.strictEqual(composeLineRange(1700000000000, 1700000003600), "1700000000000 to 1700000003600");
    assert.strictEqual(composeLineRange(-1700003600, 5), `${MINUS}1700003600 to 5`);
    // Past 2^53 exponent form stays, to the probe's five figures, with the
    // ends apart. Below it a fraction keeps the decimal the probe reads it
    // with (#1450, #1461), and the whole end takes the same decimals.
    assert.strictEqual(composeLineRange(1e17, 2e17), "1.0000e+17 to 2.0000e+17");
    assert.strictEqual(composeLineRange(1.5e9, 2.5e9 + 0.5), "1500000000.0 to 2500000000.5");
    assert.strictEqual(composeLineRange(1700000000.25, 1700000000.75), "1700000000.25 to 1700000000.75");
  });

  test("the line plot's caption reads each end with at least the probe's figures (#1461)", () => {
    const probe = (v: number) => composeProbeValue({ value: v, ...one }, "");
    // The issue's wide range past a billion: the span's rule wrote
    // `1700000000 to 1700003601`, where the probe reads .3 and .8.
    for (const [lo, hi, text] of [
      [1700000000.25, 1700003600.75, "1700000000.3 to 1700003600.8"],
      [1500.5, 2500.5, "1500.5 to 2500.5"],
      [-72.769, 31.484, `${MINUS}72.769 to 31.484`],
      [1e-9, 9.5, `1.0000e${MINUS}9 to 9.5000`],
    ] as const) {
      const caption = composeLineRange(lo, hi);
      assert.strictEqual(caption, text, `${lo} to ${hi}`);
      // Each end reads the value the probe reads, trailing zeros aside.
      const [a, b] = caption.split(" to ");
      for (const [end, v] of [[a, lo], [b, hi]] as const) {
        assert.strictEqual(Number(end.replace(/−/g, "-")), Number(probe(v).replace(/−/g, "-")), `${end} and the probe's ${probe(v)}`);
      }
    }
    // A zero end, the commonest minimum (rain, wind speed, cloud cover),
    // reads `0.0000` on the probe: padding, so it asks for no decimals of
    // its own and the other end's reading sets them.
    assert.strictEqual(composeLineRange(0, 31.4839), "0.000 to 31.484");
    assert.strictEqual(composeLineRange(0, 1234.5678), "0.0 to 1234.6");
    // Both ends take the same decimals, so a small end's six set the other's.
    assert.strictEqual(composeLineRange(1.5e-5, 100), "0.000015 to 100.000000");
    // The span may add decimals the probe does not show, never take one away.
    assert.strictEqual(composeLineRange(238.35, 300.83), "238.35 to 300.83");
    assert.strictEqual(composeLineRange(101325, 101330), "101325 to 101330");
    assert.strictEqual(composeLineRange(1700000000, 1700003600), "1700000000 to 1700003600");
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

  test("times half a second apart near 1.7e9 read apart in the probe, the colour bar, the status line and the manual range (#1450)", async () => {
    // Seconds since 1970 with a fraction, stored as doubles, one column per
    // time: half-second steps on and across a whole second, as each reads.
    const cases: Array<Array<[number, string]>> = [
      [[1700000000.25, "1700000000.3"], [1700000000.75, "1700000000.8"]],
      [[1700000000.5, "1700000000.5"], [1700000001, "1700000001"]],
      [[1700000000.6, "1700000000.6"], [1700000001.1, "1700000001.1"]],
      [[1700000000, "1700000000"], [1700000000.5, "1700000000.5"], [1700000001, "1700000001"]],
      // A range narrower than a second: the manual range's pre-filled ends
      // must not be equal.
      [[1700000000.6, "1700000000.6"], [1700000000.9, "1700000000.9"]],
    ];
    const p = await provider();
    for (const times of cases) {
      const n = times.length;
      const values = Array.from({ length: 4 * n }, (_, k) => times[k % n][0]);
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1450-"));
      const file = path.join(dir, "seconds.nc");
      fs.writeFileSync(file, classicNetcdf([["y", 4], ["x", n]], [{ name: "t", dims: ["y", "x"], attrs: {}, values }]));
      const editor = await openEditor(p, file);
      const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: 0 }));
      const [lo, hi] = [times[0], times[n - 1]];
      try {
        const drawn = await panel.waitSent(isType("gridReady"), 0, "t drawn");
        assert.strictEqual(drawn.usedMin, lo[0]);
        assert.strictEqual(drawn.usedMax, hi[0]);
        const shown = await panel.drive([]);
        assert.deepStrictEqual(shown.colorbarLabels, [lo[1], hi[1]]);
        assert.ok(shown.status.includes(`range ${lo[1]} … ${hi[1]}`), shown.status);
        assert.deepStrictEqual(shown.rangeFields, [lo[1], hi[1]], "the pre-filled ends");

        const texts: string[] = [];
        for (let i = 0; i < n; i++) {
          const from = panel.sent.length;
          await panel.probe(0.5, (i + 0.5) / n);
          const reply = await panel.waitSent(isType("probeResult"), from, "the probe");
          assert.strictEqual(reply.result.value, times[i][0]);
          const text = (await panel.drive([])).probe;
          assert.ok(text.includes(times[i][1] + " · grid "), `readout "${text}" for ${times[i][0]}`);
          texts.push(text.slice(0, text.indexOf(" · grid ")));
        }
        assert.strictEqual(new Set(texts).size, n, `${texts.join(", ")} read apart`);
      } finally {
        panel.dispose();
        editor.close();
        fs.rmSync(dir, { recursive: true, force: true });
      }
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

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };
const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");
const unminus = (text: string) => Number(text.replace(/−/g, "-"));

/** A real panel on `variable` of the classic NetCDF `bytes`, once drawn. */
async function fieldPanel(
  bytes: Buffer,
  variable: string,
): Promise<{ panel: RealPanel; drawn: Msg; close: () => void }> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1459-"));
  const file = path.join(dir, "close.nc");
  fs.writeFileSync(file, bytes);
  const native = loadNative();
  assert.ok(native, "native binding required");
  const found = native.open(bytes).variables().find((v) => v.name === variable);
  assert.ok(found, `the file has ${variable}`);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: found.index }));
  const drawn = await panel.waitSent(isType("gridReady"), 0, `${variable} drawn`);
  return {
    panel,
    drawn,
    close: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

suite("Values closer than a reading's figures read apart (#1458, #1459, #1460)", function () {
  this.timeout(120_000);

  test("the Export PNG colour bar's middle label reads what the probe reads there (#1458)", async () => {
    // Three columns: the two ends and the value the bar's middle paints.
    for (const [values, labels] of [
      [[1700000000, 1700000000.5, 1700000001], ["1700000001", "1700000000.5", "1700000000"]],
      [[1700000000, 1700001800.5, 1700003601], ["1700003601", "1700001800.5", "1700000000"]],
    ] as const) {
      const bytes = classicNetcdf([["y", 2], ["x", 3]], [{ name: "t", dims: ["y", "x"], attrs: {}, values: [...values, ...values] }]);
      const { panel, close } = await fieldPanel(bytes, "t");
      const saved = vscode.window.showSaveDialog;
      try {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
        const from = panel.received.length;
        await panel.drive([{ id: "export-png", click: true }]);
        await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
        assert.deepStrictEqual((await panel.drive([])).exportedColorbarLabels, labels, `the PNG's labels for ${values.join(", ")}`);

        const asked = panel.sent.length;
        await panel.probe(0.5, 0.5);
        const reply = await panel.waitSent(isType("probeResult"), asked, "the probe");
        assert.strictEqual(reply.result.value, values[1]);
        const probe = (await panel.drive([])).probe;
        assert.ok(probe.includes(labels[1] + " · grid "),`the probe "${probe}" reads the middle label ${labels[1]}`);
      } finally {
        vscode.window.showSaveDialog = saved;
        close();
      }
    }
  });

  test("a cross-section rail labels times a twentieth of a second apart each as itself (#1459)", async () => {
    // Seconds past a billion, five rows a twentieth of a second apart: the
    // rail's round ticks are the points themselves.
    const t = [1700000000, 1700000000.05, 1700000000.1, 1700000000.15, 1700000000.2];
    const bytes = classicNetcdf(
      [["t", t.length], ["lat", 3], ["lon", 3]],
      [
        { name: "t", dims: ["t"], attrs: { units: "s" }, values: t },
        { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
        { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20] },
        { name: "v", dims: ["t", "lat", "lon"], attrs: {}, values: Array.from({ length: 9 * t.length }, (_, k) => k) },
      ],
    );
    const { panel, close } = await fieldPanel(bytes, "v");
    try {
      // A t × lat cross-section: t down the rows.
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 0 }, { id: "slice-x", value: 1 }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.height === t.length, from, "the t × lat cross-section");
      let rails = (await panel.drive([])).rails;
      // The axis's values come after the first render.
      for (let i = 0; i < 100 && !(rails && rails.y.length >= 2 && rails.y.every((l) => unminus(l.text) > 1e9)); i++) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        rails = (await panel.drive([])).rails;
      }
      assert.ok(rails && rails.shown, "the rails are shown");
      const texts = rails.y.map((l) => l.text);
      assert.ok(texts.length >= 3, `labels down the rail: ${texts.join(", ")}`);
      assert.strictEqual(new Set(texts).size, texts.length, `no two read alike: ${texts.join(", ")}`);
      // Each label reads the time of the row it is drawn beside, to within
      // half a step: measured on the page, so no font is assumed.
      const { top, bottom } = rails.image;
      for (const label of rails.y) {
        const row = (((label.box.top + label.box.bottom) / 2 - top) / (bottom - top)) * t.length - 0.5;
        const at = t[0] + 0.05 * row;
        assert.ok(Math.abs(unminus(label.text) - at) < 0.025, `"${label.text}" is drawn beside ${at}`);
        assert.ok(t.includes(unminus(label.text)), `"${label.text}" is a point's own time`);
      }
    } finally {
      close();
    }
  });

  test("ticking Manual on a field narrower than its pre-filled text draws what Auto drew (#1460)", async () => {
    // A pressure in pascals a sixteenth of a pascal wide, and a time less
    // than 0.1 s wide past a billion: both ends fill in alike.
    for (const [lo, hi] of [[101325.12, 101325.18], [1700000000.61, 1700000000.64]] as const) {
      const values = Array.from({ length: 16 }, (_, k) => (k === 15 ? hi : lo + ((hi - lo) * k) / 15));
      const { panel, drawn, close } = await fieldPanel(classicNetcdf([["y", 4], ["x", 4]], [{ name: "p", dims: ["y", "x"], attrs: {}, values }]), "p");
      try {
        assert.deepStrictEqual([drawn.usedMin, drawn.usedMax], [lo, hi]);
        const shown = (await panel.drive([])).rangeFields;
        assert.strictEqual(shown[0], shown[1], `the two ends fill in alike: ${shown.join(", ")}`);

        let from = panel.sent.length;
        await panel.drive([MANUAL]);
        let got = await panel.waitSent(drawnOrRefused, from, "the render on Manual");
        assert.strictEqual(got.type, "gridReady", `drawn, not refused: ${got.error ?? ""}`);
        assert.deepStrictEqual([got.options.rangeMin, got.options.rangeMax], [lo, hi], "the data's exact ends are asked for");
        assert.deepStrictEqual([got.usedMin, got.usedMax], [lo, hi]);
        assert.ok(Buffer.from(got.rgba).equals(Buffer.from(drawn.rgba)), "the same picture as Auto");
        let state = await panel.drive([]);
        assert.strictEqual(state.rangeNote, null, "no refusal");
        assert.deepStrictEqual(state.rangeFields, shown, "the fields show what they showed");

        // A hide and show keeps the exact ends and the text.
        from = panel.sent.length;
        await panel.hideShow();
        got = await panel.waitSent(drawnOrRefused, from, "the render after a hide and show");
        assert.strictEqual(got.type, "gridReady", `drawn, not refused: ${got.error ?? ""}`);
        assert.deepStrictEqual([got.options.rangeMin, got.options.rangeMax], [lo, hi], "still the exact ends");
        state = await panel.drive([]);
        assert.deepStrictEqual(state.rangeFields, shown, "and the text they showed");

        // An end typed is the user's.
        const typed = Number(((lo + hi) / 2).toFixed(2));
        from = panel.sent.length;
        await panel.drive([{ id: "range-min", value: typed }]);
        got = await panel.waitSent((m: Msg) => drawnOrRefused(m) && m.options?.rangeMin === typed, from, "the typed minimum");
        assert.strictEqual(got.type, "gridReady");
        assert.strictEqual(got.options.rangeMax, hi, "the untouched maximum is still the data's");
      } finally {
        close();
      }
    }
  });
});
