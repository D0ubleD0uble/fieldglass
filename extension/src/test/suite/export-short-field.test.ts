// A PNG export of a very short field keeps its color-bar labels apart (#1842).
//
// A 73 x 12 field (a time cross-section of `air`, Y axis time, X axis lat)
// was exported at native size, so the bar was 12 px tall and its three 11 px
// labels were drawn over each other. The bar's column is now laid out at least
// as tall as its labels need; a field tall enough already exports as before.
//
// Through the real panel over a classic NetCDF file written for the test: the
// label rows are what the page says it wrote, checked against the PNG's size.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { exportColumnHeight, exportLabelBoxes } from "../../render-panel";
import {
  classicNetcdf,
  decodePng,
  isType,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const MARGIN = 14;
const TITLE_H = 46; // title and subtitle

/** Two variables over `lat` (73): `short` 12 rows of time, `tall` 60, with a
 *  spread of values so the color bar has three distinct labels. */
function file(): Buffer {
  const make = (rows: number) => {
    const out: number[] = [];
    for (let j = 0; j < rows; j++) for (let i = 0; i < 73; i++) out.push(-38.6 + (j * 73 + i) * 0.11);
    return out;
  };
  return classicNetcdf(
    [["time", 12], ["tall", 60], ["lat", 73]],
    [
      { name: "short", dims: ["time", "lat"], attrs: { units: "degC" }, values: make(12) },
      { name: "tall", dims: ["tall", "lat"], attrs: { units: "degC" }, values: make(60) },
    ],
  );
}

async function exported(name: string): Promise<{
  png: ReturnType<typeof decodePng>;
  boxes: Array<[number, number]> | null;
  labels: string[] | null;
  height: number;
}> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1842-"));
  const nc = path.join(dir, "air.nc");
  fs.writeFileSync(nc, file());
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(nc)).variables().find((x) => x.name === name);
  assert.ok(v, `${name} is renderable`);
  const editor = await openEditor(p, nc);
  const panel: RealPanel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: v.index }));
  const saved = vscode.window.showSaveDialog;
  try {
    const drawn = await panel.waitSent(isType("gridReady"), 0, `${name} drawn`);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
    const from = panel.received.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const msg = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    const bytes = Buffer.from(String(msg.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64");
    const dump = process.env.FIELDGLASS_1842_DUMP;
    if (dump) fs.writeFileSync(path.join(dump, `${name}.png`), bytes);
    const state = await panel.drive([]);
    return {
      png: decodePng(bytes),
      boxes: state.exportedLabelBoxes,
      labels: state.exportedColorbarLabels,
      height: drawn.height as number,
    };
  } finally {
    vscode.window.showSaveDialog = saved;
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

suite("Export PNG of a very short field (#1842)", function () {
  this.timeout(120_000);

  test("a 73 x 12 export keeps its color-bar label boxes apart", async () => {
    const e = await exported("short");
    assert.strictEqual(e.height, 12, "the field is 12 rows tall");
    assert.ok(e.labels && e.labels.length === 3, "three labels");
    const boxes = e.boxes;
    assert.ok(boxes && boxes.length === 3, "and a box for each");
    for (let i = 1; i < boxes.length; i++) {
      assert.ok(
        boxes[i][0] >= boxes[i - 1][1],
        `label ${i} starts at ${boxes[i][0]}, over label ${i - 1}, which ends at ${boxes[i - 1][1]}`,
      );
    }
    assert.ok(boxes[0][0] >= MARGIN + TITLE_H && boxes[2][1] <= e.png.height - MARGIN, "the labels are inside the PNG");
    assert.strictEqual(e.png.height, MARGIN + TITLE_H + exportColumnHeight(12, true) + MARGIN, "the image holds the column");
    // And the picture agrees: the label column's dark ink falls in three
    // separate bands of rows, not one smeared run.
    const x0 = MARGIN + 73 + 16 + 18 + 5;
    const rows: boolean[] = [];
    for (let y = MARGIN + TITLE_H; y < e.png.height - MARGIN; y++) {
      let dark = false;
      for (let x = x0; x < x0 + 30 && x < e.png.width; x++) {
        const i = (y * e.png.width + x) * 4;
        if (e.png.rgba[i] < 128 && e.png.rgba[i + 1] < 128 && e.png.rgba[i + 2] < 128) dark = true;
      }
      rows.push(dark);
    }
    const bands = rows.filter((d, i) => d && !rows[i - 1]).length;
    assert.strictEqual(bands, 3, `three bands of label ink, got ${bands}`);
  });

  test("a field tall enough for its labels exports at its own height", async () => {
    const e = await exported("tall");
    assert.strictEqual(e.png.height, MARGIN + TITLE_H + 60 + MARGIN);
  });

  test("the column is the raster's height once the labels fit", () => {
    assert.strictEqual(exportColumnHeight(12, true), 39);
    assert.strictEqual(exportColumnHeight(39, true), 39);
    assert.strictEqual(exportColumnHeight(400, true), 400);
    assert.strictEqual(exportColumnHeight(12, false), 12, "no bar and no key, no growth");
    assert.strictEqual(exportColumnHeight(12, true), 39, "a contour key shares the rule, so the height does not depend on bar or key");
    const boxes = exportLabelBoxes(exportColumnHeight(1, true), 3);
    assert.ok(boxes[1][0] >= boxes[0][1] && boxes[2][0] >= boxes[1][1], "boxes apart at the smallest column");
  });
});
