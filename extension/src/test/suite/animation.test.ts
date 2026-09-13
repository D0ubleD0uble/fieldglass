// Animating a slice panel along its time axis (#170).
//
// What is pinned: the addon reports which axis is time for the CF and WRF
// spellings a file uses; the axis a panel offers first and the frame playback
// steps to are the pure functions the panel script runs as serialized copies;
// a colour range locked for playback holds across frames whose own extremes
// differ; and the transport controls are in a slice panel's markup, wired, and
// absent from a GRIB panel.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type MessageMeta, type NetcdfVariableMeta } from "../../native";
import { defaultAnimationDim, nextFrame, renderImagePanelHtml, type SlicePanelData } from "../../render-panel";

function fixture(name: string): Buffer {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return fs.readFileSync(path.join(ext.extensionPath, "src", "test", "fixtures", name));
}

function variable(file: string, name: string): NetcdfVariableMeta {
  const native = loadNative();
  assert.ok(native, "native module must load");
  const found = native.NetcdfHandle.fromBytes(fixture(file))
    .variables()
    .find((v) => v.name === name || v.name.endsWith(`/${name}`));
  assert.ok(found, `${file} has ${name}`);
  return found;
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
    const handle = native.NetcdfHandle.fromBytes(fixture("netcdf4_dimscale.nc"));
    const v = variable("netcdf4_dimscale.nc", "temperature");
    const y = v.detectedYDim ?? 1;
    const x = v.detectedXDim ?? 2;
    const base = { projection: "source" as const, resampling: "nearest" as const, flipY: false };
    const frames = [0, 1].map((t) => handle.renderSlice(v.variableIndex, y, x, [t, 0, 0], base));
    assert.notDeepStrictEqual(
      [frames[0].usedMin, frames[0].usedMax],
      [frames[1].usedMin, frames[1].usedMax],
      "the two steps have their own extremes, or a lock proves nothing",
    );
    // What playback sends: the first frame's range, for every frame.
    const lock = { ...base, rangeMin: frames[0].usedMin, rangeMax: frames[0].usedMax };
    for (const t of [0, 1]) {
      const r = handle.renderSlice(v.variableIndex, y, x, [t, 0, 0], lock);
      assert.deepStrictEqual([r.usedMin, r.usedMax], [frames[0].usedMin, frames[0].usedMax], `step ${t}`);
    }
  });

  test("a slice panel carries the transport, wired; a GRIB panel does not", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const v = variable("netcdf4_dimscale.nc", "temperature");
    const slice: SlicePanelData = {
      variables: [v],
      initial: { variableIndex: v.variableIndex, yDim: 1, xDim: 2, sliceIndices: [0, 0, 0] },
    };
    const meta = { gridType: "latlon", reprojectable: true } as unknown as MessageMeta;
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
      /wrap\.innerHTML = dimStepperHtml\([^)]*\);\s*buildAnimationControls\(\);/,
    ]) {
      assert.ok(wiring.test(html), `the panel script wires ${wiring}`);
    }

    const grib = renderImagePanelHtml(webview, meta, "summary", native.colormaps(), native.combineOps());
    assert.ok(!grib.includes('id="animate-row"'), "a GRIB panel has no axis to animate");
  });
});
