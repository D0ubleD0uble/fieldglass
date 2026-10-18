// A colour range Rust refuses is shown at the min/max fields (#1155).
//
// The panel used to drop a manual range it judged wrong (both ends needed,
// `max > min`) and draw the automatic one instead, so a typo looked like the
// field's own colours. Now each end is sent as typed, Rust applies the one
// rule both hosts follow, and a refusal comes back as fields on the error
// (`rangeRefusal`): the panel writes Rust's words under the fields and marks
// the one it names. The log toggle's self-heal reads the refusal's code rather
// than matching "log10" in the text.
//
// Driven through the real panel, whose own script runs in a real webview.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

const FIXTURE = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };

/** Drive `actions`, then wait for the render answer `pred` picks out. Each
 *  control's change asks for a render of its own, so the answers to the
 *  steps in between arrive first. */
async function answer(panel: RealPanel, actions: object[], pred: (m: Msg) => boolean, what: string): Promise<Msg> {
  const from = panel.sent.length;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  await panel.drive(actions as any);
  return panel.waitSent(pred, from, what);
}

/** A refusal about `end`. */
const refusedAt = (end: string, given?: number) => (m: Msg) =>
  m.type === "gridError" && m.rangeRefusal?.end === end && (given === undefined || m.rangeRefusal?.given === given);

suite("A refused colour range is shown at its fields (#1155)", function () {
  this.timeout(120_000);

  test("an inverted range, one end past the data, and a fixed range", async () => {
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      const hi: number = first.usedMax;
      // Contours on, so a refused range has a second place it could be said.
      await answer(panel, [{ id: "overlay-contours", checked: true }], isType("contourReady"), "the contours");

      // Both ends typed, the wrong way round: refused naming both, and both
      // fields marked.
      let got = await answer(
        panel,
        [MANUAL, { id: "range-min", value: 300 }, { id: "range-max", value: 200 }],
        refusedAt("both"),
        "the inverted range refused, not drawn on the auto range",
      );
      assert.deepStrictEqual(
        [got.rangeRefusal?.reason, got.rangeRefusal?.end],
        ["inverted", "both"],
        "the refusal arrives as fields",
      );
      let state = await panel.drive([]);
      assert.strictEqual(state.rangeNote, "minimum 300 is not below maximum 200; set a minimum below the maximum");
      assert.deepStrictEqual(state.rangeInvalid, ["range-min", "range-max"]);
      // Said once: the status line points at the fields, and contours, which
      // share the range, are dropped rather than refused in the same words,
      // even when their own control asks for them again.
      assert.strictEqual(state.status, "Colour range not drawn: see the range fields");
      const asked = panel.received.length;
      const told = panel.sent.length;
      state = await panel.drive([{ id: "contour-interval", value: 5 }]);
      assert.strictEqual(state.contourStatus, "", "no contour error repeats the refusal");
      assert.ok(
        !panel.received.slice(asked).some(isType("contourRequest")),
        "no contours are asked for over a refused range",
      );
      assert.ok(!panel.sent.slice(told).some(isType("contourError")));
      await panel.drive([{ id: "contour-interval", value: "" }]);

      // A minimum past the data's maximum, with the maximum left blank: the
      // other end came from the data, and the message says so.
      const past = Math.ceil(hi) + 1000;
      got = await answer(
        panel,
        [{ id: "range-max", value: "" }, { id: "range-min", value: past }],
        refusedAt("min", past),
        "the minimum past the data refused",
      );
      assert.deepStrictEqual([got.rangeRefusal?.end, got.rangeRefusal?.given], ["min", past]);
      state = await panel.drive([]);
      assert.ok(state.rangeNote?.includes("(taken from the data); set a maximum too"), String(state.rangeNote));
      assert.deepStrictEqual(state.rangeInvalid, ["range-min"], "only the minimum is marked");

      // One end alone, inside the data, draws: the other comes from the
      // field, and the note and the marks go.
      const lo = Math.floor(hi) - 10;
      got = await answer(
        panel,
        [{ id: "range-min", value: lo }],
        (m: Msg) => m.type === "gridReady" && m.usedMin === lo,
        "one manual end drawn, not dropped",
      );
      assert.deepStrictEqual([got.usedMin, got.usedMax], [lo, hi]);
      assert.strictEqual(got.range.rangeSource, "manual_min");
      state = await panel.drive([]);
      assert.strictEqual(state.rangeNote, null);
      assert.deepStrictEqual(state.rangeInvalid, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("log10 on a field with no positive floor drops back to linear, by the refusal's code", async () => {
    const p = await provider();
    // Two copies of one message: A − B is zero everywhere, a floor log10
    // has no logarithm for.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1155-"));
    const file = path.join(dir, "pair.grib2");
    const bytes = fs.readFileSync(FIXTURE);
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let got = await answer(
        panel,
        [{ id: "scale-log", checked: true }],
        (m: Msg) => m.type === "gridReady" && m.options.scaleMode === "log10",
        "the field drawn on log10, having a positive floor",
      );

      const from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      const refused = await panel.waitSent(isType("gridError"), from, "the refused log10 difference");
      assert.strictEqual(refused.rangeRefusal?.reason, "log10_not_positive");
      got = await panel.waitSent(
        (m: Msg) => m.type === "gridReady" && m.options.scaleMode === undefined,
        from,
        "the linear redraw",
      );
      assert.strictEqual(got.options.scaleMode, undefined, "drawn linearly");
      const state = await panel.drive([]);
      assert.strictEqual(state.rangeNote, null, "an automatic range is healed, not reported at the fields");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
