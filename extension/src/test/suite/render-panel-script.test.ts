// The render panel's own script, run in a real webview through the driver in
// `real-panel.ts`.
//
// - A slice render that fails still moves the projection picker and the Overlay
//   row onto the slice the user picked (#839). They used to follow `gridReady`
//   alone, so an error left them on the previous slice's answer.
// - Every helper the script injects with `.toString()` reaches the page, shown
//   by using it (#841). The script parse check cannot see an unbound name, so a
//   dropped injection parses fine and throws only when it is called.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { reprojectionNote, UNPLACED_OVERLAY_NOTE } from "../../render-panel";
import { extensionPath, isType, openEditor, openReal, overlayTraffic, provider, refusals } from "./real-panel";

/** The note beside the picker for a slice the handle cannot place at all. */
const NOTHING_PLACES = reprojectionNote(false, null);

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

suite("The render panel's own script", function () {
  this.timeout(60_000);

  // A slice panel outlives the editor it was opened from. Close the editor,
  // let the file change on disk, and open it again: the panel now renders from
  // a handle on the new file, which does not have the variable it was drawing.
  // Its next render fails. Here that render is the first one after a restore,
  // the case the issue asks about, with a map target saved.
  test("a slice render that fails withdraws the map targets and overlays (#839)", async () => {
    const p = await provider();
    const native = loadNative();
    assert.ok(native, "native binding required");
    const original = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const replacement = fs.readFileSync(fixture("netcdf_classic_dummy.nc"));
    const temperature = native.NetcdfHandle.fromBytes(original)
      .variables()
      .find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    assert.ok(
      !native.NetcdfHandle.fromBytes(replacement)
        .variables()
        .some((v) => v.variableIndex === temperature.variableIndex),
      "the file it is replaced with has no variable at temperature's index",
    );

    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
    const file = path.join(dir, "changing.nc");
    fs.writeFileSync(file, original);
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () =>
      editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }),
    );
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      let state = await panel.drive([
        { id: "picker-projection", value: "equirectangular" },
        { id: "overlay-coastlines", checked: true },
      ]);
      await panel.waitSent(
        (m) => m.type === "gridReady" && m.options.projection === "equirectangular",
        from,
        "temperature on a map",
      );
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"));
      assert.strictEqual(state.overlayDisabled, false);

      // The file changes under the panel.
      editor.close();
      fs.writeFileSync(file, replacement);
      editor = await openEditor(p, file);

      // The panel is restored, with equirectangular saved. Its first render
      // fails.
      const before = { sent: panel.sent.length, received: panel.received.length };
      panel.rebuild();
      const failed = await panel.waitSent(isType("gridError"), before.sent, "the restored render's error");
      assert.match(failed.error, /render failed/);
      // The page offers what the slice takes: source only, with the reason,
      // and no overlays. Turning on another layer asks for nothing.
      state = await panel.drive([{ id: "overlay-graticule", checked: true }]);
      assert.deepStrictEqual(state.projectionOffers, ["source"], "no map targets for a slice nothing places");
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.reprojectNote, NOTHING_PLACES);
      assert.strictEqual(state.overlayDisabled, true, "the Overlay row is disabled");
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE, "with the reason");
      assert.deepStrictEqual(
        panel.received.slice(before.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay request for the slice that failed",
      );
      assert.deepStrictEqual(
        panel.sent.slice(before.sent).filter(refusals).map((m) => m.type),
        ["gridError"],
        "and no refusal beyond the render's own error",
      );
      // Because the error carries the slice's answers.
      assert.deepStrictEqual(failed.sliceGrid, {
        label: null,
        reprojectable: false,
        note: NOTHING_PLACES,
        placed: false,
      });

      // The file comes back. Picking the variable asks for a render again,
      // which a panel whose first render failed used to have no way to do: it
      // waited for an image before sending any change. The render draws
      // temperature, and the map targets and overlays come back with it.
      editor.close();
      fs.writeFileSync(file, original);
      editor = await openEditor(p, file);
      from = panel.sent.length;
      state = await panel.drive([{ id: "slice-variable", value: temperature.variableIndex }]);
      const drawn = await panel.waitSent(isType("gridReady"), from, "temperature again");
      assert.strictEqual(drawn.sliceGrid.placed, true);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines again");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"), `got ${state.projectionOffers.join(", ")}`);
      assert.strictEqual(state.reprojectNote, null);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // composeProbeValue writes the value part of the readout. Without it on the
  // page, a click on the image throws and the readout stays empty.
  test("a click on the image reads the value there (#841)", async () => {
    const p = await provider();
    const editor = await openEditor(p, fixture("regular_latlon_surface.grib2"));
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.probe();
      const reply = await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      assert.ok(reply.result, "the click lands on the grid");
      assert.strictEqual(typeof reply.result.value, "number", "and finds a value there");
      const state = await panel.drive([]);
      // The readout is "<lat, lon> · <value> <units> · grid i,j"; the value
      // part is composeProbeValue's, five significant figures.
      const value = Number(reply.result.value).toPrecision(5);
      assert.ok(state.probe.includes(value), `readout "${state.probe}" carries ${value}`);
    } finally {
      panel.dispose();
    }
  });
});
