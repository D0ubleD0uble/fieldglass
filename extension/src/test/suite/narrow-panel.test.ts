// The render panel's toolbar in a narrow panel (#1429, after #1393).
//
// At 300, 450 and 900 px every control in every toolbar row stays inside the
// toolbar's box: none is pushed past its right edge, where the page would
// scroll sideways to reach it. And the Bounds row's Manual fields, which keep
// their room on Auto so a gesture that ticks Manual does not move the map
// (#1228), keep no more than one control row of it: they stay on one line,
// which scrolls sideways where the four do not fit.
//
// Driven through the real panel, at the widths a user drags it to: each width
// is the page's whole width, the body's padding included.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { zoomMapWindow, type MapWindow } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type Box,
  type PageState,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

/** The widths the issue names, as the page's whole width. */
const WIDTHS = [300, 450, 900];

/** A body `width` that makes the page `px` wide: the body's padding is
 *  1.5rem a side. */
const pageWidth = (px: number) => ({ id: "body", width: `calc(${px}px - 3rem)` });

/** Fail if anything in the toolbar is drawn past its sides, or the page is
 *  wider than a window it fits in. */
function assertInsideToolbar(state: PageState, px: number, what: string): void {
  const bar = state.toolbar;
  assert.deepStrictEqual(
    bar.outside.map((c) => `${c.what} [${Math.round(c.box.left)}, ${Math.round(c.box.right)}]`),
    [],
    `${what}: drawn outside the toolbar [${Math.round(bar.box.left)}, ${Math.round(bar.box.right)}]`,
  );
  // A page wider than the window it is in (900 px in a small test window)
  // scrolls sideways for that reason alone.
  if (px <= bar.viewportWidth) assert.ok(!bar.pageScrollsSideways, `${what}: the page scrolls sideways`);
}

const BOUNDS = ["bounds-lat-min", "bounds-lat-max", "bounds-lon-min", "bounds-lon-max"];

/** Fail unless each Bounds field, focused as Tab focuses it, is inside the
 *  toolbar: the line it is on scrolls it into view. */
async function assertBoundsReachable(panel: RealPanel, px: number): Promise<void> {
  for (const id of BOUNDS) {
    const bar = (await panel.drive([{ id, focus: true }])).toolbar;
    assert.ok(bar.focused, `${px} px: #${id} takes focus`);
    assert.ok(
      bar.focused.left >= bar.box.left - 0.5 && bar.focused.right <= bar.box.right + 0.5,
      `${px} px: #${id} [${bar.focused.left}, ${bar.focused.right}] is outside the toolbar [${bar.box.left}, ${bar.box.right}] when focused`,
    );
  }
}

/** The GFS fixture twice over: a file of two messages on one grid, a u/v
 *  pair, so its panel has the Vectors row as well as Bounds and Overlay. */
function pairFile(dir: string): string {
  const gfs = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  assert.ok(fs.existsSync(gfs), `fixture missing: ${gfs}`);
  const one = fs.readFileSync(gfs);
  const file = path.join(dir, "gfs_pair.grib2");
  fs.writeFileSync(file, Buffer.concat([one, one]));
  return file;
}

/** Open the pair's first message on Equirectangular, with every overlay
 *  layer and its arrows on, so every field that shows only while its toggle
 *  is on is shown; and run `body` on it. */
async function onWindMap(body: (panel: RealPanel) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1429-"));
  const p = await provider();
  const editor = await openEditor(p, pairFile(dir));
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    let from = panel.sent.length;
    await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
    await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
    from = panel.sent.length;
    await panel.drive([
      { id: "vector-field-v", value: 1 },
      { id: "overlay-vectors", checked: true },
    ]);
    await panel.waitSent(isType("vectorResult"), from, "the arrows");
    await panel.drive(
      ["coastlines", "borders", "lakes", "rivers", "graticule", "contours"].map((layer) => ({
        id: `overlay-${layer}`,
        checked: true,
      })),
    );
    await body(panel);
  } finally {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

/** `netcdf4_dimscale.nc`'s `temperature` (time × lat × lon) on
 *  Equirectangular: a slice panel, with steppers, Compare and Line plot. */
async function onSlice(): Promise<{ panel: RealPanel; close: () => void }> {
  const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
  const native = loadNative();
  assert.ok(native, "native binding required");
  const temperature = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "temperature");
  assert.ok(temperature, "the fixture has temperature");
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
  const close = () => {
    panel.dispose();
    editor.close();
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    const from = panel.sent.length;
    await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
    await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
  } catch (e) {
    close();
    throw e;
  }
  return { panel, close };
}

const bounds = (mode: string) => ({ selector: `input[name="bounds-mode"][value="${mode}"]`, checked: true });

suite("The toolbar in a narrow panel (#1429)", function () {
  this.timeout(120_000);

  test("every control of a map's toolbar stays inside it at 300, 450 and 900 px, on Auto and Manual", async () => {
    await onWindMap(async (panel) => {
      const opened = await panel.drive([]);
      assert.strictEqual(opened.controlShown.arrows, true, "the Vectors row is shown");
      assert.strictEqual(opened.controlShown.overlay, true, "the Overlay row is shown");
      assert.strictEqual(opened.contours, true, "Contours is ticked, so its interval field is shown");
      for (const mode of ["auto", "manual"]) {
        await panel.drive([{ selector: `input[name="range-mode"][value="${mode}"]`, checked: true }, bounds(mode)]);
        for (const px of WIDTHS) {
          const state = await panel.drive([pageWidth(px)]);
          assert.ok(state.toolbar.boundsFields, `${px} px ${mode}: the Bounds row is shown`);
          assertInsideToolbar(state, px, `${px} px, ${mode}`);
          if (mode === "manual") await assertBoundsReachable(panel, px);
        }
      }
    });
  });

  test("every control of a slice panel's toolbar stays inside it at 300, 450 and 900 px, Compare on", async () => {
    const { panel, close } = await onSlice();
    try {
      const from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(isType("gridReady"), from, "the difference map");
      const opened = await panel.drive([{ id: "line-toggle", checked: true }]);
      assert.ok(opened.steppers && opened.steppers.fieldB.length > 0, "Field B's steppers are shown");
      for (const px of WIDTHS) assertInsideToolbar(await panel.drive([pageWidth(px)]), px, `${px} px`);
    } finally {
      close();
    }
  });

  test("the status, probe readout, subtitle and line-plot caption never start a line with their dot, at any width", async () => {
    const { panel, close } = await onSlice();
    try {
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const from = panel.sent.length;
      await panel.probe(0.5, 0.5);
      await panel.waitSent(isType("lineResult"), from, "the line");
      let wrapped = 0;
      for (let px = 160; px <= 700; px += 10) {
        const state = await panel.drive([pageWidth(px)]);
        for (const [what, lines] of Object.entries(state.drawnLines)) {
          if (lines.length > 1) wrapped++;
          assert.ok(lines.every((l) => !l.startsWith("·")), `${px} px: the ${what} starts a line with its dot: ${JSON.stringify(lines)}`);
        }
      }
      assert.ok(wrapped > 20, `the lines wrap at these widths (${wrapped} wrapped readings)`);
    } finally {
      close();
    }
  });

  test("the Bounds row's Manual fields keep one control row of room at 300, 450 and 900 px", async () => {
    await onWindMap(async (panel) => {
      for (const px of WIDTHS) {
        for (const mode of ["auto", "manual"]) {
          await panel.drive([bounds(mode)]);
          const bar = (await panel.drive([pageWidth(px)])).toolbar;
          assert.ok(bar.boundsFields && bar.controlHeight > 0, `${px} px ${mode}: the Bounds row is laid out`);
          const tall = bar.boundsFields.bottom - bar.boundsFields.top;
          const row = Math.max(bar.controlHeight, bar.manualHeight);
          assert.ok(tall <= row + 1, `${px} px ${mode}: the Manual fields take ${tall} px, more than one ${row} px row`);
        }
      }
    });
  });

  test("the wheel over a focused Bounds field scrolls their line where they do not fit, and the page otherwise", async () => {
    await onWindMap(async (panel) => {
      await panel.drive([bounds("manual")]);
      let bar = (await panel.drive([pageWidth(300), { id: "bounds-fieldset", scroll: true }])).toolbar;
      assert.ok(bar.boundsScroll && bar.boundsScroll.room > 0, "at 300 px the four fields do not fit their line");
      assert.strictEqual(bar.boundsScroll.left, 0, "the line starts at lat min");
      // A wheel passing over the line, scrolling the page, is not caught.
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, 0, "no field has focus: the line stays put");
      assert.strictEqual(bar.wheelPrevented, false, "and the wheel is left to scroll the page");
      // With a field focused, as a user who has clicked or tabbed into one.
      bar = (await panel.drive([{ id: "bounds-lat-min", focus: true }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, 0, "focusing lat min keeps the line at its start");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100, ctrlKey: true }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, 0, "Ctrl+wheel is a zoom, not a scroll of the line");
      assert.strictEqual(bar.wheelPrevented, false, "and is left to the page");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, bar.boundsScroll?.room, "a notch turns the line to its lon page");
      assert.strictEqual(bar.wheelPrevented, true, "and not the page as well");
      assert.strictEqual(bar.focusedId, "bounds-lon-min", "focus moves from lat min to lon min, on the page shown");
      assert.ok(bar.focused && bar.boundsFields && bar.focused.left >= bar.boundsFields.left - 0.5 && bar.focused.right <= bar.boundsFields.right + 0.5,
        "and the focused field is in view");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, bar.boundsScroll?.room, "and no further");
      assert.strictEqual(bar.wheelPrevented, false, "so the next notch is the page's");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: -100 }])).toolbar;
      assert.strictEqual(bar.boundsScroll?.left, 0, "a notch back returns to lat");
      assert.strictEqual(bar.focusedId, "bounds-lat-min", "and focus with it");

      bar = (await panel.drive([pageWidth(900), { id: "bounds-lat-min", focus: true }])).toolbar;
      // The line stops paging a frame after the resize, from its observer.
      for (let i = 0; i < 20 && (bar.boundsScroll?.room ?? 0) > 0; i++) {
        await new Promise((r) => setTimeout(r, 50));
        bar = (await panel.drive([])).toolbar;
      }
      assert.strictEqual(bar.boundsScroll?.room, 0, "at 900 px the four fields fit");
      bar = (await panel.drive([{ id: "bounds-manual-fields", wheelY: 100 }])).toolbar;
      assert.strictEqual(bar.wheelPrevented, false, "and the wheel over them is left to scroll the page");
    });
  });

  test("every Bounds field shows its whole value at 300, 450 and 900 px", async () => {
    await onWindMap(async (panel) => {
      await panel.drive([bounds("manual")]);
      for (const values of [
        ["-90.000", "90.000", "-180.000", "360.000"],
        ["-89.999", "89.999", "-179.999", "-179.999"],
      ]) {
        // Typed without a change, so no render rewrites them under the test.
        await panel.drive(BOUNDS.map((id, i) => ({ id, keys: values[i] })));
        for (const px of WIDTHS) {
          await panel.drive([pageWidth(px)]);
          // Read once the line has settled its paging, a frame after the
          // resize.
          await new Promise((r) => setTimeout(r, 150));
          const bar = (await panel.drive([])).toolbar;
          const clipped = bar.boundsPairs
            .flatMap((p) => p.clipped.map((c, i) => (c > 0 ? `${p.label} ${i ? "max" : "min"} by ${c} px` : "")))
            .filter((c) => c);
          assert.deepStrictEqual(
            clipped,
            [],
            `${px} px: ${values.join(", ")}: a field cuts its value short (pairs need ${bar.boundsPairs.map((p) => p.need)} px)`,
          );
        }
      }
    });
  });

  /** Whether each pair's fields read whole at `px`: no text wider than the
   *  room inside its field, measured in the field's own font, so a wider
   *  font on CI fails the same way a narrower one does not. */
  async function assertWhole(panel: RealPanel, what: string): Promise<void> {
    for (const px of WIDTHS) {
      await panel.drive([pageWidth(px)]);
      // The line pages from its resize observer, a frame after the resize: a
      // read repeats until nothing is cut short, or gives up and reports.
      let clipped: string[] = [];
      for (let i = 0; i < 40; i++) {
        const bar = (await panel.drive([])).toolbar;
        clipped = bar.boundsPairs
          .flatMap((p) => p.clipped.map((c, k) => (c > 0 ? `${p.label} ${k ? "max" : "min"} by ${c} px` : "")))
          .filter((c) => c);
        if (clipped.length === 0) break;
        await new Promise((r) => setTimeout(r, 50));
      }
      assert.deepStrictEqual(clipped, [], `${what} at ${px} px: a field cuts its value short`);
    }
    await panel.drive([pageWidth(900)]);
  }

  /** Run `gesture` and wait for the render it asks for to be drawn. */
  async function drawn(panel: RealPanel, gesture: () => Promise<unknown>, what: string): Promise<Msg> {
    const sent = panel.sent.length;
    await gesture();
    return panel.waitSent(isType("gridReady"), sent, `the render after ${what}`);
  }

  test("after a zoom and after a drag, every Bounds field shows its whole value at 300, 450 and 900 px (#1449)", async () => {
    await onWindMap(async (panel) => {
      const shown = async () => (await panel.drive([])).toolbar.boundsPairs.flatMap((p) => p.values);
      const titles = async () => (await panel.drive([])).toolbar.boundsPairs.flatMap((p) => p.titles);
      // West of 100 degrees, where a longitude is widest.
      await drawn(panel, () => panel.wheel([0.1, 0.3], -120), "a zoom");
      for (const value of await shown()) {
        assert.match(value, /^-?\d+\.\d{3}$/, `a zoom writes three decimals, as the page fills them in: ${value}`);
      }
      await assertWhole(panel, "after a zoom");
      await drawn(panel, () => panel.drag([0.7, 0.4], [0.4, 0.5]), "a drag");
      for (const value of await shown()) {
        assert.match(value, /^-?\d+\.\d{3}$/, `a drag writes three decimals: ${value}`);
      }
      await assertWhole(panel, "after a drag");
      // What the three decimals leave out is in the tooltip.
      const tips = await titles();
      assert.ok(
        tips.some((t) => /^-?\d+\.\d{4,}$/.test(t)),
        `a field a gesture wrote to more than three decimals gives the whole value in its tooltip: ${tips}`,
      );
    });
  });

  const KEYS = ["latMin", "latMax", "lonMin", "lonMax"] as const;
  const near = (got: MapWindow, want: MapWindow, what: string) => {
    for (const k of KEYS) assert.ok(Math.abs(got[k] - want[k]) < 1e-9, `${what}: ${k} is ${got[k]}, wanted ${want[k]}`);
  };
  const askedWindow = (m: Msg): MapWindow => ({
    latMin: m.boundsLatMin,
    latMax: m.boundsLatMax,
    lonMin: m.boundsLonMin,
    lonMax: m.boundsLonMax,
  });

  /** Click Zoom in until the window is the narrowest the map zooms to, and
   *  check each render asks for the window the zoom made. Resolves to it. */
  async function zoomDeep(panel: RealPanel, clicks = 18): Promise<MapWindow> {
    // The window the first click zooms from is the one on screen.
    const grid = panel.sent.filter(isType("gridReady")).pop() as Msg;
    let want: MapWindow = {
      latMin: grid.usedLatMin,
      latMax: grid.usedLatMax,
      lonMin: grid.usedLonMin,
      lonMax: grid.usedLonMax,
    };
    for (let i = 0; i < clicks; i++) {
      const before = panel.received.length;
      const done = await drawn(panel, () => panel.drive([{ id: "zoom-in", click: true }]), `zoom ${i}`);
      want = zoomMapWindow(want, false, 0.5, 0.5, 0.5);
      const asked = await waitUntil(
        () => panel.received.slice(before).find((m) => m.type === "rerenderRequest" && m.boundsLatMin != null),
        "the render the zoom asked for",
      );
      near(askedWindow(asked), want, `zoom ${i} asked`);
      assert.ok(done.usedLonMax > done.usedLonMin, `zoom ${i} drew`);
    }
    assert.ok(clicks < 18 || want.lonMax - want.lonMin < 0.011, `the zooms reached the narrowest window: ${want.lonMax - want.lonMin}`);
    return want;
  }

  test("a zoom carries its window to the render whole, down to the narrowest the map zooms to (#1449)", async () => {
    await onWindMap(async (panel) => {
      await zoomDeep(panel);
    });
  });

  test("a field edited back to the text it showed still asks for the whole window (#1449)", async () => {
    await onWindMap(async (panel) => {
      // Six clicks: edges of five decimals, which three would round.
      const want = await zoomDeep(panel, 6);
      const shown = (await panel.controls()) as Record<string, string>;
      // A digit typed and deleted again: input fires twice, change never.
      for (const id of BOUNDS) await panel.drive([{ id, keys: `${shown[id]}1` }, { id, keys: "" }]);
      const after = (await panel.controls()) as Record<string, string>;
      assert.deepStrictEqual(BOUNDS.map((id) => after[id]), BOUNDS.map((id) => shown[id]), "the fields read as they did");
      const before = panel.received.length;
      await panel.drive([{ id: "reverse-colormap", checked: true }]);
      const asked = await waitUntil(
        () => panel.received.slice(before).find((m) => m.type === "rerenderRequest"),
        "the render an unrelated change asks for",
      );
      near(askedWindow(asked), want, "after an unrelated change");
      // And a different text is the user's.
      await panel.drive([{ id: "bounds-lat-min", value: -12.345 }]);
      const tip = (await panel.drive([])).toolbar.boundsPairs[0].titles[0];
      assert.strictEqual(tip, "", "a typed value has no tooltip of a gesture's");
    });
  });

  test("a hide and show keeps a zoom's whole window and what the fields showed (#1449)", async () => {
    await onWindMap(async (panel) => {
      const want = await zoomDeep(panel, 6);
      const fields =async () => (await panel.drive([])).toolbar.boundsPairs.flatMap((p) => p.values);
      const tips = async () => (await panel.drive([])).toolbar.boundsPairs.flatMap((p) => p.titles);
      const shown = await fields();
      const titles = await tips();
      const from = panel.sent.length;
      await panel.hideShow();
      const back = await panel.waitSent(
        (m) => m.type === "gridReady" && m.options.boundsLatMin != null,
        from,
        "the render after a hide and show",
      );
      near(askedWindow(back.options), want, "after a hide and show");
      assert.deepStrictEqual(await fields(), shown, "the fields show what they showed");
      assert.deepStrictEqual(await tips(), titles, "and their tooltips say the same");
      for (const value of shown) assert.match(value, /^-?\d+\.\d{3}$/, `three decimals: ${value}`);
      for (const t of titles.filter((t) => t)) assert.match(t, /^-?\d+(\.\d{1,6})?$/, `a tooltip without arithmetic's trailing digits: ${t}`);
    });
  });

  test("the Bounds fields read lat [min] – [max]  lon [min] – [max]: all four where they fit, else a pair to a page, faded at the edge with more", async () => {
    await onWindMap(async (panel) => {
      await panel.drive([bounds("manual")]);
      const visible = (b: Box, strip: Box) => b.right > strip.left + 0.5 && b.left < strip.right - 0.5;
      const whole = (b: Box, strip: Box) => b.left >= strip.left - 0.5 && b.right <= strip.right + 0.5;
      let bar = (await panel.drive([pageWidth(900)])).toolbar;
      // The fade and the paging follow a resize observer and the scroll
      // event, which the page hears a frame after the layout: a read
      // repeats until it has.
      const settled = async (want: (b: typeof bar) => boolean) => {
        for (let i = 0; i < 40 && !want(bar); i++) {
          await new Promise((r) => setTimeout(r, 50));
          bar = (await panel.drive([])).toolbar;
        }
      };
      // The fade, and the ‹ › drawn in the page's clear end beside it, so
      // something on screen says the other pair is there.
      const fadeIs = async (want: { left: boolean; right: boolean }, what: string) => {
        await settled((b) => JSON.stringify(b.boundsFade) === JSON.stringify(want));
        assert.deepStrictEqual(bar.boundsFade, want, what);
        assert.deepStrictEqual(bar.boundsCue, { left: want.left ? "‹" : "", right: want.right ? "›" : "" }, `${what}: the cue drawn`);
      };
      // One page in view, the other wholly out of it, every piece of the
      // one in view whole.
      const onPage = (px: number, page: string) => {
        const strip = bar.boundsFields;
        assert.ok(strip, `${px} px: the Bounds fields are laid out`);
        for (const p of bar.boundsPairs) {
          const shown: boolean = visible(p.labelBox, strip) || p.inputs.some((b) => visible(b, strip));
          assert.strictEqual(shown, p.label === page, `${px} px on ${page}: ${p.label} is ${shown ? "" : "not "}shown`);
          if (shown) {
            for (const b of [p.labelBox, ...p.inputs]) assert.ok(whole(b, strip), `${px} px: ${p.label} is cut ${JSON.stringify(b)}`);
          }
        }
      };
      const outcome: string[] = [];
      for (const px of [900, 450, 300]) {
        bar = (await panel.drive([pageWidth(px), { id: "bounds-lat-min", focus: true }])).toolbar;
        await settled((b) => b.boundsPairs.every((p) => p.paged === (b.boundsScroll?.room ?? 0) > 0));
        const paged = bar.boundsPairs[0]?.paged ?? false;
        outcome.push(`${px}: ${paged ? "paged" : "all four"}`);
        assert.deepStrictEqual(bar.boundsPairs.map((p) => p.label), ["lat", "lon"]);
        // Wide, all four always fit; at 300 px they never do. Between, it
        // is whichever the page's font allows: all four whole, or paged.
        if (px === 900) assert.ok(!paged, "900 px: all four fields side by side");
        if (px === 300) assert.ok(paged, "300 px: a pair to a page");
        if (!paged) {
          const strip = bar.boundsFields;
          assert.ok(strip, `${px} px: the Bounds fields are laid out`);
          assert.strictEqual(bar.boundsScroll?.room, 0, `${px} px: all four shown, yet the line scrolls`);
          for (const p of bar.boundsPairs) {
            for (const b of [p.labelBox, ...p.inputs]) assert.ok(whole(b, strip), `${px} px: ${p.label} is cut ${JSON.stringify(b)}`);
          }
          await fadeIs({ left: false, right: false }, `${px} px: no edge fades`);
          continue;
        }
        onPage(px, "lat");
        await fadeIs({ left: false, right: true }, `${px} px on lat: the right edge fades`);
        bar = (await panel.drive([{ id: "bounds-lon-max", focus: true }])).toolbar;
        onPage(px, "lon");
        await fadeIs({ left: true, right: false }, `${px} px on lon: the left edge fades`);
        // Back the other way, as Shift+Tab goes: each field focused is in
        // view, its pair shown from its start, whatever scroll snapping
        // would have chosen.
        for (const id of ["bounds-lon-max", "bounds-lon-min", "bounds-lat-max", "bounds-lat-min"]) {
          bar = (await panel.drive([{ id, focus: true }])).toolbar;
          const strip = bar.boundsFields;
          assert.ok(strip && bar.focused, `${px} px: #${id} has focus`);
          assert.ok(
            bar.focused.left >= strip.left - 0.5 && bar.focused.right <= strip.right + 0.5,
            `${px} px: #${id} ${JSON.stringify(bar.focused)} is outside the line ${JSON.stringify(strip)}`,
          );
          onPage(px, id.includes("lat") ? "lat" : "lon");
        }
      }
      // Recorded in the run's output: which the page's font chose at 450 px.
      console.log(`Bounds fields (#1429): ${outcome.join(", ")}`);
    });
  });

  test("ticking Bounds Manual and back moves nothing below the toolbar, at 300, 450 and 900 px (#1228)", async () => {
    await onWindMap(async (panel) => {
      for (const px of WIDTHS) {
        await panel.drive([bounds("auto")]);
        let auto = await panel.drive([pageWidth(px)]);
        // The Bounds line settles its paging a frame after a resize, from
        // its resize observer, as it does before the user sees the frame:
        // read once it has held still.
        for (let i = 0; i < 20; i++) {
          await new Promise((r) => setTimeout(r, 50));
          const again = await panel.drive([]);
          if (Math.round(again.image.top) === Math.round(auto.image.top)) break;
          auto = again;
        }
        // Read once the line has settled, as after the resize above.
        const steady = async (state: PageState) => {
          for (let i = 0; i < 20; i++) {
            await new Promise((r) => setTimeout(r, 50));
            const again = await panel.drive([]);
            if (Math.round(again.image.top) === Math.round(state.image.top)) return again;
            state = again;
          }
          return state;
        };
        const manual = await steady(await panel.drive([bounds("manual")]));
        assert.strictEqual(Math.round(manual.image.top), Math.round(auto.image.top), `${px} px: Manual moved the map`);
        const back = await steady(await panel.drive([bounds("auto")]));
        assert.strictEqual(Math.round(back.image.top), Math.round(auto.image.top), `${px} px: Auto moved the map`);
      }
    });
  });
});
