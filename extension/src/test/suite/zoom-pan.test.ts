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
import * as os from "os";
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
    const handle = native.open(bytes);
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
      /zoomMapWindow\(z\.from, z\.mercator, z\.fx, northFraction\(z\.fy, z\.flipped\), z\.factor\)/,
      /applyMapWindow\(z\.target\(\)\)/,
      /panMapWindow\(g\.from, g\.mercator,/,
      /const target = dragTarget\(g, ev\.clientX, ev\.clientY\);\s*applyMapWindow\(target, /,
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

  /** Open the committed regular lat/lon GRIB on `projection` (a lat/lon
   *  target), with Flip Y axis ticked when `flip`, and run `body` on it. */
  async function onMap(
    flip: boolean,
    body: (panel: RealPanel) => Promise<void>,
    projection = "equirectangular",
    fixture = "regular_latlon_surface.grib2",
  ): Promise<void> {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", fixture);
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: projection }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === projection, from, "the map");
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
  async function rendered(panel: RealPanel, gesture: () => Promise<unknown>, what: string): Promise<Msg> {
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

  /** Whether latitudes `a` and `b` are within two rows of `drawn`: the same
   *  place, to a click. Rows are even in latitude on Equirectangular and in
   *  Mercator Y on Web Mercator, so the two are compared in that space. */
  function withinTwoRows(a: number, b: number, drawn: Msg): boolean {
    const y = drawn.options.projection === "web_mercator" ? mercatorY : (lat: number) => lat;
    return Math.abs(y(a) - y(b)) <= (2 * (y(drawn.usedLatMax) - y(drawn.usedLatMin))) / drawn.height;
  }

  for (const [projection, flip] of [
    ["equirectangular", true],
    ["web_mercator", true],
    ["equirectangular", false],
  ] as const) {
    const how = `${flip ? "with" : "without"} Flip Y axis on ${projection}`;
    test(`a wheel at the top edge ${how} keeps the place under the cursor`, async () => {
      await onMap(
        flip,
        async (panel) => {
          const before = lastDrawn(panel);
          const top = await latAt(panel, 0.1, "before the zoom");
          // Bottom-up rows put the south at the top of the image.
          const bottom = await latAt(panel, 0.9, "the bottom");
          assert.strictEqual(top < bottom, flip, flip ? "the top of a flipped map is south" : "and north otherwise");

          const after = await rendered(panel, () => panel.wheel([0.5, 0.1], -250), "the wheel");
          assert.strictEqual(after.options.flipY, flip, "drawn the same way up");
          assert.ok(after.usedLatMax - after.usedLatMin < before.usedLatMax - before.usedLatMin, "the wheel zoomed in");
          const still = await latAt(panel, 0.1, "after the zoom");
          assert.ok(
            withinTwoRows(still, top, before),
            `the place under the cursor stays there: ${top} before, ${still} after`,
          );
        },
        projection,
      );
    });
  }

  /** The place a click at the client point `client` reads, and the image's
   *  box then. */
  async function placeAt(panel: RealPanel, client: [number, number], what: string) {
    const sent = panel.sent.length;
    const at = await panel.probeAt(client);
    const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
    assert.ok(read && typeof read.lat === "number" && typeof read.lon === "number", `the click reads a place (${what})`);
    return { lat: read.lat as number, lon: read.lon as number, rect: at.rect };
  }

  /** Assert `b` is the place `a` is, to a pixel of either render: a probe
   *  reads its pixel's place, so each read is off by up to half a pixel. */
  function samePlace(a: { lat: number; lon: number }, b: { lat: number; lon: number }, drawn: Msg[], what: string) {
    const lonPx = drawn.reduce((t, d) => t + (d.usedLonMax - d.usedLonMin) / d.width, 0);
    const latPx = drawn.reduce((t, d) => t + (d.usedLatMax - d.usedLatMin) / d.height, 0);
    const dLon = Math.abs(((((b.lon - a.lon) % 360) + 540) % 360) - 180);
    assert.ok(dLon <= lonPx, `${what}: longitude ${a.lon} before, ${b.lon} after (allowed ${lonPx})`);
    assert.ok(Math.abs(b.lat - a.lat) <= latPx, `${what}: latitude ${a.lat} before, ${b.lat} after (allowed ${latPx})`);
  }

  /** Narrow enough that the Bounds row wraps once its Manual fields show,
   *  which is where the map used to drop (#1228). */
  const NARROW = [{ id: "body", width: "640px" }];

  test("repeated wheel ticks from Auto keep the place under a pointer held still (#1228)", async () => {
    await onMap(false, async (panel) => {
      await panel.drive(NARROW);
      assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds start on Auto");
      const before = lastDrawn(panel);
      const first = await panel.probe(0.3, 0.3);
      const start = await placeAt(panel, first.point, "before the ticks");
      const TICKS = 5;
      for (let i = 1; i <= TICKS; i++) {
        await rendered(panel, () => panel.wheel({ client: first.point }, -120), `tick ${i}`);
      }
      const after = lastDrawn(panel);
      const factor = Math.pow(1.002, -120 * TICKS);
      close(after.usedLonMax - after.usedLonMin, (before.usedLonMax - before.usedLonMin) * factor, "every tick zoomed", 1e-3);
      const end = await placeAt(panel, first.point, "after the ticks");
      samePlace(start, end, [before, after], "the place under the pointer");
    });
  });

  for (const [projection, flip] of [
    ["equirectangular", false],
    ["web_mercator", true],
    ["equirectangular", true],
  ] as const) {
    const how = `${flip ? "with" : "without"} Flip Y axis on ${projection}`;
    test(`wheel ticks while the last one's render is on its way zoom on from that one, ${how} (#1228)`, async () => {
      await onMap(
        flip,
        async (panel) => {
          const before = lastDrawn(panel);
          const first = await panel.probe(0.3, 0.3);
          const start = await placeAt(panel, first.point, "before the ticks");
          const TICKS = 3;
          const gate = panel.hold(isType("gridReady"));
          let last: Msg;
          try {
            const asks: Msg[] = [];
            for (let i = 1; i <= TICKS; i++) {
              const received = panel.received.length;
              await panel.wheel({ client: first.point }, -120);
              asks.push(
                await waitUntil(
                  () => panel.received.slice(received).find(isType("rerenderRequest")),
                  `the render tick ${i} asks for`,
                ),
              );
            }
            // Every render the ticks asked for is still on its way, and the
            // old image is moved to where it sits in the window asked for: a
            // click reads the place the ticks zoomed about.
            await waitUntil(() => (gate.held.length === TICKS ? true : undefined), "the ticks' renders, held back");
            const previewed = await placeAt(panel, first.point, "on the preview");
            samePlace(start, previewed, [before, before], "the place under the pointer on the preview");
            const sent = panel.sent.length;
            await gate.release();
            last = await panel.waitSent(
              (m) => m.type === "gridReady" && m.seq === asks[TICKS - 1].seq,
              sent,
              "the last tick's render",
            );
          } finally {
            await gate.release();
          }
          const factor = Math.pow(1.002, -120 * TICKS);
          close(
            last.usedLonMax - last.usedLonMin,
            (before.usedLonMax - before.usedLonMin) * factor,
            "every tick zoomed",
            1e-3,
          );
          const end = await placeAt(panel, first.point, "after the ticks");
          samePlace(start, end, [before, last], "the place under the pointer");
        },
        projection,
      );
    });
  }

  test("a drag while a tick's render is on its way pans on from that tick (#1228)", async () => {
    await onMap(true, async (panel) => {
      const before = lastDrawn(panel);
      const gate = panel.hold(isType("gridReady"));
      let last: Msg;
      try {
        const received = panel.received.length;
        const tick = await panel.wheel([0.5, 0.5], -120);
        await waitUntil(() => panel.received.slice(received).find(isType("rerenderRequest")), "the tick's render");
        // The box the tick was turned over, which the window asked for fills.
        const from: [number, number] = [tick.point[0], tick.rect[1] + tick.rect[3] * 0.3];
        const to: [number, number] = [tick.point[0], tick.rect[1] + tick.rect[3] * 0.5];
        const grabbed = await placeAt(panel, from, "where the drag starts");
        const dragged = panel.received.length;
        await panel.drag({ client: from }, { client: to });
        const ask = await waitUntil(
          () => panel.received.slice(dragged).find(isType("rerenderRequest")),
          "the drag's render",
        );
        const sent = panel.sent.length;
        await gate.release();
        last = await panel.waitSent((m) => m.type === "gridReady" && m.seq === ask.seq, sent, "the drag's render");
        close(
          last.usedLonMax - last.usedLonMin,
          (before.usedLonMax - before.usedLonMin) * Math.pow(1.002, -120),
          "the drag kept the tick's zoom",
          1e-3,
        );
        const dropped = await placeAt(panel, to, "where it let go");
        samePlace(grabbed, dropped, [before, last], "the place grabbed is under the pointer where it let go");
      } finally {
        await gate.release();
      }
    });
  });

  /** A global field: its Auto window is the whole world, 0 to 360. */
  const WORLD = "spectral_simple_t63.grib2";

  test("a quick scroll over the east of a world map previews the place under the pointer (#1228)", async () => {
    await onMap(
      false,
      async (panel) => {
        const before = lastDrawn(panel);
        assert.strictEqual(before.usedLonMax - before.usedLonMin, 360, "the map is the whole world");
        const first = await panel.probe(0.5, 0.8);
        const start = await placeAt(panel, first.point, "before the scroll");
        const gate = panel.hold(isType("gridReady"));
        try {
          const received = panel.received.length;
          // Five notches in one: the window asked for runs past 180 east, so
          // its west edge is written a turn away from the drawn one's.
          await panel.wheel({ client: first.point }, -600);
          await waitUntil(() => panel.received.slice(received).find(isType("rerenderRequest")), "the scroll's render");
          const previewed = await placeAt(panel, first.point, "on the preview");
          samePlace(start, previewed, [before, before], "the place under the pointer on the preview");
        } finally {
          await gate.release();
        }
      },
      "equirectangular",
      WORLD,
    );
  });

  test("a drag across most of a world map previews the place grabbed under the pointer (#1228)", async () => {
    await onMap(
      false,
      async (panel) => {
        const before = lastDrawn(panel);
        const at = await panel.probe(0.5, 0.1);
        const [left, top, width, height] = at.rect;
        const from: [number, number] = [left + width * 0.1, top + height * 0.5];
        const to: [number, number] = [left + width * 0.8, top + height * 0.5];
        const grabbed = await placeAt(panel, from, "where the drag starts");
        const gate = panel.hold(isType("gridReady"));
        try {
          const received = panel.received.length;
          // Seven tenths of the width: more than half a turn east.
          await panel.drag({ client: from }, { client: to });
          await waitUntil(() => panel.received.slice(received).find(isType("rerenderRequest")), "the drag's render");
          const previewed = await placeAt(panel, to, "on the preview");
          samePlace(grabbed, previewed, [before, before], "the place grabbed is under the pointer on the preview");
        } finally {
          await gate.release();
        }
      },
      "equirectangular",
      WORLD,
    );
  });

  test("a drag started while a wheel is settling carries on from its zoom (#1228)", async () => {
    await onMap(false, async (panel) => {
      const before = lastDrawn(panel);
      const factor = Math.pow(1.002, -120);
      const tick = await panel.wheel([0.5, 0.5], -120);
      // Well inside the 250 ms the wheel waits to settle.
      const [left, top, width, height] = tick.rect;
      await panel.drag(
        { client: [left + width * 0.5, top + height * 0.3] },
        { client: [left + width * 0.5, top + height * 0.5] },
      );
      // Past that wait, so a zoom still pending would have been asked for.
      // The page's own clock says when: its timers can run late under load.
      await panel.pageTimer(300);
      const asks = panel.received.filter(isType("rerenderRequest"));
      const last = asks[asks.length - 1];
      const drawn = await panel.waitSent((m) => m.type === "gridReady" && m.seq === last.seq, 0, "the last render");
      const window0: MapWindow = {
        latMin: before.usedLatMin,
        latMax: before.usedLatMax,
        lonMin: before.usedLonMin,
        lonMax: before.usedLonMax,
      };
      const want = panMapWindow(zoomMapWindow(window0, false, 0.5, 0.5, factor), false, 0, 0.2);
      close(drawn.usedLonMax - drawn.usedLonMin, want.lonMax - want.lonMin, "the drag kept the zoom", 1e-3);
      // A pointer lands on whole pixels, so the drag is good to a pixel.
      const row = (drawn.usedLatMax - drawn.usedLatMin) / drawn.height;
      close(drawn.usedLatMax, want.latMax, "and moved it the way it was dragged", 2 * row);
      close(drawn.usedLatMin, want.latMin, "the whole window", 2 * row);
    });
  });

  test("the map stays put on screen when a gesture ticks Bounds Manual, and when Reset view unticks it (#1228)", async () => {
    await onMap(false, async (panel) => {
      await panel.drive(NARROW);
      const at = await panel.probe(0.5, 0.5);
      await rendered(panel, () => panel.wheel({ client: at.point }, -120), "the first tick");
      assert.strictEqual((await panel.controls())["radio:bounds-mode"], "manual", "the tick set Bounds to Manual");
      const zoomed = await panel.probe(0.5, 0.5);
      assert.deepStrictEqual(zoomed.rect.map(Math.round), at.rect.map(Math.round), "the image's box did not move");
      const sent = panel.sent.length;
      await panel.drive([{ id: "zoom-reset", click: true }]);
      await panel.waitSent(isType("gridReady"), sent, "the render after Reset view");
      assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Reset view set Bounds back to Auto");
      const reset = await panel.probe(0.5, 0.5);
      assert.deepStrictEqual(reset.rect.map(Math.round), at.rect.map(Math.round), "nor after Reset view");
    });
  });

  /** Open `netcdf4_dimscale.nc`'s `temperature` (time × lat × lon) on
   *  Equirectangular, and run `body` on it. A slice panel, which has the
   *  Line plot box. */
  async function onSliceMap(body: (panel: RealPanel) => Promise<void>): Promise<void> {
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const temperature = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "temperature");
    assert.ok(temperature, "the fixture has temperature");
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      await body(panel);
    } finally {
      panel.dispose();
      editor.close();
    }
  }

  /** With Line plot ticked, click the image `fy` of the way down and `fx`
   *  across, and wait for the line through the cell there. The first click
   *  opens the plot above the map, which moves the map down, so the image is
   *  clicked twice: the second click is where the image is then, and its
   *  client point and the place it reads are returned. */
  async function plotAt(panel: RealPanel, fy: number, fx: number) {
    let at: Awaited<ReturnType<RealPanel["probe"]>> | undefined;
    let read: Msg | undefined;
    for (const what of ["the first click", "the second click"]) {
      const sent = panel.sent.length;
      at = await panel.probe(fy, fx);
      read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
      await panel.waitSent(isType("lineResult"), sent, `the line (${what})`);
    }
    assert.ok(at && read && typeof read.lat === "number", "the click reads a place");
    const state = await panel.drive([]);
    assert.strictEqual(state.linePlot?.shown, true, "the line is plotted");
    return { point: at.point, place: { lat: read.lat as number, lon: read.lon as number }, readout: state.probe };
  }

  test("repeated wheel ticks after a probe with Line plot ticked keep the place under the pointer, and the plot (#1255)", async () => {
    await onSliceMap(async (panel) => {
      await panel.drive(NARROW);
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const { point, place: start, readout } = await plotAt(panel, 0.4, 0.4);
      const first = { point };
      const before = lastDrawn(panel);
      const lines = panel.received.filter(isType("lineRequest")).length;
      const TICKS = 4;
      const shown: Array<boolean | undefined> = [];
      const readouts: string[] = [];
      for (let i = 1; i <= TICKS; i++) {
        await rendered(panel, () => panel.wheel({ client: first.point }, -120), `tick ${i}`);
        const state = await panel.drive([]);
        shown.push(state.linePlot?.shown);
        readouts.push(state.probe);
      }
      const after = lastDrawn(panel);
      const factor = Math.pow(1.002, -120 * TICKS);
      close(after.usedLonMax - after.usedLonMin, (before.usedLonMax - before.usedLonMin) * factor, "every tick zoomed", 1e-3);
      // Counted before the probe below, which asks for the line again.
      assert.strictEqual(panel.received.filter(isType("lineRequest")).length, lines, "the ticks ask for no new line");
      const end = await placeAt(panel, first.point, "after the ticks");
      samePlace(start, end, [before, after], "the place under the pointer");
      assert.deepStrictEqual(shown, Array(TICKS).fill(true), "the plot stays through every tick");
      assert.deepStrictEqual(readouts, Array(TICKS).fill(readout), "and so does the readout of its cell");
    });
  });

  test("a colour change keeps the line plot, and a step of the slice clears it (#1255)", async () => {
    await onSliceMap(async (panel) => {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const { readout } = await plotAt(panel, 0.5, 0.5);
      const native = loadNative();
      assert.ok(native, "native binding required");

      // Each of these changes only how the values are coloured.
      const current = (await panel.controls())["picker-colormap"];
      const other = native.colormaps().find((c) => c.name !== current);
      assert.ok(other, "a second colour map to choose");
      const colourOnly: Array<[string, Parameters<RealPanel["drive"]>[0]]> = [
        ["a colour map", [{ id: "picker-colormap", value: other.name }]],
        ["Reverse", [{ id: "reverse-colormap", checked: true }]],
        ["a manual range", [{ selector: 'input[name="range-mode"][value="manual"]', checked: true }, { id: "range-max", value: 400 }]],
      ];
      for (const [what, actions] of colourOnly) {
        const sent = panel.sent.length;
        await panel.drive(actions);
        await panel.waitSent(isType("gridReady"), sent, `the render after ${what}`);
        const state = await panel.drive([]);
        assert.strictEqual(state.linePlot?.shown, true, `${what} keeps the plot`);
        assert.strictEqual(state.probe, readout, `${what} keeps the readout`);
      }

      // A step of the time axis is another field: its plot and readout go.
      const sent = panel.sent.length;
      await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: 1 }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.slice?.sliceIndices?.[0] === 1, sent, "the next time step");
      const state = await panel.drive([]);
      assert.strictEqual(state.linePlot?.shown, false, "a step of the slice clears the plot");
      assert.strictEqual(state.probe, "", "and the readout");
    });
  });

  test("wheel ticks after a PNG export keep the place under the pointer (#1255)", async () => {
    // A long directory with spaces, so the export's line wraps in a narrow
    // panel: the room it keeps is the room it took, not one line.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass 1255 a directory long enough for the line to wrap "));
    const w = vscode.window as unknown as Record<string, unknown>;
    const saved = { showSaveDialog: w.showSaveDialog, showInformationMessage: w.showInformationMessage };
    w.showSaveDialog = () => Promise.resolve(vscode.Uri.file(path.join(dir, "map with a long name to wrap the export line.png")));
    w.showInformationMessage = () => Promise.resolve(undefined);
    try {
      await onMap(false, async (panel) => {
        await panel.drive(NARROW);
        const first = await panel.probe(0.3, 0.3);
        const sent = panel.sent.length;
        await panel.drive([{ id: "export-png", click: true }]);
        const done = await panel.waitSent(isType("exportPngDone"), sent, "the PNG export's outcome");
        assert.strictEqual(done.outcome.status, "saved", "the PNG is saved");
        assert.match((await panel.drive([])).exportStatus, /^Exported PNG to /, "the export says where it went");
        const start = await placeAt(panel, first.point, "before the ticks");
        const before = lastDrawn(panel);
        for (let i = 1; i <= 3; i++) {
          await rendered(panel, () => panel.wheel({ client: first.point }, -120), `tick ${i}`);
        }
        const end = await placeAt(panel, first.point, "after the ticks");
        assert.deepStrictEqual(end.rect.map(Math.round), start.rect.map(Math.round), "the image's box did not move");
        samePlace(start, end, [before, lastDrawn(panel)], "the place under the pointer");
      });
    } finally {
      w.showSaveDialog = saved.showSaveDialog;
      w.showInformationMessage = saved.showInformationMessage;
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a feature's status line cleared and said again by a gesture's render does not move the map (#1255)", async () => {
    await onMap(false, async (panel) => {
      await panel.drive(NARROW);
      const sent = panel.sent.length;
      await panel.drive([{ id: "overlay-contours", checked: true }]);
      await panel.waitSent(isType("contourReady"), sent, "the contours");
      // Every contour answer from here is held back, and answered instead
      // with a refusal, as the provider answers for a grid it cannot contour.
      // The refusal is posted through the same wrapper, so it is marked to
      // pass the hold.
      const held = panel.hold((m) => (m.type === "contourReady" || m.type === "contourError") && !m.byTest);
      const refuse = async (what: string) => {
        const asked = await waitUntil(() => {
          const all = panel.received.filter(isType("contourRequest"));
          const last = all[all.length - 1];
          return last && held.held.some((m) => m.seq === last.seq) ? last : undefined;
        }, `the contour request (${what})`);
        await panel.post({ type: "contourError", seq: asked.seq, error: "not on this grid", byTest: true });
        assert.strictEqual((await panel.drive([])).contourStatus, "Contours: not on this grid", `the line says so (${what})`);
      };
      try {
        const first = await panel.probe(0.3, 0.3);
        await rendered(panel, () => panel.wheel({ client: first.point }, -120), "the first tick");
        await refuse("the first tick");
        const start = await placeAt(panel, first.point, "before the ticks");
        const before = lastDrawn(panel);
        for (let i = 2; i <= 4; i++) {
          await rendered(panel, () => panel.wheel({ client: first.point }, -120), `tick ${i}`);
          // The render clears the line until the contours answer.
          assert.strictEqual((await panel.drive([])).contourStatus, "", `tick ${i}'s render clears the line`);
          const cleared = await placeAt(panel, first.point, `tick ${i}, the line cleared`);
          assert.deepStrictEqual(
            cleared.rect.map(Math.round),
            start.rect.map(Math.round),
            `tick ${i}: the cleared line keeps its room`,
          );
          await refuse(`tick ${i}`);
        }
        const end = await placeAt(panel, first.point, "after the ticks");
        assert.deepStrictEqual(end.rect.map(Math.round), start.rect.map(Math.round), "the image's box did not move");
        samePlace(start, end, [before, lastDrawn(panel)], "the place under the pointer");

        // Contours turned off: nothing more is coming for the line, so its
        // room goes.
        await panel.drive([{ id: "overlay-contours", checked: false }]);
        const off = await placeAt(panel, first.point, "contours off");
        assert.ok(off.rect[1] < start.rect[1], `the line's room goes with the contours: top ${start.rect[1]} → ${off.rect[1]}`);
      } finally {
        await held.release();
      }
    });
  });

  const AUTO ='input[name="bounds-mode"][value="auto"]';
  const MANUAL = 'input[name="bounds-mode"][value="manual"]';

  for (const [how, back] of [
    ["Reset view", [{ id: "zoom-reset", click: true }]],
    ["Bounds Auto", [{ selector: AUTO, checked: true }]],
  ] as const) {
    test(`a wheel between ${how} and its render leaves Bounds on Auto (#1259)`, async () => {
      await onMap(false, async (panel) => {
        const before = lastDrawn(panel);
        await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "the zoom");
        assert.strictEqual((await panel.controls())["radio:bounds-mode"], "manual", "the zoom set Bounds to Manual");
        const gate = panel.hold(isType("gridReady"));
        try {
          const received = panel.received.length;
          await panel.drive([...back]);
          const ask = await waitUntil(
            () => panel.received.slice(received).find(isType("rerenderRequest")),
            `the render ${how} asks for`,
          );
          assert.strictEqual(ask.boundsLatMin, undefined, `${how} asks for the grid's own window`);
          await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");

          // The zoomed image is still on screen, and the window asked for is
          // not known until its render lands.
          const wheeled = panel.received.length;
          const turned = await panel.wheel([0.5, 0.5], -250);
          // The map on screen zooms once that render lands, so the wheel is
          // held back, not let through to scroll the page under it (#1269).
          assert.strictEqual(turned.prevented, true, "the wheel over the map does not scroll the page");
          // Past the 250 ms the page waits for a wheel to settle.
          await panel.pageTimer(300);
          assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds stay on Auto");
          assert.deepStrictEqual(
            panel.received.slice(wheeled).filter(isType("rerenderRequest")),
            [],
            "and the wheel asks for no render",
          );
          const sent = panel.sent.length;
          await gate.release();
          const drawn = await panel.waitSent((m) => m.type === "gridReady" && m.seq === ask.seq, sent, "its render");
          close(drawn.usedLonMax - drawn.usedLonMin, before.usedLonMax - before.usedLonMin, `${how} is drawn`, 1e-6);
        } finally {
          await gate.release();
        }
        // Once it has, the wheel zooms the window drawn.
        const after = await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "a wheel after it");
        close(
          after.usedLonMax - after.usedLonMin,
          (before.usedLonMax - before.usedLonMin) * Math.pow(1.002, -250),
          "the wheel zooms the window drawn",
          1e-3,
        );
      });
    });
  }

  /** A window inside the one `drawn`, the middle half of it each way. */
  function middleHalf(drawn: Msg): MapWindow {
    return {
      latMin: drawn.usedLatMin + (drawn.usedLatMax - drawn.usedLatMin) * 0.25,
      latMax: drawn.usedLatMin + (drawn.usedLatMax - drawn.usedLatMin) * 0.75,
      lonMin: drawn.usedLonMin + (drawn.usedLonMax - drawn.usedLonMin) * 0.25,
      lonMax: drawn.usedLonMin + (drawn.usedLonMax - drawn.usedLonMin) * 0.75,
    };
  }

  for (const [projection, how, typedFor] of [
    ["equirectangular", "", middleHalf],
    // Pole to pole: Web Mercator draws it inside its band, as Rust clamps it.
    [
      "web_mercator",
      " from pole to pole",
      (drawn: Msg): MapWindow => Object.assign(middleHalf(drawn), { latMin: -90, latMax: 90 }),
    ],
  ] as const) {
    test(`a wheel after typing a Bounds field${how}, before its render, zooms the typed window on ${projection} (#1259)`, async () => {
      await onMap(false, async (panel) => {
        let sent = panel.sent.length;
        await panel.drive([{ selector: MANUAL, checked: true }]);
        const before = await panel.waitSent(isType("gridReady"), sent, "the render on Manual");
        const typed = typedFor(before);
        const mercator = projection === "web_mercator";
        const y = mercator ? mercatorY : (lat: number) => lat;
        const drawnTyped = mercator
          ? Object.assign({}, typed, {
              latMin: Math.max(-MERCATOR_MAX_LAT, typed.latMin),
              latMax: Math.min(MERCATOR_MAX_LAT, typed.latMax),
            })
          : typed;
        const gate = panel.hold(isType("gridReady"));
        let drawn: Msg;
        try {
          let received = panel.received.length;
          await panel.drive([
            { id: "bounds-lat-min", value: typed.latMin },
            { id: "bounds-lat-max", value: typed.latMax },
            { id: "bounds-lon-min", value: typed.lonMin },
            { id: "bounds-lon-max", value: typed.lonMax },
          ]);
          await waitUntil(
            () => panel.received.slice(received).find((m) => m.type === "rerenderRequest" && m.boundsLonMax === typed.lonMax),
            "the render the typed window asks for",
          );
          // The old image is still on screen; the wheel turns over its middle.
          received = panel.received.length;
          await panel.wheel([0.5, 0.5], -250);
          const ask = await waitUntil(
            () => panel.received.slice(received).find(isType("rerenderRequest")),
            "the render the wheel asks for",
          );
          const factor = Math.pow(1.002, -250);
          close(ask.boundsLonMax - ask.boundsLonMin, (typed.lonMax - typed.lonMin) * factor, "the wheel zooms the typed span", 1e-3);
          close(
            y(ask.boundsLatMax) - y(ask.boundsLatMin),
            (y(drawnTyped.latMax) - y(drawnTyped.latMin)) * factor,
            "in latitude too",
            1e-3,
          );
          // A pointer lands on whole pixels, so the middle is good to a pixel.
          const px = (typed.lonMax - typed.lonMin) / before.width;
          close((ask.boundsLonMin + ask.boundsLonMax) / 2, (typed.lonMin + typed.lonMax) / 2, "about its middle", 2 * px);
          sent = panel.sent.length;
          await gate.release();
          drawn = await panel.waitSent((m) => m.type === "gridReady" && m.seq === ask.seq, sent, "the wheel's render");
        } finally {
          await gate.release();
        }
        assert.strictEqual((await panel.controls())["radio:bounds-mode"], "manual", "Bounds stay on Manual");
        close(drawn.usedLonMax - drawn.usedLonMin, (typed.lonMax - typed.lonMin) * Math.pow(1.002, -250), "drawn zoomed", 1e-3);
      }, projection);
    });
  }

  for (const [how, early] of [
    ["Reset view", false],
    ["a typed window", false],
    // A wheel turned at once settles while the stale Mollweide image is up,
    // with no window to write: it is dropped, and the typed window kept.
    ["a typed window", true],
  ] as const) {
    const when = early ? ", with a wheel settling over it," : "";
    test(`a stale Mollweide render landing after ${how}${when} leaves the window asked for in place (#1259)`, async () => {
      await onMap(false, async (panel) => {
        await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "the zoom");
        const zoomed = lastDrawn(panel);
        const typed = middleHalf(zoomed);
        const gate = panel.hold(isType("gridReady"));
        try {
          // Three renders on their way, answered oldest first: Mollweide,
          // the zoomed window on Equirectangular again, then the last ask.
          const ask = async (actions: Parameters<RealPanel["drive"]>[0], what: string): Promise<Msg> => {
            const received = panel.received.length;
            await panel.drive(actions);
            return waitUntil(() => {
              const asks = panel.received.slice(received).filter(isType("rerenderRequest"));
              return asks.length ? asks[asks.length - 1] : undefined;
            }, `the render ${what} asks for`);
          };
          const mollweide = await ask([{ id: "picker-projection", value: "mollweide" }], "Mollweide");
          const back = await ask([{ id: "picker-projection", value: "equirectangular" }], "Equirectangular");
          assert.ok(back.boundsLatMin != null, "Equirectangular asks for the zoomed window again");
          const last =
            how === "Reset view"
              ? await ask([{ id: "zoom-reset", click: true }], how)
              : await ask(
                  [
                    { id: "bounds-lat-min", value: typed.latMin },
                    { id: "bounds-lat-max", value: typed.latMax },
                    { id: "bounds-lon-min", value: typed.lonMin },
                    { id: "bounds-lon-max", value: typed.lonMax },
                  ],
                  how,
                );
          if (how === "Reset view") {
            assert.strictEqual(last.boundsLatMin, undefined, "Reset view asks for the grid's own window");
          } else {
            assert.deepStrictEqual(
              [last.boundsLatMin, last.boundsLatMax, last.boundsLonMin, last.boundsLonMax],
              [typed.latMin, typed.latMax, typed.lonMin, typed.lonMax],
              "the last ask is the typed window",
            );
          }
          const land = async (m: Msg) => {
            await waitUntil(() => gate.held.find((g) => g.seq === m.seq), `render ${m.seq}, held back`);
            await gate.deliver((g) => g.seq === m.seq);
            // The page has drawn it once it answers after it.
            await panel.controls();
          };
          if (early) {
            // The Mollweide render is in hand before the wheel turns, so it
            // lands well inside the 250 ms the wheel waits to settle, however
            // long the render took (#1257).
            await waitUntil(() => gate.held.find((g) => g.seq === mollweide.seq), `render ${mollweide.seq}, held back`);
            const turned = panel.received.length;
            const tick = await panel.wheel([0.5, 0.5], -250);
            // The wheel was taken as a zoom, so the no-render check below is
            // about a zoom settling, not a wheel the page let go by.
            assert.strictEqual(tick.prevented, true, "the early wheel is taken, not let through to scroll");
            assert.notStrictEqual(tick.transform[1], tick.transform[0], "the early wheel zooms the preview");
            // Then wait on the page's own clock until it has settled.
            await land(mollweide);
            await panel.pageTimer(300);
            assert.deepStrictEqual(
              panel.received.slice(turned).filter(isType("rerenderRequest")),
              [],
              "the wheel settling over Mollweide asks for no render",
            );
          } else {
            await land(mollweide);
          }
          await land(back);
          assert.strictEqual(lastDrawn(panel).seq, back.seq, "the stale zoomed window is on screen");

          const wheeled = panel.received.length;
          await panel.wheel([0.5, 0.5], -250);
          if (how === "Reset view") {
            // Past the 250 ms the page waits for a wheel to settle.
            await panel.pageTimer(300);
            assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds stay on Auto");
            assert.deepStrictEqual(
              panel.received.slice(wheeled).filter(isType("rerenderRequest")),
              [],
              "and the wheel asks for no render",
            );
          } else {
            const zoom = await waitUntil(
              () => panel.received.slice(wheeled).find(isType("rerenderRequest")),
              "the render the wheel asks for",
            );
            close(
              zoom.boundsLonMax - zoom.boundsLonMin,
              (typed.lonMax - typed.lonMin) * Math.pow(1.002, -250),
              "the wheel zooms the typed window",
              1e-3,
            );
          }
        } finally {
          await gate.release();
        }
      });
    });
  }

  test("a drag with Flip Y axis on moves the map the way the cursor moves", async () => {
    await onMap(true, async (panel) => {
      const before = lastDrawn(panel);
      const grabbed = await latAt(panel, 0.3, "where the drag starts");
      const dragged = panel.received.length;
      await rendered(panel, () => panel.drag([0.5, 0.3], [0.5, 0.5]), "the drag");
      // The click a browser fires as the drag's button comes up is not a probe.
      assert.deepStrictEqual(
        panel.received.slice(dragged).filter(isType("probeRequest")),
        [],
        "the drag's click reads no value",
      );
      const dropped = await latAt(panel, 0.5, "where the cursor let go");
      assert.ok(
        withinTwoRows(dropped, grabbed, before),
        `the place grabbed is under the cursor where it let go: ${grabbed} grabbed, ${dropped} there`,
      );
    });
  });

  test("a drag released off the image does not swallow the next click", async () => {
    await onMap(false, async (panel) => {
      // Off the image the browser fires no click to use up the drag's.
      await rendered(panel, () => panel.drag([0.5, 0.3], [0.5, 0.5], true), "the drag");
      const clicked = panel.received.length;
      await latAt(panel, 0.5, "the next click on the map");
      assert.strictEqual(
        panel.received.slice(clicked).filter(isType("probeRequest")).length,
        1,
        "the click reads the value under it",
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
        const turned = await panel.wheel([0.5, 0.5], -250);
        // The map asked for takes no wheel, so the page scrolls now as it
        // will once that map is drawn (#1269).
        assert.strictEqual(turned.prevented, false, "the wheel scrolls the page");
        // Past the 250 ms the page waits for a wheel to settle.
        await panel.pageTimer(300);
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

  test("a wheel over a Mollweide map scrolls the page and leaves Bounds alone (#1269)", async () => {
    await onMap(
      false,
      async (panel) => {
        assert.strictEqual(lastDrawn(panel).options.projection, "mollweide", "a Mollweide map is on screen");
        const wheeled = panel.received.length;
        const turned = await panel.wheel([0.5, 0.5], -250);
        assert.strictEqual(turned.prevented, false, "the wheel scrolls the page");
        await panel.pageTimer(300);
        assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", "Bounds stay on Auto");
        assert.deepStrictEqual(
          panel.received.slice(wheeled).filter(isType("rerenderRequest")),
          [],
          "and the wheel asks for no render",
        );
      },
      "mollweide",
    );
  });

  /** Turn the wheel over the middle of the Mollweide image on screen while
   *  a lat/lon render is held back, and check it is held: the page does not
   *  scroll, Bounds stay on Auto, and no render is asked for (#1272). */
  async function wheelHeld(panel: RealPanel, what: string): Promise<void> {
    assert.strictEqual(lastDrawn(panel).options.projection, "mollweide", `a Mollweide map is on screen (${what})`);
    const wheeled = panel.received.length;
    const turned = await panel.wheel([0.5, 0.5], -250);
    // The map asked for zooms once it lands, so the wheel is held back
    // now rather than scrolling the page under the pointer.
    assert.strictEqual(turned.prevented, true, `the wheel does not scroll the page (${what})`);
    // Past the 250 ms the page waits for a wheel to settle.
    await panel.pageTimer(300);
    assert.strictEqual((await panel.controls())["radio:bounds-mode"], "auto", `Bounds stay on Auto (${what})`);
    assert.deepStrictEqual(
      panel.received.slice(wheeled).filter(isType("rerenderRequest")),
      [],
      `and the wheel asks for no render (${what})`,
    );
  }

  for (const target of ["equirectangular", "web_mercator"] as const) {
    test(`a wheel over a Mollweide map, while the ${target} render is on its way, is held, then zooms (#1272)`, async () => {
      await onMap(
        false,
        async (panel) => {
          const gate = panel.hold(isType("gridReady"));
          let asked: Msg;
          try {
            const received = panel.received.length;
            await panel.drive([{ id: "picker-projection", value: target }]);
            asked = await waitUntil(
              () => panel.received.slice(received).find((m) => m.type === "rerenderRequest" && m.projection === target),
              `the ${target} render`,
            );
            await waitUntil(() => gate.held.find((m) => m.seq === asked.seq), "that render, held back");
            await wheelHeld(panel, `${target} on its way`);
            const sent = panel.sent.length;
            await gate.release();
            await panel.waitSent((m) => m.type === "gridReady" && m.seq === asked.seq, sent, `the ${target} render`);
          } finally {
            await gate.release();
          }
          const before = lastDrawn(panel);
          assert.strictEqual(before.options.projection, target, `the ${target} map is drawn`);
          // Once it has, the wheel zooms the window drawn.
          const after = await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "a wheel after it");
          close(
            after.usedLonMax - after.usedLonMin,
            (before.usedLonMax - before.usedLonMin) * Math.pow(1.002, -250),
            "the wheel zooms the window drawn",
            1e-3,
          );
        },
        "mollweide",
      );
    });
  }

  test("a wheel over a stale Mollweide render landing after Reset view is held, then zooms (#1272)", async () => {
    await onMap(false, async (panel) => {
      const whole = lastDrawn(panel);
      await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "the zoom");
      const gate = panel.hold(isType("gridReady"));
      let reset: Msg;
      try {
        const ask = async (actions: Parameters<RealPanel["drive"]>[0], what: string): Promise<Msg> => {
          const received = panel.received.length;
          await panel.drive(actions);
          const m = await waitUntil(() => {
            const asks = panel.received.slice(received).filter(isType("rerenderRequest"));
            return asks.length ? asks[asks.length - 1] : undefined;
          }, `the render ${what} asks for`);
          await waitUntil(() => gate.held.find((g) => g.seq === m.seq), `render ${m.seq}, held back`);
          return m;
        };
        const mollweide = await ask([{ id: "picker-projection", value: "mollweide" }], "Mollweide");
        await ask([{ id: "picker-projection", value: "equirectangular" }], "Equirectangular");
        reset = await ask([{ id: "zoom-reset", click: true }], "Reset view");
        assert.strictEqual(reset.boundsLatMin, undefined, "Reset view asks for the grid's own window");
        await gate.deliver((g) => g.seq === mollweide.seq);
        // The page has drawn it once it answers after it.
        await panel.controls();
        await wheelHeld(panel, "the stale Mollweide image up");
        const sent = panel.sent.length;
        await gate.release();
        await panel.waitSent((m) => m.type === "gridReady" && m.seq === reset.seq, sent, "Reset view's render");
      } finally {
        await gate.release();
      }
      const before = lastDrawn(panel);
      close(before.usedLonMax - before.usedLonMin, whole.usedLonMax - whole.usedLonMin, "Reset view is drawn", 1e-6);
      const after = await rendered(panel, () => panel.wheel([0.5, 0.5], -250), "a wheel after it");
      close(
        after.usedLonMax - after.usedLonMin,
        (before.usedLonMax - before.usedLonMin) * Math.pow(1.002, -250),
        "the wheel zooms the window drawn",
        1e-3,
      );
    });
  });

  for (const inFlight of [false, true]) {
    const when = inFlight ? ", while its re-render is on its way," : "";
    test(`a wheel over a slice drawn in its source view${when} with a lat/lon target held scrolls the page (#1272)`, async () => {
      // lat_bnds is placed on nothing, so it is drawn in the source view while
      // the Equirectangular target chosen before is held for the next map
      // slice (#1099). Every render of it still asks for Equirectangular, but
      // no lat/lon map is coming from that: the wheel is never held, neither
      // once the render has answered nor while a re-render (here Flip Y axis)
      // is on its way, as during playback.
      const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
      const native = loadNative();
      assert.ok(native, "native binding required");
      const index = (name: string): number => {
        const v = native.open(fs.readFileSync(file)).variables().find((x) => x.name === name);
        assert.ok(v, `the fixture has ${name}`);
        return v.index;
      };
      const p = await provider();
      const editor = await openEditor(p, file);
      const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: index("temperature") }));
      try {
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        let from = panel.sent.length;
        await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
        await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
        from = panel.sent.length;
        let received = panel.received.length;
        const latBnds = index("lat_bnds");
        await panel.drive([{ id: "slice-variable", value: latBnds }]);
        const flat = await panel.waitSent((m) => m.type === "gridReady" && m.messageIndex === latBnds, from, "lat_bnds");
        assert.strictEqual(flat.options.projection, "source", "lat_bnds is drawn in the source view");
        const asked = panel.received.slice(received).filter(isType("rerenderRequest"));
        assert.strictEqual(asked[asked.length - 1].projection, "equirectangular", "the render asked for Equirectangular");
        assert.strictEqual(lastDrawn(panel).seq, asked[asked.length - 1].seq, "and its answer is the one drawn");

        let held: ReturnType<RealPanel["hold"]> | null = null;
        if (inFlight) {
          held = panel.hold(isType("gridReady"));
          received = panel.received.length;
          await panel.drive([{ id: "flip-y", checked: true }]);
          const again = await waitUntil(
            () => panel.received.slice(received).find(isType("rerenderRequest")),
            "the re-render of lat_bnds",
          );
          assert.strictEqual(again.projection, "equirectangular", "the re-render asks for Equirectangular too");
          await waitUntil(() => held?.held.find((m) => m.seq === again.seq), "that render, held back");
        }
        try {
          const wheeled = panel.received.length;
          const turned = await panel.wheel([0.5, 0.5], -250);
          assert.strictEqual(turned.prevented, false, "the wheel scrolls the page");
          await panel.pageTimer(300);
          assert.deepStrictEqual(
            panel.received.slice(wheeled).filter(isType("rerenderRequest")),
            [],
            "and the wheel asks for no render",
          );
        } finally {
          await held?.release();
        }
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  }

  test("a wheel over a Mollweide map whose lat/lon render failed scrolls the page (#1272)", async () => {
    // The render last asked for has answered, with an error, so no lat/lon
    // map is coming and the Mollweide image stays: the wheel is not held.
    await onMap(
      false,
      async (panel) => {
        const gate = panel.hold(isType("gridReady"));
        try {
          const received = panel.received.length;
          await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
          const asked = await waitUntil(
            () => panel.received.slice(received).find((m) => m.type === "rerenderRequest" && m.projection === "equirectangular"),
            "the Equirectangular render",
          );
          await waitUntil(() => gate.held.find((m) => m.seq === asked.seq), "that render, held back");
          // It fails instead, as the provider answers a render it could not draw.
          await panel.post({ type: "gridError", seq: asked.seq, error: "render failed" });
          await panel.controls();
          assert.strictEqual(lastDrawn(panel).options.projection, "mollweide", "the Mollweide map stays on screen");
          const wheeled = panel.received.length;
          const turned = await panel.wheel([0.5, 0.5], -250);
          assert.strictEqual(turned.prevented, false, "the wheel scrolls the page");
          await panel.pageTimer(300);
          assert.deepStrictEqual(
            panel.received.slice(wheeled).filter(isType("rerenderRequest")),
            [],
            "and the wheel asks for no render",
          );
        } finally {
          // Dropped, not delivered: its render has already answered.
          gate.held.length = 0;
          await gate.release();
        }
      },
      "mollweide",
    );
  });
});
