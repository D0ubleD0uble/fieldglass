// A very thin 2-D slice is drawn at a size the panel can paint (#1226).
//
// `time_bnds` in the 0.6.0 sample `ncep_air_daily_2024.nc` is 366 × 2. The
// image was sized to the panel's width at the raster's own aspect, so a
// 2-wide raster became about 775 × 141,000 px, past what the page would
// paint: a blank box with a broken-image glyph. Turned the other way, 366 × 2
// was a strip 4 px tall whose rows were a pixel or two each. Now the image
// is never drawn more than eight times as long one way as the other, so its
// cells stretch to stay visible, and a click still reads the cell under it.
// A tall one is drawn to the page's height, not eight panel widths. An
// ordinary aspect keeps the raster's own.
//
// Any image that would be taller than that at the panel's width is fitted to
// the page's height too, not only one held to eight to one (#1252). A 50 ×
// 366 slice (about 1:7.3) was drawn across the panel's width, about 5,000 px
// tall, while a 40 × 366 one (past 1:8) fitted the page.
//
// A lat/lon map (Equirectangular, Web Mercator) is not stretched: its
// raster has the shape of its grid or its window, and the arrows are laid
// out in square pixels of it, so a belt round the globe from 20° S to 20° N
// keeps its own shape. A tall map keeps its shape too, and is held to the
// page's height.
//
// Driven through the real panel over classic NetCDF files written for the
// test: the box measured is the page's own canvas, and the cell read is the
// one the provider answers for.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { displayAspect } from "../../render-panel";
import {
  type Box,
  classicNetcdf,
  decodePng,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";
import { expectedImageWidth } from "./row-width";

/** The longest the image is drawn against its shortest side. */
const MOST = 8;

/** A variable's value at file row `j`, column `i`: no two cells share one. */
const valueAt = (j: number, i: number) => 1000 * j + i;

function values(rows: number, cols: number): number[] {
  const out: number[] = [];
  for (let j = 0; j < rows; j++) for (let i = 0; i < cols; i++) out.push(valueAt(j, i));
  return out;
}

/** `time_bnds` as the sample has it, the same turned over, a one-row strip,
 *  an ordinary 2:1 field, and two tall sections either side of 1:8. No
 *  coordinate variables, so the rows are drawn in index order. */
function thinFile(): Buffer {
  return classicNetcdf(
    [["time", 366], ["nbnds", 2], ["one", 1], ["n", 400], ["y", 20], ["x", 40], ["c50", 50], ["c40", 40]],
    [
      { name: "time_bnds", dims: ["time", "nbnds"], attrs: {}, values: values(366, 2) },
      { name: "bnds_time", dims: ["nbnds", "time"], attrs: {}, values: values(2, 366) },
      { name: "strip", dims: ["one", "n"], attrs: {}, values: values(1, 400) },
      { name: "ordinary", dims: ["y", "x"], attrs: {}, values: values(20, 40) },
      { name: "wide_section", dims: ["time", "c50"], attrs: {}, values: values(366, 50) },
      { name: "narrow_section", dims: ["time", "c40"], attrs: {}, values: values(366, 40) },
    ],
  );
}

interface Drawn {
  panel: RealPanel;
  /** The render drawn. */
  grid: Msg;
  /** The image's content box on the page, inside its 1 px border. */
  width: number;
  height: number;
  /** The page's height. */
  viewport: number;
  /** The room the image has: from its left edge to its row's right edge. */
  room: number;
  close: () => void;
}

/** The image as laid out, once it is: its content box and the page's height. */
async function laidOut(panel: RealPanel): Promise<Layout> {
  let image: Box | undefined;
  let row: Box | undefined;
  let viewport = 0;
  for (let i = 0; i < 50; i++) {
    const state = await panel.drive([]);
    image = state.image;
    row = state.plotRow;
    viewport = state.viewportHeight;
    if (image && image.right - image.left > 2 && image.bottom - image.top > 2) break;
    await new Promise((r) => setTimeout(r, 20));
  }
  assert.ok(image && row, "the panel reports its image and its row");
  return {
    width: image.right - image.left - 2,
    height: image.bottom - image.top - 2,
    viewport,
    room: row.right - image.left,
  };
}

/** The image as laid out: its content box, the page's height, and the room
 *  its row gives it, from its left edge to the row's right edge. */
interface Layout {
  width: number;
  height: number;
  viewport: number;
  room: number;
}

/** The image is as wide as its row, or, if the row is wider than the cap
 *  (80% of the page's height times the image's aspect, #1252), as wide as the
 *  cap. {@link expectedImageWidth} is that width, so the check holds in a panel
 *  of any shape. Within 2 px either way: the border is 1 px, and a
 *  raster wider than the panel is narrowed to the row's width inside its
 *  border, so its right border is clipped. `aspect` is the raster's width over
 *  its height, which is the aspect the image is drawn at: a caller with an image
 *  held to 8:1 must pass the held aspect, not the raster's. */
function assertAcrossRow(d: Layout, aspect: number, what: string): void {
  const expected = expectedImageWidth(d.viewport, aspect, d.room);
  assert.ok(
    Math.abs(d.width + 2 - expected) <= 2,
    `${what}: the image is ${(d.width + 2).toFixed(1)} px wide; with ${d.room.toFixed(1)} px of row and a ` +
      `${d.viewport} px page it should be ${expected.toFixed(1)}`,
  );
}

/** Widen the page to `px`, as a wide editor with a short page is, and lay the
 *  image out again once it has settled at its new width. */
async function widened(panel: RealPanel, px: number): Promise<Layout> {
  await panel.drive([{ id: "body", width: `${px}px` }]);
  let last = await laidOut(panel);
  for (let i = 0; i < 50; i++) {
    await new Promise((r) => setTimeout(r, 20));
    const next = await laidOut(panel);
    const settled = Math.abs(next.width - last.width) < 0.5 && Math.abs(next.room - last.room) < 0.5;
    last = next;
    if (settled) break;
  }
  return last;
}

/** Open the file and draw `name`; the image as laid out. */
async function draw(name: string): Promise<Drawn> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1226-"));
  const file = path.join(dir, "thin.nc");
  fs.writeFileSync(file, thinFile());
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(file)).variables().find((x) => x.name === name);
  assert.ok(v, `${name} is renderable`);
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: v.index }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    const grid = await panel.waitSent(isType("gridReady"), 0, `${name} drawn`);
    return { panel, grid, ...(await laidOut(panel)), close };
  } catch (err) {
    close();
    throw err;
  }
}

/** Click Export PNG with the save dialog canceled and decode the PNG the page
 *  made. */
async function exportedPng(panel: RealPanel): Promise<{ width: number; height: number }> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    return decodePng(Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

/** Click the point `(fx, fy)` of the image: the cell asked about and the
 *  answer. */
async function click(panel: RealPanel, fx: number, fy: number): Promise<{ asked: Msg; read: Msg }> {
  const received = panel.received.length;
  const sent = panel.sent.length;
  await panel.probe(fy, fx);
  const asked = await waitUntil(() => panel.received.slice(received).find(isType("probeRequest")), "the click");
  const read = (await panel.waitSent(isType("probeResult"), sent, "the probe")).result;
  return { asked, read };
}

/** The image is no more than {@link MOST} times as long one way as the other
 *  (a pixel either way for rounding), so the page can paint it and each cell
 *  across its short side is a band a user can see. */
function assertPaintable(d: { width: number; height: number }, what: string): void {
  assert.ok(
    d.height <= MOST * d.width + 1 && d.width <= MOST * d.height + 1,
    `${what}: the image is drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, ` +
      `more than ${MOST} times as long one way as the other`,
  );
}

/** A tall image fits the page's height, so it is one column on one screen
 *  and its overlay's backing store stays far inside the browser's limit. */
function assertWithinPage(d: Layout, what: string): void {
  assert.ok(d.viewport > 0, `${what}: the page has a height`);
  assert.ok(
    d.height <= d.viewport,
    `${what}: the image is ${d.height.toFixed(1)} px tall, past the page's ${d.viewport} px`,
  );
}

/** Clicks across the image read the cell drawn there, by the request's cell
 *  and the value the provider answers with. A mouse event's position is a
 *  whole pixel, so along an axis whose cells are under 4 px wide (366 cells
 *  across the panel) the click may land on the neighboring cell; along a
 *  stretched axis, which this fix makes, it must be the cell itself. */
async function assertClicksHit(d: Drawn, what: string): Promise<void> {
  const cols = d.grid.width as number;
  const rows = d.grid.height as number;
  const flip = d.grid.options?.flipY === true;
  const slackX = d.width / cols < 4 ? 1 : 0;
  const slackY = d.height / rows < 4 ? 1 : 0;
  // Cell centers: the first and last of each axis, and one between.
  const at = (n: number) => [0.5 / n, (Math.floor(n / 3) + 0.5) / n, (n - 0.5) / n];
  for (const fy of at(rows)) {
    for (const fx of at(cols)) {
      const { asked, read } = await click(d.panel, fx, fy);
      const where = `${what}: the click at (${fx.toFixed(4)}, ${fy.toFixed(4)})`;
      const px = Math.floor(fx * cols);
      const py = Math.floor(fy * rows);
      assert.ok(
        Math.abs(asked.px - px) <= slackX && Math.abs(asked.py - py) <= slackY,
        `${where} asks for cell (${asked.px}, ${asked.py}), not (${px}, ${py})`,
      );
      assert.ok(read, `${where} reads a value`);
      const row = flip ? rows - 1 - asked.py : asked.py;
      assert.strictEqual(read.value, valueAt(row, asked.px), `${where} reads the cell it asked for`);
    }
  }
}

suite("A very thin slice is drawn at a paintable size (#1226)", function () {
  this.timeout(120_000);

  test("2 × 366 (time_bnds): drawn no taller than eight widths, and a click reads its cell", async () => {
    const d = await draw("time_bnds");
    try {
      assert.deepStrictEqual([d.grid.width, d.grid.height], [2, 366], "the raster is the 2 × 366 slice");
      assertPaintable(d, "2 × 366");
      assertWithinPage(d, "2 × 366");
      assert.ok(
        Math.abs(d.height / d.width - MOST) < 0.1,
        `2 × 366: drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at 1:${MOST}`,
      );
      await assertClicksHit(d, "2 × 366");
    } finally {
      d.close();
    }
  });

  test("366 × 2: drawn at least an eighth as tall as it is wide, and a click reads its cell", async () => {
    const d = await draw("bnds_time");
    try {
      assert.deepStrictEqual([d.grid.width, d.grid.height], [366, 2], "the raster is the 366 × 2 slice");
      assertPaintable(d, "366 × 2");
      await assertClicksHit(d, "366 × 2");
    } finally {
      d.close();
    }
  });

  test("400 × 1: a one-row strip is a band, not a hairline", async () => {
    const d = await draw("strip");
    try {
      assert.deepStrictEqual([d.grid.width, d.grid.height], [400, 1], "the raster is the one-row strip");
      assertPaintable(d, "400 × 1");
      await assertClicksHit(d, "400 × 1");
    } finally {
      d.close();
    }
  });

  test("An ordinary 40 × 20 slice keeps the raster's own aspect", async () => {
    const d = await draw("ordinary");
    try {
      assert.deepStrictEqual([d.grid.width, d.grid.height], [40, 20], "the raster is the 40 × 20 slice");
      assert.ok(
        Math.abs(d.width / d.height - 2) < 0.02,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at 2:1`,
      );
      assertAcrossRow(d, 2, "40 × 20");
    } finally {
      d.close();
    }
  });

  test("An ordinary 40 × 20 slice in a wide, short panel is held to 80% of the panel's height", async () => {
    const d = await draw("ordinary");
    try {
      // A body wider than the cap is a wide, short panel: the page's height
      // is unchanged, the row is much wider than 80% of it times 2.
      const w = await widened(d.panel, Math.ceil(0.8 * d.viewport * 2) * 2 + 400);
      const cap = 0.8 * w.viewport * 2 + 2;
      assert.ok(w.room > cap + 100, `the row (${w.room.toFixed(1)} px) is wider than the cap (${cap.toFixed(1)} px)`);
      assert.ok(
        Math.abs(w.width + 2 - cap) <= 2,
        `the image is ${(w.width + 2).toFixed(1)} px wide, not held to the cap of ${cap.toFixed(1)} px`,
      );
      assertAcrossRow(w, 2, "40 × 20 in a wide panel");
      assert.ok(Math.abs(w.width / w.height - 2) < 0.02, "and it keeps its 2:1 aspect");
    } finally {
      d.close();
    }
  });
});

suite("A tall image short of 1:8 fits the page too (#1252)", function () {
  this.timeout(120_000);

  test("50 × 366 and 40 × 366 both fit the page, the narrower no shorter, and a click reads its cell", async () => {
    let wideHeight = 0;
    const wide = await draw("wide_section");
    try {
      assert.deepStrictEqual([wide.grid.width, wide.grid.height], [50, 366], "the raster is the 50 × 366 slice");
      assertWithinPage(wide, "50 × 366");
      // Not held: 1:7.3 is drawn at its own aspect, only smaller.
      const raster = 366 / 50;
      assert.ok(
        Math.abs(wide.height / wide.width - raster) / raster < 0.03,
        `50 × 366: drawn ${wide.width.toFixed(1)} × ${wide.height.toFixed(1)} px, ` +
          `not at the raster's 1:${raster.toFixed(2)}`,
      );
      await assertClicksHit(wide, "50 × 366");
      wideHeight = wide.height;
    } finally {
      wide.close();
    }
    const narrow = await draw("narrow_section");
    try {
      assert.deepStrictEqual([narrow.grid.width, narrow.grid.height], [40, 366], "the raster is the 40 × 366 slice");
      assertWithinPage(narrow, "40 × 366");
      assert.ok(
        narrow.height >= wideHeight - 1,
        `40 × 366 is drawn ${narrow.height.toFixed(1)} px tall, shorter than 50 × 366 at ${wideHeight.toFixed(1)} px`,
      );
      await assertClicksHit(narrow, "40 × 366");
    } finally {
      narrow.close();
    }
  });
});

/** Clicks across a map drawn at an extreme shape read the cell under them: the
 *  request names the raster cell the click fell in, and the answer is the
 *  value of the grid cell it resolved to. The file's latitudes ascend, so
 *  north up the raster's top row is its last row. */
async function assertMapClicksHit(panel: RealPanel, grid: Msg, d: Layout, what: string): Promise<void> {
  const cols = grid.width as number;
  const rows = grid.height as number;
  // A mouse event is a whole pixel: along an axis under 4 px per cell it may
  // land on the neighbor.
  const slackX = d.width / cols < 4 ? 1 : 0;
  const slackY = d.height / rows < 4 ? 1 : 0;
  const at = (n: number) => [0.5 / n, (Math.floor(n / 3) + 0.5) / n, (n - 0.5) / n];
  for (const fy of at(rows)) {
    for (const fx of at(cols)) {
      const { asked, read } = await click(panel, fx, fy);
      const where = `${what}: the click at (${fx.toFixed(4)}, ${fy.toFixed(4)})`;
      assert.ok(Math.abs(asked.px - Math.floor(fx * cols)) <= slackX, `${where} asks for column ${asked.px}`);
      assert.ok(Math.abs(asked.py - Math.floor(fy * rows)) <= slackY, `${where} asks for row ${asked.py}`);
      assert.ok(read && read.value !== null, `${where} reads a value, not off the grid`);
      assert.strictEqual(read.value, valueAt(read.gridJ, read.gridI), `${where} reads the grid cell it resolved to`);
      assert.ok(
        Math.abs(read.gridI - asked.px) <= 1 && Math.abs(read.gridJ - (rows - 1 - asked.py)) <= 1,
        `${where} resolved to grid cell (${read.gridI}, ${read.gridJ}), not near (${asked.px}, ${rows - 1 - asked.py})`,
      );
    }
  }
}

suite("A lat/lon map keeps its own shape (#1226)", function () {
  this.timeout(120_000);

  const steps = (from: number, step: number, n: number) => Array.from({ length: n }, (_, k) => from + step * k);

  /** A regular lat/lon field over `lats` × `lons`, with no two cells alike. */
  function mapFile(lats: number[], lons: number[]): Buffer {
    return classicNetcdf(
      [["lat", lats.length], ["lon", lons.length]],
      [
        { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: lats },
        { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: lons },
        { name: "t", dims: ["lat", "lon"], attrs: { units: "K" }, values: values(lats.length, lons.length) },
      ],
    );
  }

  /** Draw `t` from {@link mapFile} on `projection`; the render and the image
   *  as laid out. */
  async function onMap(
    lats: number[],
    lons: number[],
    projection: string,
    body: (grid: Msg, drawn: Layout, panel: RealPanel) => void | Promise<void>,
  ): Promise<void> {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1226-"));
    const file = path.join(dir, "map.nc");
    fs.writeFileSync(file, mapFile(lats, lons));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const t = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "t");
    assert.ok(t, "t is renderable");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: t.index }));
    try {
      let grid = await panel.waitSent(isType("gridReady"), 0, "the first render");
      if (projection !== "source") {
        const from = panel.sent.length;
        await panel.drive([{ id: "picker-projection", value: projection }]);
        grid = await panel.waitSent(
          (m) => m.type === "gridReady" && m.options.projection === projection,
          from,
          `the map on ${projection}`,
        );
      }
      await body(grid, await laidOut(panel), panel);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  // A belt from 20° S to 20° N round the globe at 2.5°: 144 × 17, about 8.5:1.
  const BELT_LATS = steps(-20, 2.5, 17);
  const BELT_LONS = steps(0, 2.5, 144);

  test("a belt round the globe on Equirectangular keeps its 8.5:1, not held to 8:1", async () => {
    await onMap(BELT_LATS, BELT_LONS, "equirectangular", (grid, d) => {
      const raster = grid.width / grid.height;
      assert.ok(raster > MOST, `the raster is ${grid.width} × ${grid.height}, wider than ${MOST}:1`);
      assert.ok(
        Math.abs(d.width / d.height - raster) / raster < 0.03,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at the raster's ${raster.toFixed(2)}:1`,
      );
    });
  });

  test("the same belt in the source view, not a map target, is held to 8:1", async () => {
    await onMap(BELT_LATS, BELT_LONS, "source", (grid, d) => {
      assert.deepStrictEqual([grid.width, grid.height], [144, 17], "the source raster is the array");
      assertPaintable(d, "the source view");
      assert.ok(
        Math.abs(d.width / d.height - MOST) < 0.1,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at ${MOST}:1`,
      );
    });
  });

  test("a tall strip of longitude on Equirectangular keeps its shape and fits the page", async () => {
    // 90° S to 90° N by 10° of longitude at 1.25°: 9 × 145, 1:16.
    await onMap(steps(-90, 1.25, 145), steps(0, 1.25, 9), "equirectangular", (grid, d) => {
      const raster = grid.height / grid.width;
      assert.ok(raster > MOST, `the raster is ${grid.width} × ${grid.height}, taller than 1:${MOST}`);
      assertWithinPage(d, "the tall strip");
      // A whole pixel either way, on an image only a few dozen pixels wide.
      assert.ok(
        Math.abs(d.width - d.height / raster) <= 1,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at the raster's 1:${raster.toFixed(2)}`,
      );
    });
  });

  test("an ordinary global map fills its row, up to the 80vh cap (#1252)", async () => {
    // 90° S to 90° N round the globe at 5°: 72 × 37, about 2:1.
    await onMap(steps(-90, 5, 37), steps(0, 5, 72), "equirectangular", (grid, d) => {
      const raster = grid.width / grid.height;
      assert.ok(
        Math.abs(d.width / d.height - raster) / raster < 0.03,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at the raster's ${raster.toFixed(2)}:1`,
      );
      assertAcrossRow(d, raster, "the global map");
    });
  });

  test("a map 2 columns wide and 720 rows tall is drawn a few pixels wide, and a click reads its cells (#1251)", async () => {
    // Pole to pole at 0.25 degrees down two columns: 2 × 720. Drawn at
    // its own aspect it was under a pixel wide, and nothing could be clicked.
    await onMap(steps(-89.875, 0.25, 720), [0, 0.25], "equirectangular", async (grid, d, panel) => {
      assert.deepStrictEqual([grid.width, grid.height], [2, 720], "the raster is the 2 × 720 map");
      assertWithinPage(d, "the 2 × 720 map");
      assert.ok(d.width >= 6, `the map is ${d.width.toFixed(1)} px wide, too narrow to see or click`);
      await assertMapClicksHit(panel, grid, d, "the 2 × 720 map");
      // Export PNG writes the raster at its own size, not the drawn box's: the
      // map is 720 px tall in it, with the title and color bar around it.
      const png = await exportedPng(panel);
      assert.ok(
        png.height >= 720 && png.height < 720 + 400,
        `the PNG is ${png.width} × ${png.height} px, not the 2 × 720 raster with its labels`,
      );
    });
  });

  test("a map 720 columns wide and 2 rows tall is drawn a few pixels tall (#1251)", async () => {
    await onMap([0, 0.25], steps(0.25, 0.5, 720), "equirectangular", async (grid, d, panel) => {
      assert.deepStrictEqual([grid.width, grid.height], [720, 2], "the raster is the 720 × 2 map");
      assert.ok(d.height >= 6, `the map is ${d.height.toFixed(1)} px tall, too thin to see or click`);
      await assertMapClicksHit(panel, grid, d, "the 720 × 2 map");
    });
  });

  test("a tall map short of 1:8 keeps its shape and fits the page (#1252)", async () => {
    // 90° S to 90° N by 30° of longitude at 1.25°: 25 × 145, about 1:5.8.
    await onMap(steps(-90, 1.25, 145), steps(0, 1.25, 25), "equirectangular", (grid, d) => {
      const raster = grid.height / grid.width;
      assert.ok(raster < MOST, `the raster is ${grid.width} × ${grid.height}, not taller than 1:${MOST}`);
      assertWithinPage(d, "the 1:5.8 strip");
      assert.ok(
        Math.abs(d.height / d.width - raster) / raster < 0.03,
        `drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, not at the raster's 1:${raster.toFixed(2)}`,
      );
    });
  });
});

suite("displayAspect (#1226)", () => {
  test("leaves an aspect up to eight to one either way to the raster", () => {
    for (const [w, h] of [
      [1, 1],
      [360, 181],
      [1440, 721],
      [181, 360],
      [8, 1],
      [1, 8],
      [73, 366],
    ]) {
      assert.strictEqual(displayAspect(w, h), "", `${w} × ${h}`);
    }
  });

  test("holds a longer one to eight to one", () => {
    assert.strictEqual(displayAspect(2, 366), "1 / 8");
    assert.strictEqual(displayAspect(366, 2), "8 / 1");
    assert.strictEqual(displayAspect(1, 400), "1 / 8");
    assert.strictEqual(displayAspect(400, 1), "8 / 1");
    assert.strictEqual(displayAspect(9, 1), "8 / 1");
  });

  test("leaves an empty or unusable size alone", () => {
    for (const [w, h] of [
      [0, 0],
      [0, 5],
      [5, 0],
      [Number.NaN, 2],
    ]) {
      assert.strictEqual(displayAspect(w, h), "", `${w} × ${h}`);
    }
  });
});
