// Cross-sections: any two axes, drawn as a plot rather than a map (#171).
//
// The panel decides the mode from the axis pair, so what is pinned here is that
// decision, the labels it draws from the coordinate arrays, and that the addon
// hands those arrays over without decoding the field. The Rust half — that a
// non-horizontal pair is not placed on the Earth — is checked in
// `crates/fieldglass/tests/cross_section_placement.rs`.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type VariableInfo } from "../../native";
import {
  axisTickIndices,
  coordinateTicks,
  type CoordinateTick,
  formatAxisValue,
  isMapSlice,
  layoutAxisLabels,
  niceTicks,
  railTicks,
  renderImagePanelHtml,
  type PanelField,
  type SlicePanelData,
  typesetMinus,
  verticalAxisFlips,
} from "../../render-panel";

function extensionPath(): string {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return ext.extensionPath;
}

function fixture(name: string): Buffer {
  return fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", name));
}

/** A committed NetCDF fixture of the reader's own, read where it lives rather
 *  than copied in — the WRF files are only there. */
function readerFixture(name: string): Buffer {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", name);
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return fs.readFileSync(p);
}

function sst(): { handle: ReturnType<typeof netcdf>; v: VariableInfo } {
  const handle = netcdf(fixture("ersst_v5_187001_cdf1.nc"));
  const v = handle.variables().find((x) => x.name.endsWith("sst"));
  assert.ok(v, "sst is renderable");
  return { handle, v };
}

function netcdf(bytes: Buffer) {
  const native = loadNative();
  assert.ok(native, "native module must load");
  return native.open(bytes);
}

suite("Cross-sections", () => {
  test("only the file's own latitude/longitude pair is a map", () => {
    // sst(time, lev, lat, lon): lat by lon.
    assert.strictEqual(isMapSlice(2, 3, 2, 3), true);
    assert.strictEqual(isMapSlice(0, 3, 2, 3), false, "time by lon is a plot");
    assert.strictEqual(isMapSlice(3, 2, 2, 3), false, "the transposed pair is a plot");
    // A projected file (WRF, a swath) detects neither axis: the Rust side places
    // it, so the panel leaves it a map.
    assert.strictEqual(isMapSlice(1, 2, undefined, undefined), true);
    assert.strictEqual(isMapSlice(1, 2, null, null), true);
  });

  test("tick indices span the axis and never repeat", () => {
    assert.deepStrictEqual(axisTickIndices(10, 5), [0, 2, 5, 7, 9]);
    assert.deepStrictEqual(axisTickIndices(1, 5), [0], "a one-point axis has one tick");
    assert.deepStrictEqual(axisTickIndices(0, 5), [], "an empty axis has none");
    assert.deepStrictEqual(axisTickIndices(2, 5), [0, 1], "no more ticks than points");
    const many = axisTickIndices(720, 6);
    assert.deepStrictEqual([many[0], many[many.length - 1]], [0, 719], "first and last");
    assert.strictEqual(new Set(many).size, many.length, "no repeats");
  });

  test("a CF time is labelled as a date, and other axes as numbers", () => {
    assert.strictEqual(formatAxisValue(0, "minutes since 1870-01-01 00:00"), "1870-01-01");
    assert.strictEqual(formatAxisValue(481140, "minutes since 1870-01-01 00:00"), "1870-12-01 03:00");
    assert.strictEqual(formatAxisValue(6, "hours since 2020-01-01 00:00:00"), "2020-01-01 06:00");
    assert.strictEqual(formatAxisValue(15342.5, "days since 1978-01-01 12:00:00"), "2020-01-04");
    // Units that only look like a time, and a reference no date parser reads,
    // fall back to the number rather than inventing an epoch.
    assert.strictEqual(formatAxisValue(3, "parsecs since the big bang"), "3");
    assert.strictEqual(formatAxisValue(3, "days since whenever"), "3");
    // Plain axes: float noise trimmed, zero spelled plainly, precision kept.
    assert.strictEqual(formatAxisValue(0, "degrees_east"), "0");
    assert.strictEqual(formatAxisValue(0.1 + 0.2, ""), "0.3");
    assert.strictEqual(formatAxisValue(1013.25, "hPa"), "1013.25");
    // A true minus, as the line plot's caption writes one (#1043).
    assert.strictEqual(formatAxisValue(-88.5, "degrees_north"), "−88.5");
    assert.strictEqual(formatAxisValue(-0.0000001, ""), "−1e−7");
    assert.strictEqual(formatAxisValue(-0, ""), "0");
    assert.strictEqual(formatAxisValue(Number.NaN, "hPa"), "");
  });

  test("a CF reference date is read the way UDUNITS writes it (#1028)", () => {
    // Every NCEP/NCAR reanalysis file writes fractional seconds, which
    // Date.parse refuses, so these ticks used to read as hours.
    assert.strictEqual(formatAxisValue(1963536, "hours since 1800-01-01 00:00:0.0"), "2024-01-01");
    assert.strictEqual(formatAxisValue(1972296, "hours since 1800-01-01 00:00:0.0"), "2024-12-31");
    // One-digit fields and no time at all.
    assert.strictEqual(formatAxisValue(45000, "days since 1900-1-1"), "2023-03-17");
    // ISO, with `T` and a `Z`.
    assert.strictEqual(formatAxisValue(1700000000, "seconds since 1970-01-01T00:00:00Z"), "2023-11-14 22:13");
    // Seconds on the reference carry into the minute.
    assert.strictEqual(formatAxisValue(30, "seconds since 2000-01-01 12:00:30"), "2000-01-01 12:01");
    // A zone: midnight five and a half hours ahead of UTC is 18:30 the day
    // before, and six hours behind is 06:00.
    assert.strictEqual(formatAxisValue(0, "hours since 2020-01-01 00:00:00 +05:30"), "2019-12-31 18:30");
    assert.strictEqual(formatAxisValue(0, "hours since 2020-01-01 00:00 -6"), "2020-01-01 06:00");
    assert.strictEqual(formatAxisValue(0, "hours since 2020-01-01 00:00 UTC"), "2020-01-01");
    // A year below 100 is that year, not 19xx.
    assert.strictEqual(formatAxisValue(366, "days since 0001-01-01"), "0002-01-02");
    // A reference with something after it no rule covers is not guessed at.
    assert.strictEqual(formatAxisValue(3, "days since 2020-01-01 noon"), "3");
    // Past the range a Date can hold: the number, not a thrown error.
    assert.strictEqual(formatAxisValue(1e300, "days since 2020-01-01"), "1e+300");
    // A field out of range is not rolled into the next month or day.
    assert.strictEqual(formatAxisValue(3, "days since 2020-13-45"), "3");
    assert.strictEqual(formatAxisValue(3, "days since 2020-01-01 10:60"), "3");
    // An offset is `+h`, `+hh`, `+hhmm` or `+hh:mm`; `+123` could be either.
    assert.strictEqual(formatAxisValue(0, "hours since 2020-01-01 00:00 +0530"), "2019-12-31 18:30");
    assert.strictEqual(formatAxisValue(3, "hours since 2020-01-01 00:00 +123"), "3");
    // A date past year 9999 has no YYYY-MM-DD form.
    assert.strictEqual(formatAxisValue(1e7, "days since 2000-01-01"), "10000000");
    // A long run of trailing spaces is read at once (no backtracking).
    assert.strictEqual(formatAxisValue(0, "days since 2000-01-01" + " ".repeat(100_000)), "2000-01-01");
  });

  test("a number is signed with a true minus, and zero is not negative (#1043)", () => {
    assert.strictEqual(typesetMinus("-72.8"), "−72.8");
    assert.strictEqual(typesetMinus("1.5e-7"), "1.5e−7");
    assert.strictEqual(typesetMinus("-0.0"), "0.0");
    assert.strictEqual(typesetMinus("300"), "300");
  });

  test("a value axis is ticked at round numbers inside its range (#1043)", () => {
    assert.deepStrictEqual(niceTicks(-72.8, 31.5, 5), [-50, -25, 0, 25]);
    assert.deepStrictEqual(niceTicks(238.35, 300.83, 4), [240, 260, 280, 300]);
    assert.deepStrictEqual(niceTicks(0, 1, 5), [0, 0.2, 0.4, 0.6, 0.8, 1], "no float noise");
    assert.deepStrictEqual(niceTicks(5, 5, 4), [], "no span, no ticks");
    assert.deepStrictEqual(niceTicks(Number.NaN, 1, 4), []);
    assert.ok(niceTicks(0, 1e300, 4).length <= 100, "bounded");
  });

  test("an axis in degrees steps the way a graticule does (#1229)", () => {
    assert.deepStrictEqual(niceTicks(-87.86, 87.86, 6, true), [-60, -30, 0, 30, 60]);
    assert.deepStrictEqual(niceTicks(-87.86, 87.86, 6), [-50, 0, 50], "a number that is not an angle steps in fives");
    assert.deepStrictEqual(niceTicks(0, 357.5, 5, true), [0, 90, 180, 270]);
    assert.deepStrictEqual(niceTicks(-32.5, -27.5, 5, true), [-32, -31, -30, -29, -28]);
    assert.deepStrictEqual(niceTicks(10, 10.5, 5, true), [10, 10.1, 10.2, 10.3, 10.4, 10.5], "below a degree, tenths");
  });

  // The three axes #1229 was found on, each ticked at round latitudes at the
  // index where that latitude falls: what the zonal average's x axis and a
  // cross-section's rail show.
  suite("a coordinate axis is ticked at round values where they fall (#1229)", () => {
    const at = (ticks: CoordinateTick[] | null) => {
      assert.ok(ticks, "the axis is ticked at round values");
      return ticks.map((t) => [t.text, Number(t.index.toFixed(4))]);
    };

    test("reduced_gg_n32_smooth.grib1's zonal average: 64 Gaussian latitudes, north first", () => {
      const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib1", "tests", "fixtures", "reduced_gg_n32_smooth.grib1");
      const native = loadNative();
      assert.ok(native, "native module must load");
      const zonal = native.open(fs.readFileSync(p)).zonalMean(0);
      assert.strictEqual(zonal.coordinates?.length, 64);
      assert.strictEqual(Number(zonal.coordinates?.[0].toFixed(4)), 87.8638, "the raw first latitude #1229 saw");
      const axis = { coordinates: zonal.coordinates ?? null, units: zonal.coordinateUnits ?? null };
      // Gaussian latitudes are uneven, and none is round: each tick falls
      // between two, 0 halfway between the pair either side of the equator.
      assert.deepStrictEqual(at(coordinateTicks(axis, 6)), [
        ["−60", 53.0011], ["−30", 42.2504], ["0", 31.5], ["30", 20.7496], ["60", 9.9989],
      ]);
      assert.deepStrictEqual(at(coordinateTicks(axis, 5)), [["−45", 47.6256], ["0", 31.5], ["45", 15.3744]]);
    });

    test("samples/gfs.grib2's zonal average: 721 rows at 0.25°, north first", () => {
      // The sample is not committed; its latitudes are, as the addon reads
      // them: 90 to −90 in steps of 0.25.
      const axis = { coordinates: Array.from({ length: 721 }, (_, j) => 90 - 0.25 * j), units: "degrees_north" };
      assert.deepStrictEqual(at(coordinateTicks(axis, 6)), [
        ["−90", 720], ["−60", 600], ["−30", 480], ["0", 360], ["30", 240], ["60", 120], ["90", 0],
      ]);
    });

    test("samples/0.6.0/air_v2.zarr's time × latitude section: 73 rows at 2.5°", () => {
      const axis = { coordinates: Array.from({ length: 73 }, (_, j) => 90 - 2.5 * j), units: "degrees_north" };
      // The Y rail asks for 6 ticks, the X rail for 5.
      assert.deepStrictEqual(at(coordinateTicks(axis, 6)), [
        ["−90", 72], ["−60", 60], ["−30", 48], ["0", 36], ["30", 24], ["60", 12], ["90", 0],
      ]);
      assert.deepStrictEqual(at(coordinateTicks(axis, 5)), [["−90", 72], ["−45", 54], ["0", 36], ["45", 18], ["90", 0]]);
    });

    test("an uneven axis that rises is placed between its points", () => {
      const depth = { coordinates: [0, 10, 50, 200, 1000], units: "m", vertical: { logPressure: false } };
      assert.deepStrictEqual(at(coordinateTicks(depth, 5)), [
        ["0", 0], ["200", 3], ["400", 3.25], ["600", 3.5], ["800", 3.75], ["1000", 4],
      ]);
      // A cross-section's rail draws one row per point, where those round
      // depths would crowd the last row (#1258): it labels its points.
      assert.deepStrictEqual(railTicks(depth, 5, 5), [
        { index: 0, text: "0" }, { index: 1, text: "10" }, { index: 2, text: "50" }, { index: 3, text: "200" }, { index: 4, text: "1000" },
      ]);
    });

    // A cross-section rail spaces its rows evenly by index, so round values
    // evenly spaced in value suit it only where the axis's points are near
    // evenly spaced themselves (#1258).
    suite("a cross-section rail labels the whole of an uneven axis (#1258)", () => {
      // An ocean model's depth levels, in metres: close together near the
      // surface, far apart at depth.
      const depths = [
        0, 5, 10, 20, 30, 50, 75, 100, 125, 150, 200, 250, 300, 400, 500, 600, 700, 800, 900, 1000,
        1100, 1200, 1300, 1400, 1500, 1750, 2000, 2500, 3000, 3500, 4000, 4500, 5000,
      ];
      const depth = { coordinates: depths, units: "m", vertical: { logPressure: false } };
      // Round ticks as a rail places them, in order of index.
      const placed = (ticks: CoordinateTick[] | null) => {
        assert.ok(ticks, "the axis is ticked at round values");
        return ticks.map((t) => ({ index: t.index, text: t.text })).sort((a, b) => a.index - b.index);
      };

      test("a realistic depth axis is labelled down its whole length", () => {
        const ticks = railTicks(depth, depths.length, 6);
        const last = depths.length - 1;
        const at = ticks.map((t) => t.index);
        assert.ok(at.every((v, i) => i === 0 || v > at[i - 1]), `in order down the rail: ${at}`);
        const gaps = [at[0], ...at.slice(1).map((v, i) => v - at[i]), last - at[at.length - 1]];
        assert.ok(Math.max(...gaps) <= last / 3, `no gap past a third of the rail: ${at} of ${last}`);
        const top = ticks.filter((t) => t.index <= last / 2);
        assert.ok(top.length >= 2, `the upper half has two labels: ${ticks.map((t) => t.text)}`);
        // Each label is the depth at its row.
        for (const t of ticks) assert.strictEqual(t.text, String(depths[t.index]));
        // What #1229 drew there: round depths, all but 0 in the lower half.
        const round = coordinateTicks(depth, 6);
        assert.ok(round, "the axis has round depths");
        assert.strictEqual(round.filter((t) => t.index <= last / 2).length, 1, "round depths would leave the upper half bare");
      });

      test("even and Gaussian axes keep their round values", () => {
        const air = { coordinates: Array.from({ length: 73 }, (_, j) => 90 - 2.5 * j), units: "degrees_north" };
        assert.deepStrictEqual(railTicks(air, 73, 6), placed(coordinateTicks(air, 6)));
        assert.deepStrictEqual(railTicks(air, 73, 5), placed(coordinateTicks(air, 5)));
        const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib1", "tests", "fixtures", "reduced_gg_n32_smooth.grib1");
        const native = loadNative();
        assert.ok(native, "native module must load");
        const zonal = native.open(fs.readFileSync(p)).zonalMean(0);
        const gaussian = { coordinates: zonal.coordinates ?? null, units: zonal.coordinateUnits ?? null };
        assert.deepStrictEqual(railTicks(gaussian, 64, 6), placed(coordinateTicks(gaussian, 6)));
        assert.deepStrictEqual(railTicks(gaussian, 64, 5), placed(coordinateTicks(gaussian, 5)));
        // Round values that leave a short stretch at one end: 0, 50, 100 on
        // an axis from 0 to 130.
        const even = { coordinates: Array.from({ length: 14 }, (_, j) => 10 * j), units: "m" };
        assert.deepStrictEqual(railTicks(even, 14, 5).map((t) => [t.text, t.index]), [["0", 0], ["50", 5], ["100", 10]]);
      });

      test("an axis with no round values, or another length, keeps its points", () => {
        const days = { coordinates: [1963536, 1963560, 1963584], units: "hours since 1800-01-01 00:00:0.0" };
        assert.deepStrictEqual(
          railTicks(days, 3, 5).map((t) => t.text),
          days.coordinates.map((v) => formatAxisValue(v, days.units)),
          "a time's own dates",
        );
        assert.deepStrictEqual(railTicks(null, 4, 5), [0, 1, 2, 3].map((i) => ({ index: i, text: String(i) })), "an index");
        const short = { coordinates: [0, 10, 20], units: "m" };
        assert.deepStrictEqual(railTicks(short, 5, 5).map((t) => t.text), ["0", "10", "20", "3", "4"], "past the coordinates, an index");
      });
    });

    test("an axis keeps its own values where round ones would not do", () => {
      const days = { coordinates: [1963536, 1963560, 1963584], units: "hours since 1800-01-01 00:00:0.0" };
      assert.strictEqual(coordinateTicks(days, 5), null, "a time ticks as dates");
      const levels = { coordinates: [1000, 850, 500, 10], units: "millibar", vertical: { logPressure: true } };
      assert.strictEqual(coordinateTicks(levels, 5), null, "a pressure ticks at its levels");
      assert.strictEqual(coordinateTicks({ coordinates: [0, 10, 5], units: "m" }, 5), null, "values that turn back");
      assert.strictEqual(coordinateTicks({ coordinates: [0, 10, 10], units: "m" }, 5), null, "or repeat");
      assert.strictEqual(coordinateTicks({ coordinates: [0, Number.NaN], units: "m" }, 5), null, "a hole");
      assert.strictEqual(coordinateTicks({ coordinates: [4], units: "m" }, 5), null, "one point");
      assert.strictEqual(coordinateTicks({ coordinates: null, units: null }, 5), null, "no coordinates");
      assert.strictEqual(coordinateTicks(null, 5), null);
      // Gaussian latitudes in steps of 90 leave only the equator.
      const gaussian = { coordinates: [87.86, 30, -30, -87.86], units: "degrees_north" };
      assert.strictEqual(coordinateTicks(gaussian, 3), null, "a span too short for two round values");
    });
  });

  test("end labels are slid inside the axis, and a label that would overlap is dropped (#1043)", () => {
    // Centred on 0 and 100, on an axis from 0 to 100: both slide inward.
    assert.deepStrictEqual(layoutAxisLabels([{ centre: 0, size: 20 }, { centre: 100, size: 20 }], 0, 100, 4), [0, 80]);
    // A middle label too close to an end is dropped; the ends are kept first.
    assert.deepStrictEqual(layoutAxisLabels([{ centre: 0, size: 20 }, { centre: 12, size: 20 }, { centre: 50, size: 20 }, { centre: 100, size: 20 }], 0, 100, 4), [0, null, 40, 80]);
    // Two ends that cannot both fit: the first stays.
    assert.deepStrictEqual(layoutAxisLabels([{ centre: 0, size: 20 }, { centre: 10, size: 20 }], 0, 30, 4), [0, null]);
    // An axis shorter than two labels and the gap: one end, not both run off it.
    assert.deepStrictEqual(layoutAxisLabels([{ centre: 0, size: 14 }, { centre: 37, size: 14 }], 0, 30, 4), [0, null]);
    // A label longer than the axis is not drawn at all.
    assert.deepStrictEqual(layoutAxisLabels([{ centre: 5, size: 50 }], 0, 30, 4), [null]);
    assert.deepStrictEqual(layoutAxisLabels([], 0, 30, 4), []);
  });

  test("a vertical axis is turned the way CF's positive says (#1043)", () => {
    const axis = (coordinates: number[] | null, positive: string | null) => ({
      coordinates,
      vertical: positive === null ? null : { positive },
    });
    // NCEP/NCAR level, 1000 first, positive down: 1000 goes to the bottom.
    assert.strictEqual(verticalAxisFlips(axis([1000, 500, 10], "down")), true);
    // The same levels stored top first already read that way.
    assert.strictEqual(verticalAxisFlips(axis([10, 500, 1000], "down")), false);
    // A depth, 0 first, positive down: the surface is already at the top.
    assert.strictEqual(verticalAxisFlips(axis([0, 10, 200], "down")), false);
    // A height, 0 first, positive up: the ground goes to the bottom.
    assert.strictEqual(verticalAxisFlips(axis([0, 1000, 5000], "up")), true);
    // Not vertical, no coordinates, or one point: storage order.
    assert.strictEqual(verticalAxisFlips(axis([-90, 0, 90], null)), false);
    assert.strictEqual(verticalAxisFlips(axis(null, "down")), false);
    assert.strictEqual(verticalAxisFlips(axis([1000], "down")), false);
    assert.strictEqual(verticalAxisFlips(null), false);
    // A Z axis nothing orients is left as stored.
    assert.strictEqual(verticalAxisFlips({ coordinates: [1000, 10], vertical: { positive: null } }), false);
  });

  test("the addon reports an axis's coordinates and units", () => {
    const { handle, v } = sst();
    const time = handle.axisValues(v.index, 0);
    assert.strictEqual(time.dimension, "time");
    assert.strictEqual(time.units, "minutes since 1870-01-01 00:00");
    assert.deepStrictEqual(time.coordinates, [0], "January 1870, the file's one step");

    const lat = handle.axisValues(v.index, 2);
    assert.strictEqual(lat.units, "degrees_north");
    assert.strictEqual(lat.coordinates?.length, lat.length);
    assert.strictEqual(lat.coordinates?.[0], -88);

    // WRF's Time has no coordinate array: an axis all the same.
    const wrf = netcdf(readerFixture("wrf_lambert.nc"));
    const t2 = wrf.variables().find((x) => x.name === "T2");
    assert.ok(t2, "T2 is renderable");
    const wrfTime = wrf.axisValues(t2.index, 0);
    assert.strictEqual(wrfTime.coordinates, null);
    assert.strictEqual(wrfTime.length, 1);
  });

  test("a cross-section is not placed on the Earth, and still renders", () => {
    const { handle, v } = sst();
    const base = { projection: "source" as const, resampling: "nearest" as const, flipY: false };
    // Time by latitude: 89 columns of latitude, one row of time.
    const plot = handle.renderGrid({ variable: v.index, yDim: 0, xDim: 2, sliceIndices: [0, 0, 0, 0] }, base);
    assert.deepStrictEqual([plot.width, plot.height], [89, 1]);
    assert.strictEqual(plot.usedLatMin, null, "no geographic extent");
    // And the map target refuses it rather than inventing one.
    assert.throws(
      () => handle.renderGrid({ variable: v.index, yDim: 0, xDim: 2, sliceIndices: [0, 0, 0, 0] }, { ...base, projection: "equirectangular" }),
      /lat|reproject|grid/i,
    );
    // The real map pair still reprojects.
    const map = handle.renderGrid({ variable: v.index, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] }, { ...base, projection: "equirectangular" });
    assert.ok(map.width > 0 && map.usedLatMin != null, "lat by lon is still a map");
  });

  test("the panel carries the axis rails and wires the mode", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const { v } = sst();
    const slice: SlicePanelData = {
      variables: [v],
      initial: { variableIndex: v.index, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] },
    };
    const meta = { grid: { label: "latlon" }, reprojectable: true } as unknown as PanelField;
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      meta,
      "summary",
      native.colormaps(),
      native.combineOps(),
      slice,
    );
    for (const id of ["axis-y", "axis-x", "axis-caption", "cross-section-note", "overlay-fieldset"]) {
      assert.ok(html.includes(`id="${id}"`), `the panel has #${id}`);
    }
    for (const fn of [
      "function isMapSlice(",
      "function axisTickIndices(",
      "function formatAxisValue(",
      "function typesetMinus(",
      "function layoutAxisLabels(",
      "function verticalAxisFlips(",
      "function niceTicks(",
      "function coordinateTicks(",
    ]) {
      assert.ok(html.includes(fn), `${fn} is injected`);
    }
    for (const wiring of [
      /type: 'axisRequest', dim: dim/,
      /else if \(msg\.type === 'axisResult'\) handleAxisResult\(msg\);/,
      /const CROSS_SECTION_HIDES = \['projection', 'resampling', 'overlay'\];/,
      /animationFrameArrived\(\);\s*syncCrossSectionMode\(\);/,
    ]) {
      assert.ok(wiring.test(html), `the panel script wires ${wiring}`);
    }
  });
});
