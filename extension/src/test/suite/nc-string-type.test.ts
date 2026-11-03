// NetCDF-4 string attributes, through the built addon and the file page (#1570).
//
// An attribute of netCDF-4's `NC_STRING` type showed `char` in the Global
// attributes table's Type column, the one text type the classic format has.
// netCDF-C reports it as its own type, `string`, as `ncdump` prints it. And a
// `calendar` or `units` stored as a list of several strings is no one calendar
// or unit: the time axis is handed every element, quoted as `ncdump` prints
// them, and the panel shows the number rather than dating it on a calendar the
// file does not name for certain.
//
// The fixture is `netcdf4_string_lists.nc`, written by netCDF4-python; its
// oracle records each attribute's type as netCDF-C's `nc_inq_atttype` reports
// it.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { renderHtml } from "../../provider";
import { cfTimeOrigin, formatAxisValue } from "../../render-panel";
import { extensionPath } from "./real-panel";

const FIXTURE = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_string_lists.nc",
);

/** The cells of every row of the table under `<h2>{title}</h2>`. */
function tableRows(html: string, title: string): string[][] {
  const at = html.indexOf(`<h2>${title}</h2>`);
  assert.ok(at >= 0, `the page has a ${title} table`);
  const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html.slice(at));
  assert.ok(body, `the ${title} table has a body`);
  return [...body[1].matchAll(/<tr>([\s\S]*?)<\/tr>/g)].map((row) =>
    [...row[1].matchAll(/<td[^>]*>([\s\S]*?)<\/td>/g)].map((cell) => cell[1].trim()),
  );
}

suite("NetCDF-4 string attributes (#1570)", function () {
  this.timeout(60_000);

  test("an NC_STRING attribute's type is `string`, beside `char` and `int`", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(FIXTURE())).metadata();
    const types = Object.fromEntries(meta.globalAttributes.map((a) => [a.name, a.ncType]));
    assert.deepStrictEqual(types, { source: "string", title: "char", version: "int" });
    const time = meta.variables.find((v) => v.name === "time");
    assert.ok(time, "the fixture has a time variable");

    const html = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", FIXTURE(), undefined, meta, undefined, false, undefined,
    );
    const shown = Object.fromEntries(tableRows(html, "Global attributes").map((cells) => [cells[0], cells[1]]));
    assert.deepStrictEqual(shown, { source: "string", title: "char", version: "int" },
      "the Type column shows netCDF-C's type");
  });

  test("a calendar or units of several strings is not dated", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.open(fs.readFileSync(FIXTURE()));
    const variables = handle.variables();
    const tas = variables.find((v) => v.name === "tas");
    const pr = variables.find((v) => v.name === "pr");
    assert.ok(tas && pr, "the fixture has tas and pr");

    // Two calendars: the time keeps its one unit, and is not dated on either.
    const time = handle.axisValues(tas.index, 0);
    assert.strictEqual(time.units, "days since 2000-01-01");
    assert.strictEqual(time.calendar, '"noleap", "360_day"');
    assert.strictEqual(cfTimeOrigin(time.units, time.calendar), null);
    assert.strictEqual(formatAxisValue(31, time.units, 0, time.calendar), "31");
    // The same time on one of them alone is not dated either; on `standard`
    // it is, which is what the list keeps it from.
    assert.strictEqual(formatAxisValue(31, time.units, 0, "standard"), "2000-02-01");

    // Two time units, on the standard calendar: not dated either.
    const step = handle.axisValues(pr.index, 0);
    assert.strictEqual(step.units, '"hours since 2000-01-01", "days since 2000-01-01"');
    assert.strictEqual(step.calendar, "standard");
    assert.strictEqual(cfTimeOrigin(step.units, step.calendar), null);
    assert.strictEqual(formatAxisValue(6, step.units, 0, step.calendar), "6");
  });
});
