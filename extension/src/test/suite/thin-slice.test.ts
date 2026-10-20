// A very thin 2-D slice is drawn at a size the panel can paint (#1226).
//
// `time_bnds` in the 0.6.0 sample `ncep_air_daily_2024.nc` is 366 × 2. The
// image was sized to the panel's width at the raster's own aspect, so a
// 2-wide raster became about 775 × 141,000 px, past what the page would
// paint: a blank box with a broken-image glyph. Turned the other way, 366 × 2
// was a strip 4 px tall whose rows were a pixel or two each. Now the image
// is never drawn more than eight times as long one way as the other, so its
// cells stretch to stay visible, and a click still reads the cell under it.
// An ordinary aspect keeps the raster's own.
//
// Driven through the real panel over a classic NetCDF file written for the
// test: the box measured is the page's own canvas, and the cell read is the
// one the provider answers for.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { displayAspect } from "../../render-panel";
import {
  type Box,
  classicNetcdf,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

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
 *  and an ordinary 2:1 field. No coordinate variables, so the rows are drawn
 *  in index order. */
function thinFile(): Buffer {
  return classicNetcdf(
    [["time", 366], ["nbnds", 2], ["one", 1], ["n", 400], ["y", 20], ["x", 40]],
    [
      { name: "time_bnds", dims: ["time", "nbnds"], attrs: {}, values: values(366, 2) },
      { name: "bnds_time", dims: ["nbnds", "time"], attrs: {}, values: values(2, 366) },
      { name: "strip", dims: ["one", "n"], attrs: {}, values: values(1, 400) },
      { name: "ordinary", dims: ["y", "x"], attrs: {}, values: values(20, 40) },
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
  close: () => void;
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
    // The image laid out at the size the render gave it.
    let image: Box | undefined;
    for (let i = 0; i < 50; i++) {
      const state = await panel.drive([]);
      image = state.rails?.image;
      if (image && image.right - image.left > 2 && image.bottom - image.top > 2) break;
      await new Promise((r) => setTimeout(r, 20));
    }
    assert.ok(image, "the slice panel reports its image");
    return {
      panel,
      grid,
      width: image.right - image.left - 2,
      height: image.bottom - image.top - 2,
      close,
    };
  } catch (err) {
    close();
    throw err;
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
function assertPaintable(d: Drawn, what: string): void {
  assert.ok(
    d.height <= MOST * d.width + 1 && d.width <= MOST * d.height + 1,
    `${what}: the image is drawn ${d.width.toFixed(1)} × ${d.height.toFixed(1)} px, ` +
      `more than ${MOST} times as long one way as the other`,
  );
}

/** Clicks across the image read the cell drawn there, by the request's cell
 *  and the value the provider answers with. A mouse event's position is a
 *  whole pixel, so along an axis whose cells are under 4 px wide (366 cells
 *  across the panel) the click may land on the neighbouring cell; along a
 *  stretched axis, which this fix makes, it must be the cell itself. */
async function assertClicksHit(d: Drawn, what: string): Promise<void> {
  const cols = d.grid.width as number;
  const rows = d.grid.height as number;
  const flip = d.grid.options?.flipY === true;
  const slackX = d.width / cols < 4 ? 1 : 0;
  const slackY = d.height / rows < 4 ? 1 : 0;
  // Cell centres: the first and last of each axis, and one between.
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
    } finally {
      d.close();
    }
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
