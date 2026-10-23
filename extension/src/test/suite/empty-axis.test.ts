// A variable with no cells says why nothing is drawn (#1425).
//
// `netcdf_classic_dummy.nc` has an unlimited `time` with no records, so every
// variable on it is a plane with no cells. `AGC_1202(time, z)` used to render
// as an 8 by 0 raster; the panel's `new ImageData(rgba, 8, 0)` threw, and the
// status stayed on "Rendering…" with nothing drawn and no error. The slice is
// now refused in Rust, naming the empty axis, and the panel says so as a plain
// status, the way it says "no values" of a field with no values (#871): not an
// error, and not a failed render.
//
// Driven through the real panel, whose own script runs in a real webview.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const FIXTURE = path.join(extensionPath(), "src", "test", "fixtures", "netcdf_classic_dummy.nc");

/** Rust's words for `name`'s empty `time` axis, which the panel shows as they
 *  are. */
const refusal = (name: string) => `nothing to draw: \`time\` (dimension 0 of \`${name}\`) has length 0`;

/** A `gridError` the provider sent as a field with no cells, for `index`. */
const noCells = (m: Msg, index: number) =>
  m.type === "gridError" && m.messageIndex === index && m.nothingToDraw === true;

/** The page shows `name`'s refusal as a plain status, with nothing drawn and
 *  no colour bar, and Export PNG does not call it a failed render. */
async function assertPlain(panel: RealPanel, name: string, what: string) {
  const state = await panel.drive([{ id: "export-png", click: true }]);
  assert.strictEqual(state.status, refusal(name), `${what}: the status says why, not Rendering…`);
  assert.ok(!state.status.includes("Error") && !state.status.includes("render failed"), state.status);
  assert.strictEqual(state.canvasPainted, false, `${what}: nothing is drawn`);
  assert.strictEqual(state.colorbarShown, false, `${what}: so there is no colour bar`);
  assert.ok(!state.exportStatus.includes("failed"), `${what}: ${state.exportStatus}`);
  assert.ok(state.exportStatus.startsWith("Nothing to export"), `${what}: ${state.exportStatus}`);
}

suite("A variable across an empty axis says there is nothing to draw (#1425)", function () {
  this.timeout(120_000);

  test("AGC_1202 names its empty time axis as a plain status, and the panel stays usable", async () => {
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
      const first = await panel.waitSent(isType("gridError"), 0, "the refused first render");
      assert.strictEqual(first.nothingToDraw, true, "said as a field with no cells");
      assert.strictEqual(first.error, refusal("AGC_1202"));
      await assertPlain(panel, "AGC_1202", "the first render");

      // The cross-section the other way round: time across, z down the rows.
      let from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 1 }]);
      const swapped = await panel.waitSent((m: Msg) => noCells(m, agc), from, "the swapped axes refused");
      assert.strictEqual(swapped.error, refusal("AGC_1202"));
      const axes = await panel.controls();
      assert.deepStrictEqual([axes["slice-y"], axes["slice-x"]], ["1", "0"], "time is across now");

      // Compare, as the page asks for it: A − B, Field B the same variable.
      from = panel.sent.length;
      const asked = panel.received.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      const compareAsk = await waitUntil(
        () =>
          panel.received
            .slice(asked)
            .filter(isType("rerenderRequest"))
            .find((m: Msg) => m.compare?.op === "a_minus_b"),
        "the page asks for A − B",
      );
      const compared = await panel.waitSent(
        (m: Msg) => noCells(m, agc) && m.seq === compareAsk.seq,
        from,
        "the Compare refused",
      );
      assert.strictEqual(compared.error, refusal("AGC_1202"));
      // Field B another variable, CD_310: the same request with B's variable
      // named in its rider, as the provider's combined path takes it.
      from = panel.sent.length;
      const seqB = compareAsk.seq + 100;
      await panel.sendAsPage({ ...compareAsk, seq: seqB, compare: { ...compareAsk.compare, variableIndexB: cd } });
      const comparedB = await panel.waitSent(
        (m: Msg) => noCells(m, agc) && m.seq === seqB,
        from,
        "A − CD_310 refused",
      );
      assert.strictEqual(comparedB.error, refusal("AGC_1202"));
      await assertPlain(panel, "AGC_1202", "Compare");
      await panel.drive([{ id: "compare-op", value: "" }]);

      // The panel still answers a pick: another variable says the same of itself.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: cd }]);
      const other = await panel.waitSent((m: Msg) => noCells(m, cd), from, "CD_310 refused");
      assert.strictEqual(other.error, refusal("CD_310"));
      await assertPlain(panel, "CD_310", "another variable");
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
