// The render panel hides each control the grid can't use, and says which and
// why in one "Not available for this grid" line under the grid caption
// (#1036).
//
// The panel used to grey such a control out with a note beside it, which read
// as broken. A hidden control has to be hidden on screen, not only marked
// `hidden`: an author `display` rule beats the attribute (#1035), so these
// read each control's computed display. The line is a native `<details>`,
// collapsed, naming the controls by their labels and giving one reason per
// group, structural reasons before not-yet ones. It is absent when nothing is
// hidden and follows the slice a panel moves to.
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { NO_ZONAL_REASON, SATELLITE_NOT_YET_REASON, UNPLACED_REASON } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  overlayTraffic,
  PAGE_CONTROLS,
  type PageControl,
  type PageState,
  provider,
  type RealPanel,
  refusals,
} from "./real-panel";

function crateFixture(crate: string, name: string): string {
  return path.join(extensionPath(), "..", "crates", crate, "tests", "fixtures", name);
}

/** A NetCDF variable's index in `file`, by name. */
function variableIndex(file: string, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
    .variables()
    .find((x) => x.name === name);
  assert.ok(v, `${path.basename(file)} has ${name}`);
  return v.variableIndex;
}

/** The committed §3.20 polar stereographic fixture on an Earth of 80 km with
 *  100 km cells, written twice so the panel has a Vectors row: the same file
 *  as RELEASE-TEST-PLAN §39's `samples/0.6.0/tiny_earth_pair.grib2` (#1002). */
function tinyEarthPair(): Buffer {
  const bytes = Buffer.from(fs.readFileSync(crateFixture("fieldglass-grib2", "polar_stereographic_surface.grib2")));
  let at = 16; // past §0
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    const len = bytes.readUInt32BE(at);
    if (bytes[at + 4] === 3) {
      assert.strictEqual(bytes.readUInt16BE(at + 12), 20, "a §3.20 grid");
      bytes[at + 14] = 1; // octet 15: shape of the Earth, radius given
      bytes[at + 15] = 0; // octet 16: scale factor of the radius
      bytes.writeUInt32BE(80_000, at + 16); // octets 17-20: R = 80 km
      bytes.writeUInt32BE(100_000_000, at + 55); // octets 56-59: Dx, 10^-3 m
      bytes.writeUInt32BE(100_000_000, at + 59); // octets 60-63: Dy
      return Buffer.concat([bytes, bytes]);
    }
    at += len;
  }
  throw new Error("no §3 in the fixture");
}

/** Write `bytes` to a fresh temporary file named `name`; returns its path and
 *  a cleanup. */
function tempFile(name: string, bytes: Buffer): { file: string; done: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1036-"));
  const file = path.join(dir, name);
  fs.writeFileSync(file, bytes);
  return { file, done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

/** The page hides exactly `hidden` of the controls it has, and shows the rest.
 *  `absent` are the controls this panel never has. */
function assertHides(state: PageState, hidden: PageControl[], absent: PageControl[] = []): void {
  for (const key of PAGE_CONTROLS) {
    if (absent.includes(key)) {
      assert.strictEqual(state.controlDisplay[key], null, `${key}: this panel has none`);
    } else if (hidden.includes(key)) {
      assert.strictEqual(state.controlDisplay[key], "none", `${key} is not displayed`);
      assert.strictEqual(state.controlShown[key], false, `${key} takes no room`);
    } else {
      assert.notStrictEqual(state.controlDisplay[key], "none", `${key} is displayed`);
    }
  }
  assert.deepStrictEqual(state.shownWhileHidden, [], "nothing marked hidden is shown");
}

/** The line names `labels` in its summary and gives `items` when opened. */
function assertLine(state: PageState, labels: string[], items: Array<{ kind: string; text: string }>): void {
  const line = state.unavailable;
  assert.ok(line, "the page has the line");
  assert.notStrictEqual(line.display, "none", "the line is shown");
  assert.strictEqual(line.after, "div#projection-summary", "directly under the grid caption");
  assert.strictEqual(line.summary, `Not available for this grid: ${labels.join(", ")}`);
  assert.deepStrictEqual(line.items, items);
  assert.strictEqual(line.live, false, "static, not a live region");
}

function assertNoLine(state: PageState): void {
  assert.ok(state.unavailable, "the page has the line");
  assert.strictEqual(state.unavailable.display, "none", "the line is absent");
}

/** Check every box, hidden controls' included, the way a restored snapshot can
 *  leave them (#1027), and force a re-render, which asks again for whatever
 *  applies. The caller then reads what was asked for. */
async function checkEveryBox(panel: RealPanel): Promise<PageState> {
  const from = panel.sent.length;
  const boxes = ["overlay-coastlines", "overlay-graticule", "overlay-contours", "contours-only", "zonal-toggle"];
  if (panel.html().includes('id="overlay-vectors"')) boxes.push("overlay-vectors");
  await panel.drive(boxes.map((id) => ({ id, checked: true })));
  await panel.drive([{ id: "flip-y", checked: true }]);
  await panel.waitSent(isType("gridReady"), from, "a re-render");
  return panel.drive([]);
}

suite("Controls a grid can't use are hidden, and the panel says why (#1036)", function () {
  this.timeout(120_000);

  test("a satellite swath hides only the zonal average, and says its rows aren't latitudes", async () => {
    const p = await provider();
    // The committed equivalent of samples/mirs_swath.nc: a MiRS swath placed
    // by its 2-D coordinates.
    const file = crateFixture("fieldglass-netcdf", "mirs_swath_n21.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "TPW") }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(first.sliceGrid.placed, true, "a swath is placed");
      assert.strictEqual(first.sliceGrid.supports.zonalMean, false, "and has no rows of latitude");

      let state = await panel.drive([]);
      assertHides(state, ["zonal"], ["arrows"]);
      assertLine(state, ["Zonal average"], [{ kind: "structural", text: `Zonal average: ${NO_ZONAL_REASON}` }]);
      assert.strictEqual(state.unavailable?.open, false, "collapsed");

      state = await panel.drive([{ id: "unavailable", open: true }]);
      assert.strictEqual(state.unavailable?.open, true, "a click on the summary opens it");

      // A checked but hidden Zonal average asks for nothing, through a
      // re-render, which also keeps the line open: only its contents are
      // rewritten.
      const asked = panel.received.length;
      state = await checkEveryBox(panel);
      assert.strictEqual(state.unavailable?.open, true, "still open");
      assert.deepStrictEqual(
        panel.received.slice(asked).filter((m: Msg) => m && m.type === "zonalRequest").map((m: Msg) => m.type),
        [],
        "a checked but hidden Zonal average asks for nothing",
      );
    } finally {
      panel.dispose();
    }
  });

  test("a GRIB grid nothing places hides the projection, overlays, arrows and zonal average, for one reason", async () => {
    const p = await provider();
    const { file, done } = tempFile("tiny_earth_pair.grib2", tinyEarthPair());
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.placement, "unplaceable", "the grid is not placed");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assertHides(state, ["projection", "overlay", "arrows", "zonal"]);
      assert.strictEqual(state.controlShown.contours, false, "contours go with the Overlay row");
      assertLine(
        state,
        ["Projection", "Overlay", "Arrows", "Zonal average"],
        [{ kind: "structural", text: `Projection, Overlay, Arrows, Zonal average: ${UNPLACED_REASON}` }],
      );
      assert.doesNotMatch(panel.html(), /satellite-view/, "nothing blames a satellite view");

      const checked = await checkEveryBox(panel);
      assert.strictEqual(checked.canvasVisibility, "visible", "'only' hides no raster when there are no contours");
      assert.deepStrictEqual(
        panel.received.filter((m: Msg) => overlayTraffic(m) || (m && m.type === "zonalRequest")).map((m: Msg) => m.type),
        [],
        "the hidden boxes ask for nothing",
      );
      assert.deepStrictEqual(panel.sent.filter(refusals).map((m: Msg) => `${m.type}: ${m.error}`), [], "and nothing is refused");
    } finally {
      panel.dispose();
      done();
    }
  });

  test("a satellite view keeps structural and not-yet reasons apart", async () => {
    const p = await provider();
    const file = crateFixture("fieldglass-netcdf", "goes_geostationary.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "Rad") }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      // Placed: the projection and the coastlines stay; contours go from
      // inside the Overlay row.
      assertHides(state, ["contours", "zonal"], ["arrows"]);
      assertLine(
        state,
        ["Zonal average", "Contours", "Long CSV layout"],
        [
          { kind: "structural", text: `Zonal average: ${NO_ZONAL_REASON}` },
          { kind: "notYet", text: `Contours, Long CSV layout: ${SATELLITE_NOT_YET_REASON}` },
        ],
      );
    } finally {
      panel.dispose();
    }
  });

  test("a regular lat/lon grid shows every control, and no line", async () => {
    const p = await provider();
    // Two messages, so the Vectors row is written as well.
    const bytes = fs.readFileSync(crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
    const { file, done } = tempFile("latlon_pair.grib2", Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assertHides(state, []);
      for (const key of PAGE_CONTROLS) assert.strictEqual(state.controlShown[key], true, `${key} is on screen`);
      assertNoLine(state);
    } finally {
      panel.dispose();
      done();
    }
  });

  test("the line follows the variable a slice panel moves to", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const temperature = variableIndex(file, "temperature");
    const latBnds = variableIndex(file, "lat_bnds");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assertHides(state, [], ["arrows"]);
      assertNoLine(state);

      let from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from, "lat_bnds drawn");
      state = await panel.drive([]);
      assertHides(state, ["projection", "overlay", "zonal"], ["arrows"]);
      assertLine(
        state,
        ["Projection", "Overlay", "Zonal average", "Long CSV layout"],
        [{ kind: "structural", text: `Projection, Overlay, Zonal average, Long CSV layout: ${UNPLACED_REASON}` }],
      );

      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === temperature, from, "temperature drawn");
      state = await panel.drive([]);
      assertHides(state, [], ["arrows"]);
      assertNoLine(state);
    } finally {
      panel.dispose();
    }
  });
});
