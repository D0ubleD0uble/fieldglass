// The Vectors row is offered only when another message could be V (#1856).
//
// The row used to appear whenever a file held two messages, so a file of two
// temperature fields on different grids (samples/0.6.0/two_grids.grib2) showed
// arrows with nothing to pair. These drive a real panel over real bytes: the
// row is absent where no other message shares the drawn grid, and present
// where one does, a u/v pair or not.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, openEditor, openReal, provider } from "./real-panel";

const FIXTURES = path.join(extensionPath(), "src", "test", "fixtures");
const LATLON = path.join(FIXTURES, "regular_latlon_surface.grib2");
const LAMBERT = path.join(FIXTURES, "eta_lambert_msg0.grib2");

async function stateOf(file: string, messageIndex: number) {
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex }));
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    return await panel.drive([]);
  } finally {
    panel.dispose();
    editor.close();
  }
}

suite("The Vectors row needs a message that could be V (#1856)", function () {
  this.timeout(120_000);
  let dir: string;
  suiteSetup(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1856-"));
  });
  suiteTeardown(() => fs.rmSync(dir, { recursive: true, force: true }));

  test("two messages on different grids show no Vectors row on either", async () => {
    const file = path.join(dir, "two_grids.grib2");
    fs.writeFileSync(file, Buffer.concat([fs.readFileSync(LAMBERT), fs.readFileSync(LATLON)]));
    for (const index of [0, 1]) {
      const state = await stateOf(file, index);
      assert.strictEqual(state.controlDisplay.arrows, null, `message ${index}: no Vectors row`);
      assert.strictEqual(state.vectorNote, null, `message ${index}: no note`);
      assert.strictEqual(state.vectorRow, null, `message ${index}: no room kept for it`);
    }
  });

  test("two messages on one grid keep the Vectors row, a pair or not", async () => {
    const file = path.join(dir, "same_grid.grib2");
    const bytes = fs.readFileSync(LATLON);
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const state = await stateOf(file, 0);
    assert.notStrictEqual(state.controlDisplay.arrows, null, "the row is there");
    assert.ok(state.vectorRow, "and takes its room");
  });

  test("a third message on the drawn grid brings the row back beside one on another", async () => {
    const file = path.join(dir, "three.grib2");
    const bytes = fs.readFileSync(LATLON);
    fs.writeFileSync(file, Buffer.concat([bytes, fs.readFileSync(LAMBERT), bytes]));
    assert.notStrictEqual((await stateOf(file, 0)).controlDisplay.arrows, null);
    assert.strictEqual((await stateOf(file, 1)).controlDisplay.arrows, null);
  });
});
