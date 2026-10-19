// The addon's one `Handle` (#1073), through the real built `.node`: opened the
// same way for every container, closed by `close()` or `[Symbol.dispose]`,
// answering the browser package's listing questions for every format, and
// handing bytes over as `Uint8Array`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type FieldglassNative, type Handle } from "../../native";
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
});
