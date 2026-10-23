// A negative value is written with a true minus sign (U+2212) wherever the
// panel shows it, as the tick labels and the line plot's caption already did
// (#1390). What a user may copy back into a number field, or save, stays
// ASCII: the manual range fields and the CSV export.

import * as assert from "assert";
import * as path from "path";

import { composeProbeValue, composeRangeText } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const MINUS = "−";

suite("Negative values use a true minus sign (#1390)", function () {
  this.timeout(120_000);

  test("the probe readout", () => {
    const one = { fullDetailValue: null, fullDetailTruncation: null };
    assert.strictEqual(composeProbeValue({ value: -12.3456, ...one }, "K"), `${MINUS}12.346 K`);
    assert.strictEqual(composeProbeValue({ value: -1.5e-7, ...one }, ""), `${MINUS}1.5000e${MINUS}7`);
    // Both numbers of a band-limited map's readout carry it.
    assert.strictEqual(
      composeProbeValue({ value: -3.25, fullDetailValue: -4.5, fullDetailTruncation: 383 }, "K"),
      `${MINUS}3.2500 K shown · ${MINUS}4.5000 K at full detail (T383)`,
    );
    // A positive value, and a unit that holds a hyphen, are left alone.
    assert.strictEqual(composeProbeValue({ value: 5, ...one }, "m s-1"), "5.0000 m s-1");
    assert.strictEqual(composeProbeValue({ value: null, ...one }, "K"), "no data");
  });

  test("the status line's range", () => {
    assert.strictEqual(composeRangeText(-30, 30), `range ${MINUS}30.00 … 30.00`);
    assert.strictEqual(composeRangeText(-30, -10), `range ${MINUS}30.00 … ${MINUS}10.00`);
    assert.strictEqual(composeRangeText(0.5, 2), "range 0.5000 … 2.000");
    assert.strictEqual(composeRangeText(null, null), "no values");
  });

  test("the color bar and status of a panel, while its fields stay ASCII", async () => {
    const p = await provider();
    const fixture = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const editor = await openEditor(p, fixture);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.drive([
        { selector: 'input[name="range-mode"][value="manual"]', checked: true },
        { id: "range-min", value: -30 },
        { id: "range-max", value: 30 },
      ]);
      await panel.waitSent(
        (m: Msg) => m.type === "gridReady" && m.usedMin === -30 && m.usedMax === 30,
        from,
        "the manual range",
      );
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.colorbarLabels, [`${MINUS}30.00`, "30.00"]);
      assert.ok(state.status.includes(`range ${MINUS}30.00 … 30.00`), state.status);
      // The field a user types in or copies from is never typeset.
      assert.deepStrictEqual(state.rangeFields, ["-30", "30"]);
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
