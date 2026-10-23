// Animating a slice panel along its time axis (#170).
//
// What is pinned: the addon reports which axis is time for the CF and WRF
// spellings a file uses; the axis a panel offers first and the frame playback
// steps to are the pure functions the panel script runs as serialized copies;
// a colour range locked for playback holds across frames whose own extremes
// differ; and the transport controls are in a slice panel's markup, wired, and
// absent from a GRIB panel. Through the real panel: a frame paused on a held
// range keeps it through a hide/show and a rebuild, and a step or a control
// change then takes the frame's own range, as does a file changed on disk or
// reverted (#1227).

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type VariableInfo } from "../../native";
import {
  defaultAnimationDim,
  nextFrame,
  renderImagePanelHtml,
  typesetMinus,
  type PanelField,
  type SlicePanelData,
} from "../../render-panel";
import { classicNetcdf, isType, openEditor, openReal, provider, type Msg, type RealPanel } from "./real-panel";
import { waitFor } from "./themed-panel";

function fixture(name: string): Buffer {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return fs.readFileSync(path.join(ext.extensionPath, "src", "test", "fixtures", name));
}

function variable(file: string, name: string): VariableInfo {
  const native = loadNative();
  assert.ok(native, "native module must load");
  const found = native.open(fixture(file))
    .variables()
    .find((v) => v.name === name || v.name.endsWith(`/${name}`));
  assert.ok(found, `${file} has ${name}`);
  return found;
}

/** Each step's own extremes, every one different, so a held range shows. */
const STEPS: Array<[number, number]> = [[250, 261], [240, 275], [230, 290], [220, 300]];

/** The colour bar's labels for a range, as the panel writes them. */
const labels = ([lo, hi]: [number, number]): [string, string] => [typesetMinus(lo.toPrecision(4)), typesetMinus(hi.toPrecision(4))];

const drawnOrFailed = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** The same file with other extremes at every step, for a file that changes. */
const CHANGED: Array<[number, number]> = [[100, 111], [90, 125], [80, 140], [70, 150]];

/** A classic NetCDF file of `air(time, lat, lon)` whose steps range over `steps`. */
function stepsFile(steps: Array<[number, number]>): Buffer {
  return classicNetcdf(
    [["time", steps.length], ["lat", 3], ["lon", 4]],
    [
      { name: "time", dims: ["time"], attrs: { units: "hours since 2024-01-01 00:00:00" }, values: steps.map((_, t) => 6 * t) },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 90, 180, 270] },
      {
        name: "air",
        dims: ["time", "lat", "lon"],
        attrs: { units: "K" },
        values: steps.flatMap(([lo, hi]) => Array.from({ length: 12 }, (_, k) => lo + ((hi - lo) * k) / 11)),
      },
    ],
  );
}

/** `stepsFile(STEPS)` in a real slice panel once its first render has landed.
 *  `change` rewrites the file as the editor closes and opens it again (#839);
 *  `revert` rewrites it and runs File: Revert File on it (#1217). */
async function openSteps(): Promise<{
  panel: RealPanel;
  change: (bytes: Buffer) => Promise<void>;
  revert: (bytes: Buffer) => Promise<void>;
  close: () => void;
}> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-held-"));
  const file = path.join(dir, "steps.nc");
  fs.writeFileSync(file, stepsFile(STEPS));
  const native = loadNative();
  assert.ok(native, "native binding required");
  const air = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "air");
  assert.ok(air, "air is renderable");
  let editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: air.index }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    change: async (bytes) => {
      editor.close();
      fs.writeFileSync(file, bytes);
      editor = await openEditor(p, file);
    },
    revert: async (bytes) => {
      fs.writeFileSync(file, bytes);
      await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
    },
    close: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** Press play, let one frame land, and press pause: the frame shown is then
 *  the one after `step`, drawn on `step`'s range. The speed is slow enough
 *  that the pause lands before the next frame is asked for. */
async function playOneFrame(panel: RealPanel, step: number, steps = STEPS): Promise<void> {
  const from = panel.sent.length;
  await panel.drive([{ id: "anim-play", click: true }]);
  const frame = await panel.waitSent(drawnOrFailed, from, "a frame under playback");
  assert.strictEqual(frame.type, "gridReady", frame.error ?? "");
  const paused = await panel.drive([{ id: "anim-play", click: true }]);
  const next = (step + 1) % steps.length;
  assert.ok((paused.animateLabel ?? "").includes(`step ${next} of`), `paused on step ${next}, got ${paused.animateLabel}`);
  assert.deepStrictEqual(paused.colorbarLabels, labels(steps[step]), `step ${next} is drawn on step ${step}'s range`);
}

/** Read the page until its colour bar reads `want`. */
async function colourBar(panel: RealPanel, want: [string, string], what: string): Promise<void> {
  await waitFor(panel, (s) => s.colorbarLabels[0] === want[0] && s.colorbarLabels[1] === want[1], what).catch(
    async (err: Error) => {
      const got = (await panel.drive([])).colorbarLabels;
      throw new Error(`${err.message}: the colour bar reads ${got.join(" … ")}, not ${want.join(" … ")}`);
    },
  );
}

suite("Time animation", () => {
  test("the addon reports the time axis of a CF variable", () => {
    const temperature = variable("netcdf4_dimscale.nc", "temperature");
    assert.strictEqual(temperature.detectedTimeDim, 0);
    assert.strictEqual(temperature.dims[0].name, "time");
    assert.strictEqual(temperature.dims[0].length, 2, "two steps to animate");
    // ERSST has a time axis one step long: detected, but nothing to play.
    assert.strictEqual(variable("ersst_v5_187001_cdf1.nc", "sst").detectedTimeDim, 0);
  });

  test("nextFrame steps, wraps when looping, and stops at the end when not", () => {
    assert.strictEqual(nextFrame(0, 5, 1, true), 1);
    assert.strictEqual(nextFrame(4, 5, 1, true), 0);
    assert.strictEqual(nextFrame(4, 5, 1, false), null);
    assert.strictEqual(nextFrame(0, 5, -1, true), 4);
    assert.strictEqual(nextFrame(0, 5, -1, false), null);
    assert.strictEqual(nextFrame(0, 1, 1, true), null, "one step is nothing to animate");
    assert.strictEqual(nextFrame(0, 0, 1, true), null);
  });

  test("the axis offered first is time when it can be stepped, never an image axis", () => {
    // (time, lat, lon) with lat/lon on screen: time.
    assert.strictEqual(defaultAnimationDim([12, 90, 180], 1, 2, 0), 0);
    // Time of length 1 cannot step: the next steppable axis, a level.
    assert.strictEqual(defaultAnimationDim([1, 30, 90, 180], 2, 3, 0), 1);
    // No time detected: the first steppable non-image axis.
    assert.strictEqual(defaultAnimationDim([90, 180, 5], 0, 1, undefined), 2);
    // Time is on screen as an image axis (a Hovmöller view): not offered.
    assert.strictEqual(defaultAnimationDim([12, 90], 0, 1, 0), null);
    // Nothing left to step.
    assert.strictEqual(defaultAnimationDim([1, 90, 180], 1, 2, 0), null);
  });

  test("a range locked for playback holds across frames with different extremes", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const handle = native.open(fixture("netcdf4_dimscale.nc"));
    const v = variable("netcdf4_dimscale.nc", "temperature");
    const y = v.detectedYDim ?? 1;
    const x = v.detectedXDim ?? 2;
    const base = { projection: "source" as const, resampling: "nearest" as const, flipY: false };
    const frames = [0, 1].map((t) => handle.renderGrid({ variable: v.index, yDim: y, xDim: x, sliceIndices: [t, 0, 0] }, base));
    assert.notDeepStrictEqual(
      [frames[0].usedMin, frames[0].usedMax],
      [frames[1].usedMin, frames[1].usedMax],
      "the two steps have their own extremes, or a lock proves nothing",
    );
    // What playback sends: the first frame's range, for every frame.
    const [lo, hi] = [frames[0].usedMin, frames[0].usedMax];
    assert.ok(lo != null && hi != null, "the first frame has a range to hold");
    const lock = { ...base, rangeMin: lo, rangeMax: hi };
    for (const t of [0, 1]) {
      const r = handle.renderGrid({ variable: v.index, yDim: y, xDim: x, sliceIndices: [t, 0, 0] }, lock);
      assert.deepStrictEqual([r.usedMin, r.usedMax], [frames[0].usedMin, frames[0].usedMax], `step ${t}`);
    }
  });

  test("a slice panel carries the transport, wired; a GRIB panel does not", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const v = variable("netcdf4_dimscale.nc", "temperature");
    const slice: SlicePanelData = {
      variables: [v],
      initial: { variableIndex: v.index, yDim: 1, xDim: 2, sliceIndices: [0, 0, 0] },
    };
    const meta = { grid: { label: "latlon" }, reprojectable: true } as unknown as PanelField;
    const webview = { cspSource: "" } as unknown as vscode.Webview;
    const html = renderImagePanelHtml(webview, meta, "summary", native.colormaps(), native.combineOps(), slice);
    for (const id of ["animate-row", "animate-dim", "anim-first", "anim-back", "anim-play", "anim-forward", "anim-last", "anim-speed", "anim-loop"]) {
      assert.ok(html.includes(`id="${id}"`), `slice panel has #${id}`);
    }
    assert.ok(/function nextFrame\s*\(/.test(html), "nextFrame is injected");
    assert.ok(/function defaultAnimationDim\s*\(/.test(html), "defaultAnimationDim is injected");
    for (const wiring of [
      /blit\(msg\);\s*updateLogAvailability\(\);\s*animationFrameArrived\(\);/,
      /function handleGridError\(msg\) \{[\s\S]{0,200}stopAnimation\(\);/,
      /options\.rangeMin = animation\.lock\[0\];/,
      /wrap\.innerHTML = dimStepperHtml\([^)]*\);\s*requestStepperAxes\(\);\s*fillSliceCoordinates\(\);\s*buildAnimationControls\(\);/,
    ]) {
      assert.ok(wiring.test(html), `the panel script wires ${wiring}`);
    }

    const grib = renderImagePanelHtml(webview, meta, "summary", native.colormaps(), native.combineOps());
    assert.ok(!grib.includes('id="animate-row"'), "a GRIB panel has no axis to animate");
  });

  // Pausing ended playback, and the range it held went with it: a hide/show
  // or a rebuild (an imported colour table writes every panel again) drew the
  // paused frame on its own range, so one frame showed two colour bars
  // depending on a tab switch (#1227).
  test("a paused frame keeps the range playback held through a hide/show and a rebuild, until a step or a control change (#1227)", async function () {
    this.timeout(60_000);
    const { panel, close } = await openSteps();
    try {
      let state = await panel.drive([{ id: "anim-speed", value: "0.5" }]);
      assert.deepStrictEqual(state.colorbarLabels, labels(STEPS[0]), "step 0 on its own range");
      await playOneFrame(panel, 0);

      const again: Array<[string, () => Promise<void>]> = [
        ["a hide and show", () => panel.hideShow()],
        ["a rebuild", async () => panel.rebuild()],
        ["a second hide and show", () => panel.hideShow()],
      ];
      for (const [what, redo] of again) {
        const from = panel.sent.length;
        await redo();
        const m = await panel.waitSent(drawnOrFailed, from, `the render after ${what}`);
        assert.strictEqual(m.type, "gridReady", m.error ?? "");
        state = await panel.drive([]);
        assert.match(state.animateLabel ?? "", /step 1 of/, `after ${what}, still on step 1`);
        assert.deepStrictEqual(state.colorbarLabels, labels(STEPS[0]), `after ${what}, step 1 keeps the held range`);
      }

      // A step by hand takes the frame's own range (#1155), and a hide/show
      // after it keeps that.
      await panel.drive([{ id: "anim-forward", click: true }]);
      await colourBar(panel, labels(STEPS[2]), "step 2 on its own range");
      let from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      state = await panel.drive([]);
      assert.deepStrictEqual(state.colorbarLabels, labels(STEPS[2]), "a stepped frame keeps its own range when shown again");

      // So does a colour map changed after a pause.
      await playOneFrame(panel, 2);
      const native = loadNative();
      assert.ok(native, "native binding required");
      const current = (await panel.controls())["picker-colormap"];
      const other = native.colormaps().find((c) => c.name !== current);
      assert.ok(other, "a second colour map to choose");
      await panel.drive([{ id: "picker-colormap", value: other.name }]);
      await colourBar(panel, labels(STEPS[3]), "step 3 on its own range after a colour map change");

      // And a range changed after a pause: Manual, then back to Auto.
      await playOneFrame(panel, 3);
      await panel.drive([
        { selector: 'input[name="range-mode"][value="manual"]', checked: true },
        { selector: 'input[name="range-mode"][value="auto"]', checked: true },
      ]);
      await colourBar(panel, labels(STEPS[0]), "step 0 on its own range after a range change");
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      state = await panel.drive([]);
      assert.deepStrictEqual(state.colorbarLabels, labels(STEPS[0]), "and keeps it when shown again");
    } finally {
      close();
    }
  });

  // The held range is of the data the frame was drawn from. A file changed on
  // disk, or read again by File: Revert File, is drawn on its own range.
  test("a paused frame's held range is not carried onto a changed or reverted file (#1227)", async function () {
    this.timeout(60_000);
    const { panel, change, revert, close } = await openSteps();
    try {
      await panel.drive([{ id: "anim-speed", value: "0.5" }]);
      await playOneFrame(panel, 0);

      // The panel finds the change when it next asks: here, as it is shown again.
      await change(stepsFile(CHANGED));
      let from = panel.sent.length;
      await panel.hideShow();
      let m = await panel.waitSent(drawnOrFailed, from, "the render after the change");
      assert.strictEqual(m.type, "gridReady", m.error ?? "");
      await colourBar(panel, labels(CHANGED[1]), "step 1 of the changed file on its own range");
      let state = await panel.drive([]);
      assert.ok((state.animateLabel ?? "").includes("step 1 of"), `still on step 1, got ${state.animateLabel}`);
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      state = await panel.drive([]);
      assert.deepStrictEqual(state.colorbarLabels, labels(CHANGED[1]), "and no held range comes back with a hide and show");

      // Held again on the changed file, then reverted to the first one.
      await playOneFrame(panel, 1, CHANGED);
      from = panel.sent.length;
      await revert(stepsFile(STEPS));
      m = await panel.waitSent(drawnOrFailed, from, "the render after the revert");
      assert.strictEqual(m.type, "gridReady", m.error ?? "");
      await colourBar(panel, labels(STEPS[2]), "step 2 of the reverted file on its own range");
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
      state = await panel.drive([]);
      assert.deepStrictEqual(state.colorbarLabels, labels(STEPS[2]), "and keeps it when shown again");
    } finally {
      close();
    }
  });
});
