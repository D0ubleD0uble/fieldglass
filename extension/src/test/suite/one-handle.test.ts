// The addon's one `Handle` (#1073), through the real built `.node`: opened the
// same way for every container, closed by `close()` or `[Symbol.dispose]`,
// answering the browser package's listing questions for every format, and
// handing bytes over as `Uint8Array`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vm from "vm";
import * as vscode from "vscode";

import { loadNative, type FieldglassNative, type Handle, type RenderOptions } from "../../native";
import { listVariables } from "../../provider";

const EXT_ID = "fieldglass.fieldglass";

function fixture(name: string): Uint8Array {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return fs.readFileSync(path.join(ext.extensionPath, "src", "test", "fixtures", name));
}

function storePath(name: string): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return path.join(ext.extensionPath, "..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores", name);
}

function addon(): FieldglassNative {
  const native = loadNative();
  assert.ok(native, "native module must load");
  return native;
}

/** The `code` an addon refusal carries, as the browser package's do. */
function codeOf(f: () => unknown): string | undefined {
  try {
    f();
  } catch (e) {
    return (e as { code?: string }).code;
  }
  assert.fail("expected the call to throw");
}

suite("One handle for every container (#1073)", () => {
  test("open detects each file's format and says how it is addressed", () => {
    const native = addon();
    for (const [file, format, addressing] of [
      ["cmc_wind_300_2010052400_p012.grib", "grib1", "messages"],
      ["regular_latlon_surface.grib2", "grib2", "messages"],
      ["ersst_v5_187001_cdf1.nc", "netcdf", "variables"],
      ["netcdf4_dimscale.nc", "netcdf", "variables"],
    ] as const) {
      const handle = native.open(fixture(file));
      assert.strictEqual(handle.format(), format, file);
      assert.strictEqual(handle.addressing(), addressing, file);
      assert.strictEqual(handle.count() > 0, addressing === "messages", file);
      handle.close();
    }
    const store = native.openStore(storePath("cf_v3"));
    assert.strictEqual(store.format(), "zarr");
    assert.strictEqual(store.addressing(), "variables");
    store.close();
  });

  test("bytes that are no container are refused with a code", () => {
    assert.strictEqual(codeOf(() => addon().open(new Uint8Array([1, 2, 3]))), "unsupported_format");
  });

  test("close and Symbol.dispose release a handle, and a closed one refuses every call", () => {
    const native = addon();
    const handle: Handle = native.open(fixture("regular_latlon_surface.grib2"));
    assert.strictEqual(typeof Symbol.dispose, "symbol", "the host has Symbol.dispose");
    assert.strictEqual(handle[Symbol.dispose], handle.close, "dispose is close");
    handle[Symbol.dispose]();
    handle.close();
    assert.throws(() => handle.count(), /closed/);
    assert.throws(() => handle.message(0), /closed/);

    const field = native.open(fixture("regular_latlon_surface.grib2")).decodeDisplay(0);
    assert.strictEqual(field[Symbol.dispose], field.close, "a display grid's dispose is close");
    field.close();
    field[Symbol.dispose]();
    assert.throws(() => field.ni(), /closed/, "a closed display grid refuses a read");
    assert.throws(() => field.grid(), /closed/);
  });

  test("each mode refuses the other's question as wrong_addressing", () => {
    const native = addon();
    const grib = native.open(fixture("regular_latlon_surface.grib2"));
    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const slice = { variable: sst.index, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] };
    assert.strictEqual(codeOf(() => netcdf.decode(0)), "wrong_addressing");
    assert.strictEqual(codeOf(() => netcdf.message(0)), "wrong_addressing");
    assert.strictEqual(codeOf(() => grib.decodeSlice(slice)), "wrong_addressing");
    assert.strictEqual(
      codeOf(() => grib.renderGrid(slice, { projection: "source", resampling: "nearest", flipY: false })),
      "wrong_addressing",
    );
    // The listings answer empty for a message stream rather than refusing.
    assert.deepStrictEqual(grib.variables(), []);
    assert.deepStrictEqual(grib.leftOut(), []);
    assert.deepStrictEqual(grib.undrawable(), []);
  });

  test("a NetCDF handle says what it leaves out and what draws no map, as a store's does", () => {
    const handle = addon().open(fixture("netcdf4_unsupported_type.nc"));
    assert.deepStrictEqual(
      handle.leftOut().map((a) => [a.name, a.cause.code]),
      [
        ["station_info", "unsupported_dtype"],
        ["visits", "unsupported_dtype"],
      ],
    );
    assert.deepStrictEqual(
      handle.undrawable().map((a) => [a.name, a.cause.code]),
      [
        ["temperature", "too_few_dimensions"],
        ["time", "coordinate"],
      ],
    );
  });

  test("the panel's listing typesets each variable's units, for a NetCDF file and a Zarr store", () => {
    const native = addon();
    // The addon hands over the file's own spelling, as the browser package
    // does; the panel shows it typeset (ADR-0007).
    const netcdf = native.open(fixture("netcdf_classic_dummy.nc"));
    const raw = netcdf.variables().find((v) => v.name === "u_1205");
    assert.strictEqual(raw?.units, "m/s", "the file's own spelling crosses");
    const shown = listVariables(native, netcdf).find((v) => v.name === "u_1205");
    assert.strictEqual(shown?.units, "m s⁻¹");

    // A store of two 2-D arrays: one in CF's `m s-1`, one with blank units.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1073-"));
    try {
      const array = (name: string, units: string) => {
        fs.mkdirSync(path.join(dir, name));
        fs.writeFileSync(
          path.join(dir, name, ".zarray"),
          JSON.stringify({
            zarr_format: 2,
            shape: [2, 2],
            chunks: [2, 2],
            dtype: "<f4",
            compressor: null,
            fill_value: null,
            filters: null,
            order: "C",
          }),
        );
        fs.writeFileSync(path.join(dir, name, ".zattrs"), JSON.stringify({ _ARRAY_DIMENSIONS: ["y", "x"], units }));
        fs.writeFileSync(path.join(dir, name, "0.0"), Buffer.from(new Float32Array([1, 2, 3, 4]).buffer));
      };
      fs.writeFileSync(path.join(dir, ".zgroup"), JSON.stringify({ zarr_format: 2 }));
      array("speed", "m s-1");
      array("blank", "   ");
      const store = native.openStore(dir);
      const listed = new Map(listVariables(native, store).map((v) => [v.name, v.units]));
      assert.strictEqual(listed.get("speed"), "m s⁻¹", "a store's units are typeset too");
      assert.strictEqual(listed.get("blank"), null, "blank units are none");
      store.close();
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("decode and decodeSlice pack at the width asked for, and bytes cross as Uint8Array", () => {
    const native = addon();
    const grib = native.open(fixture("regular_latlon_surface.grib2"));
    const narrow = grib.decode(0, { dtype: "f32" });
    assert.ok(narrow.values instanceof Float32Array);
    assert.strictEqual(narrow.dtype, "f32");
    const wide = grib.decode(0, { dtype: "f64" });
    assert.ok(wide.values instanceof Float64Array);
    assert.strictEqual(wide.stats.validCount, narrow.stats.validCount);
    assert.ok(narrow.mask instanceof Uint8Array && !Buffer.isBuffer(narrow.mask), "mask is a Uint8Array");

    const csv = grib.exportCsv(0, "matrix");
    assert.ok(csv instanceof Uint8Array && !Buffer.isBuffer(csv), "CSV is a Uint8Array");
    const raster = grib.renderGrid(0, { projection: "source", resampling: "nearest", flipY: false });
    assert.ok(raster.rgba instanceof Uint8Array && !Buffer.isBuffer(raster.rgba), "rgba is a Uint8Array");

    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const slice = netcdf.decodeSlice(
      { variable: sst.index, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] },
      { dtype: "f64" },
    );
    assert.deepStrictEqual([slice.ni, slice.nj], [180, 89]);
    assert.ok(slice.stats.validCount > 0);
    // A typed array of slice indices reads as the array it holds, as the
    // browser package reads one.
    const typed = netcdf.decodeSlice({
      variable: sst.index,
      yDim: 2,
      xDim: 3,
      sliceIndices: new Uint32Array([0, 0, 0, 0]),
    });
    const plain = netcdf.decodeSlice({ variable: sst.index, yDim: 2, xDim: 3, sliceIndices: [0, 0, 0, 0] });
    assert.deepStrictEqual(Array.from(typed.values), Array.from(plain.values));
    assert.strictEqual(
      codeOf(() => netcdf.decodeSlice({ variable: sst.index, yDim: 2, xDim: 3, sliceIndex: [0] } as never)),
      "invalid_option",
      "a key the request does not take is refused, not ignored",
    );
  });

  test("binary data is not an options object, as on the browser package (#1239)", () => {
    const native = addon();
    const grib = native.open(fixture("regular_latlon_surface.grib2"));
    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const render: RenderOptions = {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    };
    const slice = {
      variable: sst.index,
      yDim: 2,
      xDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const line = {
      variable: sst.index,
      alongDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const combined = {
      a: line,
      b: line,
      op: native.combineOps()[0].value,
      holdB: false,
    };
    const lines = {
      latlon: new Float64Array(0),
      ringLengths: new Uint32Array(0),
    };
    const stats = grib.decode(0).stats;
    const display = grib.decodeDisplay(0);
    // Every object argument the addon reads; `bad` takes the object's place.
    const readers: [string, (bad: never) => unknown][] = [
      ["decode options", (bad) => grib.decode(0, bad)],
      ["decodeDisplay options", (bad) => grib.decodeDisplay(0, bad)],
      ["decodeSlice options", (bad) => netcdf.decodeSlice(slice, bad)],
      ["decodeLine options", (bad) => netcdf.decodeLine(line, bad)],
      [
        "decodeLineCombined options",
        (bad) => netcdf.decodeLineCombined(combined, bad),
      ],
      [
        "full-field stats",
        (bad) => grib.renderDisplayGrid(display, bad, render),
      ],
      ["renderGrid options", (bad) => grib.renderGrid(0, bad)],
      [
        "renderDisplayGrid options",
        (bad) => grib.renderDisplayGrid(display, stats, bad),
      ],
      ["projectOverlay options", (bad) => grib.projectOverlay(0, bad, lines)],
      ["projectContours options", (bad) => grib.projectContours(0, bad)],
      ["contour spacing", (bad) => grib.projectContours(0, render, bad)],
      [
        "projectVectors options",
        (bad) => grib.projectVectors({ u: 0, v: 0 }, bad),
      ],
      ["probePixel options", (bad) => grib.probePixel(0, bad, 0, 0)],
      ["slice request", (bad) => netcdf.decodeSlice(bad)],
      ["line request", (bad) => netcdf.decodeLine(bad)],
      ["combined line", (bad) => netcdf.decodeLineCombined(bad)],
      ["vector pair", (bad) => grib.projectVectors(bad, render)],
      ["slice axes", (bad) => netcdf.sliceGrid(bad)],
      ["polylines", (bad) => grib.projectOverlay(0, render, bad)],
    ];
    const views: [string, () => unknown][] = [
      ["Uint8Array", () => new Uint8Array(1)],
      ["Float64Array", () => new Float64Array(2)],
      ["DataView", () => new DataView(new ArrayBuffer(4))],
      ["ArrayBuffer", () => new ArrayBuffer(4)],
      ["Buffer", () => Buffer.alloc(1)],
    ];
    for (const [what, read] of readers) {
      for (const [kind, view] of views) {
        let thrown: { code?: string; message?: string } | undefined;
        try {
          read(view() as never);
        } catch (e) {
          thrown = e as { code?: string; message?: string };
        }
        assert.ok(thrown, `${what}: a ${kind} was read as an object`);
        assert.strictEqual(
          thrown.code,
          "invalid_option",
          `${what}, ${kind}: ${thrown.message}`,
        );
        assert.match(
          String(thrown.message),
          /it is not an object/,
          `${what}, ${kind}`,
        );
      }
    }
    // The spacing was read whatever it was; an array or a string is not one
    // either, as the browser package's contour options refuse them.
    for (const bad of [[], [1], "5", 5]) {
      assert.strictEqual(
        codeOf(() => grib.projectContours(0, render, bad as never)),
        "invalid_option",
        String(bad),
      );
    }

    // Left out or `null` is still the default.
    assert.strictEqual(grib.decode(0, null).dtype, grib.decode(0).dtype);
    assert.strictEqual(grib.decode(0, undefined).dtype, grib.decode(0).dtype);
    assert.strictEqual(
      grib.decodeDisplay(0, null).ni(),
      grib.decodeDisplay(0).ni(),
    );
    assert.ok(netcdf.decodeSlice(slice, null).stats.validCount > 0);
    assert.ok(netcdf.decodeLine(line, null).values.length > 0);
    const contours = grib.projectContours(0, render).xy.length;
    assert.strictEqual(
      grib.projectContours(0, render, null).xy.length,
      contours,
    );
    assert.strictEqual(
      grib.projectContours(0, render, undefined).xy.length,
      contours,
    );
    const ranged = { ...render, rangeMin: 0, rangeMax: 1 };
    assert.ok(
      grib.renderDisplayGrid(display, null, ranged).rgba.length > 0,
      "no stats with both range ends set",
    );
  });

  test("binary data is not a target or a field, and a message index or slice still is (#1239)", () => {
    const native = addon();
    const grib = native.open(fixture("regular_latlon_surface.grib2"));
    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const render: RenderOptions = {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    };
    const slice = {
      variable: sst.index,
      yDim: 2,
      xDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const lines = {
      latlon: new Float64Array(0),
      ringLengths: new Uint32Array(0),
    };
    // Each call that takes a `GridTarget` or `GridField`, on a handle of each
    // kind, so a good target can be shown to pass on the same call.
    const takers: [string, (h: Handle, target: never) => unknown][] = [
      ["renderGrid", (h, t) => h.renderGrid(t, render)],
      ["projectOverlay", (h, t) => h.projectOverlay(t, render, lines)],
      ["projectContours", (h, t) => h.projectContours(t, render)],
      ["probePixel", (h, t) => h.probePixel(t, render, 0, 0)],
      ["zonalMean", (h, t) => h.zonalMean(t)],
      ["exportCsv", (h, t) => h.exportCsv(t, "matrix")],
    ];
    const binary: [string, () => unknown][] = [
      ["Buffer", () => Buffer.alloc(1)],
      ["Uint8Array", () => new Uint8Array(1)],
      ["Float64Array", () => new Float64Array(2)],
      ["DataView", () => new DataView(new ArrayBuffer(4))],
      ["ArrayBuffer", () => new ArrayBuffer(4)],
    ];
    for (const [call, take] of takers) {
      for (const [kind, make] of binary) {
        let thrown: { code?: string; message?: string } | undefined;
        try {
          take(grib, make() as never);
        } catch (e) {
          thrown = e as { code?: string; message?: string };
        }
        assert.ok(thrown, `${call}: a ${kind} target was read`);
        assert.strictEqual(
          thrown.code,
          "invalid_option",
          `${call}, ${kind}: ${thrown.message}`,
        );
        assert.match(
          String(thrown.message),
          /binary data is neither a message index nor a slice/,
          `${call}, ${kind}`,
        );
      }
      // A message index and a slice object still read on the same call.
      take(grib, 0 as never);
      take(netcdf, slice as never);
    }
  });

  // Electron hands an empty `Float64Array` over with a data pointer that is
  // not 8-byte aligned. The addon used to make a slice of it, which a debug
  // build stops on and a release build carried on with as undefined
  // behaviour (#1247). Plain Node aligns it, so only this host shows it.
  test("empty polylines project to no overlay, on every handle (#1247)", () => {
    const native = addon();
    const render: RenderOptions = {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    };
    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const slice = {
      variable: sst.index,
      yDim: 2,
      xDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const zarr = native.openStore(storePath("cf_v3"));
    const t = zarr.variables().find((v) => v.name === "t");
    assert.ok(t && t.detectedYDim != null && t.detectedXDim != null, "the store's t has axes");
    const zarrSlice = {
      variable: t.index,
      yDim: t.detectedYDim,
      xDim: t.detectedXDim,
      sliceIndices: t.dims.map(() => 0),
    };
    const cases: [string, Handle, number | typeof slice][] = [
      ["grib1", native.open(fixture("cmc_wind_300_2010052400_p012.grib")), 0],
      ["grib2", native.open(fixture("regular_latlon_surface.grib2")), 0],
      ["netcdf", netcdf, slice],
      ["zarr", zarr, zarrSlice],
    ];
    for (const [what, handle, target] of cases) {
      const empty = handle.projectOverlay(target, render, {
        latlon: new Float64Array(0),
        ringLengths: new Uint32Array(0),
      });
      assert.strictEqual(empty.xy.length, 0, `${what}: no points`);
      assert.strictEqual(empty.segLengths.length, 0, `${what}: no runs`);
      // An empty view into a bigger buffer, at an offset, reads as empty too.
      const buffer = new ArrayBuffer(64);
      const offset = handle.projectOverlay(target, render, {
        latlon: new Float64Array(buffer, 8, 0),
        ringLengths: new Uint32Array(buffer, 4, 0),
      });
      assert.strictEqual(offset.xy.length, 0, `${what}: no points from an offset view`);
      handle.close();
    }
  });

  test("a SharedArrayBuffer and another realm's buffers are binary data too, as on the browser package (#1242)", () => {
    const native = addon();
    const grib = native.open(fixture("regular_latlon_surface.grib2"));
    const netcdf = native.open(fixture("ersst_v5_187001_cdf1.nc"));
    const sst = netcdf.variables()[0];
    const render: RenderOptions = {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    };
    const slice = {
      variable: sst.index,
      yDim: 2,
      xDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const line = {
      variable: sst.index,
      alongDim: 3,
      sliceIndices: [0, 0, 0, 0],
    };
    const combined = {
      a: line,
      b: line,
      op: native.combineOps()[0].value,
      holdB: false,
    };
    const lines = {
      latlon: new Float64Array(0),
      ringLengths: new Uint32Array(0),
    };
    const stats = grib.decode(0).stats;
    const display = grib.decodeDisplay(0);
    // Every object argument the addon reads, then every target and field.
    const objects: [string, (bad: never) => unknown][] = [
      ["decode options", (bad) => grib.decode(0, bad)],
      ["decodeDisplay options", (bad) => grib.decodeDisplay(0, bad)],
      ["decodeSlice options", (bad) => netcdf.decodeSlice(slice, bad)],
      ["decodeLine options", (bad) => netcdf.decodeLine(line, bad)],
      [
        "decodeLineCombined options",
        (bad) => netcdf.decodeLineCombined(combined, bad),
      ],
      [
        "full-field stats",
        (bad) => grib.renderDisplayGrid(display, bad, render),
      ],
      ["renderGrid options", (bad) => grib.renderGrid(0, bad)],
      [
        "renderDisplayGrid options",
        (bad) => grib.renderDisplayGrid(display, stats, bad),
      ],
      ["projectOverlay options", (bad) => grib.projectOverlay(0, bad, lines)],
      ["projectContours options", (bad) => grib.projectContours(0, bad)],
      ["contour spacing", (bad) => grib.projectContours(0, render, bad)],
      [
        "projectVectors options",
        (bad) => grib.projectVectors({ u: 0, v: 0 }, bad),
      ],
      ["probePixel options", (bad) => grib.probePixel(0, bad, 0, 0)],
      ["slice request", (bad) => netcdf.decodeSlice(bad)],
      ["line request", (bad) => netcdf.decodeLine(bad)],
      ["combined line", (bad) => netcdf.decodeLineCombined(bad)],
      ["vector pair", (bad) => grib.projectVectors(bad, render)],
      ["slice axes", (bad) => netcdf.sliceGrid(bad)],
      ["polylines", (bad) => grib.projectOverlay(0, render, bad)],
    ];
    const targets: [string, (bad: never) => unknown][] = [
      ["renderGrid", (t) => grib.renderGrid(t, render)],
      ["projectOverlay", (t) => grib.projectOverlay(t, render, lines)],
      ["projectContours", (t) => grib.projectContours(t, render)],
      ["probePixel", (t) => grib.probePixel(t, render, 0, 0)],
      ["zonalMean", (t) => grib.zonalMean(t)],
      ["exportCsv", (t) => grib.exportCsv(t, "matrix")],
    ];
    const buffers: [string, () => unknown][] = [
      ["SharedArrayBuffer", () => new SharedArrayBuffer(4)],
      ["another realm's ArrayBuffer", () => vm.runInNewContext("new ArrayBuffer(4)")],
      [
        "another realm's SharedArrayBuffer",
        () => vm.runInNewContext("new SharedArrayBuffer(4)"),
      ],
      ["another realm's Uint8Array", () => vm.runInNewContext("new Uint8Array(1)")],
    ];
    for (const [readers, words] of [
      [objects, /it is not an object/],
      [targets, /binary data is neither a message index nor a slice/],
    ] as const) {
      for (const [what, read] of readers) {
        for (const [kind, make] of buffers) {
          let thrown: { code?: string; message?: string } | undefined;
          try {
            read(make() as never);
          } catch (e) {
            thrown = e as { code?: string; message?: string };
          }
          assert.ok(thrown, `${what}: a ${kind} was read`);
          assert.strictEqual(
            thrown.code,
            "invalid_option",
            `${what}, ${kind}: ${thrown.message}`,
          );
          assert.match(String(thrown.message), words, `${what}, ${kind}`);
        }
      }
    }

    // Another realm's plain object is still an object, and its number a
    // message index.
    const f64 = vm.runInNewContext('({ dtype: "f64" })') as never;
    assert.strictEqual(grib.decode(0, f64).dtype, "f64");
    assert.ok(grib.renderGrid(vm.runInNewContext("0") as never, render).rgba.length > 0);
    assert.ok(netcdf.decodeSlice(vm.runInNewContext(`(${JSON.stringify(slice)})`) as never).stats.validCount > 0);
  });
});
