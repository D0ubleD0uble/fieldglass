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

  test("an index past the file throws rather than returning an empty object", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.Grib2Handle.fromBytes(fixture("regular_latlon_surface.grib2"));
    assert.throws(() => handle.message(99));
  });
});
