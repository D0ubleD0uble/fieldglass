// The render panel offers its overlays only for a field placed on the Earth
// (#840).
//
// Coastlines, the other map layers, the graticule, contours and arrows all
// project through the field's geometry. A field nothing places (a NetCDF slice
// with no coordinates, a cross-section, a GRIB grid whose projection cannot
// place it) still draws in its own grid coordinates, but every one of those
// requests is refused, and the panel used to send them anyway and show the
// refusal as an error.
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`: the panel is opened the way a user opens it, the driver
// flips the controls a user would, and the test reads what crossed the
// boundary in both directions.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { UNPLACED_OVERLAY_NOTE, UNPLACED_VECTORS_NOTE } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  overlayTraffic,
  provider,
  refusals,
} from "./real-panel";

suite("Overlays follow placement (#840)", function () {
  this.timeout(60_000);

  test("a NetCDF slice panel drops the overlays on a slice it cannot place, and takes them back", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables();
    const index = (name: string) => {
      const v = variables.find((x) => x.name === name);
      assert.ok(v, `the fixture has ${name}`);
      return v.variableIndex;
    };
    const temperature = index("temperature");
    const latBnds = index("lat_bnds");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // temperature is placed: the row is offered, and coastlines and
      // contours on an equirectangular map are drawn.
      let from = panel.sent.length;
      let state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      from = panel.sent.length;
      await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-graticule", checked: true },
        { id: "overlay-contours", checked: true },
      ]);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");
      await panel.waitSent(isType("contourReady"), from, "the contours");

      // Onto lat_bnds, which has no coordinates. The provider draws it in the
      // source view and the picker follows; the panel asks for no overlay and
      // so is refused nothing.
      const before = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      const drawn = await panel.waitSent(
        (m) => m.type === "gridReady" && m.messageIndex === latBnds,
        before.sent,
        "lat_bnds drawn",
      );
      assert.strictEqual(drawn.sliceGrid.placed, false, "the render says the slice is not placed");
      assert.strictEqual(drawn.options.projection, "source");
      state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.overlayDisabled, true, "the Overlay row is disabled");
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE, "with the reason");
      assert.deepStrictEqual(
        panel.received.slice(before.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay, contour or arrow request for lat_bnds",
      );
      assert.deepStrictEqual(
        panel.sent.slice(before.sent).filter(refusals).map((m) => `${m.type}: ${m.error}`),
        [],
        "and no refusal",
      );
      // The toggles keep what the user chose, for the next field that takes them.
      assert.deepStrictEqual([state.coastlines, state.graticule, state.contours], [true, true, true]);

      // "Contours only" hides the raster. With no contours to show, lat_bnds
      // would be a blank panel, so the raster stays.
      state = await panel.drive([{ id: "contours-only", checked: true }]);
      assert.strictEqual(state.canvasVisibility, "visible", "the raster stays without contours");
      state = await panel.drive([{ id: "contours-only", checked: false }]);

      // Back onto temperature: the row comes back, and the overlays are asked
      // for and drawn again without the user touching them.
      const back = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      const placed = await panel.waitSent(
        (m) => m.type === "gridReady" && m.messageIndex === temperature,
        back,
        "temperature drawn",
      );
      assert.strictEqual(placed.sliceGrid.placed, true);
      await panel.waitSent(isType("overlayReady"), back, "the coastlines again");
      await panel.waitSent(isType("contourReady"), back, "the contours again");
      state = await panel.drive([]);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);

      // A cross-section is not placed either: time down the rows of
      // temperature. Its axes are not a map, so nothing is asked for.
      const cross = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([{ id: "slice-y", value: 0 }]);
      const plane = await panel.waitSent(isType("gridReady"), cross.sent, "the cross-section");
      assert.strictEqual(plane.sliceGrid.placed, false, "a cross-section is not placed");
      state = await panel.drive([]);
      assert.strictEqual(state.overlayHidden, true, "the Overlay row is put away, as before");
      assert.deepStrictEqual(
        panel.received.slice(cross.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay request for a cross-section",
      );
      assert.deepStrictEqual(panel.sent.slice(cross.sent).filter(refusals).map((m) => m.error), []);
    } finally {
      panel.dispose();
    }
  });

  test("a NetCDF slice panel opened on an unplaceable slice starts with the overlays disabled", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const latBnds = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "lat_bnds");
    assert.ok(latBnds, "the fixture has lat_bnds");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: latBnds.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([{ id: "overlay-coastlines", checked: true }]);
      assert.strictEqual(state.overlayDisabled, true);
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE);
      assert.deepStrictEqual(panel.received.filter(overlayTraffic).map((m) => m.type), []);
      assert.deepStrictEqual(panel.sent.filter(refusals).map((m) => m.error), []);
    } finally {
      panel.dispose();
    }
  });

  test("a GRIB panel on a grid nothing places offers no overlays or arrows", async () => {
    const p = await provider();
    // Two messages, so the Vectors row is written. The committed polar
    // stereographic fixture declares degenerate projection parameters, so its
    // grid is unplaceable (#776), and copied twice it is its own u/v pair.
    const bytes = fs.readFileSync(
      path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "polar_stereographic_surface.grib2"),
    );
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-840-"));
    const file = path.join(dir, "unplaced_pair.grib2");
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.placement, "unplaceable");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-contours", checked: true },
        { id: "overlay-vectors", checked: true },
      ]);
      assert.strictEqual(state.overlayDisabled, true);
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE);
      assert.strictEqual(state.vectorDisabled, true);
      assert.strictEqual(state.vectorNote, UNPLACED_VECTORS_NOTE);
      assert.deepStrictEqual(panel.received.filter(overlayTraffic).map((m) => m.type), []);
      assert.deepStrictEqual(panel.sent.filter((m: Msg) => m && /Error$|vectorResult/.test(m.type)), []);
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
