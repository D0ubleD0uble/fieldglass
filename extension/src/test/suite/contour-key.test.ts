// Under Contours "only" the panel and the exported PNG show a contour key in
// the colour bar's place (#1463): the interval in use and the color range the
// lines are spaced over (the data range unless a manual range is set), with
// units. The colour field is not drawn then, so a gradient would suggest
// colour carries meaning. Unticking "only" (or Contours) brings the bar back.
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
async function exportedPng(panel: RealPanel): Promise<{ png: Image; key: string }> {
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
    return { png, key: (await panel.drive([])).exportedContourKey };
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

/** Whether any pixel of the PNG in the box is not white, and whether any is
 *  coloured (red, green and blue not within 8 of each other): a gradient is,
 *  the key's dark text and the page's white are not. */
function inkIn(img: Image, x0: number, y0: number, x1: number, y1: number): { ink: boolean; colour: boolean } {
  let ink = false;
  let colour = false;
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const i = (y * img.width + x) * 4;
      const [r, g, b] = [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]];
      if (r < 250 || g < 250 || b < 250) ink = true;
      if (Math.max(r, g, b) - Math.min(r, g, b) > 8) colour = true;
    }
  }
  return { ink, colour };
}

/** The column right of the map in the PNG: where the bar is drawn, or where
 *  the key is written in its place. */
function barColumn(png: Image, drawn: Msg): { ink: boolean; colour: boolean } {
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

/** The range part of a key, `210.4 … 309.7 K` as the colour bar's ends write
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

/** The whole behaviour on one panel; `expectedUnits` is the field's when the
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
    // Contours off: the colour bar, no key, whatever "only" says.
    await panel.drive([{ id: "contours-only", checked: true }]);
    let state = await panel.drive([]);
    assert.strictEqual(state.colourBar, true, "the colour bar with Contours off");
    assert.strictEqual(state.contourKey, null, "no key with Contours off");

    // Contours + only: the key replaces the bar.
    await tickContours(panel);
    state = await panel.drive([]);
    assert.strictEqual(state.colourBar, false, "no colour bar under Contours + only");
    assert.ok(state.contourKey, "the key is on screen");
    assertKeyFor(state.contourKey, drawn, units);
    assertIntervalIsTheAutomaticOne(file, target, parts(state.contourKey).interval, drawn);

    // The PNG: the bar's column holds no gradient, the key is written, and it
    // says what the screen says.
    const only = await exportedPng(panel);
    assert.strictEqual(only.key, state.contourKey, "the PNG writes the key the screen shows");
    const column = barColumn(only.png, drawn);
    assert.strictEqual(column.colour, false, "no gradient in the bar's column of the PNG");
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
    assert.strictEqual(state.colourBar, true, "the colour bar is back");
    assert.strictEqual(state.contourKey, null, "the key is gone");
    const bar = await exportedPng(panel);
    assert.strictEqual(bar.key, "", "the PNG writes no key");
    assert.strictEqual(barColumn(bar.png, drawn).colour, true, "the PNG has its gradient again");

    // Unticking Contours with "only" ticked does the same.
    await panel.drive([{ id: "contours-only", checked: true }]);
    state = await panel.drive([]);
    assert.strictEqual(state.contourKey !== null && !state.colourBar, true, "the key again");
    await panel.drive([{ id: "overlay-contours", checked: false }]);
    state = await panel.drive([]);
    assert.strictEqual(state.colourBar, true, "the bar with Contours off");
    assert.strictEqual(state.contourKey, null, "no key with Contours off");
  } finally {
    close();
  }
}

suite("The contour key replaces the colour bar under Contours only (#1463)", function () {
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
        assert.strictEqual(state.colourBar, false, `no bar over a blank map (interval "${typed}")`);
        assert.strictEqual(state.contourKey, "No contours · 4.000 K");
        assert.strictEqual((await exportedPng(panel)).key, "No contours · 4.000 K", "and the PNG says so");
      }
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
