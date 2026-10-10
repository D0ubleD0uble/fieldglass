// The render panel's own script, run in a real webview through the driver in
// `real-panel.ts`.
//
// - A slice render that fails still moves the projection picker and the Overlay
//   row onto the slice the user picked (#839). They used to follow `gridReady`
//   alone, so an error left them on the previous slice's answer.
// - A slice panel whose file changed under it takes the new file's variables,
//   and finds the one it was showing by name (#839). It used to keep the list
//   it opened with, so a variable number the new file reuses drew another
//   variable under the old name.
// - Every helper the script injects with `.toString()` reaches the page, shown
//   by using it (#841). The script parse check cannot see an unbound name, so a
//   dropped injection parses fine and throws only when it is called.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { reprojectionNote, UNPLACED_OVERLAY_NOTE } from "../../render-panel";
import {
  type Editor,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  overlayTraffic,
  provider,
  type RealPanel,
  refusals,
} from "./real-panel";

/** The note beside the picker for a slice the handle cannot place at all. */
const NOTHING_PLACES = reprojectionNote(false, null);

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

function netcdfFixture(name: string): string {
  return path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", name);
}

/** The variable index `name` has in the NetCDF file `bytes`. */
function indexOf(bytes: Buffer, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.NetcdfHandle.fromBytes(bytes).variables().find((x) => x.name === name);
  assert.ok(v, `the file has ${name}`);
  return v.variableIndex;
}

/** A copy of a NetCDF file in a temporary folder, opened in the editor with a
 *  slice panel on `name`, which can then change on disk under the panel. */
async function changingFile(
  first: Buffer,
  name: string,
): Promise<{ panel: RealPanel; change(bytes: Buffer): Promise<void>; done(): void }> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
  const file = path.join(dir, "changing.nc");
  fs.writeFileSync(file, first);
  let editor: Editor = await openEditor(p, file);
  const variableIndex = indexOf(first, name);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex }));
  return {
    panel,
    // The editor closes, which drops its handle, the file changes, and the
    // editor opens it again.
    change: async (bytes) => {
      editor.close();
      fs.writeFileSync(file, bytes);
      editor = await openEditor(p, file);
    },
    done: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

suite("The render panel's own script", function () {
  this.timeout(60_000);

  // A slice panel outlives the editor it was opened from. Close the editor and
  // the panel has no handle, so every render it asks for fails. Here that
  // render is the first after a restore, the case the issue asks about, with a
  // map target saved.
  test("a slice render that fails withdraws the map targets and overlays (#839)", async () => {
    const original = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const temperature = indexOf(original, "temperature");
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
    const file = path.join(dir, "closing.nc");
    fs.writeFileSync(file, original);
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
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

      // The editor closes under the panel, and the panel is restored with
      // equirectangular saved. Its first render fails.
      editor.close();
      const before = { sent: panel.sent.length, received: panel.received.length };
      panel.rebuild();
      const failed = await panel.waitSent(isType("gridError"), before.sent, "the restored render's error");
      assert.match(failed.error, /disposed/);
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

      // The file is opened again. Picking the variable asks for a render,
      // which a panel whose first render failed used to have no way to do: it
      // waited for an image before sending any change. The render draws
      // temperature, and the map targets and overlays come back with it.
      editor = await openEditor(p, file);
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      const drawn = await panel.waitSent(isType("gridReady"), from, "temperature again");
      assert.strictEqual(drawn.sliceGrid.placed, true);
      // On the map target saved before the editor closed: a missing handle
      // says nothing about the slice, so it did not overwrite that choice.
      assert.strictEqual(drawn.options.projection, "equirectangular", "the saved map target survives");
      await panel.waitSent(isType("overlayReady"), from, "the coastlines again");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"), `got ${state.projectionOffers.join(", ")}`);
      assert.strictEqual(state.projection, "equirectangular");
      assert.strictEqual(state.reprojectNote, null);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // The new file has the variable, under another number, and something else
  // under the old one. The panel is restored after the change.
  test("a slice panel finds its variable by name in a file that changed (#839)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("cf_packed_data.nc"));
    const before = indexOf(first, "temp");
    const after = indexOf(second, "temp");
    assert.notStrictEqual(before, after, "temp moves");
    const { panel, change, done } = await changingFile(first, "temp");
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(opened.messageIndex, before);

      await change(second);
      const from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the restored render");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, after, "temp at its number in the new file");
      assert.match(reply.titleLine, / — temp\b/);
      const state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "temp");
      assert.deepStrictEqual(state.sliceVariables, ["temp"]);
      assert.match(state.title, / — temp\b/);
    } finally {
      done();
    }
  });

  // The new file reuses the variable's number for another variable and does
  // not have it at all. Nothing is drawn under the old name: the panel says
  // the variable is gone and offers the new file's variables.
  test("a slice panel whose variable is gone says so rather than drawing another (#839)", async () => {
    // Both two-dimensional, so the old number draws in the new file.
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    const cmi = indexOf(second, "CMI");
    assert.strictEqual(cmi, indexOf(first, "temp"), "the new file has CMI at temp's number");
    const { panel, change, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // A control changes after the file did. Nothing is drawn under the old
      // name; the panel says why.
      await change(second);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.match(reply.error, /temp is no longer in the file/);
      let state = await panel.drive([]);
      assert.match(state.status, /temp is no longer in the file/);
      assert.deepStrictEqual(state.sliceVariables, ["CMI", "DQF"]);
      assert.ok(!/ — temp\b/.test(state.title), `the heading "${state.title}" does not name temp`);
      // Nothing is chosen, so any variable picked is a change, the first one
      // included; until then no other control asks for a render.
      assert.strictEqual(state.sliceVariable, "", "the picker has nothing chosen");
      const asked = panel.received.length;
      await panel.drive([{ id: "flip-y", checked: false }]);
      assert.deepStrictEqual(
        panel.received.slice(asked).filter((m) => m.type === "rerenderRequest"),
        [],
        "no render before a variable is picked",
      );

      // Picking one of the new file's variables draws it, here the first.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: cmi }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "CMI");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.match(drawn.titleLine, / — CMI\b/);
      state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "CMI");
    } finally {
      done();
    }
  });

  // Two changes leave the page back to back, after the file changed. The first
  // has the panel rewritten; the second is already on its way, naming the
  // variable by its old number, and must not be read from the new file.
  test("requests already sent when the file changed are not read from the new file (#839)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    assert.strictEqual(indexOf(second, "CMI"), indexOf(first, "temp"), "the new file has CMI at temp's number");
    const { panel, change, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(second);
      const from = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([
        { id: "flip-y", checked: true },
        { id: "reverse-colormap", checked: true },
      ]);
      assert.strictEqual(
        panel.received.slice(from.received).filter((m) => m.type === "rerenderRequest").length,
        2,
        "two renders asked for, back to back",
      );
      const refused = await panel.waitSent(isType("gridError"), from.sent, "the refusal");
      assert.match(refused.error, /temp is no longer in the file/);
      await panel.drive([]);
      assert.deepStrictEqual(
        panel.sent.slice(from.sent).filter(drawnOrRefused).map((m: Msg) => `${m.type} ${m.titleLine ?? m.error}`),
        [`gridError ${refused.error}`],
        "nothing drawn from the old numbers",
      );
    } finally {
      done();
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
