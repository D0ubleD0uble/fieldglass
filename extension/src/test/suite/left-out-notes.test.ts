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
import { ATTR_FULL_LIMIT, axisNotDated, lineNotDated, renderHtml } from "../../provider";
import { coordinateTicks, datedAxis, heldSliceParts, noteLine, railTicks } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const DAMAGED = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_damaged_text.nc",
);
const LISTS = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_string_lists.nc",
);
const NUMERIC_CALENDAR = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf_numeric_calendar.nc",
);
const UNREAD_AXES = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_unread_axes.nc",
);
const WHOLE = () => path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_string_attributes.nc",
);

const LONG_NAME_NOTE = "Attribute \"long_name\" not read: the file's record of the text is damaged";
const TITLE_NOTE = "Attribute \"title\" not read: the file's record of the text is damaged";

/** The text of some of the page's markup as a reader sees it: each tag a
 *  space, entities read, whitespace collapsed. It is given only a table row,
 *  a section between two headings, or a note line, never a script or a
 *  style, so a tag is everything from a `<` to the next `>`. */
function shownText(html: string): string {
  let text = "";
  let inTag = false;
  for (const c of html) {
    if (c === "<") {
      inTag = true;
      text += " ";
    } else if (c === ">" && inTag) {
      inTag = false;
    } else if (!inTag) {
      text += c;
    }
  }
  return text
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
  assert.ok(end > at, "a section follows the global attributes");
  return shownText(html.slice(at, end));
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
    const long = "attribute \"" + "n".repeat(65_536) + "<b>\" not read: the file's record of the text is damaged";
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
      assert.ok(shown.startsWith("Attribute \"nnn"), shown.slice(0, 40));
      assert.ok(shown.includes(" … "), "the cut is marked");
      assert.ok(shown.endsWith("not read: the file's record of the text is damaged"), "the reason stays");
    }
  });

  test("noteLine caps a note at the page's limit, never splitting a character", () => {
    // The panel sets its variable note with this function, so this is its cap.
    assert.strictEqual(noteLine(undefined), "");
    assert.strictEqual(noteLine("  attribute \"a\" not read: x "), "Attribute \"a\" not read: x");
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
    const row = rowText(editor.html(), "Variables", "temperature");
    assert.ok(row.includes(LONG_NAME_NOTE), `the editor's own page has it: ${row}`);
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

  test("an axis the engine says is not dated is labeled with numbers, whatever its units say (#1687)", () => {
    // Units and a calendar that date it, as a calendar read in part keeps
    // its first element, and the reason not to.
    const axis = {
      coordinates: [0, 31, 59, 90, 120, 151],
      units: "days since 2000-01-01",
      calendar: "standard",
      notDated: "not shown as a date: its calendar was read only in part",
    };
    assert.strictEqual(datedAxis({ ...axis, notDated: null }), true, "the control: without the reason it is dated");
    assert.strictEqual(datedAxis(axis), false);
    const numbers = (ticks: { text: string }[] | null) => (ticks || []).map((t) => t.text);
    const date = /\d{4}-\d\d-\d\d/;
    assert.ok(numbers(coordinateTicks(axis, 5)).every((t) => !date.test(t)), JSON.stringify(coordinateTicks(axis, 5)));
    assert.ok(numbers(railTicks(axis, axis.coordinates.length, 5)).every((t) => !date.test(t)));
    const held = heldSliceParts(["time", "y", "x"], { yDim: 1, xDim: 2, sliceIndices: [2, 0, 0] }, () => axis);
    assert.deepStrictEqual(held, [{ name: "time", index: 2, coordinate: "59 days since 2000-01-01" }]);
  });

  // `netcdf_numeric_calendar.nc` (#1645): `t`'s time has `calendar = 360`,
  // `u`'s has `calendar = 1, 2`. A number names no calendar for certain, so
  // neither is dated, on the standard calendar or any other, and the stepper
  // says why.
  for (const name of ["t", "u"]) {
    test(`a calendar of numbers is no date: ${name}'s stepper is a number and says why`, async () => {
      const native = loadNative();
      assert.ok(native, "native binding required");
      const handle = native.open(fs.readFileSync(NUMERIC_CALENDAR()));
      const variable = handle.variables().find((v) => v.name === name);
      assert.ok(variable, `the fixture has ${name}`);
      // The engine's reading: no calendar and no notDated, as in 0.6.1,
      // and the note in calendarNotDated.
      const axis = handle.axisValues(variable.index, 0);
      assert.strictEqual(axis.calendar, null);
      assert.strictEqual(axis.notDated, null);
      assert.strictEqual(axis.calendarNotDated, "not shown as a date: its calendar is not text");
      assert.strictEqual(datedAxis(axis), true, "the control: read without the note it would be dated");
      assert.strictEqual(datedAxis(axisNotDated(axis)), false);
      const line = handle.decodeLine({ variable: variable.index, alongDim: 0, sliceIndices: [0, 0, 0] });
      assert.strictEqual(line.coordinateCalendarNotDated, axis.calendarNotDated);
      const read = lineNotDated(line);
      assert.strictEqual(
        datedAxis({ coordinates: read.coordinates, units: read.coordinateUnits, calendar: read.coordinateCalendar, notDated: read.coordinateNotDated }),
        false,
        "the line plot labels it with numbers",
      );

      const p = await provider();
      const editor = await openEditor(p, NUMERIC_CALENDAR());
      const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variable.index }));
      try {
        await panel.waitSent(isType("gridReady"), 0, `${name} drawn`);
        await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
        const state = await panel.drive([]);
        const steppers = state.steppers;
        assert.ok(steppers, "the panel has steppers");
        const time = steppers.main.find((s) => s.dim === 0);
        assert.ok(time, "time has a stepper");
        assert.ok(time.text.includes("(not shown as a date: its calendar is not text)"), time.text);
        // The number and its units, where the standard calendar reads `2000-01-01`.
        assert.ok(time.text.includes("0 days since 2000-01-01"), `no date: ${time.text}`);
        assert.ok(!/\d{4}-\d\d-\d\d/.test(time.text.replace("days since 2000-01-01", "")), time.text);
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  }

  // `netcdf4_unread_axes.nc` (#1687): `tas`'s time has units read only in
  // part, kept as their first element, `days since 2000-01-01`, which would
  // date it; and `y` and `x` lost their units and standard_name, so they are
  // read as latitude and longitude from their names, with values that place
  // nothing (#1719).
  test("a time whose units were read in part is a number, and the stepper says why", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const tas = native.open(fs.readFileSync(UNREAD_AXES())).variables().find((v) => v.name === "tas");
    assert.ok(tas, "the fixture has tas");
    const p = await provider();
    const editor = await openEditor(p, UNREAD_AXES());
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: tas.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "tas drawn");
      await panel.waitSent((m: Msg) => m && m.type === "axisResult" && m.dim === 0, 0, "the time axis's values");
      const state = await panel.drive([]);
      const steppers = state.steppers;
      assert.ok(steppers, "the panel has steppers");
      const time = steppers.main.find((s) => s.dim === 0);
      assert.ok(time, "time has a stepper");
      assert.ok(time.text.includes("(not shown as a date: its units were read only in part)"), time.text);
      // The number and its units, where a dated axis reads `2000-01-01`.
      assert.ok(time.text.includes("0 days since 2000-01-01"), `no date: ${time.text}`);
      assert.ok(
        state.text.includes("\"y\" is read as latitude from its name alone"),
        `the variable says how it is placed: ${state.text}`,
      );
      // `y` and `x` run from -1000 to 1000 and -2000 to 2000, which no
      // latitude or longitude can (#1719): the note says so, and the field
      // is not placed, so no coastlines or graticule are offered over it.
      assert.ok(
        state.text.includes("its values run from -2000 to 2000, more than twice around the globe"),
        `the note says why the field is not placed: ${state.text}`,
      );
      assert.strictEqual(state.controlDisplay.overlay, "none", "no overlays over a field nothing places");
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
