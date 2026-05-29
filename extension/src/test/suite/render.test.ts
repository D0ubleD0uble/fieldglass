// Integration tests for the render pipeline + webview wire format.
//
// What these tests pin down
// -------------------------
// 1. The Buffer→Uint8Array wrap in `buildGridReadyMessage` is the fix for
//    the bug where `gridReady` payloads silently corrupted in transit
//    (VS Code's `extHostWebviewMessaging.ts::getTypedArrayType` switches
//    on `value.constructor.name`; Node `Buffer` doesn't match any of the
//    standard TypedArray names, so it falls back to `JSON.stringify` and
//    emits `{type:"Buffer", data:[…]}`).
//
// 2. The wire-shape assumption underlying that fix — that a `Uint8Array`
//    *does* survive `webview.postMessage` as a real `Uint8Array` while a
//    `Buffer` does *not* — by round-tripping both through a real probe
//    webview. If VS Code ever changes that contract, this test fails and
//    we know the wrap is no longer necessary (or no longer sufficient).
//
// 3. Each user-visible file format (GRIB1, GRIB2, NetCDF) can be opened
//    and produces sensible output via the native binding the extension
//    talks to.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import {
  buildGridReadyMessage,
  resolveRerenderOptions,
  type GridReadyMessage,
} from "../../provider";
import { loadNative, type RenderOptions } from "../../native";
import { buildGraticule, flattenLonLatLines, loadCoastline } from "../../overlay";

const EXT_ID = "fieldglass.fieldglass";

function fixturePath(name: string): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  if (!ext) {
    throw new Error(`extension ${EXT_ID} not found`);
  }
  return path.join(ext.extensionPath, "src", "test", "fixtures", name);
}

function defaultRenderOptions(): RenderOptions {
  return { projection: "source", resampling: "nearest", flipY: false };
}

// --------------------------------------------------------------------------
// Probe webview: a stripped-down panel that echoes back the *shape* of any
// `rgba` field it receives. The echo lets the extension-side test assert
// what VS Code's serializer actually delivered to the webview, which is the
// only place where the Buffer-vs-Uint8Array distinction visibly matters.
// --------------------------------------------------------------------------

interface ProbeEcho {
  rgbaCtor: string;
  rgbaByteLength: number | null;
  rgbaLength: number | null;
  rgbaHasDataArray: boolean;
  rgbaDataLength: number | null;
  firstBytes: number[] | null;
}

const PROBE_HTML = `<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"></head><body>
<script>
  (function () {
    const vscode = acquireVsCodeApi();
    function describe(v) {
      const ctor = v && v.constructor ? v.constructor.name : typeof v;
      const byteLength = (v && typeof v.byteLength === 'number') ? v.byteLength : null;
      const length = (v && typeof v.length === 'number') ? v.length : null;
      const hasDataArray = !!(v && Array.isArray(v.data));
      const dataLength = hasDataArray ? v.data.length : null;
      let firstBytes = null;
      if (v instanceof Uint8Array || v instanceof Uint8ClampedArray) {
        firstBytes = Array.from(v.slice(0, 4));
      } else if (hasDataArray) {
        firstBytes = v.data.slice(0, 4);
      }
      return { rgbaCtor: ctor, rgbaByteLength: byteLength, rgbaLength: length,
               rgbaHasDataArray: hasDataArray, rgbaDataLength: dataLength,
               firstBytes };
    }
    window.addEventListener('message', (ev) => {
      const m = ev.data;
      if (!m || typeof m.type !== 'string') return;
      if (m.type === 'probe') {
        vscode.postMessage(Object.assign({ type: 'probeEcho' }, describe(m.rgba)));
      }
    });
    vscode.postMessage({ type: 'ready' });
  })();
</script>
</body></html>`;

async function withProbePanel<T>(
  fn: (panel: vscode.WebviewPanel) => Promise<T>,
): Promise<T> {
  const panel = vscode.window.createWebviewPanel(
    "fieldglass.test.probe",
    "fieldglass probe",
    { viewColumn: vscode.ViewColumn.One, preserveFocus: true },
    { enableScripts: true, retainContextWhenHidden: false },
  );
  panel.webview.html = PROBE_HTML;
  try {
    // Wait for the script to mount and post `ready` before we drive it.
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("probe panel never posted ready")),
        10_000,
      );
      const sub = panel.webview.onDidReceiveMessage((m) => {
        if (m && m.type === "ready") {
          clearTimeout(timer);
          sub.dispose();
          resolve();
        }
      });
    });
    return await fn(panel);
  } finally {
    panel.dispose();
  }
}

async function probeRgbaShape(
  panel: vscode.WebviewPanel,
  rgba: unknown,
): Promise<ProbeEcho> {
  const echo = new Promise<ProbeEcho>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("probe panel never posted echo")),
      10_000,
    );
    const sub = panel.webview.onDidReceiveMessage((m) => {
      if (m && m.type === "probeEcho") {
        clearTimeout(timer);
        sub.dispose();
        resolve(m as ProbeEcho);
      }
    });
  });
  await panel.webview.postMessage({ type: "probe", rgba });
  return echo;
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

suite("Render pipeline", () => {
  test("buildGridReadyMessage wraps the napi Buffer as a Uint8Array view", () => {
    // Synthesise a `RenderedGrid` whose `rgba` is a Node `Buffer`, then run
    // it through the helper that production uses. The wrap must produce a
    // real Uint8Array (not a Buffer subclass) whose bytes match. This is
    // the regression test for the specific defect: if the wrap is ever
    // removed or weakened, `constructor.name` will revert to `"Buffer"` and
    // VS Code's serializer will corrupt the payload again.
    const rgba = Buffer.from([10, 20, 30, 40, 50, 60, 70, 80]);
    const meta = {
      messageIndex: 0,
      offsetBytes: 0,
      parameterName: "",
      parameterUnits: "",
      parameterAbbreviation: "",
      level: "",
      levelType: "",
      referenceTime: "",
      forecastHours: 0,
      forecastDisplay: "",
      originatingCentre: "",
      gridType: null,
      gridNi: null,
      gridNj: null,
      latFirst: null,
      lonFirst: null,
      latLast: null,
      lonLast: null,
      format: "grib1",
      edition: null,
      discipline: null,
      totalLengthBytes: null,
      productionStatus: null,
      dataType: null,
      lambertLad: null,
      lambertLov: null,
      lambertDxMetres: null,
      lambertDyMetres: null,
      lambertLatin1: null,
      lambertLatin2: null,
      gaussianNParallels: null,
    };
    const options = defaultRenderOptions();
    const message: GridReadyMessage = buildGridReadyMessage(
      { rgba, width: 2, height: 1, usedMin: 0, usedMax: 1, projectionSummary: "" },
      meta,
      options,
    );

    assert.strictEqual(message.rgba.constructor.name, "Uint8Array",
      "wrap must produce a real Uint8Array, not a Buffer (subclass)");
    assert.strictEqual(message.rgba.byteLength, 8);
    assert.deepStrictEqual(Array.from(message.rgba), [10, 20, 30, 40, 50, 60, 70, 80]);
    assert.strictEqual(message.type, "gridReady");
    assert.strictEqual(message.width, 2);
    assert.strictEqual(message.height, 1);
  });

  test("Uint8Array survives webview.postMessage; raw Buffer does NOT", async () => {
    // This is the contract that the wrap relies on. If VS Code ever stops
    // mangling Node Buffers (or stops preserving Uint8Array), the
    // assertions here will flip and we'll know the wrap can change.
    await withProbePanel(async (panel) => {
      const goodEcho = await probeRgbaShape(panel, new Uint8Array([10, 20, 30, 40]));
      assert.strictEqual(goodEcho.rgbaCtor, "Uint8Array",
        `expected Uint8Array, got ${goodEcho.rgbaCtor}`);
      assert.strictEqual(goodEcho.rgbaByteLength, 4);
      assert.deepStrictEqual(goodEcho.firstBytes, [10, 20, 30, 40]);

      const badEcho = await probeRgbaShape(panel, Buffer.from([10, 20, 30, 40]));
      // A Node Buffer ends up serialised via `Buffer.prototype.toJSON`, so
      // the webview sees `{type:"Buffer", data:[10,20,30,40]}` — a plain
      // object with no `byteLength`, no typed-array constructor, and a
      // `data: number[]` field. That's the shape that breaks
      // `new ImageData()` if anyone forgets to wrap.
      assert.notStrictEqual(badEcho.rgbaCtor, "Uint8Array",
        "raw Buffer must NOT round-trip as Uint8Array — if this passes, " +
        "VS Code's serializer learnt about Node Buffer and the wrap in " +
        "buildGridReadyMessage is no longer required");
      assert.ok(badEcho.rgbaHasDataArray,
        `expected {type:"Buffer", data:[…]} fallback, got ${badEcho.rgbaCtor}`);
      assert.strictEqual(badEcho.rgbaDataLength, 4);
    });
  });

  test("GRIB1: renderGrid output post-wrap reaches the webview intact", async () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);
    const messages = handle.messages();
    assert.ok(messages.length > 0, "GRIB1 fixture should contain at least one message");

    const rendered = handle.renderGrid(0, defaultRenderOptions());
    const message = buildGridReadyMessage(rendered, messages[0], defaultRenderOptions());

    await withProbePanel(async (panel) => {
      const echo = await probeRgbaShape(panel, message.rgba);
      assert.strictEqual(echo.rgbaCtor, "Uint8Array");
      assert.strictEqual(echo.rgbaByteLength, message.width * message.height * 4,
        "rgba byteLength must equal width*height*4 — anything else means " +
        "the webview would fail `new ImageData(rgba, w, h)`");
      assert.deepStrictEqual(echo.firstBytes, Array.from(message.rgba.slice(0, 4)));
    });
  });

  test("GRIB1 equirectangular: antimeridian-tight bounds echoed + manual override honored", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);

    const eqr: RenderOptions = {
      projection: "equirectangular",
      resampling: "nearest",
      flipY: false,
    };
    const auto = handle.renderGrid(0, eqr);

    // The equirectangular target echoes its geographic extent back...
    assert.ok(
      auto.usedLatMin !== undefined && auto.usedLatMin !== null,
      "equirectangular render must echo geographic bounds",
    );
    // ...and the dateline-crossing CMC grid resolves to a tight longitude
    // span (<180°), not the spurious ~312° box naive min/max would produce.
    const lonSpan = (auto.usedLonMax as number) - (auto.usedLonMin as number);
    assert.ok(lonSpan > 0 && lonSpan < 180, `expected tight lon span (<180°), got ${lonSpan}`);
    // The top edge bows toward the pole to ~80.6°N — well above the highest
    // corner (60.5°N). A four-corner box would report the corner value, so
    // this guards the perimeter-sampling fix end-to-end.
    assert.ok(
      (auto.usedLatMax as number) > 75,
      `lat_max should follow the edge toward the pole, got ${auto.usedLatMax}`,
    );

    // A manual window is rendered and echoed back verbatim (including a value
    // the user could have typed for an antimeridian view).
    const windowed = handle.renderGrid(0, {
      ...eqr,
      boundsLatMin: 30,
      boundsLatMax: 60,
      boundsLonMin: -140,
      boundsLonMax: -60,
    });
    assert.strictEqual(windowed.usedLatMin, 30);
    assert.strictEqual(windowed.usedLatMax, 60);
    assert.strictEqual(windowed.usedLonMin, -140);
    assert.strictEqual(windowed.usedLonMax, -60);

    // A partial/invalid box silently falls back to the computed extent.
    const partial = handle.renderGrid(0, { ...eqr, boundsLatMin: 30 });
    assert.strictEqual(partial.usedLatMin, auto.usedLatMin);
    assert.strictEqual(partial.usedLonMin, auto.usedLonMin);

    // Source projection has no geographic extent.
    const src = handle.renderGrid(0, defaultRenderOptions());
    assert.ok(src.usedLatMin === undefined || src.usedLatMin === null);
  });

  test("GRIB1 web mercator: renders, echoes bounds clamped to the Mercator band", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);

    const merc = handle.renderGrid(0, {
      projection: "web_mercator",
      resampling: "nearest",
      flipY: false,
    });

    // Web Mercator is a warped lat/lon target, so it echoes geographic bounds.
    assert.ok(
      merc.usedLatMin !== undefined && merc.usedLatMin !== null,
      "web mercator render must echo geographic bounds",
    );
    // The CMC top edge bows to ~80.6°N, beyond Mercator's ~85.05° limit it
    // stays — but the clamp must keep the extent inside the valid band.
    assert.ok(
      (merc.usedLatMax as number) <= 85.06 && (merc.usedLatMin as number) >= -85.06,
      `lat extent must be clamped to the Mercator band, got ${merc.usedLatMin}..${merc.usedLatMax}`,
    );
    assert.ok(
      /web mercator/.test(merc.projectionSummary),
      `summary should name the target, got: ${merc.projectionSummary}`,
    );
    // The RGBA buffer is the source-dim raster, fully populated.
    assert.strictEqual(merc.rgba.length, merc.width * merc.height * 4);
  });

  test("GRIB1 azimuthal targets: orthographic + polar stereographic render via presets", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);

    const ortho = handle.renderGrid(0, {
      projection: "orthographic",
      projectionPreset: "north_pole",
      resampling: "nearest",
      flipY: false,
    });
    // Azimuthal targets fit a disc to the raster — no lat/lon-box extent.
    assert.ok(
      ortho.usedLatMin === undefined || ortho.usedLatMin === null,
      "orthographic target has no geographic box extent",
    );
    assert.ok(/orthographic/.test(ortho.projectionSummary), ortho.projectionSummary);
    assert.strictEqual(ortho.rgba.length, ortho.width * ortho.height * 4);

    const polar = handle.renderGrid(0, {
      projection: "polar_stereographic",
      projectionPreset: "north",
      resampling: "nearest",
      flipY: false,
    });
    assert.ok(polar.usedLatMin === undefined || polar.usedLatMin === null);
    assert.ok(/polar stereographic/.test(polar.projectionSummary), polar.projectionSummary);
    assert.strictEqual(polar.rgba.length, polar.width * polar.height * 4);
  });

  test("GRIB2: renderGrid output post-wrap reaches the webview intact", async () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("regular_latlon_surface.grib2"));
    const handle = native.Grib2Handle.fromBytes(bytes);
    const messages = handle.messages();
    assert.ok(messages.length > 0, "GRIB2 fixture should contain at least one message");

    const rendered = handle.renderGrid(0, defaultRenderOptions());
    const message = buildGridReadyMessage(rendered, messages[0], defaultRenderOptions());

    await withProbePanel(async (panel) => {
      const echo = await probeRgbaShape(panel, message.rgba);
      assert.strictEqual(echo.rgbaCtor, "Uint8Array");
      assert.strictEqual(echo.rgbaByteLength, message.width * message.height * 4);
    });
  });

  test("NetCDF: openNetcdf returns populated DatasetMeta for classic CDF", () => {
    // NetCDF has no render-panel path today, but the editor opens these
    // files and the dataset table is populated from `openNetcdf`. Pin its
    // contract so a regression in the napi binding for this format gets
    // caught here rather than as a blank table in the editor.
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("netcdf_classic_dummy.nc"));
    const dataset = native.openNetcdf(bytes);

    assert.strictEqual(dataset.backing, "classic");
    assert.ok(dataset.fullyParsed,
      "classic CDF should be fully parsed (HDF5/NetCDF-4 deep parsing is a " +
      "separate workstream; classic should not regress)");
    assert.ok(dataset.dimensions.length > 0, "expected at least one dimension");
    assert.ok(dataset.variables.length > 0, "expected at least one variable");
  });
});

suite("rerenderRequest option clamp", () => {
  // `resolveRerenderOptions` is the provider-side glue between the picker and
  // the native render. The #71 regression lived here: the clamp predated the
  // new targets and snapped everything except "equirectangular" back to
  // "source", so Web Mercator / orthographic / polar-stereographic and their
  // presets silently did nothing. These pin the clamp so adding a picker
  // option without wiring it through here fails loudly.

  test("every picker projection survives the clamp", () => {
    const pickerProjections: ReadonlyArray<RenderOptions["projection"]> = [
      "source",
      "equirectangular",
      "web_mercator",
      "orthographic",
      "polar_stereographic",
    ];
    for (const projection of pickerProjections) {
      assert.strictEqual(
        resolveRerenderOptions({ projection }).projection,
        projection,
        `${projection} must pass through, not snap to source`,
      );
    }
  });

  test("forwards the azimuthal centre/hemisphere preset untouched", () => {
    const ortho = resolveRerenderOptions({
      projection: "orthographic",
      projectionPreset: "north_pole",
    });
    assert.strictEqual(ortho.projectionPreset, "north_pole");

    const polar = resolveRerenderOptions({
      projection: "polar_stereographic",
      projectionPreset: "south",
    });
    assert.strictEqual(polar.projectionPreset, "south");
  });

  test("unknown projection / resampling snap to their defaults", () => {
    const r = resolveRerenderOptions({
      projection: "mollweide" as RenderOptions["projection"],
      resampling: "lanczos" as RenderOptions["resampling"],
    });
    assert.strictEqual(r.projection, "source");
    assert.strictEqual(r.resampling, "nearest");
  });

  test("bilinear resampling is preserved; manual bounds pass through", () => {
    const r = resolveRerenderOptions({
      projection: "web_mercator",
      resampling: "bilinear",
      boundsLatMin: 10,
      boundsLatMax: 60,
      boundsLonMin: -140,
      boundsLonMax: -60,
    });
    assert.strictEqual(r.resampling, "bilinear");
    assert.deepStrictEqual(
      [r.boundsLatMin, r.boundsLatMax, r.boundsLonMin, r.boundsLonMax],
      [10, 60, -140, -60],
    );
  });
});

suite("overlay geometry", () => {
  // overlay.ts produces only geographic (lat, lon) polylines — the projection
  // into pixels lives in Rust. These pin the flat-array contract shape every
  // layer hands to `projectOverlay`.

  test("flattenLonLatLines swaps to lat,lon order and counts rings", () => {
    // Input is GeoJSON-order [lon, lat, …]; output must be [lat, lon, …].
    const g = flattenLonLatLines([[10, 20, 11, 21]]);
    assert.deepStrictEqual(Array.from(g.latlon), [20, 10, 21, 11]);
    assert.deepStrictEqual(Array.from(g.ringLengths), [2]);
  });

  test("buildGraticule yields in-range lat,lon lines with a consistent shape", () => {
    const g = buildGraticule(30);
    assert.ok(g.ringLengths.length > 0, "graticule has lines");
    const total = Array.from(g.ringLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, g.latlon.length, "ringLengths must cover every vertex");
    for (let i = 0; i < g.latlon.length; i += 2) {
      assert.ok(g.latlon[i] >= -90 - 1e-9 && g.latlon[i] <= 90 + 1e-9, "lat in range");
      assert.ok(g.latlon[i + 1] >= -180 - 1e-9 && g.latlon[i + 1] <= 180 + 1e-9, "lon in range");
    }
  });

  test("loadCoastline parses the bundled asset into flat lat,lon", () => {
    const c = loadCoastline();
    assert.ok(c.ringLengths.length > 0, "coastline has polylines");
    const total = Array.from(c.ringLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, c.latlon.length);
  });
});

suite("overlay projection (native)", () => {
  // The forward projection runs in Rust via `projectOverlay`. These pin the
  // additive napi contract: a well-formed ProjectedOverlay whose segLengths
  // account for every xy pair, and an empty result for the source projection.

  function grib1Handle() {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    return native.Grib1Handle.fromBytes(bytes);
  }

  test("projectOverlay maps coastline into the warped raster's pixel space", () => {
    const handle = grib1Handle();
    const coast = loadCoastline();
    const eqr: RenderOptions = {
      projection: "equirectangular",
      resampling: "nearest",
      flipY: false,
    };
    const out = handle.projectOverlay(0, eqr, coast.latlon, coast.ringLengths);
    const total = Array.from(out.segLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, out.xy.length, "segLengths must account for every xy pair");
    assert.ok(out.xy.length > 0, "coastline should project to a non-empty geometry");
  });

  test("source projection yields an empty overlay (no geographic forward map)", () => {
    const handle = grib1Handle();
    const g = buildGraticule(30);
    const out = handle.projectOverlay(0, defaultRenderOptions(), g.latlon, g.ringLengths);
    assert.strictEqual(out.xy.length, 0);
    assert.strictEqual(out.segLengths.length, 0);
  });
});
