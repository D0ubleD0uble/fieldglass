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

import { loadNative, type MessageMeta, type NetcdfVariableMeta } from "../../native";
import {
  axisTickIndices,
  formatAxisValue,
  isMapSlice,
  renderImagePanelHtml,
  type SlicePanelData,
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

function sst(): { handle: ReturnType<typeof netcdf>; v: NetcdfVariableMeta } {
  const handle = netcdf(fixture("ersst_v5_187001_cdf1.nc"));
  const v = handle.variables().find((x) => x.name.endsWith("sst"));
  assert.ok(v, "sst is renderable");
  return { handle, v };
}

function netcdf(bytes: Buffer) {
  const native = loadNative();
  assert.ok(native, "native module must load");
  return native.NetcdfHandle.fromBytes(bytes);
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
    assert.strictEqual(formatAxisValue(-88.5, "degrees_north"), "-88.5");
    assert.strictEqual(formatAxisValue(Number.NaN, "hPa"), "");
  });

  test("the addon reports an axis's coordinates and units", () => {
    const { handle, v } = sst();
    const time = handle.axisValues(v.variableIndex, 0);
    assert.strictEqual(time.dimension, "time");
    assert.strictEqual(time.units, "minutes since 1870-01-01 00:00");
    assert.deepStrictEqual(time.coordinates, [0], "January 1870, the file's one step");

    const lat = handle.axisValues(v.variableIndex, 2);
    assert.strictEqual(lat.units, "degrees_north");
    assert.strictEqual(lat.coordinates?.length, lat.length);
    assert.strictEqual(lat.coordinates?.[0], -88);

    // WRF's Time has no coordinate array: an axis all the same.
    const wrf = netcdf(readerFixture("wrf_lambert.nc"));
    const t2 = wrf.variables().find((x) => x.name === "T2");
    assert.ok(t2, "T2 is renderable");
    const wrfTime = wrf.axisValues(t2.variableIndex, 0);
    assert.strictEqual(wrfTime.coordinates, undefined);
    assert.strictEqual(wrfTime.length, 1);
  });

  test("a cross-section is not placed on the Earth, and still renders", () => {
    const { handle, v } = sst();
    const base = { projection: "source" as const, resampling: "nearest" as const, flipY: false };
    // Time by latitude: 89 columns of latitude, one row of time.
    const plot = handle.renderSlice(v.variableIndex, 0, 2, [0, 0, 0, 0], base);
    assert.deepStrictEqual([plot.width, plot.height], [89, 1]);
    assert.strictEqual(plot.usedLatMin, undefined, "no geographic extent");
    // And the map target refuses it rather than inventing one.
    assert.throws(
      () => handle.renderSlice(v.variableIndex, 0, 2, [0, 0, 0, 0], { ...base, projection: "equirectangular" }),
      /lat|reproject|grid/i,
    );
    // The real map pair still reprojects.
    const map = handle.renderSlice(v.variableIndex, 2, 3, [0, 0, 0, 0], { ...base, projection: "equirectangular" });
    assert.ok(map.width > 0 && map.usedLatMin != null, "lat by lon is still a map");
  });

  test("the panel carries the axis rails and wires the mode", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const { v } = sst();
    const slice: SlicePanelData = {
      variables: [v],
      initial: { variableIndex: v.variableIndex, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] },
    };
    const meta = { gridType: "latlon", reprojectable: true } as unknown as MessageMeta;
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
    for (const fn of ["function isMapSlice(", "function axisTickIndices(", "function formatAxisValue("]) {
      assert.ok(html.includes(fn), `${fn} is injected`);
    }
    for (const wiring of [
      /type: 'axisRequest', dim: dim/,
      /else if \(msg\.type === 'axisResult'\) handleAxisResult\(msg\);/,
      /projection\.disabled = cross;/,
      /animationFrameArrived\(\);\s*syncCrossSectionMode\(\);/,
    ]) {
      assert.ok(wiring.test(html), `the panel script wires ${wiring}`);
    }
  });
});
