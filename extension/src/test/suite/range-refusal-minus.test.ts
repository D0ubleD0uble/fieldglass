// A range refusal names the end it read off the data the way the colour bar
// writes it, with a true minus (U+2212), while a value the user typed keeps
// the form the field holds (#1420). Rust's sentence stays ASCII, as the
// browser host shows it; the extension typesets only that number.

import * as assert from "assert";
import * as path from "path";

import { typesetDataEnd } from "../../provider";
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
    const wind = path.join(extensionPath(), "src", "test", "fixtures", "cmc_wind_300_2010052400_p012.grib");
    const editor = await openEditor(p, wind);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.ok(first.usedMin < 0, "the fixture's field has negative values");
      const bar = (await panel.drive([])).colorbarLabels[0];
      assert.ok(bar.startsWith("−"), bar);
      const from = panel.sent.length;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await panel.drive([MANUAL, { id: "range-min", value: "" }, { id: "range-max", value: 400 }, { id: "scale-log", checked: true }] as any);
      await panel.waitSent(
        (m: Msg) => m.type === "gridError" && m.rangeRefusal?.reason === "log10_not_positive",
        from,
        "the log10 refusal naming the data's minimum",
      );
      const state = await panel.drive([]);
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
