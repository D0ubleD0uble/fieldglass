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
  const?: unknown;
  items?: SchemaNode;
  properties?: Record<string, SchemaNode>;
  required?: string[];
  "x-fieldglass-added-in"?: string;
}

/** The keys an object the library returns must carry: the definition's
 *  `required` list, and every key added after the type shipped
 *  (`x-fieldglass-added-in`, #1552). The schema leaves an added-in key out of
 *  `required` only so that an older object a host hands back still
 *  type-checks; everything this version returns has it, `null` when there is
 *  nothing to report (#1577). */
function returnedKeys(def: SchemaNode): string[] {
  const added = Object.entries(def.properties ?? {})
    .filter(([, node]) => node["x-fieldglass-added-in"] !== undefined)
    .map(([key]) => key);
  return [...(def.required ?? []), ...added];
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

/** The variant of a tagged union (`oneOf` of objects, each pinning its tag
 *  with a `const`) that an object's tag selects. */
function variantOf(
  def: SchemaNode | undefined,
  object: Record<string, unknown>,
): SchemaNode | undefined {
  return def?.oneOf?.find(
    (member) =>
      member.properties !== undefined &&
      Object.entries(member.properties).some(([, node]) => node.const !== undefined) &&
      Object.entries(member.properties).every(
        ([key, node]) => node.const === undefined || object[key] === node.const,
      ),
  );
}

/** Every disagreement between a returned object and its schema definition:
 *  a required or added-in key missing, a key the definition does not name,
 *  an `undefined` anywhere, or a `null` where the schema does not admit one.
 *  Recurses into nested definitions (`grid`), into each element of an array
 *  whose `items` is a definition (`dims`), and into the variant a tagged
 *  union's tag selects (`identification`, #773). An object nested inline
 *  under a property rather than through a `$ref` (`Georef.geometry`, `scan`)
 *  is not walked; `api_rules`' `added_in_keys` holds a mark there. Nor are
 *  nested arrays (an `items` of type array) or `prefixItems` tuples. */
function disagreements(
  value: unknown,
  name: string,
  defs: Record<string, SchemaNode>,
  where: string,
): string[] {
  const object = value as Record<string, unknown>;
  const def = defs[name]?.properties ? defs[name] : variantOf(defs[name], object);
  assert.ok(def?.properties, `the schema defines ${name} as an object, or a variant matches ${where}`);
  const out: string[] = [];
  for (const key of returnedKeys(def)) {
    if (!(key in object)) out.push(`${where}.${key} is missing`);
  }
  for (const key of Object.keys(object)) {
    if (!(key in def.properties)) out.push(`${where}.${key} is not in the schema`);
  }
  for (const [key, node] of Object.entries(def.properties)) {
    if (!(key in object)) continue;
    const v = object[key];
    if (v === undefined) {
      out.push(`${where}.${key} is undefined`);
    } else if (v === null) {
      if (!nullable(node)) out.push(`${where}.${key} is null, which the schema does not admit`);
    } else if (Array.isArray(v)) {
      const element = node.items;
      const inner = element && referenced(element);
      if (!element || !inner || !walkable(defs, inner)) continue;
      v.forEach((item, i) => {
        const at = `${where}.${key}[${i}]`;
        if (item === undefined) {
          out.push(`${at} is undefined`);
        } else if (item === null) {
          if (!nullable(element)) out.push(`${at} is null, which the schema does not admit`);
        } else if (typeof item === "object") {
          out.push(...disagreements(item, inner, defs, at));
        }
      });
    } else {
      const inner = referenced(node);
      if (inner && typeof v === "object" && walkable(defs, inner)) {
        out.push(...disagreements(v, inner, defs, `${where}.${key}`));
      }
    }
  }
  return out;
}

/** Whether a definition is an object or a tagged union of them, which
 *  `disagreements` can hold a value to. */
function walkable(defs: Record<string, SchemaNode>, name: string): boolean {
  return Boolean(defs[name]?.properties || defs[name]?.oneOf);
}

suite("API wire contract (#574)", () => {
  // The #288 message: GRIB1 spectral coefficients on a declared grid nothing
  // places a point on, with none of GRIB2's identification.
  test("a spectral GRIB1 message's absent fields are present nulls", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const info: MessageInfo = native.open(
      fixture("spectral_simple_t63.grib1"),
    ).message(0);

    // GRIB2's identification is not "null" on a GRIB1 message, it is not
    // there: the edition tag says which fields exist (#773).
    assert.strictEqual(info.identification.edition, "grib1");
    assert.deepStrictEqual(
      Object.keys(info.identification).sort(),
      ["edition", "p1Octet"],
      "only GRIB1's own identification",
    );
    for (const key of ["discipline", "productionStatus", "dataType", "p1Octet", "edition"]) {
      assert.ok(!(key in info), `${key} is not a top-level key (#773)`);
    }
    assert.ok(info.grid, "the message declares a grid");
    for (const key of ["boundsLonlat", "corners", "proj4", "affine"] as const) {
      assert.ok(key in info.grid, `grid.${key} is present`);
      assert.strictEqual(info.grid[key], null, `grid.${key} is null, not undefined`);
    }
    // The strict guard the declaration asks for is the right one: this is the
    // shape of the check that failed open in #288.
    assert.ok(info.grid.boundsLonlat === null, "=== null sees the absence");
  });

  // The other half of #773: a GRIB2 message carries its own identification,
  // and GRIB1's P1 octet is not on it.
  test("a GRIB2 message's identification is tagged grib2 and holds only its own fields", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const info: MessageInfo = native.open(
      fixture("regular_latlon_surface.grib2"),
    ).message(0);
    const id = info.identification;
    assert.strictEqual(id.edition, "grib2");
    assert.deepStrictEqual(Object.keys(id).sort(), [
      "dataType",
      "discipline",
      "edition",
      "productionStatus",
    ]);
    if (id.edition === "grib2") {
      assert.strictEqual(typeof id.discipline, "string");
      assert.ok(id.discipline.length > 0, "the discipline is named");
    }
    assert.strictEqual(typeof info.referenceTime, "string");
    assert.strictEqual(typeof info.totalLengthBytes, "number");
  });

  test("every message agrees with the schema its declaration came from", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const defs = schemaDefs();
    const messages: [string, MessageInfo][] = [
      [
        "spectral_simple_t63.grib1",
        native.open(fixture("spectral_simple_t63.grib1")).message(0),
      ],
      [
        "cmc_wind_300_2010052400_p012.grib",
        native.open(fixture("cmc_wind_300_2010052400_p012.grib")).message(0),
      ],
      [
        "regular_latlon_surface.grib2",
        native.open(fixture("regular_latlon_surface.grib2")).message(0),
      ],
      [
        "healpix_n4_ring.grib2",
        native.open(fixture("healpix_n4_ring.grib2")).message(0),
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

  // A key added after its type shipped is optional in the declaration only so
  // an older object a host keeps and hands back (`MessageInfo` to wasm
  // `verifyMessage`) still type-checks. What this version returns carries it
  // like any other key (#1577), so the check above holds it as required, and
  // an object without it is a disagreement.
  test("a returned message carries every key added after its type shipped", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const defs = schemaDefs();
    const message = defs.MessageInfo;
    assert.ok(message?.properties, "the schema defines MessageInfo as an object");
    // The keys a returned message must carry beyond what the schema requires.
    const required = message.required ?? [];
    const added = returnedKeys(message).filter((key) => !required.includes(key));
    // A new `x-fieldglass-added-in` key lands here and in the Rust pin
    // (`api_rules::a_key_added_after_its_type_shipped_is_optional_and_nullable`).
    assert.deepStrictEqual(added, ["offsetHours", "originatingCentreCode"], "the keys MessageInfo gained after 0.6.0");

    for (const name of ["spectral_simple_t63.grib1", "regular_latlon_surface.grib2"]) {
      const info = native.open(fixture(name)).message(0) as unknown as Record<string, unknown>;
      // Both editions state the center's number (GRIB1 PDS octet 5, GRIB2 §1
      // octets 6-7), so the key carries a value here, not just a `null`.
      assert.strictEqual(typeof info.originatingCentreCode, "number", `${name}: the center is stated`);
      // Both are forecasts from the reference time, so each has an offset.
      assert.strictEqual(typeof info.offsetHours, "number", `${name}: the offset is stated`);
      for (const key of added) {
        assert.ok(key in info, `${name}: ${key} is present`);
        assert.notStrictEqual(info[key], undefined, `${name}: ${key} is not undefined`);
        const older: Record<string, unknown> = { ...info };
        delete older[key];
        assert.deepStrictEqual(
          disagreements(older, "MessageInfo", defs, name),
          [`${name}.${key} is missing`],
          `${name}: a returned message without ${key} disagrees with the schema`,
        );
      }
    }
  });

  // The same for a key added deeper in: each step of a statistic's fingerprint
  // gained its increment in 0.6.2 (#1636). The extension's own fixtures have
  // no statistic, so this reads the umbrella crate's monthly mean of 6-hour
  // accumulations, two time ranges.
  test("a returned statistical step carries the increment added after it shipped", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const defs = schemaDefs();
    const step = defs.StatisticalStep;
    assert.ok(step?.properties, "the schema defines StatisticalStep as an object");
    const required = step.required ?? [];
    const added = Object.keys(step.properties).filter((key) => !required.includes(key)).sort();
    assert.deepStrictEqual(added, ["increment", "incrementType", "incrementUnit"], "the keys StatisticalStep gained after 0.6.1");

    const name = "monthly_mean_of_6h_accums.grib2";
    const bytes = fs.readFileSync(
      path.join(extensionPath(), "..", "crates", "fieldglass", "tests", "fixtures", name),
    );
    const info = native.open(bytes).message(0) as unknown as Record<string, unknown>;
    assert.deepStrictEqual(disagreements(info, "MessageInfo", defs, name), []);
    const fingerprint = info.fingerprint as { statistics: Record<string, unknown>[] };
    // Runs 6 hours apart (type 1) over continuous 6-hour accumulations (type 2).
    assert.deepStrictEqual(
      fingerprint.statistics.map((s) => [s.incrementType, s.incrementUnit, s.increment]),
      [[1, 1, 6], [2, 1, 0]],
    );
    for (const key of added) {
      const older = {
        ...info,
        fingerprint: {
          ...fingerprint,
          statistics: fingerprint.statistics.map((s, i) => {
            if (i !== 0) return s;
            const copy: Record<string, unknown> = { ...s };
            delete copy[key];
            return copy;
          }),
        },
      };
      assert.deepStrictEqual(
        disagreements(older, "MessageInfo", defs, name),
        [`${name}.fingerprint.statistics[0].${key} is missing`],
        `a returned step without ${key} disagrees with the schema`,
      );
    }
  });

  // The check reaches an array's elements too: a returned `VariableInfo`'s
  // `dims` are `DimensionInfo` objects, held to their definition one by one.
  // No element type of a NetCDF variable carries an added-in key, so one is
  // marked on a copy of the schema, and an element without it is a
  // disagreement (#1577).
  test("an array element of a returned object is held to its definition", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const defs = schemaDefs();
    const variable = native
      .open(fixture("netcdf4_dimscale.nc"))
      .variables()
      .find((v) => v.dims.length > 0);
    assert.ok(variable, "the fixture has a variable with dimensions");
    const where = `VariableInfo ${variable.name}`;
    assert.deepStrictEqual(disagreements(variable, "VariableInfo", defs, where), []);

    // A dimension with a key its definition does not name is found inside the
    // array, which shows the elements are walked at all.
    const extra = {
      ...variable,
      dims: variable.dims.map((d, i) => (i === 0 ? { ...d, stray: 1 } : d)),
    };
    assert.deepStrictEqual(disagreements(extra, "VariableInfo", defs, where), [
      `${where}.dims[0].stray is not in the schema`,
    ]);

    // Mark `name` added-in on a copy, out of `required` as `api_rules` writes
    // it: an element without it is missing it.
    const marked: Record<string, SchemaNode> = JSON.parse(JSON.stringify(defs));
    const dimension = marked.DimensionInfo;
    assert.ok(dimension?.properties?.name, "DimensionInfo has a name");
    dimension.properties.name["x-fieldglass-added-in"] = "0.6.1";
    dimension.required = (dimension.required ?? []).filter((k) => k !== "name");
    assert.deepStrictEqual(disagreements(variable, "VariableInfo", marked, where), []);
    const older = {
      ...variable,
      dims: variable.dims.map((d, i) => {
        if (i !== 0) return d;
        const copy: Record<string, unknown> = { ...d };
        delete copy.name;
        return copy;
      }),
    };
    assert.deepStrictEqual(disagreements(older, "VariableInfo", marked, where), [
      `${where}.dims[0].name is missing`,
    ]);
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
    const classic = native.open(fixture("netcdf_classic_dummy.nc"));
    nullUnderItsKey("DatasetMeta", classic.metadata(), ["note", "hdf5SuperblockVersion"]);

    // `lat_bnds` has no coordinate arrays of its own and no time axis.
    const nc = native.open(fixture("netcdf4_dimscale.nc"));
    const bounds = nc.variables().find((v) => v.name === "lat_bnds");
    assert.ok(bounds, "the fixture has lat_bnds");
    nullUnderItsKey("VariableInfo", bounds, ["detectedTimeDim"]);
    for (const key of ["detectedYDim", "detectedXDim"]) {
      assert.ok(key in bounds, `VariableInfo.${key} is present`);
    }
    const [y, x] = [bounds.dims.length - 2, bounds.dims.length - 1];

    // AxisValues and LineResult: an axis with no coordinate array.
    nullUnderItsKey("AxisValues", nc.axisValues(bounds.index, x), ["coordinates", "calendar"]);
    nullUnderItsKey(
      "LineResult",
      nc.decodeLine({ variable: bounds.index, alongDim: x, sliceIndices: bounds.dims.map(() => 0) }),
      ["coordinates", "coordinateUnits", "coordinateCalendar"],
    );

    // RenderedGrid: the source view has no geographic extent to echo.
    const options = {
      projection: "source" as const,
      resampling: "nearest" as const,
      flipY: false,
    };
    const zero = bounds.dims.map(() => 0);
    nullUnderItsKey("RenderedGrid", nc.renderGrid({ variable: bounds.index, yDim: y, xDim: x, sliceIndices: zero }, options), [
      "usedLatMin",
      "usedLatMax",
      "usedLonMin",
      "usedLonMax",
    ]);

    // ProbeResult: a cell nothing places has a value and no position.
    const probe = nc.probePixel({ variable: bounds.index, yDim: y, xDim: x, sliceIndices: zero }, options, 0, 0);
    assert.ok(probe, "the pixel is on the raster");
    nullUnderItsKey("ProbeResult", probe, ["lat", "lon"]);
    assert.strictEqual(typeof probe.value, "number", "and it still reads the value");

    // SliceGrid has no optional field; its answers for the same slice are the
    // ones the panel gates its projection picker on.
    assert.deepStrictEqual(nc.sliceGrid({ variable: bounds.index, yDim: y, xDim: x }), {
      label: "source",
      placement: "unplaceable",
      reprojectable: false,
      supports: { contours: false, vectors: false, longCsv: false, zonalMean: false, bilinear: false },
    });
  });

  test("an index past the file throws rather than returning an empty object", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.open(fixture("regular_latlon_surface.grib2"));
    assert.throws(() => handle.message(99));
  });
});
