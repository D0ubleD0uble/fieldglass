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
  gribFieldLabel,
  resolveGribCompare,
  resolveInterval,
  resolveNetcdfCompare,
  resolveRerenderOptions,
  type GridReadyMessage,
} from "../../provider";
import { loadNative, type MessageMeta, type RenderOptions } from "../../native";
import {
  buildGraticule,
  flattenLonLatLines,
  loadVectorLayer,
  VECTOR_LAYERS,
} from "../../overlay";
import { renderImagePanelHtml, type SlicePanelData } from "../../render-panel";

/** The real colormap registry from Rust. The panel is fed the same data in
 *  production, so the panel tests exercise the actual names and stops rather
 *  than a fixture that could drift from the registry. */
function registry() {
  const native = loadNative();
  assert.ok(native, "native binding required");
  return native.colormaps();
}

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
      packing: null,
      reprojectable: true,
      jScansPositive: null,
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

  test("GRIB1: renderGridCombined produces a difference field (#239)", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);
    const n = handle.messages().length;
    assert.ok(n >= 1, "fixture should contain at least one message");

    // A − A: every present cell is exactly zero, so the used range collapses —
    // a property independent of how many messages the fixture holds.
    const self = handle.renderGridCombined(0, 0, "a_minus_b", defaultRenderOptions());
    assert.strictEqual(self.usedMin, 0, "self-difference minimum is 0");
    assert.strictEqual(self.usedMax, 0, "self-difference maximum is 0");
    assert.strictEqual(
      self.rgba.length,
      self.width * self.height * 4,
      "combined render is a normal, paint-ready RGBA grid",
    );

    // Comparing distinct messages renders a valid grid of the same geometry.
    const other = handle.renderGridCombined(0, n - 1, "a_minus_b", defaultRenderOptions());
    assert.strictEqual(other.rgba.length, other.width * other.height * 4);

    // A bad op is a clear error, not a silent mis-render.
    assert.throws(
      () => handle.renderGridCombined(0, 0, "product" as never, defaultRenderOptions()),
      /unknown combine op/,
    );
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

  test("GRIB1 azimuthal targets: orthographic + polar stereographic render with free-form centre", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    const handle = native.Grib1Handle.fromBytes(bytes);

    // Free-form orthographic centre (lon/lat), not a named preset.
    const ortho = handle.renderGrid(0, {
      projection: "orthographic",
      centerLat: 55.0,
      centerLon: -100.0,
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

    // Polar stereographic: hemisphere preset + free-form central meridian.
    const polar = handle.renderGrid(0, {
      projection: "polar_stereographic",
      projectionPreset: "north",
      centerLon: -45.0,
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

  test("GRIB2: a spherical-harmonic message renders via inverse-transform synthesis (#303)", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("spectral_simple_t63.grib2"));
    const handle = native.Grib2Handle.fromBytes(bytes);
    const messages = handle.messages();
    assert.strictEqual(messages[0].gridType, "spherical_harmonic", "T63 spectral");
    // The message itself has no grid (napi `None` surfaces as `undefined`)...
    assert.ok(messages[0].gridNi == null, "spectral message declares no Ni");

    // ...but renderGrid synthesizes a global lat/lon grid (T63 → 256×128) via
    // the inverse spherical-harmonic transform and paints it.
    const rendered = handle.renderGrid(0, defaultRenderOptions());
    assert.strictEqual(rendered.width, 256);
    assert.strictEqual(rendered.height, 128);
    assert.strictEqual(
      rendered.rgba.length,
      rendered.width * rendered.height * 4,
      "RGBA buffer matches the synthesized grid",
    );
    // A realistic ~281 K temperature field, not garbage.
    assert.ok(
      rendered.usedMin > 200 && rendered.usedMax < 350,
      `spectral field range ${rendered.usedMin}..${rendered.usedMax} K`,
    );
  });

  test("GRIB1: a spherical-harmonic message renders via inverse-transform synthesis (#303)", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("spectral_simple_t63.grib1"));
    const handle = native.Grib1Handle.fromBytes(bytes);
    const messages = handle.messages();
    assert.strictEqual(messages[0].gridType, "spherical_harmonic", "T63 spectral");
    assert.ok(messages[0].gridNi == null, "spectral message declares no Ni");

    // GRIB1 spectral renders through the same shared inverse-transform engine.
    const rendered = handle.renderGrid(0, defaultRenderOptions());
    assert.strictEqual(rendered.width, 256);
    assert.strictEqual(rendered.height, 128);
    assert.strictEqual(rendered.rgba.length, rendered.width * rendered.height * 4);
    assert.ok(
      rendered.usedMin > 200 && rendered.usedMax < 350,
      `spectral field range ${rendered.usedMin}..${rendered.usedMax} K`,
    );
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
      "mollweide",
      "robinson",
      "equal_earth",
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

  test("forwards the free-form projection centre untouched", () => {
    const ortho = resolveRerenderOptions({
      projection: "orthographic",
      centerLat: 37.5,
      centerLon: -122.25,
    });
    assert.strictEqual(ortho.centerLat, 37.5);
    assert.strictEqual(ortho.centerLon, -122.25);

    // A central meridian rides through on centerLon for the polar target.
    const polar = resolveRerenderOptions({
      projection: "polar_stereographic",
      projectionPreset: "north",
      centerLon: -45.0,
    });
    assert.strictEqual(polar.centerLon, -45.0);
  });

  test("unknown projection / resampling snap to their defaults", () => {
    // "aitoff" is a real projection we don't offer — a plausible typo/stale
    // client value, not one of ours. (This used to read "mollweide", which
    // stopped being unknown the moment the picker gained it.)
    const r = resolveRerenderOptions({
      projection: "aitoff" as RenderOptions["projection"],
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

  test("scaleMode: only log10 passes; default and typo drop to linear", () => {
    // Omitted → undefined (native default is linear), so an untouched panel
    // renders exactly as before.
    assert.strictEqual(resolveRerenderOptions({}).scaleMode, undefined);
    // Explicit linear also normalises to undefined rather than round-tripping.
    assert.strictEqual(
      resolveRerenderOptions({ scaleMode: "linear" }).scaleMode,
      undefined,
    );
    // Log10 is the one value that turns the mode on.
    assert.strictEqual(
      resolveRerenderOptions({ scaleMode: "log10" }).scaleMode,
      "log10",
    );
    // A stale/typo'd value snaps to the default rather than reaching a Rust
    // error, mirroring the projection/colormap clamps.
    assert.strictEqual(
      resolveRerenderOptions({
        scaleMode: "symlog" as RenderOptions["scaleMode"],
      }).scaleMode,
      undefined,
    );
  });

  test("resolveGribCompare: valid rider passes, junk falls back to single render", () => {
    // A well-formed rider passes through as a difference-map request.
    assert.deepStrictEqual(
      resolveGribCompare({ compare: { op: "a_minus_b", messageIndexB: 3 } }),
      { op: "a_minus_b", messageIndexB: 3 },
    );
    // No rider → single render.
    assert.strictEqual(resolveGribCompare({}), undefined);
    assert.strictEqual(resolveGribCompare({ compare: undefined }), undefined);
    // Unknown op → single render (never round-trips a value Rust would reject).
    assert.strictEqual(
      resolveGribCompare({ compare: { op: "product", messageIndexB: 1 } }),
      undefined,
    );
    // A non-integer or negative index is rejected.
    assert.strictEqual(
      resolveGribCompare({ compare: { op: "ratio", messageIndexB: 1.5 } }),
      undefined,
    );
    assert.strictEqual(
      resolveGribCompare({ compare: { op: "ratio", messageIndexB: -1 } }),
      undefined,
    );
    // messageIndexB 0 is a valid field (the first message).
    assert.deepStrictEqual(
      resolveGribCompare({ compare: { op: "mean", messageIndexB: 0 } }),
      { op: "mean", messageIndexB: 0 },
    );
  });

  test("resolveInterval: only a positive finite number survives", () => {
    assert.strictEqual(resolveInterval(5), 5);
    assert.strictEqual(resolveInterval(0.5), 0.5);
    assert.strictEqual(resolveInterval(0), undefined, "zero → auto levels");
    assert.strictEqual(resolveInterval(-2), undefined);
    assert.strictEqual(resolveInterval(undefined), undefined);
    assert.strictEqual(resolveInterval(NaN), undefined);
    assert.strictEqual(resolveInterval("5" as unknown), undefined, "not a number");
  });

  test("resolveNetcdfCompare: valid rider passes, junk falls back to single render", () => {
    assert.deepStrictEqual(
      resolveNetcdfCompare({
        compare: { op: "a_minus_b", variableIndexB: 0, sliceIndicesB: [1, 0] },
      }),
      { op: "a_minus_b", variableIndexB: 0, sliceIndicesB: [1, 0] },
    );
    // No rider → single render.
    assert.strictEqual(resolveNetcdfCompare({}), undefined);
    // Unknown op → single render.
    assert.strictEqual(
      resolveNetcdfCompare({ compare: { op: "product", variableIndexB: 0, sliceIndicesB: [0] } }),
      undefined,
    );
    // Indices must be a non-negative integer array.
    assert.strictEqual(
      resolveNetcdfCompare({ compare: { op: "mean", variableIndexB: 0, sliceIndicesB: "0" } }),
      undefined,
    );
    assert.strictEqual(
      resolveNetcdfCompare({ compare: { op: "mean", variableIndexB: 0, sliceIndicesB: [-1] } }),
      undefined,
    );
    assert.strictEqual(
      resolveNetcdfCompare({ compare: { op: "mean", variableIndexB: 1.5, sliceIndicesB: [0] } }),
      undefined,
    );
    // An empty index array is valid (a 2-D variable with no extra dimensions).
    assert.deepStrictEqual(
      resolveNetcdfCompare({ compare: { op: "ratio", variableIndexB: 2, sliceIndicesB: [] } }),
      { op: "ratio", variableIndexB: 2, sliceIndicesB: [] },
    );
  });

  test("gribFieldLabel: concise, skips placeholder level", () => {
    const base = {
      messageIndex: 3,
      parameterAbbreviation: "TMP",
      parameterName: "Temperature",
      level: "500",
      forecastDisplay: "+6h",
    } as MessageMeta;
    assert.strictEqual(gribFieldLabel(base), "#3 · TMP · 500 · +6h");
    // A placeholder level ("—") is dropped rather than shown.
    assert.strictEqual(
      gribFieldLabel({ ...base, level: "—" } as MessageMeta),
      "#3 · TMP · +6h",
    );
    // Falls back to the full name when there is no abbreviation.
    assert.strictEqual(
      gribFieldLabel({ ...base, parameterAbbreviation: "" } as MessageMeta),
      "#3 · Temperature · 500 · +6h",
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

  test("buildGraticule emits no duplicate meridian at the antimeridian", () => {
    // A meridian is a constant-longitude line; +180 and -180 are the same line.
    // The loop excludes +180 so the antimeridian (-180) is stroked once. Walk
    // each line and collect its constant longitude; no two meridians may share
    // one, and +180 must never appear.
    const g = buildGraticule(30);
    const meridianLons = new Set<number>();
    let p = 0;
    for (const len of Array.from(g.ringLengths)) {
      const firstLon = g.latlon[p * 2 + 1];
      let constantLon = true;
      for (let v = 0; v < len; v++) {
        if (g.latlon[(p + v) * 2 + 1] !== firstLon) {
          constantLon = false;
          break;
        }
      }
      if (constantLon) {
        assert.ok(firstLon < 180 - 1e-9, `meridian at +180 must be excluded, got ${firstLon}`);
        assert.ok(!meridianLons.has(firstLon), `duplicate meridian at lon ${firstLon}`);
        meridianLons.add(firstLon);
      }
      p += len;
    }
  });

  test("loadVectorLayer parses the bundled coastline into flat lat,lon", () => {
    const c = loadVectorLayer("coastline");
    assert.ok(c.ringLengths.length > 0, "coastline has polylines");
    const total = Array.from(c.ringLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, c.latlon.length);
  });

  test("every bundled vector layer loads with in-range coordinates", () => {
    // Each Natural Earth asset must satisfy the OverlayGeometry contract the
    // Rust `projectOverlay` reads: flat [lat, lon, …] with ringLengths summing
    // to half its length. A layer whose coordinates were left in GeoJSON
    // (lon, lat) order would sail through the length check but put every
    // latitude outside ±90 — so check the ranges too, which is what catches a
    // swapped-axis asset.
    for (const layer of VECTOR_LAYERS) {
      const g = loadVectorLayer(layer);
      assert.ok(g.ringLengths.length > 0, `${layer} has polylines`);
      const total = Array.from(g.ringLengths).reduce((a, b) => a + b, 0);
      assert.strictEqual(total * 2, g.latlon.length, `${layer} length matches rings`);
      for (let i = 0; i < g.latlon.length; i += 2) {
        const lat = g.latlon[i];
        const lon = g.latlon[i + 1];
        assert.ok(lat >= -90 && lat <= 90, `${layer}: latitude ${lat} out of range`);
        assert.ok(lon >= -180 && lon <= 180, `${layer}: longitude ${lon} out of range`);
      }
      // A polyline needs two vertices to stroke; a stray single point would
      // silently draw nothing.
      for (const n of g.ringLengths) {
        assert.ok(n >= 2, `${layer}: a polyline with ${n} vertices cannot stroke`);
      }
    }
  });

  test("the vector layers are distinct and cached", () => {
    // A copy-paste slip in the asset table (two layers pointing at one file)
    // would otherwise go unnoticed — the panel would offer four toggles that
    // draw the same lines.
    const seen = new Map<string, string>();
    for (const layer of VECTOR_LAYERS) {
      const g = loadVectorLayer(layer);
      const key = `${g.ringLengths.length}:${g.latlon.length}:${g.latlon[0]}`;
      const clash = seen.get(key);
      assert.ok(!clash, `${layer} loaded the same geometry as ${clash}`);
      seen.set(key, layer);
    }
    // The cache hands back the same object rather than re-reading the file.
    assert.strictEqual(loadVectorLayer("rivers"), loadVectorLayer("rivers"));
  });
});

suite("render-panel HTML", () => {
  // The webview's azimuthal-centre inputs are the contract with Rust's
  // `orthographic_from_options` / `polar_stereographic_from_options`:
  // orthographic reads a free-form centre lon + lat, polar stereographic reads
  // a hemisphere plus a central meridian. Pin the input ids and defaults so the
  // picker can't drift away from what the native side reads (`currentOptions`
  // pulls these by id).

  function fakeMeta(): MessageMeta {
    return {
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
      packing: null,
      reprojectable: true,
      jScansPositive: null,
    };
  }

  test("every projection the picker actually offers survives the clamp", () => {
    // The structural version of the clamp test above: instead of a hand-kept
    // list, read the <option> values straight out of the rendered picker. A new
    // target added to the picker but not to `PROJECTIONS` would silently snap
    // back to "source" — which is exactly what shipped for Mollweide, since the
    // hand-kept list can't notice an option it was never told about.
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      registry(),
    );
    const select = /<select id="picker-projection">([\s\S]*?)<\/select>/.exec(html);
    assert.ok(select, "the projection picker must be in the panel HTML");
    const offered = [...select[1].matchAll(/<option value="([^"]+)"/g)].map((m) => m[1]);
    assert.ok(offered.length >= 6, `expected the full picker, got ${offered.join(", ")}`);
    for (const projection of offered) {
      assert.strictEqual(
        resolveRerenderOptions({ projection } as Partial<RenderOptions>).projection,
        projection,
        `the picker offers "${projection}" but the clamp drops it back to source`,
      );
    }
    // The world targets (Mollweide, Robinson, Equal Earth) share one control,
    // the central meridian `currentOptions` reads as `picker-world-meridian`.
    assert.ok(
      /<input type="number" id="picker-world-meridian"[^>]*>/.test(html),
      "the shared world-projection central meridian input must exist",
    );
  });

  test("the colormap picker offers the whole Rust registry and defaults to viridis", () => {
    const cmaps = registry();
    assert.ok(cmaps.length >= 8, `expected the full registry, got ${cmaps.length}`);

    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      cmaps,
    );
    const select = /<select id="picker-colormap">([\s\S]*?)<\/select>/.exec(html);
    assert.ok(select, "the colormap picker must be in the panel HTML");
    const offered = [...select[1].matchAll(/<option value="([^"]+)"/g)].map((m) => m[1]);
    assert.deepStrictEqual(
      offered.slice().sort(),
      cmaps.map((c) => c.name).sort(),
      "the picker must offer exactly the colormaps the renderer has",
    );
    // Every offered name must survive the provider clamp, or picking it would
    // silently paint the default instead — the #71 failure mode.
    for (const name of offered) {
      assert.strictEqual(
        resolveRerenderOptions({ colormap: name }).colormap,
        name,
        `the picker offers "${name}" but the clamp drops it`,
      );
    }
    // viridis is selected out of the box, so an untouched panel renders exactly
    // as it did before the registry existed.
    assert.ok(
      /<option value="viridis" selected>/.test(html),
      "viridis must be the pre-selected default",
    );
    assert.ok(/id="reverse-colormap"/.test(html), "the reverse toggle must exist");
    assert.ok(/id="scale-log"/.test(html), "the log10 scale toggle must exist");
    // The legend gradient is data, not CSS: the stops ride along in the script
    // payload and syncColorbar paints from them.
    assert.ok(/const COLORMAPS = \[/.test(html), "the registry must be embedded");
    assert.ok(
      !/rgb\(68, 1, 84\)/.test(html),
      "the hardcoded viridis CSS gradient must be gone",
    );
  });

  test("an unknown colormap snaps to the native default", () => {
    // Same clamp philosophy as the projection: a stale or typo'd name must not
    // round-trip into an error popup from Rust.
    assert.strictEqual(
      resolveRerenderOptions({ colormap: "jet" }).colormap,
      undefined,
      "unknown colormap must fall back to the native default",
    );
    assert.strictEqual(resolveRerenderOptions({}).reverseColormap, false);
    assert.strictEqual(
      resolveRerenderOptions({ colormap: "turbo", reverseColormap: true }).reverseColormap,
      true,
    );
  });

  test("the Compare picker appears only with two or more fields (#239)", () => {
    const html2 = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      registry(),
      undefined,
      [
        { index: 0, label: "#0 · A" },
        { index: 1, label: "#1 · B" },
      ],
    );
    assert.ok(/id="compare-op"/.test(html2), "the operation selector must exist");
    assert.ok(/id="compare-field-b"/.test(html2), "the Field B selector must exist");
    assert.ok(
      /value="a_minus_b"/.test(html2) && /value="ratio"/.test(html2),
      "the five combine operations must be offered",
    );
    // The op wire values in the picker must be the ones resolveGribCompare accepts.
    for (const op of ["a_minus_b", "b_minus_a", "a_plus_b", "mean", "ratio"]) {
      assert.ok(
        html2.includes(`value="${op}"`),
        `the picker must offer the "${op}" operation`,
      );
      assert.ok(
        resolveGribCompare({ compare: { op, messageIndexB: 1 } }),
        `resolveGribCompare must accept the picker's "${op}"`,
      );
    }

    // A single-field file (or none) shows no Compare row at all.
    const html1 = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      registry(),
      undefined,
      [{ index: 0, label: "#0 · only" }],
    );
    assert.ok(!/id="compare-op"/.test(html1), "no Compare row for a lone field");
    const htmlNone = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      registry(),
    );
    assert.ok(!/id="compare-op"/.test(htmlNone), "no Compare row when unset");
  });

  test("azimuthal centre inputs expose the free-form fields currentOptions reads", () => {
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeMeta(),
      "summary",
      registry(),
    );
    // The orthographic centre and the polar central meridian are free-form
    // number inputs (no preset <select> for the ortho centre any more); the
    // polar hemisphere stays a two-option select.
    assert.ok(
      !/id="picker-preset-ortho"/.test(html),
      "the fixed-preset ortho select must be gone — the centre is free-form now",
    );
    for (const id of [
      "picker-center-lon",
      "picker-center-lat",
      "picker-central-meridian",
    ]) {
      assert.ok(
        new RegExp(`<input type="number" id="${id}"[^>]*>`).test(html),
        `${id} number input must exist (read by currentOptions)`,
      );
    }
    // The free-form fields default to 0 so the initial view matches the prior
    // Atlantic / 0° central-meridian default.
    for (const id of ["picker-center-lon", "picker-center-lat", "picker-central-meridian"]) {
      assert.ok(
        new RegExp(`id="${id}"[^>]*value="0"`).test(html),
        `${id} must default to 0`,
      );
    }
    // The polar hemisphere is still a north/south select.
    const hemi = /<select id="picker-preset-polar">([\s\S]*?)<\/select>/.exec(html);
    assert.ok(hemi, "picker-preset-polar hemisphere select must exist");
    const values: string[] = [];
    const optionRe = /<option value="([^"]*)"/g;
    let m: RegExpExecArray | null;
    while ((m = optionRe.exec(hemi[1])) !== null) {
      values.push(m[1]);
    }
    assert.deepStrictEqual(
      values,
      ["north", "south"],
      "polar hemisphere select must match Rust's recognised set",
    );
  });
});

suite("overlay projection (native)", () => {
  // The forward projection runs in Rust via `projectOverlay`. These pin the
  // additive napi contract: a well-formed ProjectedOverlay whose segLengths
  // account for every xy pair, for both a warped target and the source
  // projection (which maps through the grid's own inverse).

  function grib1Handle() {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("cmc_wind_300_2010052400_p012.grib"));
    return native.Grib1Handle.fromBytes(bytes);
  }

  test("projectOverlay maps coastline into the warped raster's pixel space", () => {
    const handle = grib1Handle();
    const coast = loadVectorLayer("coastline");
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

  test("source projection projects the overlay through the grid's inverse map", () => {
    // The source raster paints grid point (i, j) at pixel (i, j), so the
    // overlay maps through the grid's own inverse — coastlines/graticule show
    // on the source projection too, not only the warped targets.
    const handle = grib1Handle();
    const coast = loadVectorLayer("coastline");
    const out = handle.projectOverlay(0, defaultRenderOptions(), coast.latlon, coast.ringLengths);
    const total = Array.from(out.segLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, out.xy.length, "segLengths must account for every xy pair");
    assert.ok(out.xy.length > 0, "coastline should project onto the source raster");
  });

  test("projectOverlay throws on an unrenderable target so the panel can recover", () => {
    // A Web Mercator window lying entirely poleward of the ±85.05° cutoff is
    // rejected by the warp setup. projectOverlay must surface that as a thrown
    // error (the provider reports it as a seq-tagged `overlayError`, which the
    // panel handles by re-arming the overlay instead of dead-ending it).
    const handle = grib1Handle();
    const coast = loadVectorLayer("coastline");
    const opts: RenderOptions = {
      projection: "web_mercator",
      resampling: "nearest",
      flipY: false,
      boundsLatMin: 86,
      boundsLatMax: 88,
      boundsLonMin: -10,
      boundsLonMax: 10,
    };
    assert.throws(
      () => handle.projectOverlay(0, opts, coast.latlon, coast.ringLengths),
      /Web Mercator/,
    );
  });
});

suite("NetCDF 2-D slice rendering (#122)", () => {
  // End-to-end across the napi boundary: the committed classic ERSST fixture
  // (time × lev × lat × lon, regular 2° grid) lists `sst` as renderable, and a
  // slice renders through both the source and equirectangular targets.

  function netcdfHandle() {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(fixturePath("ersst_v5_187001_cdf1.nc"));
    return native.NetcdfHandle.fromBytes(bytes);
  }

  test("variables() detects the horizontal axes of a 4-D field", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst, "sst must be renderable");
    assert.strictEqual(sst.dims.length, 4, "sst is time × lev × lat × lon");
    // lat is axis 2, lon axis 3 in declared order.
    assert.strictEqual(sst.detectedYDim, 2);
    assert.strictEqual(sst.detectedXDim, 3);
  });

  test("renderSlice paints a slice in source and equirectangular projections", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst);
    const indices = sst.dims.map(() => 0);
    const source = handle.renderSlice(
      sst.variableIndex,
      sst.detectedYDim ?? 2,
      sst.detectedXDim ?? 3,
      indices,
      defaultRenderOptions(),
    );
    assert.strictEqual(source.width, 180);
    assert.strictEqual(source.height, 89);
    assert.strictEqual(source.rgba.length, 180 * 89 * 4);

    const warped = handle.renderSlice(
      sst.variableIndex,
      2,
      3,
      indices,
      { projection: "equirectangular", resampling: "nearest", flipY: false },
    );
    assert.ok(warped.width > 0 && warped.height > 0);
    assert.ok(warped.usedLatMin !== undefined, "warp echoes its extent back");
  });

  test("renderSliceCombined differences two slices of a variable (#239)", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst);
    const indices = sst.dims.map(() => 0);
    const [y, x] = [sst.detectedYDim ?? 2, sst.detectedXDim ?? 3];

    // A − A on identical slice indices: every present cell is exactly 0.
    const self = handle.renderSliceCombined(
      sst.variableIndex, y, x, indices,
      sst.variableIndex, indices,
      "a_minus_b", defaultRenderOptions(),
    );
    assert.strictEqual(self.usedMin, 0, "self-difference minimum is 0");
    assert.strictEqual(self.usedMax, 0, "self-difference maximum is 0");
    assert.strictEqual(self.rgba.length, self.width * self.height * 4);

    // Stepping the lev dimension for field B gives a real difference of the
    // same geometry (the "two levels / two time steps" workflow).
    const bIndices = indices.slice();
    bIndices[1] = Math.min(1, Math.max(0, sst.dims[1].length - 1)); // lev axis
    const diff = handle.renderSliceCombined(
      sst.variableIndex, y, x, indices,
      sst.variableIndex, bIndices,
      "a_minus_b", defaultRenderOptions(),
    );
    assert.strictEqual(diff.rgba.length, diff.width * diff.height * 4);

    // A bad op is a clear error.
    assert.throws(
      () => handle.renderSliceCombined(
        sst.variableIndex, y, x, indices,
        sst.variableIndex, indices,
        "product" as never, defaultRenderOptions(),
      ),
      /unknown combine op/,
    );
  });

  test("projectContours returns isoline runs for a NetCDF slice (#238)", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst);
    const indices = sst.dims.map(() => 0);
    const [y, x] = [sst.detectedYDim ?? 2, sst.detectedXDim ?? 3];

    // Auto levels: a real SST field has interior isolines.
    const auto = handle.projectContours(sst.variableIndex, y, x, indices, {
      projection: "equirectangular",
      resampling: "nearest",
      flipY: false,
    });
    assert.ok(auto.xy.length > 0, "auto-level contours produce runs");
    assert.strictEqual(auto.xy.length % 2, 0, "xy is flat (x, y) pairs");
    assert.ok(auto.segLengths.length > 0, "runs are grouped by vertex count");

    // A manual interval also produces runs (a different set of levels).
    const manual = handle.projectContours(sst.variableIndex, y, x, indices, {
      projection: "source",
      resampling: "nearest",
      flipY: false,
    }, 2);
    assert.ok(manual.xy.length > 0, "manual-interval contours produce runs");
  });

  test("probe reads the field under a NetCDF pixel (#172)", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst);
    const indices = sst.dims.map(() => 0);
    const [y, x] = [sst.detectedYDim ?? 2, sst.detectedXDim ?? 3];
    const opts = { projection: "source" as const, resampling: "nearest" as const, flipY: false };

    // A pixel well inside the 180×89 grid resolves to a cell.
    const r = handle.probe(sst.variableIndex, y, x, indices, opts, 90, 44);
    assert.ok(r, "an in-grid pixel returns a result");
    assert.strictEqual(r.gridI, 90);
    assert.strictEqual(r.gridJ, 44);
    // It is geolocated (regular lat/lon grid) and reports a value or a hole.
    assert.strictEqual(typeof r.lat, "number");
    assert.strictEqual(typeof r.lon, "number");
    assert.ok(r.value === undefined || typeof r.value === "number");

    // A click past the raster is nothing.
    assert.strictEqual(
      handle.probe(sst.variableIndex, y, x, indices, opts, 9999, 9999),
      null,
      "off-raster returns null",
    );
  });

  test("projectOverlay maps a coastline onto the synthesised lat/lon grid", () => {
    const handle = netcdfHandle();
    const sst = handle.variables().find((v) => v.name === "sst");
    assert.ok(sst);
    const coast = loadVectorLayer("coastline");
    const out = handle.projectOverlay(
      sst.variableIndex,
      2,
      3,
      { projection: "equirectangular", resampling: "nearest", flipY: false },
      coast.latlon,
      coast.ringLengths,
    );
    const total = Array.from(out.segLengths).reduce((a, b) => a + b, 0);
    assert.strictEqual(total * 2, out.xy.length, "segLengths must account for every xy pair");
    assert.ok(out.xy.length > 0, "coastline should project onto the slice raster");
  });

  test("the panel HTML embeds the slice picker controls when given slice data", () => {
    const handle = netcdfHandle();
    const variables = handle.variables();
    const sst = variables.find((v) => v.name === "sst");
    assert.ok(sst);
    const slice: SlicePanelData = {
      variables,
      initial: {
        variableIndex: sst.variableIndex,
        yDim: 2,
        xDim: 3,
        sliceIndices: sst.dims.map(() => 0),
      },
    };
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      // The panel only reads a handful of header fields; reuse the GRIB fake.
      fakeNetcdfMeta(),
      "summary",
      registry(),
      slice,
    );
    for (const id of ["slice-variable", "slice-y", "slice-x", "slice-dims"]) {
      assert.ok(
        new RegExp(`id="${id}"`).test(html),
        `slice control ${id} must be present`,
      );
    }
    assert.ok(/const SLICE = \{/.test(html), "the SLICE payload must be embedded");
    // The NetCDF Compare row (#239) is an operation selector plus a Field B
    // stepper container — no message dropdown (that's the GRIB variant).
    // The contour overlay controls (#238) are present in the panel HTML.
    for (const id of ["overlay-contours", "contour-interval", "contours-only", "color-contours"]) {
      assert.ok(html.includes(`id="${id}"`), `contour control ${id} must be present`);
    }
    // The point-probe readout element (#172) is present.
    assert.ok(html.includes('id="probe"'), "the probe readout element must be present");
    // Contour errors show in their own line, not #status, so they don't
    // clobber the render summary (#338).
    assert.ok(
      html.includes('id="contour-status"'),
      "the contour-status line must be present",
    );
    assert.ok(/id="compare-op"/.test(html), "the compare operation selector must exist");
    assert.ok(/id="compare-dims"/.test(html), "the Field B stepper container must exist");
    assert.ok(!/id="compare-field-b"/.test(html), "NetCDF uses steppers, not a message dropdown");
    for (const op of ["a_minus_b", "b_minus_a", "a_plus_b", "mean", "ratio"]) {
      assert.ok(
        html.includes(`value="${op}"`),
        `the compare picker must offer the "${op}" operation`,
      );
    }
    // A GRIB panel (no slice data) must not grow the slice row.
    const gribHtml = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      fakeNetcdfMeta(),
      "summary",
      registry(),
    );
    assert.ok(/const SLICE = null/.test(gribHtml), "SLICE must be null without slice data");
    assert.ok(!/id="slice-variable"/.test(gribHtml), "no slice row without slice data");
  });

  function fakeNetcdfMeta(): MessageMeta {
    return {
      messageIndex: 0,
      offsetBytes: 0,
      parameterName: "sst",
      parameterUnits: "degree_C",
      parameterAbbreviation: "sst",
      level: "",
      levelType: "",
      referenceTime: "",
      forecastHours: 0,
      forecastDisplay: "",
      originatingCentre: "",
      gridType: "latlon",
      gridNi: null,
      gridNj: null,
      latFirst: null,
      lonFirst: null,
      latLast: null,
      lonLast: null,
      format: "netcdf",
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
      packing: null,
      reprojectable: true,
      jScansPositive: null,
    };
  }
});
