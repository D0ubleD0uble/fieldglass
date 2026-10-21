// Arrows are never drawn on a stretched picture (#1251).
//
// Follow-on from #1226, which holds a raster longer than 8:1 at 8:1 on a view
// whose shape carries no geographic meaning (the source projection). A GRIB
// source view of a grid that thin with Vectors on was stretched too, but Rust
// lays the arrows out in square raster pixels: stretched, every arrow not
// along the stretched axis changed direction and length, and the arrow key,
// which is scaled by the horizontal axis alone, gave the wrong speed for it.
//
// Now the picture is not stretched while arrows are drawn, and if it still is
// not square (a few pixels wide, rounded) the key is left off and says why.
//
// Driven through the real panel over GRIB2 messages built here: a constant
// u/v pair on a regular lat/lon grid, so every arrow points the same way and
// the angle and length it should have in the picture can be worked out.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { VECTOR_KEY_STRETCHED, VECTOR_KEY_LEFT_OFF } from "../../render-panel";
import { isType, type Msg, openEditor, openReal, provider, type PageState } from "./real-panel";

/** One GRIB2 message: a constant field on an `ni` × `nj` regular lat/lon grid
 *  of a tenth of a degree, scanned west to east, north to south, with parameter
 *  `category`-`number` (2-2 is u, 2-3 is v). Simple packing at 0 bits per
 *  value, so the field is its reference value. */
function message(ni: number, nj: number, category: number, number: number, value: number): Buffer {
  const s1 = Buffer.alloc(21);
  s1.writeUInt32BE(21, 0);
  s1[4] = 1;
  s1.writeUInt16BE(7, 5);
  s1[9] = 1; // master table version
  s1[11] = 1; // reference time: start of forecast
  s1.writeUInt16BE(2026, 12);
  s1[14] = 1;
  s1[15] = 1;

  const s3 = Buffer.alloc(72);
  s3.writeUInt32BE(72, 0);
  s3[4] = 3;
  s3.writeUInt32BE(ni * nj, 6);
  s3[14] = 6; // shape of the earth: sphere of 6,371,229 m
  s3.writeUInt32BE(ni, 30);
  s3.writeUInt32BE(nj, 34);
  s3.writeUInt32BE(0xffffffff, 42);
  s3.writeUInt32BE(45_000_000, 46); // La1
  s3.writeUInt32BE(0, 50); // Lo1
  s3[54] = 0x30;
  s3.writeUInt32BE(45_000_000 - (nj - 1) * 100_000, 55); // La2
  s3.writeUInt32BE((ni - 1) * 100_000, 59); // Lo2
  s3.writeUInt32BE(100_000, 63); // Di
  s3.writeUInt32BE(100_000, 67); // Dj
  s3[71] = 0; // scanning mode

  const s4 = Buffer.alloc(34);
  s4.writeUInt32BE(34, 0);
  s4[4] = 4;
  s4[9] = category;
  s4[10] = number;
  s4[17] = 1; // time unit: hour
  s4[22] = 103; // 10 m above ground
  s4.writeUInt32BE(10, 24);
  s4[28] = 255;

  const s5 = Buffer.alloc(21);
  s5.writeUInt32BE(21, 0);
  s5[4] = 5;
  s5.writeUInt32BE(ni * nj, 5);
  s5.writeFloatBE(value, 11);
  s5[19] = 0; // bits per value

  const s6 = Buffer.from([0, 0, 0, 6, 6, 255]);
  const s7 = Buffer.from([0, 0, 0, 5, 7]);
  const body = Buffer.concat([s1, s3, s4, s5, s6, s7, Buffer.from("7777")]);
  const s0 = Buffer.alloc(16);
  s0.write("GRIB", 0, "latin1");
  s0[7] = 2; // edition
  s0.writeBigUInt64BE(BigInt(16 + body.length), 8);
  return Buffer.concat([s0, body]);
}

/** A file of two messages: u = 6 and v = 3 on an `ni` × `nj` grid, so an arrow
 *  rises one unit for every two it runs east. */
function pairFile(ni: number, nj: number): { dir: string; file: string } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1251-"));
  const file = path.join(dir, "thin.grib2");
  fs.writeFileSync(file, Buffer.concat([message(ni, nj, 2, 2, 6), message(ni, nj, 2, 3, 3)]));
  return { dir, file };
}

const NOTES = [VECTOR_KEY_STRETCHED, VECTOR_KEY_LEFT_OFF];

/** Open the pair on the source view, arrows on, and read the page once the
 *  arrows have been drawn. */
async function arrowsOnSource(
  ni: number,
  nj: number,
  body: (state: PageState, grid: Msg, result: Msg) => void,
): Promise<void> {
  const { dir, file } = pairFile(ni, nj);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  try {
    // A GRIB lat/lon grid opens on the source view: the raster is the array.
    const grid = await panel.waitSent(isType("gridReady"), 0, "the first render");
    const from = panel.sent.length;
    await panel.drive([
      { id: "vector-field-v", value: 1 },
      { id: "overlay-vectors", checked: true },
    ]);
    const result = await panel.waitSent(isType("vectorResult"), from, "the arrows");
    assert.ok(!result.error, `the arrows: ${result.error}`);
    // Let the page settle on the answer: a round trip through it handles
    // everything posted before it.
    const state = await panel.drive([]);
    body(state, grid, result);
  } finally {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

/** The overlay's backing pixels per raster pixel, across and down. */
function scales(state: PageState, grid: Msg): [number, number] {
  const key = state.vectorKey;
  const canvas = key ? key.canvas : undefined;
  assert.ok(canvas, "the overlay is measured through the key it drew");
  return [canvas.width / grid.width, canvas.height / grid.height];
}

suite("Arrows are not drawn on a stretched picture (#1251)", function () {
  this.timeout(120_000);

  test("a source view longer than 8:1 keeps square pixels with arrows on, and its key", async () => {
    await arrowsOnSource(240, 20, (state, grid) => {
      assert.deepStrictEqual([grid.width, grid.height], [240, 20], "the source raster is the 12:1 grid");
      assert.ok(state.image, "the image is laid out");
      assert.ok(state.vectorKey, `a key is drawn on a picture with square pixels: "${state.text}"`);
      const [sx, sy] = scales(state, grid);
      assert.ok(
        Math.abs(sx / sy - 1) <= 0.05,
        `a raster pixel is ${sx.toFixed(2)} × ${sy.toFixed(2)} px on screen, not square`,
      );
    });
  });

  test("an arrow points on screen the way Rust laid it out in raster pixels", async () => {
    await arrowsOnSource(240, 20, (state, grid, result) => {
      const xy: ArrayLike<number> = result.xy;
      const segs: ArrayLike<number> = result.segLengths;
      assert.ok(segs.length > 0, "arrows are drawn");
      // The first arrow's tail and tip, in raster pixels, then on screen.
      const raster = Math.atan2(xy[1] - xy[3], xy[2] - xy[0]);
      const [sx, sy] = scales(state, grid);
      const screen = Math.atan2((xy[1] - xy[3]) * sy, (xy[2] - xy[0]) * sx);
      const deg = (r: number) => ((r * 180) / Math.PI).toFixed(1);
      assert.ok(
        Math.abs(screen - raster) < 0.02,
        `the first arrow points ${deg(raster)} degrees in the raster and ${deg(screen)} on screen`,
      );
    });
  });

  test("a source view far past 8:1 either has square pixels or no key, and says why", async () => {
    // 4 × 400 is 1:100, a column a few pixels wide on a normal page: too narrow
    // for its pixels to be square after rounding, or for a key to fit.
    await arrowsOnSource(4, 400, (state, grid) => {
      assert.deepStrictEqual([grid.width, grid.height], [4, 400]);
      if (state.vectorKey) {
        const [sx, sy] = scales(state, grid);
        assert.ok(Math.abs(sx / sy - 1) <= 0.05, `a key is drawn on a ${sx.toFixed(2)} × ${sy.toFixed(2)} px stretch`);
      } else {
        assert.ok(
          NOTES.some((n) => state.text.includes(n)),
          `no key and no reason on the page: "${state.text}"`,
        );
      }
    });
  });
});
