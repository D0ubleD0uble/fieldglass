// The rails, the readouts and the CSV export follow the slice drawn (#1145).
//
// Between a change of variable, of image axes or of the Compare rider and the
// render it asks for, the old image is still on screen while the pickers
// already name the new one. The rails were redrawn from the pickers at once,
// and the probe, the line plot and the CSV export asked about the pickers'
// slice, so a click in that window read the new variable's cell beside a
// label of the old one, over the old one's image. #1114 fixed the same race
// for the rows' orientation and the map geometry.
//
// Driven through the real panel: the file is written to disk, opened the way
// a user opens it, and the render each change asks for is held back as if
// still on its way. Only VS Code's dialogs are stubbed, for the CSV export.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { CSV_FIELD_A_NOTE } from "../../render-panel";
import {
  classicNetcdf,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RailsState,
  type RealPanel,
  waitUntil,
} from "./real-panel";

// The NCEP/NCAR reanalysis's 17 pressure levels, surface first, and a
// profile like 30° S 25° E's, as `axes.test.ts` has them.
const LEVELS = [1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 70, 50, 30, 20, 10];
const AIR = [31, 24, 19, 8, 0, -8, -18, -31, -40, -50, -62, -72, -66, -61, -54, -50, -43];
const LAT3 = [-32.5, -30, -27.5];
const LON3 = [22.5, 25, 27.5];
// `air` adds 100 per longitude step, so a Compare of two longitudes reads
// their distance, and `rh` is 500 plus the level's index everywhere: no
// value of one is a value of the other.
const air = (k: number, i: number) => AIR[k] + 100 * i;
const rh = (k: number) => 500 + k;

/** `air(level, lat, lon)` and `rh(level, lat, lon)` on the 17 levels. */
function twoVariables(): Buffer {
  const cells = (f: (k: number, i: number) => number) => {
    const out: number[] = [];
    for (let k = 0; k < LEVELS.length; k++) {
      for (let j = 0; j < LAT3.length; j++) {
        for (let i = 0; i < LON3.length; i++) out.push(f(k, i));
      }
    }
    return out;
  };
  return classicNetcdf(
    [["level", LEVELS.length], ["lat", 3], ["lon", 3]],
    [
      { name: "level", dims: ["level"], attrs: { units: "millibar", positive: "down", axis: "Z" }, values: LEVELS },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: LAT3 },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: LON3 },
      { name: "air", dims: ["level", "lat", "lon"], attrs: { units: "degC" }, values: cells(air) },
      { name: "rh", dims: ["level", "lat", "lon"], attrs: { units: "%" }, values: cells((k) => rh(k)) },
    ],
  );
}

interface Section {
  panel: RealPanel;
  dir: string;
  air: number;
  rh: number;
  close: () => void;
}

/** Open {@link twoVariables} in the editor and draw `air` as a level ×
 *  latitude section at the first longitude: the ground at the bottom. */
async function airSection(): Promise<Section> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1145-"));
  const file = path.join(dir, "two.nc");
  fs.writeFileSync(file, twoVariables());
  const native = loadNative();
  assert.ok(native, "native binding required");
  const vars = native.open(fs.readFileSync(file)).variables();
  const airVar = vars.find((v) => v.name === "air");
  const rhVar = vars.find((v) => v.name === "rh");
  assert.ok(airVar && rhVar, "air and rh are renderable");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: airVar.index }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    const from = panel.sent.length;
    await panel.drive([{ id: "slice-y", value: 0 }, { id: "slice-x", value: 1 }]);
    await panel.waitSent(
      (m) => m.type === "gridReady" && m.width === 3 && m.height === LEVELS.length && m.options.flipY === true,
      from,
      "the level × latitude section",
    );
  } catch (err) {
    close();
    throw err;
  }
  return { panel, dir, air: airVar.index, rh: rhVar.index, close };
}

/** The last render the page drew. */
function lastDrawn(panel: RealPanel): Msg {
  const drew = panel.sent.filter(isType("gridReady"));
  assert.ok(drew.length > 0, "a render was drawn");
  return drew[drew.length - 1];
}

/** Click the point `fy` of the way down the image; the request the page sent
 *  and the value read. */
async function click(panel: RealPanel, fy: number, what: string): Promise<{ asked: Msg; read: Msg }> {
  const received = panel.received.length;
  const sent = panel.sent.length;
  await panel.probe(fy);
  const asked = await waitUntil(() => panel.received.slice(received).find(isType("probeRequest")), `the click (${what})`);
  const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
  return { asked, read };
}

/** On the air section drawn at longitude `lon`, or a Compare of it with
 *  `valueAt`: the rails label the levels, 10 mb at the top, and a click by
 *  the top and the bottom label reads that level of the image on screen,
 *  asking about the slice drawn. */
async function readsTheSection(
  panel: RealPanel,
  airIndex: number,
  valueAt: (k: number) => number,
  what: string,
): Promise<{ rails: RailsState; asked: Msg }> {
  const rails = (await panel.drive([])).rails;
  assert.ok(rails?.shown, `${what}: the rails are shown, as the image on screen is a section`);
  assert.deepStrictEqual(
    [rails.y[0].text, rails.y[rails.y.length - 1].text],
    ["10", "1000"],
    `${what}: the rails label the image on screen`,
  );
  let asked: Msg = null;
  for (const [fy, k] of [[0.02, LEVELS.length - 1], [0.98, 0]] as const) {
    const got = await click(panel, fy, `${what}, at ${fy}`);
    asked = got.asked;
    assert.strictEqual(asked.slice?.variableIndex, airIndex, `${what}: the click asks about the variable drawn`);
    assert.deepStrictEqual([asked.slice.yDim, asked.slice.xDim], [0, 1], `${what}: and its axes`);
    assert.strictEqual(got.read?.value, valueAt(k), `${what}: the click by "${LEVELS[k]}" reads that level`);
  }
  return { rails, asked };
}

suite("The rails, the readouts and the CSV export follow the slice drawn (#1145)", function () {
  this.timeout(120_000);

  test("after a variable switch, before its render arrives", async () => {
    const { panel, air: airIndex, rh: rhIndex, close } = await airSection();
    try {
      // The line plot is on, so a click asks for the cell's line as well.
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const gate = panel.hold(isType("gridReady"));
      const received = panel.received.length;
      await panel.drive([{ id: "slice-variable", value: rhIndex }]);
      await waitUntil(
        () => panel.received.slice(received).find((m: Msg) => m.type === "rerenderRequest" && m.slice?.variableIndex === rhIndex),
        "the render the variable asks for",
      );
      await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");

      const lines = panel.received.length;
      const lineSent = panel.sent.length;
      await readsTheSection(panel, airIndex, (k) => air(k, 0), "while rh is on its way");
      // The line through the last cell clicked is a line of the drawn slice.
      const line = await waitUntil(() => panel.received.slice(lines).find(isType("lineRequest")), "the line asked for");
      assert.strictEqual(line.slice?.variableIndex, airIndex, "the line is asked of the variable drawn");
      await panel.waitSent(isType("lineResult"), lineSent, "the line");
      const plot = (await panel.drive([])).linePlot;
      assert.ok(plot?.label?.includes("air"), `the plot names the variable drawn: ${plot?.label}`);

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).slice?.variableIndex === rhIndex ? true : undefined), "rh drawn");
      const state = await panel.drive([]);
      assert.ok(!state.rails?.shown, "rh is drawn as a map, which has no rails");
      const after = await click(panel, 0.5, "on rh");
      assert.strictEqual(after.asked.slice?.variableIndex, rhIndex, "once drawn, the click asks about rh");
      assert.strictEqual(after.read?.value, rh(0), "and reads it");
    } finally {
      close();
    }
  });

  test("after a Y-axis switch, before its render arrives", async () => {
    const { panel, air: airIndex, close } = await airSection();
    try {
      // Longitude's values are known already (its stepper asked for them),
      // so the render goes at once, and is held. The rails are redrawn as
      // the axes change.
      const gate = panel.hold(isType("gridReady"));
      const received = panel.received.length;
      await panel.drive([{ id: "slice-y", value: 2 }]);
      const asked = await waitUntil(
        () => panel.received.slice(received).find((m: Msg) => m.type === "rerenderRequest" && m.slice?.yDim === 2),
        "the render the Y axis asks for",
      );
      assert.deepStrictEqual([asked.slice.yDim, asked.slice.xDim], [2, 1], "a longitude × latitude section");
      await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
      await readsTheSection(panel, airIndex, (k) => air(k, 0), "while longitude × latitude is on its way");

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).slice?.yDim === 2 ? true : undefined), "the new section");
      const rails = (await panel.drive([])).rails;
      assert.ok(rails?.shown, "the new section has rails");
      assert.ok(!rails.y.some((l) => l.text === "1000" || l.text === "10"), "which no longer name levels");
      // Its rows are longitudes and its slice the surface: the bottom row
      // is the last longitude.
      const bottom = await click(panel, 0.98, "on the new section");
      assert.strictEqual(bottom.asked.slice?.yDim, 2, "once drawn, the click asks about it");
      assert.strictEqual(bottom.read?.value, air(0, 2), "and reads it");
    } finally {
      close();
    }
  });

  test("with Compare on, after Field B and the operation change, before the render arrives", async () => {
    const { panel, air: airIndex, close } = await airSection();
    const w = vscode.window as unknown as Record<string, unknown>;
    const saved = { showSaveDialog: w.showSaveDialog, showQuickPick: w.showQuickPick };
    try {
      // A minus B, with Field B at the last longitude: 200 less everywhere.
      let from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(isType("gridReady"), from, "the difference map");
      from = panel.sent.length;
      await panel.drive([{ selector: '#compare-dims .slice-number[data-dim="2"]', value: 2 }]);
      await panel.waitSent(
        (m) => m.type === "gridReady" && m.compareSliceIndicesB?.[2] === 2,
        from,
        "the difference with Field B at the last longitude",
      );
      const before = await readsTheSection(panel, airIndex, () => -200, "the difference drawn");
      assert.deepStrictEqual(
        [before.asked.compare?.op, before.asked.compare?.sliceIndicesB?.[2]],
        ["a_minus_b", 2],
        "the click carries the rider drawn",
      );

      // Field B steps back one longitude, its render held.
      const gate = panel.hold(isType("gridReady"));
      let received = panel.received.length;
      await panel.drive([{ selector: '#compare-dims .slice-number[data-dim="2"]', value: 1 }]);
      await waitUntil(
        () => panel.received.slice(received).find((m: Msg) => m.type === "rerenderRequest" && m.compare?.sliceIndicesB?.[2] === 1),
        "the render Field B asks for",
      );
      const stepped = await readsTheSection(panel, airIndex, () => -200, "while Field B's render is on its way");
      assert.strictEqual(stepped.asked.compare?.sliceIndicesB?.[2], 2, "the click asks about the Field B drawn");

      // Compare goes off, its render held too: the difference is still on
      // screen, so the click reads it and the CSV is Field A's.
      received = panel.received.length;
      await panel.drive([{ id: "compare-op", value: "" }]);
      await waitUntil(
        () => panel.received.slice(received).find((m: Msg) => m.type === "rerenderRequest" && !m.compare),
        "the render with Compare off",
      );
      const off = await readsTheSection(panel, airIndex, () => -200, "while the single field is on its way");
      assert.strictEqual(off.asked.compare?.op, "a_minus_b", "the click asks about the operation drawn");

      const prompts: string[] = [];
      w.showQuickPick = (_items: unknown, opts?: { placeHolder?: string }) => {
        prompts.push(opts?.placeHolder ?? "");
        return Promise.resolve(undefined);
      };
      w.showSaveDialog = () => Promise.resolve(undefined);
      await panel.drive([{ id: "export-csv", click: true }]);
      await waitUntil(() => (prompts.length > 0 ? true : undefined), "the CSV layout prompt");
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await ((await provider()) as any)._csvExport;
      assert.ok(prompts[0].startsWith(CSV_FIELD_A_NOTE), `the CSV of a difference on screen is Field A's: ${prompts[0]}`);

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).compare === null ? true : undefined), "the single field drawn");
      const single = await click(panel, 0.02, "on the single field");
      assert.strictEqual(single.asked.compare, undefined, "once drawn, the click asks about one field");
      assert.strictEqual(single.read?.value, air(LEVELS.length - 1, 0), "and reads it");
    } finally {
      Object.assign(w, saved);
      close();
    }
  });

  test("a GRIB Compare map, after the operation changes, before the render arrives", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1145-"));
    const file = path.join(dir, "pair.grib2");
    const bytes = fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"));
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const single = await click(panel, 0.5, "one field");
      assert.ok(typeof single.read?.value === "number" && single.read.value !== 0, "the field reads a value");
      const asked = panel.received.length;
      const drew = panel.sent.filter(isType("gridReady")).length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      // Every render the two controls asked for has been drawn.
      await waitUntil(() => {
        const renders = panel.received.slice(asked).filter(isType("rerenderRequest"));
        const last = renders[renders.length - 1];
        if (last?.compare?.messageIndexB !== 1) return undefined;
        return panel.sent.filter(isType("gridReady")).length >= drew + renders.length ? true : undefined;
      }, "the difference map");
      const diff = await click(panel, 0.5, "the difference");
      assert.strictEqual(diff.read?.value, 0, "a message less itself is zero");

      const gate = panel.hold(isType("gridReady"));
      const received = panel.received.length;
      await panel.drive([{ id: "compare-op", value: "a_plus_b" }]);
      await waitUntil(
        () => panel.received.slice(received).find((m: Msg) => m.type === "rerenderRequest" && m.compare?.op === "a_plus_b"),
        "the render the operation asks for",
      );
      const waiting = await click(panel, 0.5, "while the sum is on its way");
      assert.strictEqual(waiting.asked.compare?.op, "a_minus_b", "the click asks about the operation drawn");
      assert.strictEqual(waiting.read?.value, 0, "and reads the difference on screen");

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).compare?.op === "a_plus_b" ? true : undefined), "the sum drawn");
      const sum = await click(panel, 0.5, "the sum");
      assert.strictEqual(sum.read?.value, 2 * single.read.value, "once drawn, the click reads the sum");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("Export CSV during a variable switch saves the field on screen, named for it", async () => {
    const { panel, dir, air: airIndex, rh: rhIndex, close } = await airSection();
    const w = vscode.window as unknown as Record<string, unknown>;
    const saved = {
      showSaveDialog: w.showSaveDialog,
      showQuickPick: w.showQuickPick,
      showInformationMessage: w.showInformationMessage,
    };
    const dest = vscode.Uri.file(path.join(dir, "out.csv"));
    let suggested: vscode.Uri | undefined;
    try {
      w.showQuickPick = (items: Array<{ format?: string }>) =>
        Promise.resolve(Array.isArray(items) ? items.find((i) => i.format === "matrix") : undefined);
      w.showSaveDialog = (opts: { defaultUri?: vscode.Uri }) => {
        suggested = opts.defaultUri;
        return Promise.resolve(dest);
      };
      w.showInformationMessage = () => Promise.resolve(undefined);

      const gate = panel.hold(isType("gridReady"));
      const received = panel.received.length;
      await panel.drive([{ id: "slice-variable", value: rhIndex }]);
      await waitUntil(() => gate.held.find(isType("gridReady")), "rh's render, held back");
      await panel.drive([{ id: "export-csv", click: true }]);
      const asked = await waitUntil(
        () => panel.received.slice(received).find(isType("exportSliceCsv")),
        "the export",
      );
      assert.deepStrictEqual(
        asked.slice,
        { variableIndex: airIndex, yDim: 0, xDim: 1, sliceIndices: [0, 0, 0] },
        "the export asks for the slice drawn",
      );
      await waitUntil(() => (suggested ? true : undefined), "the save dialog");
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await ((await provider()) as any)._csvExport;
      const name = path.basename(suggested?.fsPath ?? "");
      assert.match(name, /_air_/, `the file is named for the field on screen: ${name}`);
      assert.doesNotMatch(name, /_rh/, `not the one on its way: ${name}`);
      // The matrix is the section in storage order: a row per level, surface
      // first, a cell per latitude.
      const rows = fs.readFileSync(dest.fsPath, "utf8").trim().split("\n");
      assert.deepStrictEqual(
        rows,
        LEVELS.map((_, k) => [0, 1, 2].map(() => String(air(k, 0))).join(",")),
        "the CSV holds the field on screen",
      );
      await gate.release();
    } finally {
      Object.assign(w, saved);
      close();
    }
  });
});
