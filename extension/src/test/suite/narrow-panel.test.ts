// The render panel's toolbar in a narrow panel (#1429, after #1393).
//
// At 300, 450 and 900 px every control in every toolbar row stays inside the
// toolbar's box: none is pushed past its right edge, where the page would
// scroll sideways to reach it. And the Bounds row's Manual fields, which keep
// their room on Auto so a gesture that ticks Manual does not move the map
// (#1228), keep no more than one control row of it: they stay on one line,
// which scrolls sideways where the four do not fit.
//
// Driven through the real panel, at the widths a user drags it to: each width
// is the page's whole width, the body's padding included.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
} from "./real-panel";

/** The widths the issue names, as the page's whole width. */
const WIDTHS = [300, 450, 900];

/** A body `width` that makes the page `px` wide: the body's padding is
 *  1.5rem a side. */
const pageWidth = (px: number) => ({ id: "body", width: `calc(${px}px - 3rem)` });

/** Fail if anything in the toolbar is drawn past its sides, or the page is
 *  wider than a window it fits in. */
function assertInsideToolbar(state: PageState, px: number, what: string): void {
  const bar = state.toolbar;
  assert.deepStrictEqual(
    bar.outside.map((c) => `${c.what} [${Math.round(c.box.left)}, ${Math.round(c.box.right)}]`),
    [],
    `${what}: drawn outside the toolbar [${Math.round(bar.box.left)}, ${Math.round(bar.box.right)}]`,
  );
  // A page wider than the window it is in (900 px in a small test window)
  // scrolls sideways for that reason alone.
  if (px <= bar.viewportWidth) assert.ok(!bar.pageScrollsSideways, `${what}: the page scrolls sideways`);
}

const BOUNDS = ["bounds-lat-min", "bounds-lat-max", "bounds-lon-min", "bounds-lon-max"];

/** Fail unless each Bounds field, focused as Tab focuses it, is inside the
 *  toolbar: the line it is on scrolls it into view. */
async function assertBoundsReachable(panel: RealPanel, px: number): Promise<void> {
  for (const id of BOUNDS) {
    const bar = (await panel.drive([{ id, focus: true }])).toolbar;
    assert.ok(bar.focused, `${px} px: #${id} takes focus`);
    assert.ok(
      bar.focused.left >= bar.box.left - 0.5 && bar.focused.right <= bar.box.right + 0.5,
      `${px} px: #${id} [${bar.focused.left}, ${bar.focused.right}] is outside the toolbar [${bar.box.left}, ${bar.box.right}] when focused`,
    );
  }
}

/** The GFS fixture twice over: a file of two messages on one grid, a u/v
 *  pair, so its panel has the Vectors row as well as Bounds and Overlay. */
function pairFile(dir: string): string {
  const gfs = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  assert.ok(fs.existsSync(gfs), `fixture missing: ${gfs}`);
  const one = fs.readFileSync(gfs);
  const file = path.join(dir, "gfs_pair.grib2");
  fs.writeFileSync(file, Buffer.concat([one, one]));
  return file;
}

/** Open the pair's first message on Equirectangular, with every overlay
 *  layer and its arrows on, so every field that shows only while its toggle
 *  is on is shown; and run `body` on it. */
async function onWindMap(body: (panel: RealPanel) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1429-"));
  const p = await provider();
  const editor = await openEditor(p, pairFile(dir));
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    let from = panel.sent.length;
    await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
    await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
    from = panel.sent.length;
    await panel.drive([
      { id: "vector-field-v", value: 1 },
      { id: "overlay-vectors", checked: true },
    ]);
    await panel.waitSent(isType("vectorResult"), from, "the arrows");
    await panel.drive(
      ["coastlines", "borders", "lakes", "rivers", "graticule", "contours"].map((layer) => ({
        id: `overlay-${layer}`,
        checked: true,
      })),
    );
    await body(panel);
  } finally {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const bounds = (mode: string) => ({ selector: `input[name="bounds-mode"][value="${mode}"]`, checked: true });

suite("The toolbar in a narrow panel (#1429)", function () {
  this.timeout(120_000);

  test("every control of a map's toolbar stays inside it at 300, 450 and 900 px, on Auto and Manual", async () => {
    await onWindMap(async (panel) => {
      const opened = await panel.drive([]);
      assert.strictEqual(opened.controlShown.arrows, true, "the Vectors row is shown");
      assert.strictEqual(opened.controlShown.overlay, true, "the Overlay row is shown");
      assert.strictEqual(opened.contours, true, "Contours is ticked, so its interval field is shown");
      for (const mode of ["auto", "manual"]) {
        await panel.drive([{ selector: `input[name="range-mode"][value="${mode}"]`, checked: true }, bounds(mode)]);
        for (const px of WIDTHS) {
          const state = await panel.drive([pageWidth(px)]);
          assert.ok(state.toolbar.boundsFields, `${px} px ${mode}: the Bounds row is shown`);
          assertInsideToolbar(state, px, `${px} px, ${mode}`);
          if (mode === "manual") await assertBoundsReachable(panel, px);
        }
      }
    });
  });

  test("every control of a slice panel's toolbar stays inside it at 300, 450 and 900 px, Compare on", async () => {
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(isType("gridReady"), from, "the difference map");
      const opened = await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.ok(opened.steppers && opened.steppers.fieldB.length > 0, "Field B's steppers are shown");
      for (const px of WIDTHS) assertInsideToolbar(await panel.drive([pageWidth(px)]), px, `${px} px`);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("the Bounds row's Manual fields keep one control row of room at 300, 450 and 900 px", async () => {
    await onWindMap(async (panel) => {
      for (const px of WIDTHS) {
        for (const mode of ["auto", "manual"]) {
          await panel.drive([bounds(mode)]);
          const bar = (await panel.drive([pageWidth(px)])).toolbar;
          assert.ok(bar.boundsFields && bar.controlHeight > 0, `${px} px ${mode}: the Bounds row is laid out`);
          const tall = bar.boundsFields.bottom - bar.boundsFields.top;
          const row = Math.max(bar.controlHeight, bar.manualHeight);
          assert.ok(tall <= row + 1, `${px} px ${mode}: the Manual fields take ${tall} px, more than one ${row} px row`);
        }
      }
    });
  });

  test("the wheel over the Bounds fields scrolls their line where they do not fit, and the page where they do", async () => {
    await onWindMap(async (panel) => {
      await panel.drive([bounds("manual")]);
      let bar = (await panel.drive([pageWidth(300), { id: "bounds-fieldset", scroll: true }])).toolbar;
      assert.ok(bar.boundsScroll && bar.boundsScroll.room > 0, "at 300 px the four fields do not fit their line");
      assert.strictEqual(bar.boundsScroll.left, 0, "the line starts at lat min");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, Math.min(100, bar.boundsScroll?.room ?? 0), "the wheel scrolls the line");
      assert.strictEqual(bar.wheelPrevented, true, "and not the page as well");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 10_000 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, bar.boundsScroll?.room, "as far as lon max, no further");

      bar = (await panel.drive([pageWidth(900)])).toolbar;
      assert.strictEqual(bar.boundsScroll?.room, 0, "at 900 px the four fields fit");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.wheelPrevented, false, "and the wheel over them is left to scroll the page");
    });
  });

  test("ticking Bounds Manual and back moves nothing below the toolbar, at 300, 450 and 900 px (#1228)", async () => {
    await onWindMap(async (panel) => {
      for (const px of WIDTHS) {
        await panel.drive([bounds("auto")]);
        const auto = await panel.drive([pageWidth(px)]);
        const manual = await panel.drive([bounds("manual")]);
        assert.strictEqual(Math.round(manual.image.top), Math.round(auto.image.top), `${px} px: Manual moved the map`);
        const back = await panel.drive([bounds("auto")]);
        assert.strictEqual(Math.round(back.image.top), Math.round(auto.image.top), `${px} px: Auto moved the map`);
      }
    });
  });
});
