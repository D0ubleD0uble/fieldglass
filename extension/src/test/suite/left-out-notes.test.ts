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
import { ATTR_FULL_LIMIT, renderHtml } from "../../provider";
import { noteLine } from "../../render-panel";
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

/** The raw inner HTML of every note line on the page, in page order. */
function noteLines(html: string): string[] {
  return [...html.matchAll(/<div class="render-legend attrs-not-read">([\s\S]*?)<\/div>/g)].map((m) => m[1]);
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

  test("each note is one plain line, whole, with no parse error and nothing to open", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(DAMAGED())).metadata();
    const html = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", DAMAGED(), undefined, meta, undefined, false, undefined,
    );
    // Escaped text and no markup: no <details>, no "Show more", no cut. The
    // reader is told the text is damaged, not where in the bytes.
    assert.deepStrictEqual(noteLines(html).map(shownText), [TITLE_NOTE, LONG_NAME_NOTE]);
    for (const line of noteLines(html)) assert.ok(!line.includes("<"), line);
  });

  test("a note naming a 64 KiB attribute is cut and keeps its reason", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(DAMAGED())).metadata();
    const long = "attribute `" + "n".repeat(65_536) + "<b>` not read: the file's record of the text is damaged";
    const temperature = meta.variables.findIndex((v) => v.name === "temperature");
    assert.ok(temperature >= 0, "temperature is listed");
    const variables = meta.variables.map((v, i) => (i === temperature ? { ...v, attributesNotRead: long } : v));
    const html = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", DAMAGED(), undefined, { ...meta, globalAttributesNotRead: long, variables }, undefined, false, undefined,
    );
    const lines = noteLines(html);
    assert.strictEqual(lines.length, 2, "the global note and temperature's");
    for (const line of lines) {
      assert.ok(!line.includes("<"), "escaped, with no markup to open");
      assert.ok(line.includes("&lt;b&gt;"), "the name's markup is shown as text");
      const shown = shownText(line).trim();
      assert.ok(shown.length <= ATTR_FULL_LIMIT, `${shown.length} code units`);
      assert.ok(shown.startsWith("Attribute `nnn"), shown.slice(0, 40));
      assert.ok(shown.includes(" … "), "the cut is marked");
      assert.ok(shown.endsWith("not read: the file's record of the text is damaged"), "the reason stays");
    }
  });

  test("noteLine caps a note at the page's limit, never splitting a character", () => {
    // The panel sets its variable note with this function, so this is its cap.
    assert.strictEqual(noteLine(undefined), "");
    assert.strictEqual(noteLine("  attribute `a` not read: x "), "Attribute `a` not read: x");
    const at = noteLine("a".repeat(ATTR_FULL_LIMIT));
    assert.strictEqual(at, "A" + "a".repeat(ATTR_FULL_LIMIT - 1), "a note at the limit is whole");
    const emoji = "\u{1F600}";
    // Both parities at both cuts: the head's end and the tail's start each
    // land once on a pair's second half.
    for (const [pad, reason] of [[0, "reason"], [1, "reason"], [0, "reasons"], [1, "reasons"]] as const) {
      const cut = noteLine("a".repeat(pad) + emoji.repeat(ATTR_FULL_LIMIT) + reason);
      assert.ok(cut.length <= ATTR_FULL_LIMIT, `${pad}: ${cut.length}`);
      assert.ok(cut.endsWith(emoji + reason), `${pad}: the end, with the reason, stays`);
      assert.ok(cut.includes(" … "), "the cut is marked");
      for (let i = 0; i < cut.length; i++) {
        const c = cut.charCodeAt(i);
        if (c >= 0xd800 && c <= 0xdbff) {
          const next = cut.charCodeAt(i + 1);
          assert.ok(next >= 0xdc00 && next <= 0xdfff, `${pad}: a high half at ${i} keeps its low half`);
          i += 1;
        } else {
          assert.ok(!(c >= 0xdc00 && c <= 0xdfff), `${pad}: a low half at ${i} without its high half`);
        }
      }
    }
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
