// The Vectors row says when the arrows' u and v are not a u/v pair (#1049).
//
// From the 0.6.0 manual pass (§6): the V component picker takes any message in
// the file, so wind speed paired with itself draws arrows as confident as a
// real pair's. Drawing stays permitted; the row now says so. Whether two
// messages pair is Rust's answer, read from the parameter tables
// (`Session::vector_pairing`), and reaches the page as
// `ProjectedVectors.pairing`.
//
// Driven through the real panel on committed fixtures. No committed GRIB file
// holds a wind component, so the UGRD/VGRD pair is the GFS fixture with its §4
// parameter octets rewritten, as the release plan builds its samples, and the
// WIND + WIND pair is CMC's wind speed twice over, as `samples/0.6.0/cmc_pair.grib` is.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { composeVectorPairingNote } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

function fixture(crate: string, name: string): Buffer {
  const p = path.join(extensionPath(), "..", "crates", crate, "tests", "fixtures", name);
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

/** The GFS message as discipline-0 parameter `category`-`number`. */
function gfsAs(category: number, number: number): Buffer {
  const msg = Buffer.from(fixture("fieldglass-grib2", "gfs_c255_latlon.grib2"));
  const s4 = section(msg, 4);
  msg[s4 + 9] = category;
  msg[s4 + 10] = number;
  return msg;
}

function file(name: string, ...messages: Buffer[]): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-vector-pairing-"));
  const out = path.join(dir, name);
  fs.writeFileSync(out, Buffer.concat(messages));
  return out;
}

const mark = (panel: RealPanel) => ({ sent: panel.sent.length, received: panel.received.length });

/** Open message `u` of `path`, pick message `v` as the V component, tick
 *  Arrows, and read the result and the page once it has landed. */
async function arrows(path: string, u: number, v: number) {
  const p = await provider();
  const editor = await openEditor(p, path);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: u }));
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    const from = mark(panel);
    await panel.drive([
      { id: "vector-field-v", value: v },
      { id: "overlay-vectors", checked: true },
    ]);
    await panel.waitSent(isType("vectorResult"), from.sent, `the arrows for #${u} with #${v}`);
    let result: Msg;
    let state;
    for (;;) {
      state = await panel.drive([]);
      const request = panel.received.filter(isType("vectorRequest")).pop();
      result = panel.sent.find((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq);
      if (result) break;
      await panel.waitSent((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq, from.sent, "the latest arrows");
    }
    assert.ok(!result.error, `#${u} with #${v}: ${result.error}`);
    // Ticked off, the note goes with the arrows.
    const off = await panel.drive([{ id: "overlay-vectors", checked: false }]);
    return { result, state, off };
  } finally {
    panel.dispose();
    editor.close();
  }
}

suite("Vector pairing note (#1049)", () => {
  test("UGRD with VGRD: no note; VGRD as u: the swapped-order note", async function () {
    this.timeout(120_000);
    const pair = file("gfs_wind.grib2", gfsAs(2, 2), gfsAs(2, 3));
    try {
      const right = await arrows(pair, 0, 1);
      assert.strictEqual(right.result.pairing, "recognised");
      assert.deepStrictEqual(right.result.pairNames, ["UGRD", "VGRD"]);
      assert.strictEqual(right.state.vectorPairNote, "", "a u/v pair gets no note");
      assert.ok(right.result.segLengths.length > 0, "and its arrows draw");

      const swapped = await arrows(pair, 1, 0);
      assert.strictEqual(swapped.result.pairing, "swapped");
      const note = composeVectorPairingNote("swapped", "VGRD", "UGRD");
      assert.ok(note.includes("wrong way round"), note);
      assert.strictEqual(swapped.state.vectorPairNote, note, "the swapped pair says so");
      assert.ok(swapped.state.text.includes(note), "and the note is shown");
      assert.ok(swapped.result.segLengths.length > 0, "the arrows still draw");
      assert.ok(swapped.state.vectorKey, "with their key");
      assert.strictEqual(swapped.off.vectorPairNote, "", "unticking Arrows clears the note");
    } finally {
      fs.rmSync(path.dirname(pair), { recursive: true, force: true });
    }
  });

  test("WIND with WIND: the not-a-pair note, and the arrows still draw", async function () {
    this.timeout(120_000);
    const cmc = fixture("fieldglass-grib1", "cmc_wind_300_2010052400_p012.grib");
    const pair = file("cmc_pair.grib", cmc, cmc);
    try {
      const wind = await arrows(pair, 0, 1);
      assert.strictEqual(wind.result.pairing, "not_a_pair");
      const note = composeVectorPairingNote("not_a_pair", "WIND", "WIND");
      assert.strictEqual(note, "These messages aren't a u/v pair (WIND, WIND). The arrows treat them as the u and v components.");
      assert.strictEqual(wind.state.vectorPairNote, note);
      assert.ok(wind.state.text.includes(note), "the note is shown");
      assert.ok(wind.result.segLengths.length > 0, "the arrows still draw");
      assert.ok(wind.state.vectorKey, "with their key");
      // The grid-axes note of #241 is a separate line, and still there.
      assert.match(wind.state.vectorNote ?? "", /rotated to true north/);
      assert.strictEqual(wind.off.vectorPairNote, "", "unticking Arrows clears the note");
    } finally {
      fs.rmSync(path.dirname(pair), { recursive: true, force: true });
    }
  });
});
