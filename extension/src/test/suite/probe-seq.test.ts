// A probe reply for an older click or an older render is dropped (#1146).
//
// Probe requests carry a sequence number, as arrows, overlays and contours do,
// and the page ignores a reply that is not for its latest click on its latest
// render. Before, a reply held back past a newer render still filled the
// readout and the line plot with the old image's cell, and two quick clicks
// could be answered out of order.
//
// Driven through the real panel: the provider's answers are held back and
// delivered when the test chooses.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel, waitUntil } from "./real-panel";

async function temperaturePanel(): Promise<{ panel: RealPanel; close: () => void }> {
  const p = await provider();
  const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
  const native = loadNative();
  assert.ok(native, "native binding required");
  const temperature = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
    .variables()
    .find((v) => v.name === "temperature");
  assert.ok(temperature, "the fixture has temperature");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

suite("A probe reply for an older click or render is dropped (#1146)", function () {
  this.timeout(120_000);

  test("a reply held past a newer render fills neither the readout nor the line", async () => {
    const { panel, close } = await temperaturePanel();
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const held = panel.hold((m: Msg) => m.type === "probeResult");
      await panel.probe();
      await waitUntil(() => (held.held.length === 1 ? true : undefined), "the held probe reply");
      assert.ok(held.held[0].result, "the click landed on the grid");

      const from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      await panel.waitSent(isType("gridReady"), from, "the newer render");

      await held.deliver(() => true);
      const state = await panel.drive([]);
      assert.strictEqual(state.probe, "", "the old image's cell is not in the readout");
      assert.strictEqual(state.linePlot?.shown, false, "nor in the line plot");
      assert.deepStrictEqual(panel.received.filter(isType("lineRequest")), [], "and no line is asked for");
      await held.release();
    } finally {
      close();
    }
  });

  test("two quick probes answered in reverse order show the second", async () => {
    const { panel, close } = await temperaturePanel();
    try {
      const held = panel.hold((m: Msg) => m.type === "probeResult");
      await panel.probe(0.25);
      await panel.probe(0.75);
      await waitUntil(() => (held.held.length === 2 ? true : undefined), "both held probe replies");
      const [first, second] = held.held.slice();
      assert.notDeepStrictEqual(
        [first.result.gridI, first.result.gridJ],
        [second.result.gridI, second.result.gridJ],
        "the clicks read different cells",
      );
      await held.deliver((m) => m === second);
      const shown = (await panel.drive([])).probe;
      assert.notStrictEqual(shown, "");
      await held.deliver((m) => m === first);
      assert.strictEqual((await panel.drive([])).probe, shown, "the older click's reply does not replace it");
      await held.release();
    } finally {
      close();
    }
  });
});
