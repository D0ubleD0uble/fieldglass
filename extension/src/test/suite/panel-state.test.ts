// A render panel keeps every control across a rebuild (#1027).
//
// VS Code tears a hidden panel's page down (`retainContextWhenHidden: false`)
// and an imported colour table writes every open panel again, so the page is
// rebuilt from its saved state far more often than it is opened. Arrows, the
// zonal average, the animation and the line plot's axis were left out of that
// state and came back at their defaults.
//
// The test does not list the controls. It moves every enabled control on a real
// page off its value, rebuilds the page, and requires every control to read the
// same as before. A control added later and not saved fails it with no edit
// here. The few it does not move are named below with the reason.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
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

const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** Not moved, though still compared after the rebuild. Each reshapes the panel
 *  rather than setting a value, and is saved through `sliceState` and held by
 *  its own tests (#822, #839). */
const STRUCTURAL = ["slice-variable", "slice-y", "slice-x"];

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

/** Wait until the page and the provider have stopped talking: every render,
 *  overlay and line the moved controls asked for has been answered. */
async function settle(panel: RealPanel): Promise<void> {
  let count = -1;
  let since = Date.now();
  await waitUntil(() => {
    const now = panel.sent.length + panel.received.length;
    if (now !== count) {
      count = now;
      since = Date.now();
      return undefined;
    }
    return Date.now() - since > 750 ? true : undefined;
  }, "the panel to settle");
}

/** Move every control, rebuild, and report the controls that did not come
 *  back as they were. Returns the ids moved. */
async function roundTrip(panel: RealPanel, values: Record<string, string | number>): Promise<string[]> {
  await panel.waitSent(drawnOrRefused, 0, "the first render");
  const { moved, unknown } = await panel.perturb(STRUCTURAL, values);
  assert.deepStrictEqual(unknown, [], "the driver knows how to move every kind of control");
  await settle(panel);
  const before = await panel.controls();

  const from = panel.received.length;
  panel.rebuild();
  await waitUntil(() => panel.received.slice(from).find(isType("ready")), "the rebuilt page");
  await settle(panel);
  const after = await panel.controls();

  const lost = Object.keys(before)
    .filter((id) => after[id] !== before[id])
    .map((id) => `${id}: ${JSON.stringify(before[id])} came back as ${JSON.stringify(after[id])}`);
  assert.deepStrictEqual(lost, [], "every control survives the rebuild");
  return moved;
}

suite("A render panel keeps its controls across a rebuild (#1027)", function () {
  this.timeout(90_000);

  test("a GRIB panel keeps every control, arrows and zonal average included", async () => {
    const p = await provider();
    // Two messages on a placed lat/lon grid, so the Vectors and Compare rows
    // are written and arrows are offered.
    const bytes = fs.readFileSync(fixture("regular_latlon_surface.grib2"));
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1027-"));
    const file = path.join(dir, "pair.grib2");
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const moved = await roundTrip(panel, {
        "range-min": 280,
        "range-max": 300,
        "bounds-lat-min": 5,
        "bounds-lat-max": 25,
        "bounds-lon-min": 10,
        "bounds-lon-max": 50,
      });
      for (const id of ["overlay-vectors", "vector-field-v", "vector-spacing", "color-vectors", "width-vectors", "zonal-toggle"]) {
        assert.ok(moved.includes(id), `the test moved ${id}`);
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF slice panel keeps every control, the animation and the line plot's axis included", async () => {
    const p = await provider();
    const file = fixture("netcdf4_dimscale.nc");
    // `temperature` is time × lat × lon: an Animate row on `time`, and three
    // axes to plot a line along.
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.variableIndex }));
    try {
      const moved = await roundTrip(panel, {
        "range-min": 1,
        "range-max": 2,
        "bounds-lat-min": -5,
        "bounds-lat-max": 5,
        "bounds-lon-min": 10,
        "bounds-lon-max": 200,
      });
      for (const id of ["anim-speed", "anim-loop", "line-axis", "zonal-toggle"]) {
        assert.ok(moved.includes(id), `the test moved ${id}`);
      }
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
