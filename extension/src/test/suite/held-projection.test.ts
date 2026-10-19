// A projection put away with its picker comes back (#1099).
//
// A slice panel hides the projection picker for a cross-section (#171) and for
// a slice nothing places (#822, #1036), and draws those in the source view. A
// hidden control keeps its value for the next field that takes it. Overlays,
// arrows and Resampling (#1091) did; the projection was reset to Source, so the
// next map slice drew in Source. These run the panel's own script in a real
// webview, through the driver in `real-panel.ts`, and read what the render asked
// for as well as what the page shows.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

const FILE = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");

/** A NetCDF variable's index in the fixture, by name. */
function variableIndex(name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(FILE))
    .variables()
    .find((x) => x.name === name);
  assert.ok(v, `the fixture has ${name}`);
  return v.variableIndex;
}

/** The slice's own controls, which the steps below move on purpose, and the
 *  animated axis, which is rebuilt for each axis pair. */
const RESHAPED = /^(slice-variable|slice-y|slice-x|animate-dim)$/;

/** Every control on the page but the slice's own, by id. */
async function settings(panel: RealPanel): Promise<Record<string, string | boolean>> {
  const all = await panel.controls();
  return Object.fromEntries(Object.entries(all).filter(([id]) => !RESHAPED.test(id)));
}

/** Choose Mollweide on a 30° meridian and Bilinear, and wait for that map. */
async function onMollweide(panel: RealPanel): Promise<void> {
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  const from = panel.sent.length;
  await panel.drive([
    { id: "picker-world-meridian", value: 30 },
    { id: "picker-projection", value: "mollweide" },
    { id: "picker-resampling", value: "bilinear" },
  ]);
  await panel.waitSent(
    (m: Msg) => m.type === "gridReady" && m.options.projection === "mollweide" && m.options.resampling === "bilinear",
    from,
    "temperature on Mollweide",
  );
}

/** The map slice is drawn on the projection chosen before, with its meridian
 *  and Resampling, and the picker and Resampling show them again. */
async function assertBackOnMollweide(panel: RealPanel, from: { sent: number; received: number }, what: string) {
  const drawn = await panel.waitSent(
    (m: Msg) => m.type === "gridReady" && m.sliceGrid && m.sliceGrid.placed,
    from.sent,
    `the map again (${what})`,
  );
  const asked = panel.received.slice(from.received).filter(isType("rerenderRequest"));
  assert.ok(asked.length > 0, `a render was asked for (${what})`);
  const last = asked[asked.length - 1];
  assert.strictEqual(last.projection, "mollweide", `the render asks for Mollweide (${what})`);
  assert.strictEqual(last.centerLon, 30, `on its meridian (${what})`);
  assert.strictEqual(drawn.options.projection, "mollweide", `drawn on Mollweide (${what})`);
  assert.strictEqual(drawn.options.resampling, "bilinear", `with the kept Resampling (${what})`);
  const state = await panel.drive([]);
  assert.strictEqual(state.projection, "mollweide", `the picker shows Mollweide (${what})`);
  assert.strictEqual(state.controlShown.projection, true, `the picker is back (${what})`);
  assert.strictEqual(state.controlShown.resampling, true, `Resampling is back (${what})`);
  assert.strictEqual(state.resampling, "bilinear", `with its value (${what})`);
  assert.deepStrictEqual(state.shownWhileHidden, []);
}

function mark(panel: RealPanel) {
  return { sent: panel.sent.length, received: panel.received.length };
}

suite("A projection hidden with its picker comes back (#1099)", function () {
  this.timeout(120_000);

  test("after a cross-section, the map draws on the projection chosen before", async () => {
    const p = await provider();
    const editor = await openEditor(p, FILE);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex("temperature") }));
    try {
      await onMollweide(panel);
      const before = await settings(panel);

      // time down the rows: a cross-section, drawn in index space.
      let from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from.sent, "the cross-section");
      const cross = panel.received.slice(from.received).filter(isType("rerenderRequest"));
      assert.strictEqual(cross[cross.length - 1].projection, "source", "a cross-section asks for the source view");
      let state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.projection, "none", "the picker is hidden");
      assert.strictEqual(state.controlDisplay.resampling, "none", "and Resampling");

      // Back to lat down the rows.
      from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 1 }]);
      await assertBackOnMollweide(panel, from, "cross-section");
      // Nothing else the cross-section hid came back changed either.
      assert.deepStrictEqual(await settings(panel), before, "every control reads as before the cross-section");

      state = await panel.drive([]);
      assert.strictEqual(state.controlShown.overlay, true);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("after a variable nothing places, the next placed one draws on the projection chosen before", async () => {
    const p = await provider();
    const temperature = variableIndex("temperature");
    const latBnds = variableIndex("lat_bnds");
    const editor = await openEditor(p, FILE);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await onMollweide(panel);
      const before = await settings(panel);

      let from = mark(panel);
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      const flat = await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from.sent, "lat_bnds drawn");
      assert.strictEqual(flat.options.projection, "source", "lat_bnds is drawn in the source view");
      let state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.deepStrictEqual(state.projectionOffers, ["source"]);
      assert.strictEqual(state.controlDisplay.projection, "none", "the picker is hidden");

      // A re-render of lat_bnds still draws it in the source view.
      from = mark(panel);
      await panel.drive([{ id: "flip-y", checked: true }]);
      const again = await panel.waitSent(isType("gridReady"), from.sent, "lat_bnds again");
      assert.strictEqual(again.options.projection, "source");
      from = mark(panel);
      await panel.drive([{ id: "flip-y", checked: false }]);
      await panel.waitSent(isType("gridReady"), from.sent, "lat_bnds unflipped");

      from = mark(panel);
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      await assertBackOnMollweide(panel, from, "unplaced variable");
      assert.deepStrictEqual(await settings(panel), before, "every control reads as before lat_bnds");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a rebuild while the projection is held keeps it", async () => {
    const p = await provider();
    const temperature = variableIndex("temperature");
    const latBnds = variableIndex("lat_bnds");
    const editor = await openEditor(p, FILE);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await onMollweide(panel);

      // Held by a cross-section, then the page is written again (#1027).
      let from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from.sent, "the cross-section");
      from = mark(panel);
      panel.rebuild();
      const restored = await panel.waitSent(isType("gridReady"), from.sent, "the cross-section, rebuilt");
      assert.strictEqual(restored.options.projection, "source", "the rebuilt cross-section is in the source view");
      let state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.projection, "none");
      from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 1 }]);
      await assertBackOnMollweide(panel, from, "cross-section, rebuilt");

      // Held by a variable nothing places, then written again.
      from = mark(panel);
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from.sent, "lat_bnds drawn");
      from = mark(panel);
      panel.rebuild();
      const flat = await panel.waitSent(isType("gridReady"), from.sent, "lat_bnds, rebuilt");
      assert.strictEqual(flat.options.projection, "source");
      state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.projection, "none");
      from = mark(panel);
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      await assertBackOnMollweide(panel, from, "unplaced variable, rebuilt");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a choice made on the map replaces the held one", async () => {
    const p = await provider();
    const editor = await openEditor(p, FILE);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex("temperature") }));
    try {
      await onMollweide(panel);
      let from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from.sent, "the cross-section");
      from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 1 }]);
      await assertBackOnMollweide(panel, from, "cross-section");

      // Source, chosen on the map, is the choice the next cross-section keeps.
      from = mark(panel);
      await panel.drive([{ id: "picker-projection", value: "source" }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "source", from.sent, "Source");
      from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from.sent, "the cross-section again");
      from = mark(panel);
      await panel.drive([{ id: "slice-y", value: 1 }]);
      const drawn = await panel.waitSent(
        (m: Msg) => m.type === "gridReady" && m.sliceGrid && m.sliceGrid.placed,
        from.sent,
        "the map, on Source",
      );
      assert.strictEqual(drawn.options.projection, "source");
      const state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.resampling, "none", "nothing to resample on Source");
      assert.strictEqual(state.resampling, "bilinear", "kept while hidden");
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
