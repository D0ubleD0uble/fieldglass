// The wire contract for the API's own types, through the built addon (#574).
//
// A returned `MessageInfo` carries every key, and a field with nothing to report
// is `null`, never `undefined` and never missing (decided 2026-09-29). Its
// declaration (`api.generated.ts`) is generated from the Rust schema and says
// `field: T | null`. #288 was a declaration and a runtime that disagreed about
// exactly this, so the check here is against a real message read through the
// real addon, and against the schema the declaration came from, rather than
// against either alone.
//
// The Rust gates cannot see this: napi-rs's `serde_json::Value` conversion runs
// only inside Node.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type MessageInfo } from "../../native";

const EXT_ID = "fieldglass.fieldglass";

function extensionPath(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return ext.extensionPath;
}

function fixture(name: string): Buffer {
  return fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", name));
}

interface SchemaNode {
  type?: string | string[];
  anyOf?: SchemaNode[];
  oneOf?: SchemaNode[];
  $ref?: string;
  properties?: Record<string, SchemaNode>;
  required?: string[];
}

/** The schema the declarations are generated from, read from the repository
 *  checkout the test host runs in. */
function schemaDefs(): Record<string, SchemaNode> {
  const file = path.join(
    extensionPath(),
    "..",
    "crates",
    "fieldglass",
    "schema",
    "api.schema.json",
  );
  return (JSON.parse(fs.readFileSync(file, "utf8")) as { $defs: Record<string, SchemaNode> })
    .$defs;
}

/** Whether a property's schema admits `null`. */
function nullable(node: SchemaNode): boolean {
  const kinds = Array.isArray(node.type) ? node.type : node.type ? [node.type] : [];
  if (kinds.includes("null")) return true;
  return [...(node.anyOf ?? []), ...(node.oneOf ?? [])].some(nullable);
}

/** The definition a property's schema points at, if it is a `$ref` or a
 *  nullable one. */
function referenced(node: SchemaNode): string | undefined {
  if (node.$ref) return node.$ref.replace("#/$defs/", "");
  for (const member of [...(node.anyOf ?? []), ...(node.oneOf ?? [])]) {
    if (member.$ref) return member.$ref.replace("#/$defs/", "");
  }
  return undefined;
}

/** Every disagreement between a returned object and its schema definition:
 *  a required key missing, an `undefined` anywhere, or a `null` where the
 *  schema does not admit one. Recurses into nested definitions (`grid`). */
function disagreements(
  value: unknown,
  name: string,
  defs: Record<string, SchemaNode>,
  where: string,
): string[] {
  const def = defs[name];
  assert.ok(def?.properties, `the schema defines ${name} as an object`);
  const object = value as Record<string, unknown>;
  const out: string[] = [];
  for (const key of def.required ?? []) {
    if (!(key in object)) out.push(`${where}.${key} is missing`);
  }
  for (const [key, node] of Object.entries(def.properties)) {
    if (!(key in object)) continue;
    const v = object[key];
    if (v === undefined) {
      out.push(`${where}.${key} is undefined`);
    } else if (v === null) {
      if (!nullable(node)) out.push(`${where}.${key} is null, which the schema does not admit`);
    } else {
      const inner = referenced(node);
      if (inner && defs[inner]?.properties) {
        out.push(...disagreements(v, inner, defs, `${where}.${key}`));
      }
    }
  }
  return out;
}

suite("API wire contract (#574)", () => {
  // The #288 message: GRIB1 spectral coefficients on a declared grid nothing
  // places a point on, with none of GRIB2's identification.
  test("a spectral GRIB1 message's absent fields are present nulls", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const info: MessageInfo = native.Grib1Handle.fromBytes(
      fixture("spectral_simple_t63.grib1"),
    ).message(0);

    for (const key of ["discipline", "productionStatus", "dataType"] as const) {
      assert.ok(key in info, `${key} is present`);
      assert.strictEqual(info[key], null, `${key} is null, not undefined`);
    }
    assert.ok(info.grid, "the message declares a grid");
    for (const key of ["boundsLonlat", "corners", "proj4", "x0", "dx"] as const) {
      assert.ok(key in info.grid, `grid.${key} is present`);
      assert.strictEqual(info.grid[key], null, `grid.${key} is null, not undefined`);
    }
    // The strict guard the declaration asks for is the right one: this is the
    // shape of the check that failed open in #288.
    assert.ok(info.grid.boundsLonlat === null, "=== null sees the absence");
  });

  test("every message agrees with the schema its declaration came from", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const defs = schemaDefs();
    const messages: [string, MessageInfo][] = [
      [
        "spectral_simple_t63.grib1",
        native.Grib1Handle.fromBytes(fixture("spectral_simple_t63.grib1")).message(0),
      ],
      [
        "cmc_wind_300_2010052400_p012.grib",
        native.Grib1Handle.fromBytes(fixture("cmc_wind_300_2010052400_p012.grib")).message(0),
      ],
      [
        "regular_latlon_surface.grib2",
        native.Grib2Handle.fromBytes(fixture("regular_latlon_surface.grib2")).message(0),
      ],
      [
        "healpix_n4_ring.grib2",
        native.Grib2Handle.fromBytes(fixture("healpix_n4_ring.grib2")).message(0),
      ],
    ];
    let nulls = 0;
    for (const [name, info] of messages) {
      assert.deepStrictEqual(disagreements(info, "MessageInfo", defs, name), []);
      nulls += Object.values(info).filter((v) => v === null).length;
    }
    // A corpus with no absent field would pass the check above vacuously.
    assert.ok(nulls > 0, "at least one field is null across the messages checked");
  });

  // The addon's own objects keep the same contract (#574): every key present,
  // `null` for a field with nothing to report. Each case below is read through
  // the built addon on a real file chosen so the field is absent, because a
  // declaration of `T | null` over a value that is really `undefined` is the
  // #288 shape.
  test("the addon's own returned objects carry absent fields as present nulls", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const nullUnderItsKey = (what: string, object: object, keys: string[]) => {
      const record = object as Record<string, unknown>;
      for (const key of keys) {
        assert.ok(key in record, `${what}.${key} is present`);
        assert.strictEqual(record[key], null, `${what}.${key} is null, not undefined`);
      }
      for (const [key, value] of Object.entries(record)) {
        assert.notStrictEqual(value, undefined, `${what}.${key} is never undefined`);
      }
    };

    // DatasetMeta: a classic file has no note and no HDF5 superblock.
    const classic = native.NetcdfHandle.fromBytes(fixture("netcdf_classic_dummy.nc"));
    nullUnderItsKey("DatasetMeta", classic.metadata(), ["note", "hdf5SuperblockVersion"]);

    // `lat_bnds` has no coordinate arrays of its own and no time axis.
    const nc = native.NetcdfHandle.fromBytes(fixture("netcdf4_dimscale.nc"));
    const bounds = nc.variables().find((v) => v.name === "lat_bnds");
    assert.ok(bounds, "the fixture has lat_bnds");
    nullUnderItsKey("NetcdfVariableMeta", bounds, ["detectedTimeDim"]);
    for (const key of ["detectedYDim", "detectedXDim"]) {
      assert.ok(key in bounds, `NetcdfVariableMeta.${key} is present`);
    }
    const [y, x] = [bounds.dims.length - 2, bounds.dims.length - 1];

    // AxisValues and LineResult: an axis with no coordinate array.
    nullUnderItsKey("AxisValues", nc.axisValues(bounds.variableIndex, x), ["coordinates"]);
    nullUnderItsKey(
      "LineResult",
      nc.line(bounds.variableIndex, x, bounds.dims.map(() => 0)),
      ["coordinates", "coordinateUnits"],
    );

    // RenderedGrid: the source view has no geographic extent to echo.
    const options = {
      projection: "source" as const,
      resampling: "nearest" as const,
      flipY: false,
    };
    const zero = bounds.dims.map(() => 0);
    nullUnderItsKey("RenderedGrid", nc.renderSlice(bounds.variableIndex, y, x, zero, options), [
      "usedLatMin",
      "usedLatMax",
      "usedLonMin",
      "usedLonMax",
    ]);

    // ProbeResult: a cell nothing places has a value and no position.
    const probe = nc.probe(bounds.variableIndex, y, x, zero, options, 0, 0);
    assert.ok(probe, "the pixel is on the raster");
    nullUnderItsKey("ProbeResult", probe, ["lat", "lon"]);
    assert.strictEqual(typeof probe.value, "number", "and it still reads the value");

    // SliceGrid has no optional field; its answers for the same slice are the
    // ones the panel gates its projection picker on.
    assert.deepStrictEqual(nc.sliceGrid(bounds.variableIndex, y, x), {
      label: "source",
      placement: "unplaceable",
      reprojectable: false,
    });
  });

  test("an index past the file throws rather than returning an empty object", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.Grib2Handle.fromBytes(fixture("regular_latlon_surface.grib2"));
    assert.throws(() => handle.message(99));
  });
});
