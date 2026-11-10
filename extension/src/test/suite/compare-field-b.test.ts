// Field B's picker shows only while Compare has an operation (#1715).
//
// With Compare `(off)`, Field B changes nothing on the map, but its picker
// (a GRIB message, or a slice panel's steppers, not-dated note and all) still
// showed. It now hides the way the graticule spacing does (#1260): it keeps its
// room, so turning Compare on or off does not move the toolbar or the map
// under it (#1228), and the Field B chosen before comes back with it. A slice
// panel's steppers keep one control row of room and no more, as the Bounds
// fields do (#1429). The Vectors row's V component and arrow spacing follow
// Arrows the same way. Driven through the real panel and the real provider.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, type Msg, openEditor, openReal, type PageState, provider, type RealPanel } from "./real-panel";

const GRIB = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const SLICE = () => path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
/** `sst` is time × lev × lat × lon: two Field B steppers, which wrap to
 *  several lines in a narrow panel. */
const ERSST = () => path.join(extensionPath(), "src", "test", "fixtures", "ersst_v5_187001_cdf1.nc");

/** The page `px` wide, as the narrow-panel tests set it (#1429). */
const pageWidth = (px: number) => ({ id: "body", width: `calc(${px}px - 3rem)` });

/** A panel on message 0 of three copies of the GRIB fixture: a Compare row
 *  and a Vectors row, each with messages to pick. */
async function openTriple(): Promise<{ panel: RealPanel; close: () => void }> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1715-"));
  const file = path.join(dir, "triple.grib2");
  const bytes = fs.readFileSync(GRIB());
  fs.writeFileSync(file, Buffer.concat([bytes, bytes, bytes]));
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
  } catch (e) {
    close();
    throw e;
  }
  return { panel, close };
}

/** A slice panel on `variable` of `file`. */
async function openSlice(file: string, variable: string): Promise<{ panel: RealPanel; close: () => void }> {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const found = native.open(fs.readFileSync(file)).variables().find((v) => v.name === variable);
  assert.ok(found, `the fixture has ${variable}`);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: found.index }));
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
  assert.strictEqual(b.cut, 0, `${what}: every Field B stepper is shown whole`);
}

/** Hidden, Field B's steppers keep one control row of room and no more
 *  (#1429's rule for the Bounds fields), inside the toolbar. */
function assertOneRow(state: PageState, what: string): void {
  const b = state.compareFieldBControl;
  assert.ok(b, `${what}: the panel has Field B's steppers`);
  const height = b.box.bottom - b.box.top;
  assert.ok(height <= b.rowHeight + 0.5, `${what}: the hidden steppers keep ${height} px, more than one row of ${b.rowHeight} px`);
  assert.ok(
    b.box.left >= state.toolbar.box.left - 0.5 && b.box.right <= state.toolbar.box.right + 0.5,
    `${what}: the hidden steppers stay inside the toolbar`,
  );
  assert.ok(!state.toolbar.pageScrollsSideways, `${what}: the page scrolls sideways`);
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

/** Step Field B's stepper for `dim` to `index` and wait for the render. */
async function stepFieldB(panel: RealPanel, dim: number, index: number): Promise<void> {
  const from = panel.sent.length;
  await panel.drive([{ selector: `#compare-dims .slice-number[data-dim="${dim}"]`, value: index }]);
  await panel.waitSent(isType("gridReady"), from, "the render with Field B stepped");
}

suite("Field B's picker shows only while Compare is on (#1715)", function () {
  this.timeout(120_000);

  test("GRIB: the Field B message picker hides with Compare off, keeps its room and its choice", async () => {
    const { panel, close } = await openTriple();
    try {
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
      close();
    }
  });

  test("slice panel: Field B's steppers hide with Compare off, keep their room and their indices", async () => {
    const { panel, close } = await openSlice(SLICE(), "temperature");
    try {
      const opened = await panel.drive([]);
      assert.ok(opened.steppers && opened.steppers.fieldB.length > 0, "the variable has a Field B stepper");
      const dim = opened.steppers.fieldB[0].dim;
      assertHidden(opened, "Compare off as the panel opens");
      const before = layout(opened);

      let { state } = await setOp(panel, "a_minus_b");
      assertShown(state, "Compare on");
      assert.deepStrictEqual(layout(state), before, "turning Compare on moves nothing");

      await stepFieldB(panel, dim, 1);
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
      close();
    }
  });

  test("a narrow slice panel: hidden, Field B keeps one control row; shown, every stepper", async () => {
    const { panel, close } = await openSlice(ERSST(), "sst");
    try {
      // The hidden row stays one row at 450 px and below, where its steppers
      // would wrap (#1729 widened what can wrap in them).
      let state = await panel.drive([]);
      for (const px of [450, 300, 200]) {
        state = await panel.drive([pageWidth(px)]);
        assertHidden(state, `${px} px, Compare off`);
        assertOneRow(state, `${px} px, Compare off`);
      }
      state = await panel.drive([pageWidth(300)]);
      const hidden = state.compareFieldBControl;
      assert.ok(hidden);

      ({ state } = await setOp(panel, "a_minus_b"));
      assertShown(state, "300 px, Compare on");
      const shown = state.compareFieldBControl;
      assert.ok(shown);
      assert.strictEqual(shown.steppers, 2, "time and lev");
      assert.ok(
        shown.box.bottom - shown.box.top > hidden.box.bottom - hidden.box.top,
        "shown, the steppers take more than the one row kept for them",
      );

      ({ state } = await setOp(panel, ""));
      assertHidden(state, "300 px, Compare off again");
      assertOneRow(state, "300 px, Compare off again");
    } finally {
      close();
    }
  });

  test("a narrow slice panel keeps Field B's index across the toggle", async () => {
    const { panel, close } = await openSlice(SLICE(), "temperature");
    try {
      let state = await panel.drive([pageWidth(300)]);
      assertOneRow(state, "300 px, Compare off");
      assert.ok(state.steppers && state.steppers.fieldB.length > 0, "the variable has a Field B stepper");
      const dim = state.steppers.fieldB[0].dim;
      await setOp(panel, "a_minus_b");
      await stepFieldB(panel, dim, 1);

      let compare: Msg;
      ({ state } = await setOp(panel, ""));
      assertHidden(state, "300 px, Compare off");
      assertOneRow(state, "300 px, Compare off, stepped");
      assert.strictEqual(state.steppers?.fieldB.find((s) => s.dim === dim)?.index, "1", "Field B's index is kept");

      ({ state, compare } = await setOp(panel, "a_minus_b"));
      assertShown(state, "300 px, Compare on again");
      assert.strictEqual(compare?.sliceIndicesB?.[dim], 1, "the render compares with the index set before");
    } finally {
      close();
    }
  });
});

// The Vectors row's V component and arrow spacing show only while Arrows is
// ticked, as the graticule spacing does with Graticule (#1260): the same
// pattern as Field B (#1715). The arrows' color and weight stay shown, as
// every overlay layer's do.
suite("The V component and arrow spacing show only while Arrows is on (#1715)", function () {
  this.timeout(120_000);

  test("they hide with Arrows off, keep their room and the V component chosen", async () => {
    const hidden = { "vector-field-v-label": "hidden", "vector-spacing-label": "hidden" };
    const shown = { "vector-field-v-label": "visible", "vector-spacing-label": "visible" };
    const where = (s: PageState) => ({ toolbar: s.toolbar.box, image: s.image.top });
    const { panel, close } = await openTriple();
    try {
      let state = await panel.drive([]);
      assert.strictEqual(state.controlShown.arrows, true, "the Vectors row is shown");
      assert.deepStrictEqual(state.vectorFieldVisibility, hidden, "Arrows off as the panel opens");
      const before = where(state);

      state = await panel.drive([{ id: "overlay-vectors", checked: true }, { id: "vector-field-v", value: 2 }]);
      assert.deepStrictEqual(state.vectorFieldVisibility, shown, "Arrows on");
      assert.deepStrictEqual(where(state), before, "ticking Arrows moves nothing");

      state = await panel.drive([{ id: "overlay-vectors", checked: false }]);
      assert.deepStrictEqual(state.vectorFieldVisibility, hidden, "Arrows off again");
      assert.deepStrictEqual(where(state), before, "unticking Arrows moves nothing");
      assert.strictEqual((await panel.controls())["vector-field-v"], "2", "the V component is kept while hidden");

      // A tab switch writes the page again from its saved state.
      await panel.drive([{ id: "overlay-vectors", checked: true }]);
      let from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      assert.deepStrictEqual((await panel.drive([])).vectorFieldVisibility, shown, "Arrows on, shown again");
      await panel.drive([{ id: "overlay-vectors", checked: false }]);
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a second hide and show");
      assert.deepStrictEqual((await panel.drive([])).vectorFieldVisibility, hidden, "Arrows off, shown again");
    } finally {
      close();
    }
  });
});
