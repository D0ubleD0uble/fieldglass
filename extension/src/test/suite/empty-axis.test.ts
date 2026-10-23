// A variable with no cells says why nothing is drawn (#1425).
//
// `netcdf_classic_dummy.nc` has an unlimited `time` with no records, so every
// variable on it is a plane with no cells. `AGC_1202(time, z)` used to render
// as an 8 by 0 raster; the panel's `new ImageData(rgba, 8, 0)` threw, and the
// status stayed on "Rendering…" with nothing drawn and no error. The slice is
// now refused in Rust, naming the empty axis, so the panel says why.
//
// Driven through the real panel, whose own script runs in a real webview.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const FIXTURE = path.join(extensionPath(), "src", "test", "fixtures", "netcdf_classic_dummy.nc");

/** The refusal for `name`'s empty `time` axis, as the provider sends it. */
const refusal = (name: string) => `render failed: nothing to draw: \`time\` (dimension 0 of \`${name}\`) has length 0`;

suite("A variable across an empty axis says there is nothing to draw (#1425)", function () {
  this.timeout(120_000);

  test("AGC_1202 names its empty time axis, shows no colour bar, and the panel stays usable", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.open(fs.readFileSync(FIXTURE)).variables();
    const index = (name: string) => {
      const v = variables.find((x) => x.name === name);
      assert.ok(v, `${name} is offered for rendering`);
      return v.index;
    };
    const agc = index("AGC_1202");
    const cd = index("CD_310");

    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: agc }));
    try {
      const err = await panel.waitSent(isType("gridError"), 0, "the refused first render");
      assert.strictEqual(err.error, refusal("AGC_1202"));
      const state = await panel.drive([]);
      assert.strictEqual(state.status, `Error: ${refusal("AGC_1202")}`, "the status says why, not Rendering…");
      assert.strictEqual(state.canvasPainted, false, "nothing is drawn");
      assert.strictEqual(state.colorbarShown, false, "so there is no colour bar");

      // The cross-section the other way round: time across, z down the rows.
      let from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 1 }]);
      const swapped = await panel.waitSent(
        (m: Msg) => m.type === "gridError" && m.messageIndex === agc,
        from,
        "the swapped axes refused",
      );
      assert.strictEqual(swapped.error, refusal("AGC_1202"));
      const axes = await panel.controls();
      assert.deepStrictEqual([axes["slice-y"], axes["slice-x"]], ["1", "0"], "time is across now");

      // Compare: A − B of the same variable, B at its own indices.
      from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      const compared = await panel.waitSent(isType("gridError"), from, "the Compare refused");
      assert.strictEqual(compared.error, refusal("AGC_1202"));
      await panel.drive([{ id: "compare-op", value: "" }]);

      // The panel still answers a pick: another variable says the same of itself.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: cd }]);
      const other = await panel.waitSent(
        (m: Msg) => m.type === "gridError" && m.messageIndex === cd,
        from,
        "CD_310 refused",
      );
      assert.strictEqual(other.error, refusal("CD_310"));
      const after = await panel.drive([]);
      assert.strictEqual(after.status, `Error: ${refusal("CD_310")}`);
      assert.strictEqual(after.colorbarShown, false);
      assert.strictEqual(after.canvasPainted, false);
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
