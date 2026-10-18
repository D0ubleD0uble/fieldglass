// A refused arrow pair says so on a line of its own (#1051).
//
// From the 0.6.0 manual pass (§6.9): with message 0 drawn and Arrows on,
// picking a V component whose frame differs is refused, and picking a good one
// then drew the arrows with the refusal still beside them, because arrows and
// contours shared one status line that only the contour path ever cleared.
//
// Driven through the real panel. The refused pair is three GFS messages where
// the middle one states its components along the grid's axes (Flag Table 3.3
// bit 5, the 0x08 resolution-flags bit of §3), as `samples/0.6.0/hrrr_frames.grib2`
// does: #0 with #1 is refused (#805), #0 with #2 draws.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

const REFUSAL = /^Vectors: .*component frame differs \(u: earth-relative, v: grid-relative\)/;

function gfs(gridRelative: boolean): Buffer {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  const msg = Buffer.from(fs.readFileSync(p));
  let at = 16;
  while (msg[at + 4] !== 3) at += msg.readUInt32BE(at);
  // §3 octet 55: the resolution and component flags.
  if (gridRelative) msg[at + 54] |= 0x08;
  else msg[at + 54] &= ~0x08;
  // Parameter 2-2 / 2-3, so the pair is a recognised u and v.
  let s4 = 16;
  while (msg[s4 + 4] !== 4) s4 += msg.readUInt32BE(s4);
  msg[s4 + 9] = 2;
  msg[s4 + 10] = 2;
  return msg;
}

const latest = (panel: RealPanel, type: string): Msg => panel.received.filter(isType(type)).pop();

/** Wait for the reply to the most recent request of `type`. */
async function reply(panel: RealPanel, request: string, answer: string, what: string): Promise<Msg> {
  const seq = latest(panel, request).seq;
  return panel.waitSent((m: Msg) => isType(answer)(m) && m.seq === seq, 0, what);
}

suite("Arrow refusal has its own status line (#1051)", () => {
  test("refusal, then success, then contour and untick leave each other alone", async function () {
    this.timeout(180_000);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-vector-status-"));
    const file = path.join(dir, "frames.grib2");
    fs.writeFileSync(file, Buffer.concat([gfs(false), gfs(true), gfs(false)]));
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // #0 with #1: refused, and the refusal is on screen.
      await panel.drive([
        { id: "vector-field-v", value: 1 },
        { id: "overlay-vectors", checked: true },
      ]);
      const refused = await reply(panel, "vectorRequest", "vectorResult", "the refusal");
      assert.match(refused.error, /component frame differs/);
      let state = await panel.drive([]);
      assert.match(state.vectorStatus, REFUSAL);
      assert.strictEqual(state.contourStatus, "", "not on the contour line");

      // A contour success does not clear it.
      state = await panel.drive([{ id: "overlay-contours", checked: true }]);
      await reply(panel, "contourRequest", "contourReady", "the contours");
      state = await panel.drive([]);
      assert.match(state.vectorStatus, REFUSAL, "a contour success leaves the arrow refusal");
      assert.strictEqual(state.contourStatus, "");

      // A contour refusal shows beside it, without replacing it.
      await panel.post({ type: "contourError", seq: latest(panel, "contourRequest").seq, error: "no isolines here" });
      state = await panel.drive([]);
      assert.strictEqual(state.contourStatus, "Contours: no isolines here");
      assert.match(state.vectorStatus, REFUSAL, "a contour refusal leaves the arrow refusal");

      // #0 with #2: the arrows draw, the refusal goes, the contour line stays.
      await panel.drive([{ id: "vector-field-v", value: 2 }]);
      const drawn = await reply(panel, "vectorRequest", "vectorResult", "the good pair");
      assert.ok(!drawn.error, drawn.error);
      state = await panel.drive([]);
      assert.strictEqual(state.vectorStatus, "", "a valid pair clears the refusal");
      assert.strictEqual(state.contourStatus, "Contours: no isolines here", "an arrow success leaves the contour refusal");

      // Back to the refused pair, then untick Arrows: the message goes.
      await panel.drive([{ id: "vector-field-v", value: 1 }]);
      await reply(panel, "vectorRequest", "vectorResult", "the refusal again");
      state = await panel.drive([]);
      assert.match(state.vectorStatus, REFUSAL);
      state = await panel.drive([{ id: "overlay-vectors", checked: false }]);
      assert.strictEqual(state.vectorStatus, "", "unticking Arrows clears the message");
      assert.strictEqual(state.contourStatus, "Contours: no isolines here");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
