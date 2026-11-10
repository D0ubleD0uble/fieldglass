// The Vectors row keeps its height through a zoom (#1309, after #1228, #1255).
//
// "Full-length arrow: …" and the arrow key's note both follow the window: the
// speed a full-length arrow stands for changes with the arrows in view, and
// is empty when none is drawn, and the key's note says when the map is too
// small for it. In a panel narrow enough to wrap the Vectors row, a tick that
// changes either re-wrapped the row and moved the map under a pointer held
// still, so the next tick zoomed on another place.
//
// Driven through the real panel on a u/v pair of GFS messages. A tick's
// arrows come back from the provider with the speed the test chooses (a long
// number, none, a short one), since which speed a window gives is the
// library's business and is not what is pinned here: that the row, wherever
// it wraps, does not move the map.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel, waitUntil } from "./real-panel";

/** The GFS message as discipline-0 parameter `category`-`number`: no
 *  committed GRIB file holds a wind component (as vector-pairing.test.ts). */
function gfsAs(category: number, number: number): Buffer {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  const msg = Buffer.from(fs.readFileSync(p));
  let at = 16;
  while (msg.toString("ascii", at, at + 4) !== "7777") {
    if (msg[at + 4] === 4) {
      msg[at + 9] = category;
      msg[at + 10] = number;
      return msg;
    }
    at += msg.readUInt32BE(at);
  }
  throw new Error("no section 4");
}

/** A body `width` that makes the page `px` wide (1.5rem of padding a side). */
const pageWidth = (px: number) => ({ id: "body", width: `calc(${px}px - 3rem)` });

/** The place a click reads, and the image's box then: at `client`, or at 0.4
 *  of the way across and down the image when not given. */
async function placeAt(panel: RealPanel, client: [number, number] | undefined, what: string) {
  const sent = panel.sent.length;
  const at = client ? await panel.probeAt(client) : await panel.probe(0.4, 0.4);
  const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
  assert.ok(read && typeof read.lat === "number" && typeof read.lon === "number", `the click reads a place (${what})`);
  return { lat: read.lat as number, lon: read.lon as number, rect: at.rect, point: at.point };
}

suite("The Vectors row during a zoom (#1309)", function () {
  this.timeout(240_000);

  test("wheel ticks that change the arrow label keep the place under a pointer held still, at every width", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1309-"));
    const file = path.join(dir, "gfs_wind.grib2");
    fs.writeFileSync(file, Buffer.concat([gfsAs(2, 2), gfsAs(2, 3)]));
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    let held: ReturnType<RealPanel["hold"]> | undefined;
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      from = panel.sent.length;
      await panel.drive([
        { id: "vector-field-v", value: 1 },
        { id: "overlay-vectors", checked: true },
      ]);
      await panel.waitSent(isType("vectorResult"), from, "the arrows");

      // Each tick's arrows say a different speed: long, none, short, none.
      const speeds = [12345.6, null, 7.5, null];
      let wrapped = 0;
      for (const px of [300, 360, 420, 480, 540, 600]) {
        await panel.drive([pageWidth(px)]);
        const state = await panel.drive([]);
        assert.ok(state.vectorRow, `${px} px: the Vectors row is shown`);
        if (state.vectorRow.height > 1.5 * state.vectorRow.control) wrapped++;

        // Back to a full view, so every width starts from the same picture.
        if ((await panel.controls())["radio:bounds-mode"] !== "auto") {
          from = panel.sent.length;
          await panel.drive([{ id: "zoom-reset", click: true }]);
          await panel.waitSent(isType("gridReady"), from, `${px} px: the full view`);
        }
        const first = await placeAt(panel, undefined, `${px} px, the full view`);
        // The first tick of a gesture on Auto ticks Bounds Manual, which
        // shows the lat/lon row: that is #1228's, and at 300 px it still
        // moves the map by a row. Start from Manual.
        from = panel.sent.length;
        await panel.wheel({ client: first.point }, -120);
        await panel.waitSent(isType("gridReady"), from, `${px} px: the first tick`);
        // The Bounds line pages itself a frame after a resize, from an
        // observer: wait for the box to stop moving.
        let start = await placeAt(panel, first.point, `${px} px, before the ticks`);
        for (let settle = 0; settle < 20; settle++) {
          await new Promise((r) => setTimeout(r, 100));
          const again = await placeAt(panel, first.point, `${px} px, settling`);
          const steady = again.rect.map(Math.round).join() === start.rect.map(Math.round).join();
          start = again;
          if (steady) break;
        }
        held = panel.hold((m) => m.type === "vectorResult" && !m.byTest);
        for (let i = 0; i < speeds.length; i++) {
          const asked = panel.received.filter(isType("vectorRequest")).length;
          const sent = panel.sent.length;
          await panel.wheel({ client: start.point }, -120);
          const drawn = await panel.waitSent(isType("gridReady"), sent, `${px} px: tick ${i + 1}'s render`);
          const request = await waitUntil(
            () => panel.received.filter(isType("vectorRequest")).slice(asked).pop(),
            `${px} px: tick ${i + 1}'s arrows request`,
          );
          const real = await waitUntil(
            () => held?.held.find((m) => m.seq === request.seq),
            `${px} px: tick ${i + 1}'s arrows`,
          );
          assert.ok(!real.error, `${px} px: the arrows draw: ${real.error}`);
          await panel.post({ ...real, referenceSpeed: speeds[i], byTest: true });
          assert.ok(drawn.usedLonMax - drawn.usedLonMin < 360, `${px} px: tick ${i + 1} zoomed`);
          const now = await placeAt(panel, start.point, `${px} px, tick ${i + 1}`);
          assert.deepStrictEqual(
            now.rect.map(Math.round),
            start.rect.map(Math.round),
            `${px} px: tick ${i + 1} (speed ${speeds[i]}) moved the map`,
          );
        }
        const end = await placeAt(panel, start.point, `${px} px, after the ticks`);
        const drawnNow = panel.sent.filter(isType("gridReady")).pop();
        const pixel = ((drawnNow.usedLonMax - drawnNow.usedLonMin) / drawnNow.width) * 2;
        assert.ok(
          Math.abs(end.lon - start.lon) <= pixel && Math.abs(end.lat - start.lat) <= pixel,
          `${px} px: the place under the pointer holds: ${start.lat}, ${start.lon} before, ${end.lat}, ${end.lon} after`,
        );
        await held.release();
        held = undefined;
      }
      assert.ok(wrapped >= 2, `the Vectors row wraps at some of these widths (${wrapped})`);
    } finally {
      await held?.release();
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
