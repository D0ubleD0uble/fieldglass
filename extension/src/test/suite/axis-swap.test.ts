// Picking the other axis picker's dimension swaps the two (#1387).
//
// Setting the Y axis to the X axis's dimension, or X to Y's, was refused
// with "The X and Y axes must be different dimensions." and the picker
// snapped back. A 2-D variable such as `time_bnds` then had no way to draw
// its transpose, and a 3-D or 4-D one needed a detour through a third
// dimension. Driven through the real panel: the file is written to disk and
// opened the way a user opens it.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { pickedAxes } from "../../render-panel";
import {
  classicNetcdf,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const TIMES = 4;
const LEVELS = 3;
const LATS = 3;
const LONS = 5;

/** `time_bnds(time, nbnds)` beside `air(time, level, lat, lon)`, on a
 *  latitude/longitude grid, every value distinct. */
function file(): Buffer {
  const range = (n: number, f: (i: number) => number) => Array.from({ length: n }, (_, i) => f(i));
  const air: number[] = [];
  for (let t = 0; t < TIMES; t++) {
    for (let k = 0; k < LEVELS; k++) {
      for (let j = 0; j < LATS; j++) {
        for (let i = 0; i < LONS; i++) air.push(1000 * t + 100 * k + 10 * j + i);
      }
    }
  }
  return classicNetcdf(
    [["time", TIMES], ["nbnds", 2], ["level", LEVELS], ["lat", LATS], ["lon", LONS]],
    [
      { name: "time", dims: ["time"], attrs: { units: "hours since 2024-01-01" }, values: range(TIMES, (t) => 24 * t) },
      { name: "time_bnds", dims: ["time", "nbnds"], attrs: {}, values: range(TIMES * 2, (n) => 24 * Math.floor(n / 2) + 24 * (n % 2)) },
      { name: "level", dims: ["level"], attrs: { units: "millibar", positive: "down" }, values: [1000, 850, 500] },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: range(LONS, (i) => 10 * i) },
      { name: "air", dims: ["time", "level", "lat", "lon"], attrs: { units: "K" }, values: air },
    ],
  );
}

interface Opened {
  panel: RealPanel;
  close: () => void;
}

/** Open {@link file} in the editor and draw the variable named `name`. */
async function open(name: string): Promise<Opened> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1387-"));
  const at = path.join(dir, "swap.nc");
  fs.writeFileSync(at, file());
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(at)).variables().find((x) => x.name === name);
  assert.ok(v, `${name} is renderable`);
  const editor = await openEditor(p, at);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: v.index }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
  } catch (err) {
    close();
    throw err;
  }
  return { panel, close };
}

/** Set `id` to `value` and wait for the render it asks for and its answer:
 *  the slice asked for, the grid drawn and the page read back after. */
async function pick(panel: RealPanel, id: "slice-y" | "slice-x", value: number) {
  const received = panel.received.length;
  const sent = panel.sent.length;
  const state = await panel.drive([{ id, value }]);
  const asked: Msg = await waitUntil(
    () => panel.received.slice(received).find(isType("rerenderRequest")),
    `the render ${id} = ${value} asks for`,
  );
  const drawn: Msg = await panel.waitSent(isType("gridReady"), sent, `the render for ${id} = ${value}`);
  const after = await panel.drive([]);
  const controls = await panel.controls();
  return { state, asked, drawn, after, controls };
}

suite("Swapping the image axes (#1387)", function () {
  this.timeout(60_000);

  test("pickedAxes swaps on the other picker's dimension and moves one axis otherwise", () => {
    assert.deepStrictEqual(pickedAxes("y", 1, 0, 1), { yDim: 1, xDim: 0 }, "Y to X's dimension swaps");
    assert.deepStrictEqual(pickedAxes("x", 0, 0, 1), { yDim: 1, xDim: 0 }, "X to Y's dimension swaps");
    assert.deepStrictEqual(pickedAxes("y", 0, 2, 3), { yDim: 0, xDim: 3 }, "Y to a third dimension moves Y alone");
    assert.deepStrictEqual(pickedAxes("x", 1, 2, 3), { yDim: 2, xDim: 1 }, "X to a third dimension moves X alone");
    assert.deepStrictEqual(pickedAxes("y", 2, 2, 3), { yDim: 2, xDim: 3 }, "the same pick changes nothing");
  });

  test("a 2-D variable's axes swap, both ways", async () => {
    const { panel, close } = await open("time_bnds");
    try {
      const first = panel.sent.filter(isType("gridReady"))[0];
      assert.deepStrictEqual([first.width, first.height], [2, TIMES], "time_bnds opens as time × nbnds");

      const y = await pick(panel, "slice-y", 1);
      assert.deepStrictEqual([y.asked.slice.yDim, y.asked.slice.xDim], [1, 0], "Y = nbnds asks for the transpose");
      assert.deepStrictEqual([y.drawn.width, y.drawn.height], [TIMES, 2], "which draws nbnds × time");
      assert.ok(!/must be different/.test(y.after.status), `no refusal in the status: ${y.after.status}`);
      assert.deepStrictEqual([y.controls["slice-y"], y.controls["slice-x"]], ["1", "0"], "both pickers show the swap");

      const x = await pick(panel, "slice-x", 1);
      assert.deepStrictEqual([x.asked.slice.yDim, x.asked.slice.xDim], [0, 1], "X = nbnds swaps them back");
      assert.deepStrictEqual([x.drawn.width, x.drawn.height], [2, TIMES], "which draws time × nbnds again");
      assert.ok(!/must be different/.test(x.after.status), `no refusal in the status: ${x.after.status}`);
      assert.deepStrictEqual([x.controls["slice-y"], x.controls["slice-x"]], ["0", "1"]);
    } finally {
      close();
    }
  });

  test("a 4-D variable's swap keeps the held indices and rebuilds the steppers", async () => {
    const { panel, close } = await open("air");
    try {
      const first = panel.sent.filter(isType("gridReady"))[0];
      assert.deepStrictEqual([first.width, first.height], [LONS, LATS], "air opens as its lat × lon map");
      // On Mollweide, holding time 3 and level 2.
      let sent = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "mollweide" }]);
      await panel.waitSent(
        (m: Msg) => m.type === "gridReady" && m.options?.projection === "mollweide",
        sent,
        "the map on Mollweide",
      );
      // One step at a time, each drawn before the next, so no render is
      // still on its way when the axes change.
      for (const [dim, index] of [[0, 3], [1, 2]]) {
        sent = panel.sent.length;
        await panel.drive([{ selector: `#slice-dims .slice-number[data-dim="${dim}"]`, value: index }]);
        await panel.waitSent(isType("gridReady"), sent, `the map with dimension ${dim} at ${index}`);
      }

      // Y = lon, X's dimension: the transpose, a lon × lat plot.
      const swap = await pick(panel, "slice-y", 3);
      assert.deepStrictEqual([swap.asked.slice.yDim, swap.asked.slice.xDim], [3, 2], "Y = lon swaps with X");
      assert.deepStrictEqual(swap.asked.slice.sliceIndices.slice(0, 2), [3, 2], "time 3 and level 2 are still held");
      assert.deepStrictEqual([swap.drawn.width, swap.drawn.height], [LATS, LONS], "which draws lon × lat");
      // air = 1000 t + 100 k + 10 j + i: time 3, level 2 spans 3200 to 3224.
      assert.deepStrictEqual([swap.drawn.usedMin, swap.drawn.usedMax], [3200, 3224], "of the held slice");
      assert.ok(!/must be different/.test(swap.after.status), `no refusal in the status: ${swap.after.status}`);
      const held = swap.after.steppers?.main.map((s) => [s.dim, s.index]);
      assert.deepStrictEqual(held, [[0, "3"], [1, "2"]], "the steppers still hold time 3 and level 2");
      // The transposed pair is not the map: it is drawn in index space, and
      // Mollweide is held for the map's return.
      assert.strictEqual(swap.drawn.options.projection, "source", "the transpose is not reprojected");
      assert.strictEqual(swap.after.projection, "source", "the picker reads Source");
      assert.strictEqual(swap.after.controlShown.projection, false, "and is put away");
      assert.ok(swap.after.text.includes("Cross-section:"), "the cross-section note shows");

      // X = lon swaps them back: the map, on Mollweide again.
      const map = await pick(panel, "slice-x", 3);
      assert.deepStrictEqual([map.asked.slice.yDim, map.asked.slice.xDim], [2, 3], "X = lon swaps back");
      assert.deepStrictEqual(map.asked.slice.sliceIndices.slice(0, 2), [3, 2], "still at time 3, level 2");
      assert.strictEqual(map.drawn.options.projection, "mollweide", "the map is drawn on Mollweide");
      assert.strictEqual(map.after.projection, "mollweide", "and the picker reads Mollweide");
      assert.ok(!map.after.text.includes("Cross-section:"), "the cross-section note goes");

      // X = time moves X alone; Y = time then swaps Y with it.
      await pick(panel, "slice-x", 0);
      const back = await pick(panel, "slice-y", 0);
      assert.deepStrictEqual([back.asked.slice.yDim, back.asked.slice.xDim], [0, 2], "Y = time swaps with X");
      assert.deepStrictEqual(back.asked.slice.sliceIndices[1], 2, "level 2 is still held");
      assert.deepStrictEqual([back.drawn.width, back.drawn.height], [LATS, TIMES], "which draws time × lat");
      // Level 2 at the first longitude: 1000 t + 200 + 10 j.
      assert.deepStrictEqual([back.drawn.usedMin, back.drawn.usedMax], [200, 3220], "at level 2");
      const steppers = back.after.steppers?.main.map((s) => s.dim);
      assert.deepStrictEqual(steppers, [1, 3], "the steppers are level's and lon's now");
      const animate = back.controls["animate-dim"];
      assert.ok(animate === "1" || animate === "3", `Animate steps a held dimension, not ${animate}`);
    } finally {
      close();
    }
  });
});
