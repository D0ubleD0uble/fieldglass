// Zoom and pan on the lat/lon targets (#245).
//
// A gesture is turned into a new lat/lon window by two pure functions, which the
// panel script runs as serialized copies of the ones tested here. What is pinned:
// the point under the pointer stays under it when zooming, in latitude on the
// equirectangular target and in Mercator Y on Web Mercator (the space the image
// rows are linear in); spans stay inside the world; a window is slid back from a
// pole rather than squashed; panning crosses the antimeridian; and a window that
// does cross it renders through the addon. The panel markup and the script
// wiring are checked last. Then the gestures through the real panel: with Flip
// Y axis ticked the top of the image is south, and a gesture over an image whose
// next render cannot take a window leaves the bounds alone (#1147).

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type RenderOptions } from "../../native";
import {
  panMapWindow,
  renderImagePanelHtml,
  zoomMapWindow,
  type MapWindow,
  type PanelField,
} from "../../render-panel";
import { isType, type Msg, openEditor, openReal, provider, type RealPanel, waitUntil } from "./real-panel";

const MERCATOR_MAX_LAT = 85.05112877980659;
const EPS = 1e-9;

/** Mercator Y, written out here rather than taken from the code under test. */
function mercatorY(lat: number): number {
  return Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360));
}

/** The geographic point at image fraction `(fx, fy)` of `w`. */
function pointAt(w: MapWindow, mercator: boolean, fx: number, fy: number): { lat: number; lon: number } {
  const lon = w.lonMin + fx * (w.lonMax - w.lonMin);
  if (!mercator) return { lat: w.latMax - fy * (w.latMax - w.latMin), lon };
  const y = mercatorY(w.latMax) - fy * (mercatorY(w.latMax) - mercatorY(w.latMin));
  return { lat: ((2 * Math.atan(Math.exp(y)) - Math.PI / 2) * 180) / Math.PI, lon };
}

function close(a: number, b: number, what: string, eps = EPS): void {
  assert.ok(Math.abs(a - b) <= eps, `${what}: ${a} vs ${b}`);
}

const GLOBE: MapWindow = { latMin: -90, latMax: 90, lonMin: -180, lonMax: 180 };
const EUROPE: MapWindow = { latMin: 35, latMax: 70, lonMin: -10, lonMax: 40 };

function extensionPath(): string {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return ext.extensionPath;
}

suite("Zoom and pan", () => {
  test("zooming keeps the point under the pointer, on both targets", () => {
    for (const mercator of [false, true]) {
      for (const [fx, fy] of [
        [0.5, 0.5],
        [0.2, 0.7],
        [0.9, 0.1],
      ]) {
        for (const factor of [0.5, 0.8, 1.6]) {
          const before = pointAt(EUROPE, mercator, fx, fy);
          const zoomed = zoomMapWindow(EUROPE, mercator, fx, fy, factor);
          const after = pointAt(zoomed, mercator, fx, fy);
          const label = `mercator=${mercator} at (${fx}, ${fy}) ×${factor}`;
          close(after.lon, before.lon, `${label} lon`);
          close(after.lat, before.lat, `${label} lat`);
          close(zoomed.lonMax - zoomed.lonMin, (EUROPE.lonMax - EUROPE.lonMin) * factor, `${label} lon span`);
        }
      }
    }
  });

  test("the equirectangular target zooms linearly in latitude and Web Mercator in Mercator Y", () => {
    const zoomed = zoomMapWindow(EUROPE, false, 0.5, 0.5, 0.5);
    close(zoomed.latMax - zoomed.latMin, 17.5, "latitude span halves");
    const merc = zoomMapWindow(EUROPE, true, 0.5, 0.5, 0.5);
    close(
      mercatorY(merc.latMax) - mercatorY(merc.latMin),
      (mercatorY(EUROPE.latMax) - mercatorY(EUROPE.latMin)) / 2,
      "Mercator Y span halves",
    );
    assert.ok(Math.abs(merc.latMax - merc.latMin - 17.5) > 0.1, "which is not the latitude span halving");
  });

  test("spans stay inside the world and above a floor", () => {
    const out = zoomMapWindow(GLOBE, false, 0.3, 0.3, 8);
    close(out.lonMax - out.lonMin, 360, "longitude span caps at a full turn");
    close(out.latMin, -90, "latitude caps at the south pole");
    close(out.latMax, 90, "and the north pole");

    const merc = zoomMapWindow({ latMin: -80, latMax: 80, lonMin: 0, lonMax: 360 }, true, 0.5, 0.5, 8);
    close(merc.latMax, MERCATOR_MAX_LAT, "Mercator caps at its band", 1e-6);
    close(merc.latMin, -MERCATOR_MAX_LAT, "on both sides", 1e-6);

    let tiny = EUROPE;
    for (let i = 0; i < 40; i++) tiny = zoomMapWindow(tiny, false, 0.5, 0.5, 0.25);
    close(tiny.lonMax - tiny.lonMin, 0.01, "longitude span floors", 1e-9);
    close(tiny.latMax - tiny.latMin, 0.01, "latitude span floors", 1e-9);
  });

  test("a window is slid back from a pole, not squashed", () => {
    const arctic: MapWindow = { latMin: 60, latMax: 90, lonMin: 0, lonMax: 90 };
    const out = zoomMapWindow(arctic, false, 0.5, 0.5, 2);
    close(out.latMax, 90, "the north edge stops at the pole");
    close(out.latMax - out.latMin, 60, "and the span still doubles");

    const panned = panMapWindow(arctic, false, 0, 0.5);
    close(panned.latMax, 90, "a drag down past the pole stops there");
    close(panned.latMax - panned.latMin, 30, "keeping the span");

    const south = panMapWindow({ latMin: -80, latMax: -40, lonMin: 0, lonMax: 90 }, true, 0, -2);
    close(south.latMin, -MERCATOR_MAX_LAT, "Mercator stops at its band", 1e-6);
  });

  test("panning follows the pointer and crosses the antimeridian", () => {
    const pacific: MapWindow = { latMin: -30, latMax: 30, lonMin: 150, lonMax: 210 };
    // Dragging right by half the image moves the view west by half its span.
    const west = panMapWindow(pacific, false, 0.5, 0);
    close(west.lonMin, 120, "lon min");
    close(west.lonMax, 180, "lon max");
    // Dragging the map up brings what is south of it into view.
    const south = panMapWindow(pacific, false, 0, -0.25);
    close(south.latMax, 15, "lat max");
    close(south.latMin, -45, "lat min");

    // Far enough east that the west edge passes 180: it is written in [-180, 180)
    // with the span kept, so the numbers do not grow lap after lap.
    const east = panMapWindow(pacific, false, -0.75, 0);
    close(east.lonMin, -165, "west edge normalised");
    close(east.lonMax, -105, "span kept");
    let lapped = pacific;
    for (let i = 0; i < 30; i++) lapped = panMapWindow(lapped, false, -1, 0);
    assert.ok(lapped.lonMin >= -180 && lapped.lonMin < 180, `after 30 laps: ${lapped.lonMin}`);
    close(lapped.lonMax - lapped.lonMin, 60, "span after laps", 1e-6);
  });

  test("a panned window across the antimeridian renders through the addon", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const bytes = fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "spectral_simple_t63.grib2"));
    const handle = native.Grib2Handle.fromBytes(bytes);
    const render = (w: MapWindow) => {
      const options: RenderOptions = {
        projection: "equirectangular",
        resampling: "nearest",
        flipY: false,
        boundsLatMin: w.latMin,
        boundsLatMax: w.latMax,
        boundsLonMin: w.lonMin,
        boundsLonMax: w.lonMax,
      };
      return handle.renderGrid(0, options);
    };
    // A window written across the dateline, and the one a drag produces from
    // it: a sixth of the image to the left moves it 10 degrees east, 160..220.
    const written = { latMin: -30, latMax: 30, lonMin: 150, lonMax: 210 };
    const dragged = panMapWindow(written, false, -1 / 6, 0);
    close(dragged.lonMin, 160, "the dragged window's west edge");
    for (const w of [written, dragged]) {
      const r = render(w);
      assert.ok(r.width > 0 && r.height > 0);
      close(r.usedLonMin ?? NaN, w.lonMin, "the render echoes the west edge", 1e-6);
      close(r.usedLonMax ?? NaN, w.lonMax, "and the east edge", 1e-6);
      // A global field: both edge columns, either side of the dateline, paint.
      for (const x of [0, r.width - 1]) {
        let opaque = 0;
        for (let y = 0; y < r.height; y++) opaque += r.rgba[(y * r.width + x) * 4 + 3] === 255 ? 1 : 0;
        assert.strictEqual(opaque, r.height, `column ${x} of ${JSON.stringify(w)} is fully painted`);
      }
    }
  });

  test("the panel offers the zoom controls with the bounds, and wires the gestures", () => {
    const native = loadNative();
    assert.ok(native, "native module must load");
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      { grid: { label: "latlon" }, reprojectable: true } as unknown as PanelField,
      "summary",
      native.colormaps(),
      native.combineOps(),
    );
    const bounds = /<fieldset id="bounds-fieldset" hidden>([\s\S]*?)<\/fieldset>/.exec(html);
    assert.ok(bounds, "the bounds fieldset exists, hidden until a lat/lon target is picked");
    for (const id of ["zoom-in", "zoom-out", "zoom-reset"]) {
      assert.ok(bounds[1].includes(`id="${id}"`), `${id} sits with the bounds it edits`);
    }
    // The script runs the functions tested above, not a copy of the rule.
    assert.ok(/function zoomMapWindow\s*\(/.test(html), "zoomMapWindow is injected");
    assert.ok(/function panMapWindow\s*\(/.test(html), "panMapWindow is injected");
    for (const wiring of [
      /addEventListener\('wheel', onWheel, \{ passive: false \}\)/,
      /addEventListener\('mousedown', onMouseDown\)/,
      /applyMapWindow\(zoomMapWindow\(w, isMercator\(\), z\.fx, northFraction\(z\.fy, flipped\), z\.factor\)\)/,
      /applyMapWindow\(panMapWindow\(w, isMercator\(\),/,
      /if \(suppressNextClick\)/,
    ]) {
      assert.ok(wiring.test(html), `the panel script wires ${wiring}`);
    }
  });
});

/** The latest render the page was sent. */
function lastDrawn(panel: RealPanel): Msg {
  const drew = panel.sent.filter(isType("gridReady"));
  assert.ok(drew.length > 0, "a render was drawn");
  return drew[drew.length - 1];
}

suite("Zoom and pan through the panel (#1147)", function () {
  this.timeout(120_000);

  /** Open the committed regular lat/lon GRIB on the equirectangular target,
   *  with Flip Y axis ticked when `flip`, and run `body` on the panel. */
  async function onMap(flip: boolean, body: (panel: RealPanel) => Promise<void>): Promise<void> {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      if (flip) {
        from = panel.sent.length;
        await panel.drive([{ id: "flip-y", checked: true }]);
        await panel.waitSent((m) => m.type === "gridReady" && m.options.flipY === true, from, "the map, flipped");
      }
      await body(panel);
    } finally {
      panel.dispose();
      editor.close();
    }
  }

  /** The latitude a click reads in the image's middle column, `fy` of the
   *  way down it. The probe reports the pixel's own place, not a cell's. */
  async function latAt(panel: RealPanel, fy: number, what: string): Promise<number> {
    const sent = panel.sent.length;
    await panel.probe(fy);
    const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
    assert.ok(read && typeof read.lat === "number", `the click at ${fy} reads a latitude (${what})`);
    return read.lat;
  }

  /** Run `gesture` and wait for the render it asks for to land. */
  async function rendered(panel: RealPanel, gesture: () => Promise<void>, what: string): Promise<Msg> {
    const received = panel.received.length;
    const sent = panel.sent.length;
    await gesture();
    const asked = await waitUntil(
      () => panel.received.slice(received).find(isType("rerenderRequest")),
      `the render ${what} asks for`,
    );
    assert.ok(asked.boundsLatMin != null, `${what} renders a manual window`);
    return panel.waitSent(isType("gridReady"), sent, `the render after ${what}`);
  }

  /** Two rows of `drawn`, in degrees: how close a place must stay to count
   *  as the same one. */
  function twoRows(drawn: Msg): number {
    return (2 * (drawn.usedLatMax - drawn.usedLatMin)) / drawn.height;
  }

  test("a wheel at the top edge with Flip Y axis on keeps the place under the cursor", async () => {
    await onMap(true, async (panel) => {
      const before = lastDrawn(panel);
      const top = await latAt(panel, 0.1, "before the zoom");
      // The rows are bottom-up, so the top of the image is its south.
      assert.ok(top < (await latAt(panel, 0.9, "the bottom")), "the top of a flipped map is south");

      const after = await rendered(panel, () => panel.wheel([0.5, 0.1], -250), "the wheel");
      assert.strictEqual(after.options.flipY, true, "still drawn bottom-up");
      assert.ok(after.usedLatMax - after.usedLatMin < before.usedLatMax - before.usedLatMin, "the wheel zoomed in");
      const still = await latAt(panel, 0.1, "after the zoom");
      assert.ok(
        Math.abs(still - top) <= twoRows(before),
        `the place under the cursor stays there: ${top} before, ${still} after`,
      );
    });
  });

  test("a drag with Flip Y axis on moves the map the way the cursor moves", async () => {
    await onMap(true, async (panel) => {
      const before = lastDrawn(panel);
      const grabbed = await latAt(panel, 0.3, "where the drag starts");
      await rendered(panel, () => panel.drag([0.5, 0.3], [0.5, 0.5]), "the drag");
      const dropped = await latAt(panel, 0.5, "where the cursor let go");
      assert.ok(
        Math.abs(dropped - grabbed) <= twoRows(before),
        `the place grabbed is under the cursor where it let go: ${grabbed} grabbed, ${dropped} there`,
      );
    });
  });

  test("a wheel over the old image, while a Mollweide render is on its way, leaves Bounds on Auto", async () => {
    await onMap(false, async (panel) => {
      assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds start on Auto");
      const gate = panel.hold(isType("gridReady"));
      try {
        const received = panel.received.length;
        await panel.drive([{ id: "picker-projection", value: "mollweide" }]);
        await waitUntil(
          () =>
            panel.received.slice(received).find((m) => m.type === "rerenderRequest" && m.projection === "mollweide"),
          "the Mollweide render",
        );
        await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
        assert.strictEqual(lastDrawn(panel).options.projection, "equirectangular", "the old map is on screen");

        const wheeled = panel.received.length;
        await panel.wheel([0.5, 0.5], -250);
        // Past the 250 ms the page waits for a wheel to settle.
        await new Promise((r) => setTimeout(r, 750));
        assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds stay on Auto");
        assert.deepStrictEqual(
          panel.received.slice(wheeled).filter(isType("rerenderRequest")),
          [],
          "and the wheel asks for no render",
        );
      } finally {
        await gate.release();
      }
    });
  });
});
