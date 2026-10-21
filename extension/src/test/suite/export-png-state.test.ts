// What Export PNG draws and when it refuses (#1270, #1187), through the real
// panel: the PNG is read back from the page's own canvas.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import {
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

const FIXTURES = path.join(extensionPath(), "src", "test", "fixtures");
const FIXTURE = path.join(FIXTURES, "regular_latlon_surface.grib2");

type Image = ReturnType<typeof decodePng>;

/** Click Export PNG with the save dialog answered "cancel", and read back the
 *  PNG the page made. */
async function exportedPng(panel: RealPanel): Promise<Image> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    const sentFrom = panel.sent.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    await panel.waitSent(isType("exportPngDone"), sentFrom, "the PNG export's outcome");
    return decodePng(Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

const isWhite = (img: Image, x: number, y: number): boolean => {
  const i = (y * img.width + x) * 4;
  return img.rgba[i] === 255 && img.rgba[i + 1] === 255 && img.rgba[i + 2] === 255;
};

// The raster is drawn at its own size, 14 px in from the left, below the title
// block (the PNG's height less both 14 px margins and the raster's height).
const MARGIN = 14;

suite("Export PNG keeps what the screen shows, and refuses a stale picture", () => {
  test("Contours + only leaves the raster out of the PNG; Contours off with only ticked keeps it (#1270)", async function () {
    this.timeout(120_000);
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Contours off, only ticked (the #1265 state): the raster is drawn on
      // screen, so it is in the PNG.
      await panel.drive([{ id: "contours-only", checked: true }]);
      let state = await panel.drive([]);
      assert.strictEqual(state.canvasVisibility, "visible", "the raster stays with Contours off");
      const off = await exportedPng(panel);

      const from = panel.sent.length;
      await panel.drive([{ id: "overlay-contours", checked: true }]);
      await panel.waitSent(isType("contourReady"), from, "the contours");
      state = await panel.drive([]);
      assert.strictEqual(state.canvasVisibility, "hidden", "the screen leaves the raster out");
      const only = await exportedPng(panel);

      assert.deepStrictEqual([only.width, only.height], [off.width, off.height]);
      let whiteOnly = 0;
      let whiteOff = 0;
      let total = 0;
      let witness: [number, number] | null = null;
      const top = only.height - MARGIN - drawn.height;
      assert.ok(top >= MARGIN, `the map sits below the title block (${top})`);
      for (let y = top; y < top + drawn.height; y++) {
        for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
          total += 1;
          if (isWhite(only, x, y)) {
            whiteOnly += 1;
            // A pixel with no isoline or coastline: background with only on.
            if (!witness && !isWhite(off, x, y)) witness = [x, y];
          }
          if (isWhite(off, x, y)) whiteOff += 1;
        }
      }
      assert.ok(whiteOnly / total > 0.5, `background with only on: ${whiteOnly}/${total} white`);
      assert.ok(whiteOff / total < 0.1, `the raster's colours with only off: ${whiteOff}/${total} white`);
      assert.ok(witness, "a pixel that is background with only on and the raster's colour with it off");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("Export PNG after a render that failed for another reason posts nothing and says why (#1187)", async function () {
    this.timeout(120_000);
    const p = await provider();
    // Two grids, so a Compare of them fails before the range is read.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1187-"));
    const file = path.join(dir, "two-grids.grib2");
    fs.writeFileSync(
      file,
      Buffer.concat([fs.readFileSync(FIXTURE), fs.readFileSync(path.join(FIXTURES, "eta_lambert_msg0.grib2"))]),
    );
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    // A refusal asks for no dialog, but never leave a real one open.
    const savedDialog = vscode.window.showSaveDialog;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      await panel.waitSent((m: Msg) => m.type === "gridError" && !m.rangeRefusal, from, "the different-grid error");
      const asked = panel.received.length;
      let state = await panel.drive([{ id: "export-png", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportPng")), "no PNG is written of the old picture");
      assert.ok(state.exportStatus.startsWith("Export failed:"), state.exportStatus);
      assert.ok(state.exportStatus.includes("last render failed"), state.exportStatus);

      // A render that draws clears it, and the export goes ahead.
      from = panel.sent.length;
      await panel.drive([{ id: "compare-field-b", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from, "the redraw");
      state = await panel.drive([]);
      const before = panel.received.length;
      const sentBefore = panel.sent.length;
      await panel.drive([{ id: "export-png", click: true }]);
      assert.ok(panel.received.slice(before).some(isType("exportPng")), "export works after a render that drew");
      await panel.waitSent(isType("exportPngDone"), sentBefore, "the PNG export's outcome");
    } finally {
      vscode.window.showSaveDialog = savedDialog;
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
