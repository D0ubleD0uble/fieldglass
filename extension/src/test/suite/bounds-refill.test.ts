// The Manual Bounds fields follow the field drawn (#1514).
//
// The four fields were filled once, from the first picture, and only the zoom
// reset cleared them, so ticking Manual after picking a variable on another
// grid jumped the map to the first field's extent. While Bounds is on Auto the
// page now refills every field that still holds what it gave it, and leaves
// one the user typed alone. Run in a real webview through the driver in
// `real-panel.ts`, the way `range-refill.test.ts` does for the color range.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { classicNetcdf, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

const MANUAL = { selector: 'input[name="bounds-mode"][value="manual"]', checked: true };
const AUTO = { selector: 'input[name="bounds-mode"][value="auto"]', checked: true };
const EQUIRECT = { id: "picker-projection", value: "equirectangular" };
const IDS = ["bounds-lat-min", "bounds-lat-max", "bounds-lon-min", "bounds-lon-max"];
const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** A classic NetCDF file of two variables on different lat/lon grids: `a` on
 *  10 to 50 degrees north and 0 to 30 east, `b` on 20.125 to 60.125 north and
 *  100.125 to 130.125 east. */
function twoGridFile(): Buffer {
  const lat = (from: number) => [0, 1, 2, 3, 4].map((k) => from + 10 * k);
  const lon = (from: number) => [0, 1, 2, 3].map((k) => from + 10 * k);
  const values = (base: number) => Array.from({ length: 20 }, (_, k) => base + k);
  return classicNetcdf(
    [["lat1", 5], ["lon1", 4], ["lat2", 5], ["lon2", 4]],
    [
      { name: "lat1", dims: ["lat1"], attrs: { units: "degrees_north" }, values: lat(10) },
      { name: "lon1", dims: ["lon1"], attrs: { units: "degrees_east" }, values: lon(0) },
      { name: "lat2", dims: ["lat2"], attrs: { units: "degrees_north" }, values: lat(20.125) },
      { name: "lon2", dims: ["lon2"], attrs: { units: "degrees_east" }, values: lon(100.125) },
      { name: "a", dims: ["lat1", "lon1"], attrs: {}, values: values(280) },
      { name: "b", dims: ["lat2", "lon2"], attrs: {}, values: values(0) },
    ],
  );
}

const box = (m: Msg) => [m.usedLatMin, m.usedLatMax, m.usedLonMin, m.usedLonMax];
const shown = async (panel: RealPanel) => {
  const c = await panel.controls();
  return IDS.map((id) => Number(c[id]));
};

suite("The Manual Bounds fields follow the field drawn (#1514)", function () {
  this.timeout(120_000);

  /** A panel on a file of two variables on different grids, `a` drawn. */
  async function twoGrids() {
    const bytes = twoGridFile();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1514-"));
    const file = path.join(dir, "two-grids.nc");
    fs.writeFileSync(file, bytes);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const vars = native.open(bytes).variables();
    const index = ["a", "b"].map((n) => vars.find((v) => v.name === n)!.index);
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index[0] }));
    await panel.waitSent(isType("gridReady"), 0, "the first variable");
    let from = panel.sent.length;
    await panel.drive([EQUIRECT]);
    const a = await panel.waitSent(drawnOrRefused, from, "Equirectangular");
    assert.strictEqual(a.type, "gridReady", `${a.error ?? ""}`);
    const pick = async (which: number) => {
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: index[which] }]);
      const m = await panel.waitSent(drawnOrRefused, from, `variable ${which}`);
      assert.strictEqual(m.type, "gridReady", `${m.error ?? ""}`);
      return m;
    };
    return { panel, a, pick, close: () => { panel.dispose(); editor.close(); fs.rmSync(dir, { recursive: true, force: true }); } };
  }

  test("picking a variable on another grid refills the fields, and Manual draws that extent", async () => {
    const { panel, a, pick, close } = await twoGrids();
    try {
      assert.deepStrictEqual(await shown(panel), box(a), "the first variable's extent");
      const b = await pick(1);
      assert.notDeepStrictEqual(box(b), box(a), "another extent");
      assert.deepStrictEqual(await shown(panel), box(b), "the second variable's extent");

      const from = panel.sent.length;
      await panel.drive([MANUAL]);
      const manual = await panel.waitSent(drawnOrRefused, from, "Manual");
      assert.strictEqual(manual.type, "gridReady", `${manual.error ?? ""}`);
      assert.deepStrictEqual(
        [manual.options.boundsLatMin, manual.options.boundsLatMax, manual.options.boundsLonMin, manual.options.boundsLonMax],
        box(b),
      );
      assert.deepStrictEqual(box(manual), box(b), "the same window as Auto");
    } finally {
      close();
    }
  });

  test("a field the user typed survives a later switch on Auto; the others follow", async () => {
    const { panel, a, pick, close } = await twoGrids();
    try {
      await panel.drive([{ id: "bounds-lat-min", value: 12 }]);
      const b = await pick(1);
      const now = await shown(panel);
      assert.strictEqual(now[0], 12, "the typed minimum latitude stays");
      assert.deepStrictEqual(now.slice(1), box(b).slice(1), "the other three follow");
      assert.notStrictEqual(box(a)[1], box(b)[1]);
    } finally {
      close();
    }
  });

  test("the fields still refill after a hide and show", async () => {
    const { panel, a, pick, close } = await twoGrids();
    try {
      const b = await pick(1);
      let from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(drawnOrRefused, from, "after a hide and show");
      assert.deepStrictEqual(await shown(panel), box(b), "restored");
      const back = await pick(0);
      assert.deepStrictEqual(box(back), box(a));
      assert.deepStrictEqual(await shown(panel), box(a), "refilled after the restore");
      from = panel.sent.length;
      await panel.drive([MANUAL]);
      await panel.waitSent(drawnOrRefused, from, "Manual");
      // A tab switch on Manual keeps the window, and a pick leaves it alone.
      await panel.hideShow();
      assert.deepStrictEqual(await shown(panel), box(a), "Manual window kept");
      await pick(1);
      assert.deepStrictEqual(await shown(panel), box(a), "Manual is never refilled");
    } finally {
      close();
    }
  });

  test("a typed field stays through Manual and back to Auto", async () => {
    const { panel, pick, close } = await twoGrids();
    try {
      await panel.drive([MANUAL, { id: "bounds-lat-min", value: 5 }]);
      await pick(1);
      assert.strictEqual((await shown(panel))[0], 5, "typed on Manual stays");
      const from = panel.sent.length;
      await panel.drive([AUTO]);
      const auto = await panel.waitSent(drawnOrRefused, from, "back to Auto");
      assert.strictEqual(auto.type, "gridReady", `${auto.error ?? ""}`);
      assert.deepStrictEqual(box(auto).slice(1), [60.125, 100.125, 130.125], "the second variable's own extent");
      assert.strictEqual((await shown(panel))[0], 5, "a typed field stays after Auto too");
      assert.deepStrictEqual((await shown(panel)).slice(1), box(auto).slice(1), "and the others come from Auto's picture");
    } finally {
      close();
    }
  });
});
