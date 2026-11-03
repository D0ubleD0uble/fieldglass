// What the reader left out, and why a time is not dated, said on screen (#1604).
//
// A NetCDF-4 variable kept without an attribute it could not read (#1569)
// was drawn as usual, and nothing said the attribute had been there. A
// global attribute left out was dropped without a word. A time whose
// `calendar` or `units` is a list of several strings read as a number
// (#1570), which looks like a fault. Each now says so: the metadata page
// names the attribute and the reason beside the variable and under the
// global attributes, and the render panel names it beside the variable
// picker, and why the time is a number beside its stepper.
//
// `netcdf4_damaged_text.nc` is `netcdf4_string_attributes.nc` with the
// global-heap IDs of `temperature`'s `long_name` and of the global `title`
// damaged (NOTICE.md); `netcdf4_string_lists.nc` has `tas`, whose time has
// two calendars, and `pr`, whose step has two units.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { renderHtml } from "../../provider";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const DAMAGED = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_damaged_text.nc",
);
const LISTS = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_string_lists.nc",
);
const WHOLE = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_string_attributes.nc",
);

const LONG_NAME_NOTE = "Attribute `long_name` not read: the file's record of the text is damaged";
const TITLE_NOTE = "Attribute `title` not read: the file's record of the text is damaged";

/** The page's text as a reader sees it: tags dropped, entities read,
 *  whitespace collapsed. */
function shownText(html: string): string {
  return html
    .replace(/<script[\s\S]*?<\/script>/g, "")
    .replace(/<style[\s\S]*?<\/style>/g, "")
    .replace(/<[^>]+>/g, " ")
    .replace(/&#39;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ");
}

/** The text of the row under `<h2>{title}</h2>` whose first cell is `name`. */
function rowText(html: string, title: string, name: string): string {
  const at = html.indexOf(`<h2>${title}</h2>`);
  assert.ok(at >= 0, `the page has a ${title} table`);
  const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html.slice(at));
  assert.ok(body, `the ${title} table has a body`);
  const row = [...body[1].matchAll(/<tr>([\s\S]*?)<\/tr>/g)]
    .map((r) => r[1])
    .find((r) => r.trimStart().startsWith(`<td>${name}</td>`));
  assert.ok(row, `the ${title} table lists ${name}`);
  return shownText(row);
}

/** The Global attributes section: from its heading to the next one. */
function globalsSection(html: string): string {
  const at = html.indexOf("<h2>Global attributes</h2>");
  assert.ok(at >= 0, "the page has a Global attributes section");
  const end = html.indexOf("<h2>", at + 1);
  return shownText(html.slice(at, end < 0 ? undefined : end));
}

suite("What was left out, and why a time is not dated (#1604)", function () {
  this.timeout(120_000);

  test("the metadata page names the attribute a variable lacks, and the global left out", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(DAMAGED())).metadata();
    const html = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", DAMAGED(), undefined, meta, undefined, false, undefined,
    );
    assert.ok(rowText(html, "Variables", "temperature").includes(LONG_NAME_NOTE),
      rowText(html, "Variables", "temperature"));
    assert.ok(!rowText(html, "Variables", "time").includes("not read"), "time lost nothing");
    const globals = globalsSection(html);
    assert.ok(globals.includes(TITLE_NOTE), globals);
    assert.ok(globals.includes("history"), "the globals that were read are listed as before");

    // A file that lost nothing says nothing.
    const whole = native.open(fs.readFileSync(WHOLE())).metadata();
    const plain = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", WHOLE(), undefined, whole, undefined, false, undefined,
    );
    assert.ok(!plain.includes("attrs-not-read"), "no note on an undamaged file");
  });

  test("the render panel names it beside the variable", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.open(fs.readFileSync(DAMAGED())).variables()
      .find((v) => v.name === "temperature");
    assert.ok(temperature, "temperature is kept");
    const p = await provider();
    const editor = await openEditor(p, DAMAGED());
    assert.ok(shownText(editor.html()).includes(LONG_NAME_NOTE), "the editor's own page has it");
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "temperature drawn");
      const state = await panel.drive([]);
      assert.ok(state.text.includes(LONG_NAME_NOTE), state.text);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  for (const [name, why] of [
    ["tas", "(not shown as a date: its calendar holds several values)"],
    ["pr", "(not shown as a date: its units hold several values)"],
  ] as const) {
    test(`the stepper says why ${name}'s time is a number`, async () => {
      const native = loadNative();
      assert.ok(native, "native binding required");
      const variable = native.open(fs.readFileSync(LISTS())).variables().find((v) => v.name === name);
      assert.ok(variable, `the fixture has ${name}`);
      const p = await provider();
      const editor = await openEditor(p, LISTS());
      const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variable.index }));
      try {
        await panel.waitSent(isType("gridReady"), 0, `${name} drawn`);
        await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
        const state = await panel.drive([]);
        const steppers = state.steppers;
        assert.ok(steppers, "the panel has steppers");
        const time = steppers.main.find((s) => s.dim === 0);
        assert.ok(time, "time has a stepper");
        assert.ok(time.text.includes(why), time.text);
        // Only the time says it: the variable lost no attribute.
        assert.ok(!state.text.includes("not read"), state.text);
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  }
});
