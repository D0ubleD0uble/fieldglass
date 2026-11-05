// Field B's picker shows only while Compare has an operation (#1715).
//
// With Compare `(off)`, Field B changes nothing on the map, but its picker
// (a GRIB message, or a slice panel's steppers, not-dated note and all) still
// showed. It now hides the way the graticule spacing does (#1260): it keeps its
// room, so turning Compare on or off does not move the toolbar or the map
// under it (#1228), and the Field B chosen before comes back with it. Driven
// through the real panel and the real provider.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, type Msg, openEditor, openReal, type PageState, provider, type RealPanel } from "./real-panel";

const GRIB = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const SLICE = () => path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");

/** Where Field B's picker and the map sit, measured from the toolbar's top,
 *  with the toolbar's height: what must not move as Compare is toggled. The
 *  line above the toolbar can wrap differently with Compare on, so positions
 *  are taken from the toolbar, not the page. */
function layout(state: PageState) {
  const b = state.compareFieldBControl;
  assert.ok(b, "the panel has Field B's picker");
  const top = state.toolbar.box.top;
  return {
    fieldB: { left: b.box.left, top: b.box.top - top, right: b.box.right, bottom: b.box.bottom - top },
    toolbarHeight: state.toolbar.box.bottom - top,
    imageBelowToolbar: state.image.top - state.toolbar.box.bottom,
  };
}

function assertHidden(state: PageState, what: string): void {
  const b = state.compareFieldBControl;
  assert.ok(b, `${what}: the panel has Field B's picker`);
  assert.strictEqual(b.visibility, "hidden", `${what}: Field B's picker is hidden`);
  assert.ok(b.controls.length > 0, `${what}: Field B's picker has controls`);
  assert.ok(b.controls.every((v) => v === "hidden"), `${what}: every Field B control is hidden: ${b.controls}`);
  assert.ok(b.box.right - b.box.left > 0 && b.box.bottom - b.box.top > 0, `${what}: its room is kept`);
}

function assertShown(state: PageState, what: string): void {
  const b = state.compareFieldBControl;
  assert.ok(b, `${what}: the panel has Field B's picker`);
  assert.strictEqual(b.visibility, "visible", `${what}: Field B's picker is shown`);
  assert.ok(b.controls.every((v) => v === "visible"), `${what}: every Field B control is shown: ${b.controls}`);
}

/** Set the operation and wait for the render it asks for; returns the page
 *  and the compare rider that render asked with (none for a single field). */
async function setOp(panel: RealPanel, op: string): Promise<{ state: PageState; compare: Msg }> {
  const asked = panel.received.length;
  const from = panel.sent.length;
  await panel.drive([{ id: "compare-op", value: op }]);
  await panel.waitSent(isType("gridReady"), from, `the render with Compare ${op || "off"}`);
  const asks = panel.received.slice(asked).filter(isType("rerenderRequest"));
  assert.ok(asks.length > 0, `Compare ${op || "off"} asked for a render`);
  return { state: await panel.drive([]), compare: asks[asks.length - 1].compare };
}

suite("Field B's picker shows only while Compare is on (#1715)", function () {
  this.timeout(120_000);

  test("GRIB: the Field B message picker hides with Compare off, keeps its room and its choice", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1715-"));
    const file = path.join(dir, "triple.grib2");
    const bytes = fs.readFileSync(GRIB());
    fs.writeFileSync(file, Buffer.concat([bytes, bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const opened = await panel.drive([]);
      assertHidden(opened, "Compare off as the panel opens");
      const before = layout(opened);

      let { state } = await setOp(panel, "a_minus_b");
      assertShown(state, "Compare on");
      assert.deepStrictEqual(layout(state), before, "turning Compare on moves nothing");

      let from = panel.sent.length;
      await panel.drive([{ id: "compare-field-b", value: 2 }]);
      await panel.waitSent(isType("gridReady"), from, "the render with Field B message 2");

      let compare: Msg;
      ({ state, compare } = await setOp(panel, ""));
      assert.strictEqual(compare, undefined, "Compare off asks for no Field B");
      assertHidden(state, "Compare off again");
      assert.strictEqual(state.compareFieldB, "2", "Field B is kept while hidden");
      assert.deepStrictEqual(layout(state), before, "turning Compare off moves nothing");

      ({ state, compare } = await setOp(panel, "a_minus_b"));
      assertShown(state, "Compare on again");
      assert.strictEqual(state.compareFieldB, "2", "the Field B chosen before");
      assert.deepStrictEqual(compare, { op: "a_minus_b", messageIndexB: 2 }, "the render compares with it");
      assert.deepStrictEqual(layout(state), before, "turning Compare on again moves nothing");

      // A tab switch writes the page again from its saved state.
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      assertShown(await panel.drive([]), "Compare on, shown again");
      await setOp(panel, "");
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a second hide and show");
      state = await panel.drive([]);
      assertHidden(state, "Compare off, shown again");
      assert.strictEqual(state.compareFieldB, "2", "Field B is kept across the hide and show");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("slice panel: Field B's steppers hide with Compare off, keep their room and their indices", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.open(fs.readFileSync(SLICE())).variables().find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const p = await provider();
    const editor = await openEditor(p, SLICE());
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const opened = await panel.drive([]);
      assert.ok(opened.steppers && opened.steppers.fieldB.length > 0, "the variable has a Field B stepper");
      const dim = opened.steppers.fieldB[0].dim;
      assertHidden(opened, "Compare off as the panel opens");
      const before = layout(opened);

      let { state } = await setOp(panel, "a_minus_b");
      assertShown(state, "Compare on");
      assert.deepStrictEqual(layout(state), before, "turning Compare on moves nothing");

      const from = panel.sent.length;
      await panel.drive([{ selector: `#compare-dims .slice-number[data-dim="${dim}"]`, value: 1 }]);
      await panel.waitSent(isType("gridReady"), from, "the render with Field B stepped");
      // Field B's coordinate text follows its index, so its width does too:
      // what must not move is measured again at the new index.
      const stepped = layout(await panel.drive([]));

      let compare: Msg;
      ({ state, compare } = await setOp(panel, ""));
      assert.strictEqual(compare, undefined, "Compare off asks for no Field B");
      assertHidden(state, "Compare off again");
      assert.strictEqual(state.steppers?.fieldB.find((s) => s.dim === dim)?.index, "1", "Field B's index is kept");
      assert.deepStrictEqual(layout(state), stepped, "turning Compare off moves nothing");

      ({ state, compare } = await setOp(panel, "a_minus_b"));
      assertShown(state, "Compare on again");
      assert.strictEqual(compare?.sliceIndicesB?.[dim], 1, "the render compares with the index set before");
      assert.deepStrictEqual(layout(state), stepped, "turning Compare on again moves nothing");
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
