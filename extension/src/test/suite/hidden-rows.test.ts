// A toolbar row the panel puts away with `hidden` is not displayed (#1035).
//
// `.toolbar-row { display: flex }` is an author rule, and an author rule beats
// the user-agent `[hidden] { display: none }`, so the Message row (a GRIB
// panel's) and the Animate row (a slice panel's) showed on every panel,
// empty. Checking the `hidden` attribute cannot see that, so these read the
// computed `display` in a real webview, through the driver in `real-panel.ts`.
// The driver also lists every element on the page that carries `hidden` and is
// displayed anyway, which sweeps the whole panel for the same trap.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, openEditor, openReal, provider } from "./real-panel";

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

/** One GRIB2 message (2 m temperature) with its §4 parameter number set. */
function field(number: number): Buffer {
  const bytes = fs.readFileSync(fixture("regular_latlon_surface.grib2"));
  let at = 16;
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    if (bytes[at + 4] === 4) {
      bytes[at + 10] = number;
      return bytes;
    }
    at += bytes.readUInt32BE(at);
  }
  throw new Error("the fixture has no §4");
}

function variable(file: string, name: string) {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const found = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
    .variables()
    .find((v) => v.name === name);
  assert.ok(found, `the fixture has ${name}`);
  return found;
}

suite("Hidden toolbar rows are not displayed (#1035)", function () {
  this.timeout(60_000);

  test("a fresh GRIB panel shows no Message row, and nothing else hidden", async () => {
    const p = await provider();
    const editor = await openEditor(p, fixture("regular_latlon_surface.grib2"));
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assert.strictEqual(state.messageRowDisplay, "none");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a 2-D NetCDF variable shows no Animate row, and nothing else hidden", async () => {
    const p = await provider();
    const file = fixture("netcdf4_dimscale.nc");
    const latBnds = variable(file, "lat_bnds");
    assert.strictEqual(latBnds.dims.length, 2, "nothing to animate");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: latBnds.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assert.strictEqual(state.animateRowDisplay, "none");
      assert.strictEqual(state.messageRowDisplay, null, "a slice panel has no Message row");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("the Animate row shows when the variable has an axis to step", async () => {
    const p = await provider();
    const file = fixture("netcdf4_dimscale.nc");
    const temperature = variable(file, "temperature");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assert.notStrictEqual(state.animateRowDisplay, "none");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  // The path #959 added: the file changes and the message is gone.
  test("the Message row shows, with options, after the file changes ambiguously", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1035-"));
    const file = path.join(dir, "changing.grib2");
    fs.writeFileSync(file, Buffer.concat([field(0), field(2)]));
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      editor.close();
      fs.writeFileSync(file, Buffer.concat([field(2), field(3)]));
      editor = await openEditor(p, file);
      const from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      await panel.waitSent(isType("gridError"), from, "the refusal");
      const state = await panel.drive([]);
      assert.notStrictEqual(state.messageRowDisplay, "none");
      assert.strictEqual(state.messageOffers?.length, 2);
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
