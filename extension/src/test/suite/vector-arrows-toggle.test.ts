// Ticking Arrows moves nothing, and the V component starts on v (#1843).
//
// From the 0.6.2 manual pass. With a u/v pair drawn, ticking Arrows gave the
// Vectors row an empty second line once the arrows' answer landed, and Color
// Range and the map moved down by it; unticking took it back. #1715 holds the
// row still up to the answer, so the check here waits for the answer and for
// the row's room to settle. And the V component started on the panel's own
// message, so the first tick drew u against itself under a "not a u/v pair"
// note; it now starts on the message that pairs with it, where the file has
// one.
//
// Driven through the real panel and the real provider. No committed GRIB file
// holds a wind component, so the pairs are committed fixtures with their §4
// parameter octets rewritten, as vector-pairing.test.ts builds them.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, openEditor, openReal, type PageState, provider, type RealPanel } from "./real-panel";

function fixture(name: string): Buffer {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", name);
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return fs.readFileSync(p);
}

/** The offset of a GRIB2 message's section `number`. */
function section(msg: Buffer, number: number): number {
  let at = 16;
  while (msg.toString("ascii", at, at + 4) !== "7777") {
    if (msg[at + 4] === number) return at;
    at += msg.readUInt32BE(at);
  }
  throw new Error(`no section ${number}`);
}

/** Message `name` as discipline-0 parameter `category`-`number`, optionally
 *  `hours` ahead (octets 19-22 of §4). */
function as(name: string, category: number, number: number, hours?: number): Buffer {
  const msg = Buffer.from(fixture(name));
  const s4 = section(msg, 4);
  msg[s4 + 9] = category;
  msg[s4 + 10] = number;
  if (hours !== undefined) msg.writeUInt32BE(hours, s4 + 18);
  return msg;
}

const GFS = "gfs_c255_latlon.grib2";
const HRRR = "hrrr_complex_spd_lambert.grib2";
const UGRD = [2, 2] as const;
const VGRD = [2, 3] as const;
const TMP = [0, 0] as const;

function file(name: string, ...messages: Buffer[]): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1843-"));
  const out = path.join(dir, name);
  fs.writeFileSync(out, Buffer.concat(messages));
  return out;
}

/** The page `px` wide, as the narrow-panel tests set it (#1429). */
const pageWidth = (px: number) => ({ id: "body", width: `calc(${px}px - 3rem)` });

async function open(file: string, message: number): Promise<{ panel: RealPanel; close: () => void }> {
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: message }));
  const close = () => {
    panel.dispose();
    editor.close();
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
  } catch (e) {
    close();
    throw e;
  }
  return { panel, close };
}

/** Where the rows under the Vectors row and the map sit, from the toolbar's
 *  top, with the Vectors row's height. */
function where(s: PageState) {
  assert.ok(s.vectorRow, "the Vectors row is shown");
  const top = s.toolbar.box.top;
  return {
    toolbarHeight: Math.round(s.toolbar.box.bottom - top),
    image: Math.round(s.image.top - top),
    vectorRow: Math.round(s.vectorRow.height),
  };
}

/** The page once the arrows asked for since `from` have landed and the row's
 *  room, which is measured a frame later on a resize, has settled. */
async function landed(panel: RealPanel, from: number, what: string): Promise<PageState> {
  await panel.waitSent(isType("vectorResult"), from, `${what}: the arrows`);
  let last = await panel.drive([]);
  for (let i = 0; i < 20; i++) {
    await new Promise((r) => setTimeout(r, 100));
    const now = await panel.drive([]);
    const same = JSON.stringify(where(now)) === JSON.stringify(where(last));
    last = now;
    if (same) break;
  }
  return last;
}

suite("Ticking Arrows moves nothing, and V starts on v (#1843)", function () {
  this.timeout(180_000);

  test("a u/v pair: the arrows' answer adds no line to the Vectors row, and Color Range and the map stay", async () => {
    const pair = file("gfs_wind.grib2", as(GFS, ...UGRD), as(GFS, ...VGRD));
    const { panel, close } = await open(pair, 0);
    try {
      // Picked, so this holds whatever V starts on.
      await panel.drive([{ id: "vector-field-v", value: 1 }]);
      for (const px of [1800, 1200]) {
        await panel.drive([pageWidth(px)]);
        const before = where(await panel.drive([]));

        for (const round of ["tick", "re-tick"]) {
          const from = panel.sent.length;
          await panel.drive([{ id: "overlay-vectors", checked: true }]);
          const on = await landed(panel, from, `${px} px, ${round}`);
          assert.strictEqual(on.vectorPairNote, "", `${px} px, ${round}: a u/v pair gets no note`);
          assert.deepStrictEqual(where(on), before, `${px} px, ${round}: Arrows on moved the rows or the map`);
          const back = await panel.drive([{ id: "overlay-vectors", checked: false }]);
          assert.deepStrictEqual(where(back), before, `${px} px, ${round}: Arrows off moved the rows or the map`);
        }
      }
    } finally {
      close();
      fs.rmSync(path.dirname(pair), { recursive: true, force: true });
    }
  });

  test("V starts on the message that pairs with u, on a global and a Lambert grid", async () => {
    for (const grid of [GFS, HRRR]) {
      // Temperature first and a v six hours on before the v that matches, so
      // neither the first option nor the first VGRD is the answer by luck.
      const f = file("wind.grib2", as(grid, ...TMP), as(grid, ...UGRD), as(grid, ...VGRD, 6), as(grid, ...VGRD));
      try {
        const { panel, close } = await open(f, 1);
        try {
          assert.strictEqual((await panel.controls())["vector-field-v"], "3", `${grid}: V starts on #3, the v at u's time`);
          const from = panel.sent.length;
          await panel.drive([{ id: "overlay-vectors", checked: true }]);
          const on = await landed(panel, from, grid);
          const result = panel.sent.filter(isType("vectorResult")).pop();
          assert.strictEqual(result.pairing, "recognized", `${grid}: the first tick draws a u/v pair`);
          assert.strictEqual(on.vectorPairNote, "", `${grid}: with no note`);
        } finally {
          close();
        }

        // V's own panel: its u is the partner, the wrong way round, so
        // nothing pairs and V starts where it always has.
        const v = await open(f, 3);
        try {
          assert.strictEqual((await v.panel.controls())["vector-field-v"], "0", `${grid}: a v panel keeps the first message`);
        } finally {
          v.close();
        }
      } finally {
        fs.rmSync(path.dirname(f), { recursive: true, force: true });
      }
    }
  });

  test("with no message to pair, V starts on the first message as before", async () => {
    const f = file("no_pair.grib2", as(GFS, ...UGRD), as(GFS, ...TMP), as(GFS, ...VGRD, 6));
    try {
      const { panel, close } = await open(f, 0);
      try {
        assert.strictEqual((await panel.controls())["vector-field-v"], "0");
      } finally {
        close();
      }
    } finally {
      fs.rmSync(path.dirname(f), { recursive: true, force: true });
    }
  });
});
