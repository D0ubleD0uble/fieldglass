// Under Contours "only" the panel and the exported PNG show a contour key in
// the color bar's place (#1463): the interval in use and the color range the
// lines are spaced over (the data range unless a manual range is set), with
// units. The color field is not drawn then, so a gradient would suggest
// color carries meaning. Unticking "only" (or Contours) brings the bar back.
//
// Through the real panel over real fixture bytes: a GRIB message and a NetCDF
// variable (the two panels share one page), read back from the page and from
// the PNG the page makes.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { type GridTarget, loadNative } from "../../native";
import { composeContourKey } from "../../render-panel";
import { waitForRead } from "./themed-panel";
import {
  classicNetcdf,
  decodePng,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const GRIB = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const MARGIN = 14;
const GAP = 16;
const BAR_WIDTH = 18;

type Image = ReturnType<typeof decodePng>;

/** Click Export PNG with the save dialog answered "cancel": the PNG the page
 *  made, and what the page says it wrote in the bar's place. */
async function exportedPng(panel: RealPanel): Promise<{ png: Image; key: string; lines: string[] }> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    const sentFrom = panel.sent.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    await panel.waitSent(isType("exportPngDone"), sentFrom, "the PNG export's outcome");
    const png = decodePng(Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
    const state = await panel.drive([]);
    return { png, key: state.exportedContourKey, lines: state.exportedContourKeyLines };
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

/** Whether any pixel of the PNG in the box is not white, and whether any is
 *  colored (red, green and blue not within 8 of each other): a gradient is,
 *  the key's dark text and the page's white are not. */
function inkIn(img: Image, x0: number, y0: number, x1: number, y1: number): { ink: boolean; color: boolean } {
  let ink = false;
  let color = false;
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const i = (y * img.width + x) * 4;
      const [r, g, b] = [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]];
      if (r < 250 || g < 250 || b < 250) ink = true;
      if (Math.max(r, g, b) - Math.min(r, g, b) > 8) color = true;
    }
  }
  return { ink, color };
}

/** The column right of the map in the PNG: where the bar is drawn, or where
 *  the key is written in its place. */
function barColumn(png: Image, drawn: Msg): { ink: boolean; color: boolean } {
  const top = png.height - MARGIN - drawn.height;
  const x = MARGIN + drawn.width + GAP;
  return inkIn(png, x, top, x + BAR_WIDTH, top + drawn.height);
}

/** Open `file` and draw its first message or the variable named `variable`. */
async function openPanel(
  file: string,
  variable?: string,
): Promise<{ panel: RealPanel; drawn: Msg; close: () => void }> {
  const p = await provider();
  const editor = await openEditor(p, file);
  let open: () => void = () => editor.send({ type: "decodeGrid", messageIndex: 0 });
  if (variable) {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const found = native.open(fs.readFileSync(file)).variables().find((v) => v.name === variable);
    assert.ok(found, `the file has ${variable}`);
    open = () => editor.send({ type: "renderVariable", variableIndex: found.index });
  }
  const panel = await openReal(p, open);
  const close = () => {
    panel.dispose();
    editor.close();
  };
  try {
    return { panel, drawn: await panel.waitSent(isType("gridReady"), 0, "the first render"), close };
  } catch (err) {
    close();
    throw err;
  }
}

/** Tick Contours and wait for the isolines. */
async function tickContours(panel: RealPanel): Promise<void> {
  const from = panel.sent.length;
  await panel.drive([{ id: "overlay-contours", checked: true }]);
  await panel.waitSent(isType("contourReady"), from, "the contours");
}

/** The range part of a key, `210.4 … 309.7 K` as the color bar's ends write
 *  it, and its interval. */
function parts(key: string): { interval: number; units: string; range: string } {
  const prefix = "Contours every ";
  assert.ok(key.startsWith(prefix), `"${key}" reads Contours every <interval> <units> · <range> <units>`);
  const [head, range, ...extra] = key.slice(prefix.length).split(" · ");
  assert.ok(range && extra.length === 0, `one " · " in "${key}"`);
  const [interval, ...units] = head.split(" ");
  assert.ok(Number(interval) > 0, `an interval in "${key}"`);
  return { interval: Number(interval), units: units.join(" "), range };
}

function assertKeyFor(key: string, drawn: Msg, units: string): void {
  const { interval, range, units: shown } = parts(key);
  assert.strictEqual(shown, units, `units after the interval: ${key}`);
  assert.ok(interval > 0, `a positive interval: ${key}`);
  assert.ok(range.endsWith(` ${units}`), `units after the range: ${key}`);
  const ends = range.slice(0, range.length - units.length - 1).split(" … ");
  assert.strictEqual(ends.length, 2, `a range with two ends: ${key}`);
  assert.strictEqual(
    Number(ends[0].replace("\u2212", "-")).toPrecision(4),
    Number(drawn.usedMin).toPrecision(4),
    `the range starts at the used minimum ${drawn.usedMin}: ${key}`,
  );
  assert.strictEqual(
    Number(ends[1].replace("\u2212", "-")).toPrecision(4),
    Number(drawn.usedMax).toPrecision(4),
    `the range ends at the used maximum ${drawn.usedMax}: ${key}`,
  );
}

/** The levels the addon draws for the key's interval are the ones it draws
 *  with no interval: the key says the spacing in use, not another one. */
function assertIntervalIsTheAutomaticOne(file: string, target: GridTarget, interval: number, drawn: Msg): void {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const handle = native.open(fs.readFileSync(file));
  const options = {
    projection: "equirectangular" as const,
    resampling: "nearest" as const,
    flipY: false,
    width: drawn.width,
    height: drawn.height,
  };
  const auto = handle.projectContours(target, options, null);
  const keyed = handle.projectContours(target, options, { interval });
  assert.ok(auto.segLengths.length > 0, "the fixture has isolines at the automatic spacing");
  assert.deepStrictEqual(Array.from(keyed.segLengths), Array.from(auto.segLengths), "the same runs");
  assert.deepStrictEqual(Array.from(keyed.xy), Array.from(auto.xy), "at the same places");
}

/** The key breaks only between its two parts, never inside the range, at a
 *  wide panel and a narrow one (#1510): the browser's own lines are the key's
 *  two parts, and so are the lines the PNG records. */
async function assertBreaksBetweenParts(panel: RealPanel, key: string): Promise<void> {
  const want = key.split(" · ");
  for (const width of ["1400px", "420px"]) {
    await panel.drive([{ id: "body", width }]);
    const state = await panel.drive([]);
    assert.deepStrictEqual(state.contourKeyLines, want, `the key's lines at ${width}`);
  }
  await panel.drive([{ id: "body", width: "" }]);
}

/** The whole behavior on one panel; `expectedUnits` is the field's when the
 *  test knows them (a fixture it wrote), `null` for a committed file. */
async function checkPanel(
  file: string,
  expectedUnits: string | null,
  target: GridTarget,
  variable?: string,
): Promise<void> {
  const { panel, drawn, close } = await openPanel(file, variable);
  try {
    // The units the page draws the field with: the file's own.
    const units = String(drawn.parameterUnits);
    assert.ok(units, "the field has units");
    if (expectedUnits !== null) assert.strictEqual(units, expectedUnits);
    // Contours off: the color bar, no key, whatever "only" says.
    await panel.drive([{ id: "contours-only", checked: true }]);
    let state = await panel.drive([]);
    assert.strictEqual(state.colorBar, true, "the color bar with Contours off");
    assert.strictEqual(state.contourKey, null, "no key with Contours off");

    // Contours + only: the key replaces the bar.
    await tickContours(panel);
    state = await panel.drive([]);
    assert.strictEqual(state.colorBar, false, "no color bar under Contours + only");
    assert.ok(state.contourKey, "the key is on screen");
    assertKeyFor(state.contourKey, drawn, units);
    await assertBreaksBetweenParts(panel, state.contourKey);
    assertIntervalIsTheAutomaticOne(file, target, parts(state.contourKey).interval, drawn);

    // The PNG: the bar's column holds no gradient, the key is written, and it
    // says what the screen says.
    const only = await exportedPng(panel);
    assert.strictEqual(only.key, state.contourKey, "the PNG writes the key the screen shows");
    assert.deepStrictEqual(only.lines, state.contourKey.split(" \u00b7 "), "the PNG breaks only between the parts");
    const column = barColumn(only.png, drawn);
    assert.strictEqual(column.color, false, "no gradient in the bar's column of the PNG");
    assert.strictEqual(only.png.width > MARGIN * 2 + drawn.width + GAP + BAR_WIDTH, true, "room for the key beside the map");
    // The key's own text sits at the top of that column, in dark ink.
    const keyX = MARGIN + drawn.width + GAP;
    const top = only.png.height - MARGIN - drawn.height;
    assert.ok(inkIn(only.png, keyX, top, only.png.width - 1, top + 34).ink, "the key's text is in the PNG");

    // A typed interval is the one in use.
    await panel.drive([{ id: "contour-interval", value: 0.5 }]);
    state = await panel.drive([]);
    assert.ok(
      state.contourKey && state.contourKey.startsWith(`Contours every 0.5${units ? " " + units : ""} · `),
      `the typed interval: ${state.contourKey}`,
    );
    assert.strictEqual((await exportedPng(panel)).key, state.contourKey, "and the PNG says so too");
    await panel.drive([{ id: "contour-interval", value: "" }]);

    // Unticking "only" brings the bar back and removes the key, on screen and
    // in the PNG.
    await panel.drive([{ id: "contours-only", checked: false }]);
    state = await panel.drive([]);
    assert.strictEqual(state.colorBar, true, "the color bar is back");
    assert.strictEqual(state.contourKey, null, "the key is gone");
    const bar = await exportedPng(panel);
    assert.strictEqual(bar.key, "", "the PNG writes no key");
    assert.strictEqual(barColumn(bar.png, drawn).color, true, "the PNG has its gradient again");

    // Unticking Contours with "only" ticked does the same.
    await panel.drive([{ id: "contours-only", checked: true }]);
    state = await panel.drive([]);
    assert.strictEqual(state.contourKey !== null && !state.colorBar, true, "the key again");
    await panel.drive([{ id: "overlay-contours", checked: false }]);
    state = await panel.drive([]);
    assert.strictEqual(state.colorBar, true, "the bar with Contours off");
    assert.strictEqual(state.contourKey, null, "no key with Contours off");
  } finally {
    close();
  }
}

suite("The contour key replaces the color bar under Contours only (#1463)", function () {
  this.timeout(180_000);

  test("a GRIB panel: on screen and in the PNG, and the bar returns when only is unticked", async () => {
    await checkPanel(GRIB, null, 0);
  });

  test("a NetCDF panel: the same, with the variable's units", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1463-"));
    const file = path.join(dir, "key.nc");
    const lats = 12;
    const lons = 16;
    const values: number[] = [];
    for (let j = 0; j < lats; j++) for (let i = 0; i < lons; i++) values.push(210.4 + (99.3 * (j * lons + i)) / (lats * lons - 1));
    fs.writeFileSync(
      file,
      classicNetcdf(
        [["lat", lats], ["lon", lons]],
        [
          { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: Array.from({ length: lats }, (_, j) => -55 + 10 * j) },
          { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: Array.from({ length: lons }, (_, i) => 22.5 * i) },
          { name: "temp", dims: ["lat", "lon"], attrs: { units: "K" }, values },
        ],
      ),
    );
    try {
      const native = loadNative();
      assert.ok(native, "native binding required");
      const found = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "temp");
      assert.ok(found);
      await checkPanel(file, "K", { variable: found.index, yDim: 0, xDim: 1, sliceIndices: [0, 0] }, "temp");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a flat field under only says there are no contours, with or without an interval typed", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1463-flat-"));
    const file = path.join(dir, "flat.nc");
    fs.writeFileSync(
      file,
      classicNetcdf(
        [["lat", 3], ["lon", 4]],
        [
          { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
          { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 10, 20, 30] },
          { name: "temp", dims: ["lat", "lon"], attrs: { units: "K" }, values: new Array(12).fill(4) },
        ],
      ),
    );
    const { panel, close } = await openPanel(file, "temp");
    try {
      await panel.drive([{ id: "overlay-contours", checked: true }, { id: "contours-only", checked: true }]);
      for (const typed of ["", "2"]) {
        await panel.drive([{ id: "contour-interval", value: typed }]);
        const state = await panel.drive([]);
        assert.strictEqual(state.colorBar, false, `no bar over a blank map (interval "${typed}")`);
        assert.strictEqual(state.contourKey, "No contours · 4.000 K");
        await assertBreaksBetweenParts(panel, "No contours · 4.000 K");
        const flat = await exportedPng(panel);
        assert.strictEqual(flat.key, "No contours · 4.000 K", "and the PNG says so");
        assert.deepStrictEqual(flat.lines, ["No contours", "4.000 K"]);
      }
    } finally {
      close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a long range with a long CF unit string keeps the map and the page whole, and breaks inside the range only when it must", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1510-"));
    const file = path.join(dir, "long.nc");
    const units = "seconds since 1970-01-01 00:00:00";
    const lats = 4;
    const lons = 6;
    const values: number[] = [];
    for (let k = 0; k < lats * lons; k++) values.push(1700000000.61 + (0.03 * k) / (lats * lons - 1));
    fs.writeFileSync(
      file,
      classicNetcdf(
        [["lat", lats], ["lon", lons]],
        [
          { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-30, -10, 10, 30] },
          { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 20, 40, 60, 80, 100] },
          { name: "t", dims: ["lat", "lon"], attrs: { units }, values },
        ],
      ),
    );
    const { panel, close } = await openPanel(file, "t");
    try {
      await panel.drive([{ id: "overlay-contours", checked: true }, { id: "contours-only", checked: true }]);
      let state = await panel.drive([]);
      const sep = " \u00b7 ";
      assert.ok(state.contourKey, "the key is on screen");
      const keyText: string = state.contourKey;
      const range = keyText.split(sep)[1];
      assert.ok(range.length > 30, `a long range: ${range}`);
      for (const width of ["1400px", "420px", "300px"]) {
        await panel.drive([{ id: "body", width }]);
        state = await panel.drive([]);
        const lines = state.contourKeyLines ?? [];
        assert.ok(state.contourKeyRight <= state.renderAreaRight + 1, `the key stays inside the panel at ${width}`);
        assert.strictEqual(state.contourKeyOverflows, false, `no key line runs past the key at ${width}`);
        assert.ok(
          state.canvasWidth >= state.renderAreaWidth * 0.55,
          `the map keeps its share at ${width}: ${state.canvasWidth} of ${state.renderAreaWidth}`,
        );
        assert.strictEqual(
          lines.join("").replace(/ /g, ""),
          keyText.replace(sep, "").replace(/ /g, ""),
          `all the text at ${width}`,
        );
        if (width === "1400px") assert.deepStrictEqual(lines, keyText.split(sep), "two lines while the range fits");
        // At 300px the cap holds less than a part, so wrapping inside one is
        // expected there; only the fit and the map's share are asserted.
        if (width !== "300px") {
          assert.ok(lines.some((l) => /1700000000\.[0-9]+ \u2026/.test(l)), `the low end stays with its ellipsis at ${width}: ${lines.join(" | ")}`);
        }
      }
      await panel.drive([{ id: "body", width: "" }]);
    } finally {
      close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("the key's numbers keep close values apart, and a flat or unitless field reads plainly", () => {
    // Ends a hundredth apart past four figures read apart (#1459's rule).
    assert.strictEqual(composeContourKey(null, 1.0001, 1.0004, ""), "Contours every 0.00005 · 1.00010 … 1.00040");
    assert.strictEqual(composeContourKey(0.0001, 1.0001, 1.0002, "m"), "Contours every 0.0001 m · 1.0001 … 1.0002 m");
    // The interval's decimals are a floor for the ends.
    assert.strictEqual(
      composeContourKey(0.25, 1000.1, 1999.9, "Pa"),
      "Contours every 0.25 Pa · 1000.10 … 1999.90 Pa",
    );
    // A whole part past four figures is written in full.
    assert.strictEqual(
      composeContourKey(null, 1700000000, 1700086400, "s"),
      "Contours every 10000 s · 1700000000 … 1700086400 s",
    );
    // The automatic spacing: about eight levels, 1, 2, 5 or 10 times a power of ten.
    assert.strictEqual(composeContourKey(null, 210.4, 309.7, "K").split(" · ")[0], "Contours every 10 K");
    assert.strictEqual(composeContourKey(null, 0, 1, "1"), "Contours every 0.1 · 0.000 … 1.000");
    // A negative end has a true minus; a flat field has no contours.
    assert.strictEqual(composeContourKey(5, -12.5, 7.5, "K"), "Contours every 5 K · −12.50 … 7.500 K");
    // A flat field has no levels whatever is typed: one text, never the bar.
    assert.strictEqual(composeContourKey(2, 4, 4, "K"), "No contours · 4.000 K");
    assert.strictEqual(composeContourKey(null, 4, 4, "K"), "No contours · 4.000 K");
  });
});

// ---- The key says so when no contour is drawn (#1484) ----------------------

const CONTOUR_HEX = "#ff00ff";
const NO_CONTOURS = "No contours drawn";

/** Pixels of the PNG's map near the Contours color: 0 when it holds no
 *  isoline. */
function pngContourInk(png: Image, drawn: Msg): number {
  const want = [1, 3, 5].map((i) => parseInt(CONTOUR_HEX.slice(i, i + 2), 16));
  const top = png.height - MARGIN - drawn.height;
  let n = 0;
  for (let y = top; y < top + drawn.height; y++) {
    for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
      const i = (y * png.width + x) * 4;
      if (want.every((v, k) => Math.abs(png.rgba[i + k] - v) < 60)) n++;
    }
  }
  return n;
}

/** Contours + only in a color the map holds nowhere else, and the isolines
 *  drawn: the key describes them, on screen and in the PNG. */
async function contoursOnlyDrawn(panel: RealPanel, drawn: Msg): Promise<void> {
  const from = panel.sent.length;
  await panel.drive([
    { id: "overlay-coastlines", checked: false },
    { id: "color-contours", value: CONTOUR_HEX },
    { id: "overlay-contours", checked: true },
    { id: "contours-only", checked: true },
  ]);
  await panel.waitSent(isType("contourReady"), from, "the contours");
  const state = await panel.drive([]);
  assert.ok(state.contourKey && state.contourKey.startsWith("Contours every "), `the key: ${state.contourKey}`);
  assert.ok(state.contourInk > 0, "the isolines are on screen");
  const png = await exportedPng(panel);
  assert.strictEqual(png.key, state.contourKey, "the PNG writes the key the screen shows");
  assert.ok(pngContourInk(png.png, drawn) > 0, "the isolines are in the PNG");
}

suite("The contour key claims no lines that are not drawn (#1484)", function () {
  this.timeout(180_000);

  test("an interval past the level cap is refused, and the key and the PNG say no contours are drawn", async () => {
    const { panel, drawn, close } = await openPanel(GRIB);
    try {
      // About 40 K of range (270.5 … 311.1 K): 0.01 asks for about 4,060
      // levels. Rust used to draw the first 2001, over the bottom 20 K, under
      // a key naming the whole range.
      assert.ok(Number(drawn.usedMax) - Number(drawn.usedMin) > 30, `a wide range: ${drawn.usedMin} … ${drawn.usedMax}`);
      await contoursOnlyDrawn(panel, drawn);

      let from = panel.sent.length;
      await panel.drive([{ id: "contour-interval", value: 0.01 }]);
      await panel.waitSent(isType("contourError"), from, "the refused interval");
      let state = await panel.drive([]);
      const said = /^Contours: contour interval 0\.01 would draw (\d+) levels, more than the 2000 allowed; choose a wider interval$/.exec(
        state.contourStatus,
      );
      assert.ok(said, `the status line names the interval and the count: ${state.contourStatus}`);
      const expected = (Number(drawn.usedMax) - Number(drawn.usedMin)) / 0.01;
      assert.ok(Math.abs(Number(said[1]) - expected) <= 2, `about ${expected.toFixed(0)} levels: ${said[1]}`);
      assert.strictEqual(state.contourKey, NO_CONTOURS, "the key claims no contours");
      await assertBreaksBetweenParts(panel, NO_CONTOURS);
      assert.strictEqual(state.colorBar, false, "and the bar does not stand in for it");
      assert.strictEqual(state.contourInk, 0, "no isoline on screen");
      const png = await exportedPng(panel);
      assert.strictEqual(png.key, NO_CONTOURS, "the PNG says the same");
      assert.deepStrictEqual(png.lines, [NO_CONTOURS], "on one line");
      assert.strictEqual(pngContourInk(png.png, drawn), 0, "no isoline in the PNG");

      // An interval under the cap draws again, and the key describes it.
      from = panel.sent.length;
      await panel.drive([{ id: "contour-interval", value: 5 }]);
      await panel.waitSent(isType("contourReady"), from, "the contours at 5");
      state = await panel.drive([]);
      assert.ok(state.contourKey && state.contourKey.startsWith("Contours every 5 "), `the key: ${state.contourKey}`);
      assert.ok(state.contourInk > 0, "the isolines are back");
      assert.strictEqual(state.contourStatus, "", "the refusal is gone");
    } finally {
      close();
    }
  });

  test("a refused color range under only: the key says no contours are drawn, and Export PNG refuses", async () => {
    const { panel, drawn, close } = await openPanel(GRIB);
    try {
      await contoursOnlyDrawn(panel, drawn);
      let from = panel.sent.length;
      await panel.drive([
        { selector: 'input[name="range-mode"][value="manual"]', checked: true },
        { id: "range-min", value: 300 },
        { id: "range-max", value: 290 },
      ]);
      await panel.waitSent((m: Msg) => m.type === "gridError" && !!m.rangeRefusal, from, "the refused range");
      let state = await panel.drive([]);
      assert.ok(state.rangeNote, "the refusal is under the range fields");
      assert.strictEqual(state.contourKey, NO_CONTOURS, "the key claims no contours");
      await assertBreaksBetweenParts(panel, NO_CONTOURS);
      assert.strictEqual(state.colorBar, false, "and the bar does not stand in for it");
      assert.strictEqual(state.contourInk, 0, "no isoline on screen");
      // Export PNG refuses a refused range rather than save the old picture
      // (#1174), so no PNG claims contours either.
      const asked = panel.received.length;
      await panel.drive([{ id: "export-png", click: true }]);
      state = await waitForRead(
        () => panel.drive([]),
        (s) => s.exportStatus.startsWith("Export failed: the color range in the fields was refused"),
        "Export PNG's refusal",
      );
      assert.strictEqual(panel.received.slice(asked).filter(isType("exportPng")).length, 0, "no PNG was made");

      // Fixed, the contours and their key come back.
      from = panel.sent.length;
      await panel.drive([{ id: "range-min", value: 250 }]);
      await panel.waitSent(isType("contourReady"), from, "the contours over the fixed range");
      state = await panel.drive([]);
      assert.ok(state.contourKey && state.contourKey.startsWith("Contours every "), `the key: ${state.contourKey}`);
      assert.ok(state.contourInk > 0, "the isolines are back");
    } finally {
      close();
    }
  });
});
