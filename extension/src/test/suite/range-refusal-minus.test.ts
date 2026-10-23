// A range refusal names the end it read off the data the way the colour bar
// writes it, with a true minus (U+2212), while a value the user typed keeps
// the form the field holds (#1420). Rust's sentence stays ASCII, as the
// browser host shows it; the extension typesets only that number.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { typesetDataEnd } from "../../provider";
import { typesetMinus } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };

suite("A range refusal writes the data's end as the colour bar does (#1420)", function () {
  this.timeout(120_000);

  test("only the number before '(taken from the data)' is typeset", () => {
    assert.strictEqual(
      typesetDataEnd(
        "a log10 scale needs a finite, positive minimum; the range starts at -30.00 (taken from the data); set a minimum above 0",
      ),
      "a log10 scale needs a finite, positive minimum; the range starts at −30.00 (taken from the data); set a minimum above 0",
    );
    // A typed value keeps its hyphen; the data end beside it does not.
    assert.strictEqual(
      typesetDataEnd("minimum -30 is above the field's maximum -1.5e-7 (taken from the data); set a maximum too"),
      "minimum -30 is above the field's maximum −1.5e−7 (taken from the data); set a maximum too",
    );
    assert.strictEqual(
      typesetDataEnd("minimum -30 is not below maximum -40; set a minimum below the maximum"),
      "minimum -30 is not below maximum -40; set a minimum below the maximum",
    );
  });

  test("a negative minimum under log10 reads as the colour bar does", async () => {
    const p = await provider();
    // ERSST's sst has a minimum of -1.8 (sea ice), so the data's own end is negative.
    const file = path.join(extensionPath(), "src", "test", "fixtures", "ersst_v5_187001_cdf1.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const sst = native
      .open(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "sst");
    assert.ok(sst, "sst is renderable");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: sst.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Log is only offered over a positive floor, so draw on log with a typed
      // minimum, then clear it: the minimum comes from the data, and is -1.8.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const typed: any[] = [MANUAL, { id: "range-min", value: 1 }, { id: "range-max", value: 40 }, { id: "scale-log", checked: true }];
      let from = panel.sent.length;
      await panel.drive(typed);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.scaleMode === "log10", from, "log10 drawn");
      from = panel.sent.length;
      await panel.drive([{ id: "range-min", value: "" }]);
      const refused = await panel.waitSent(
        (m: Msg) =>
          m.type === "gridError" &&
          m.rangeRefusal?.reason === "log10_not_positive" &&
          (m.rangeRefusal.fieldMin ?? 0) < 0,
        from,
        "the log10 refusal naming the data's negative minimum",
      );
      const state = await panel.drive([]);
      // The bar writes an end as `typesetMinus(v.toPrecision(4))` (render-panel.ts).
      const bar = typesetMinus((refused.rangeRefusal?.fieldMin as number).toPrecision(4));
      assert.ok(bar.startsWith("−"), `the colour bar writes it with a true minus: ${bar}`);
      assert.ok(
        state.rangeNote?.includes(`starts at ${bar} (taken from the data)`),
        `${state.rangeNote} vs bar ${bar}`,
      );
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
