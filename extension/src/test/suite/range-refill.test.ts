// The Manual range's pre-filled ends follow the field drawn (#1475).
//
// The two ends were filled once, from the first picture, and never again, so
// ticking Manual after picking another message or variable drew the new field
// on the first one's range. While the range is on Auto the page now refills
// every end that still holds the value it gave it, and leaves one the user
// typed alone. Run in a real webview through the driver in `real-panel.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  classicNetcdf,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
} from "./real-panel";

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };
const AUTO = { selector: 'input[name="range-mode"][value="auto"]', checked: true };
const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** The fixture's one GRIB2 message as parameter `number`, with its §5
 *  reference value (octets 12-15, a float32) set to `reference`: the packed
 *  values are the same, so the field's range moves by the difference. */
function field(number: number, reference: number): Buffer {
  const bytes = fs.readFileSync(
    path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"),
  );
  let at = 16;
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    if (bytes[at + 4] === 4) bytes[at + 10] = number;
    if (bytes[at + 4] === 5) bytes.writeFloatBE(reference, at + 11);
    at += bytes.readUInt32BE(at);
  }
  return bytes;
}

suite("The Manual range's ends follow the field drawn (#1475)", function () {
  this.timeout(120_000);

  test("a GRIB message switch refills the ends, and Manual draws what Auto drew", async () => {
    const before = field(0, 270);
    const after = Buffer.concat([field(2, 5), field(3, 900)]);
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1475-"));
    const file = path.join(dir, "switch.grib2");
    fs.writeFileSync(file, before);
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first message");
      const filled = (await panel.drive([])).rangeFields.map(Number);
      assert.ok(Math.abs(filled[0] - first.usedMin) < 0.01 && Math.abs(filled[1] - first.usedMax) < 0.01, `filled from the first: ${filled}`);

      // The file changes to two other fields; picking one draws it.
      editor.close();
      fs.writeFileSync(file, after);
      editor = await openEditor(p, file);
      let from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridError"), from, "the refusal");
      from = panel.sent.length;
      await panel.drive([{ id: "message-pick", value: 1 }]);
      const second = await panel.waitSent(drawnOrRefused, from, "the picked message");
      assert.strictEqual(second.type, "gridReady", `${second.error ?? ""}`);
      assert.ok(second.usedMin > 800, `a different range: ${second.usedMin}`);
      const state = await panel.drive([]);
      assert.ok(
        Math.abs(Number(state.rangeFields[0]) - second.usedMin) < 0.01 && Math.abs(Number(state.rangeFields[1]) - second.usedMax) < 0.01,
        `the ends are the second field's: ${state.rangeFields}`,
      );

      from = panel.sent.length;
      await panel.drive([MANUAL]);
      const manual = await panel.waitSent(drawnOrRefused, from, "Manual");
      assert.strictEqual(manual.type, "gridReady", `${manual.error ?? ""}`);
      assert.deepStrictEqual([manual.options.rangeMin, manual.options.rangeMax], [second.usedMin, second.usedMax]);
      assert.ok(Buffer.from(manual.rgba).equals(Buffer.from(second.rgba)), "the same picture as Auto");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  /** A panel on the first of two variables of a classic NetCDF file. */
  async function twoVariables(): Promise<{ panel: RealPanel; a: Msg; index: number[]; close: () => void }> {
    const bytes = classicNetcdf(
      [["y", 4], ["x", 4]],
      [
        { name: "a", dims: ["y", "x"], attrs: {}, values: Array.from({ length: 16 }, (_, k) => 280 + k) },
        { name: "b", dims: ["y", "x"], attrs: {}, values: Array.from({ length: 16 }, (_, k) => 0.25 + k * 0.5) },
      ],
    );
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1475-"));
    const file = path.join(dir, "two.nc");
    fs.writeFileSync(file, bytes);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const vars = native.open(bytes).variables();
    const index = ["a", "b"].map((n) => vars.find((v) => v.name === n)!.index);
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index[0] }));
    const a = await panel.waitSent(isType("gridReady"), 0, "a drawn");
    return { panel, a, index, close: () => { panel.dispose(); editor.close(); fs.rmSync(dir, { recursive: true, force: true }); } };
  }

  test("a NetCDF variable switch refills the ends, and Manual draws what Auto drew", async () => {
    const { panel, a, index, close } = await twoVariables();
    try {
      assert.deepStrictEqual([a.usedMin, a.usedMax], [280, 295]);
      let from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: index[1] }]);
      const b = await panel.waitSent(isType("gridReady"), from, "b drawn");
      assert.deepStrictEqual([b.usedMin, b.usedMax], [0.25, 7.75]);
      const shown = (await panel.drive([])).rangeFields;
      assert.deepStrictEqual(shown.map(Number), [0.25, 7.75], "b's ends");

      from = panel.sent.length;
      await panel.drive([MANUAL]);
      const manual = await panel.waitSent(drawnOrRefused, from, "Manual");
      assert.strictEqual(manual.type, "gridReady", `${manual.error ?? ""}`);
      assert.deepStrictEqual([manual.options.rangeMin, manual.options.rangeMax], [0.25, 7.75]);
      assert.ok(Buffer.from(manual.rgba).equals(Buffer.from(b.rgba)), "the same picture as Auto");

      // Shown again, the panel restores what it showed.
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(drawnOrRefused, from, "after a hide and show");
      assert.deepStrictEqual((await panel.drive([])).rangeFields, shown);
    } finally {
      close();
    }
  });

  test("an end the user typed survives a later variable switch on Auto; the other refills", async () => {
    const { panel, index, close } = await twoVariables();
    try {
      await panel.drive([{ id: "range-min", value: 285 }, AUTO]);
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: index[1] }]);
      await panel.waitSent(isType("gridReady"), from, "b drawn");
      const shown = (await panel.drive([])).rangeFields;
      assert.deepStrictEqual(shown.map(Number), [285, 7.75], "the typed minimum stays, the maximum follows");
    } finally {
      close();
    }
  });
});
