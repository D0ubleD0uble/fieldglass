// The map-first render panel (#1054).
//
// The map and its color bar come first, under a header of fixed height; the
// status lines, the readout and the plots come after the map; and the controls
// sit in a drawer on the right, a sheet under everything else when the panel
// is narrow. The drawer's sections are Data, Color, Projection, Overlays,
// Compare and Animate. The page saves whether the drawer is open, which
// sections were closed and which was used last, and opens the drawer on that
// one again.
//
// Before this, the canvas came after about ten toolbar rows and the line plot
// opened above it, so the map moved under the pointer when a plot opened or a
// control changed (20 to 46 px in the manual pass of 2026-11-04).
//
// Driven through the real panel: the controls are set as a user sets them, and
// the boxes are the page's own layout.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { DRAWER_SECTIONS } from "../../render-panel";
import {
  extensionPath,
  isType,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
} from "./real-panel";

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

type Opened = { panel: RealPanel; close: () => void };

/** A slice panel on `netcdf4_dimscale.nc`'s `temperature`, time × lat × lon. */
async function slicePanel(): Promise<Opened> {
  const p = await provider();
  const file = fixture("netcdf4_dimscale.nc");
  const native = loadNative();
  assert.ok(native, "native binding required");
  const temperature = native.open(fs.readFileSync(file)).variables().find((v) => v.name === "temperature");
  assert.ok(temperature, "the fixture has temperature");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

/** A GRIB panel on the first message of `file`. */
async function gribPanel(file: string): Promise<Opened> {
  const p = await provider();
  const editor = await openEditor(p, fixture(file));
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

/** The page once what it was sent has been handled and laid out. */
async function settled(panel: RealPanel): Promise<PageState> {
  await panel.pageTimer(60);
  return panel.drive([]);
}

/** Set `actions`, wait for the answer they ask for (`true` for a render, or
 *  the answer's type), and read the page. */
async function change(
  panel: RealPanel,
  actions: Parameters<RealPanel["drive"]>[0],
  answer: boolean | string,
): Promise<PageState> {
  const from = panel.sent.length;
  await panel.drive(actions);
  if (answer !== false) {
    const type = answer === true ? "gridReady" : answer;
    await panel.waitSent(isType(type), from, `the ${type} after ${JSON.stringify(actions)}`);
  }
  return settled(panel);
}

const near = (a: number, b: number) => Math.abs(a - b) < 0.5;

/** The map's top and left are where they were. */
function assertMapStays(state: PageState, before: PageState, what: string): void {
  assert.ok(
    near(state.image.top, before.image.top) && near(state.image.left, before.image.left),
    `${what}: the map moved from (${before.image.left}, ${before.image.top}) to (${state.image.left}, ${state.image.top})`,
  );
  assert.strictEqual(state.layout.scrollY, before.layout.scrollY, `${what}: the page did not scroll`);
}

const sectionIds = (state: PageState, which: (s: { open: boolean; shown: boolean }) => boolean) =>
  (state.layout.drawer?.sections ?? []).filter(which).map((s) => s.id);

/** The header's pieces, the only things on screen above the map. */
const HEADER_IDS = ["panel-header", "title-line", "projection-summary", "unavailable"];

suite("Map-first render panel (#1054)", () => {
  test("the map comes first: only the header is above it, and it is on screen without scrolling", async function () {
    this.timeout(120_000);
    for (const open of [slicePanel, () => gribPanel("cmc_wind_300_2010052400_p012.grib")]) {
      const { panel, close } = await open();
      try {
        const state = await settled(panel);
        const above = state.layout.aboveMap.filter((id) => !HEADER_IDS.includes(id));
        assert.deepStrictEqual(above, [], "nothing but the header comes before the map");
        assert.ok(state.layout.renderArea && state.layout.status, "the map and the status line are shown");
        assert.ok(state.layout.status.top >= state.layout.renderArea.bottom - 0.5, "the status line is under the map");
        assert.strictEqual(state.layout.scrollY, 0, "the page is at its top");
        assert.ok(state.image.top >= 0 && state.image.bottom <= state.viewportHeight,
          `the whole map is in the window: ${JSON.stringify(state.image)} in ${state.viewportHeight}`);
        // Wide enough here for the drawer beside the map.
        const drawer = state.layout.drawer;
        assert.ok(drawer, "the page has a drawer");
        assert.ok(drawer.box.left >= state.layout.renderArea.right - 0.5, "the drawer is right of the map");
        assert.strictEqual(drawer.expanded, "true", "the drawer starts open");
        assert.ok(drawer.bodyShown, "with the controls on screen");
      } finally {
        close();
      }
    }
  });

  test("the drawer's sections are Data, Color, Projection, Overlays, Compare and Animate, and an empty one is not drawn", async function () {
    this.timeout(120_000);
    assert.deepStrictEqual(DRAWER_SECTIONS.map((s) => s.label), ["Data", "Color", "Projection", "Overlays", "Compare", "Animate"]);
    const slice = await slicePanel();
    try {
      const state = await settled(slice.panel);
      assert.deepStrictEqual(
        state.layout.drawer?.sections.map((s) => [s.id, s.label]),
        DRAWER_SECTIONS.map((s) => [s.id, s.label]),
        "a slice panel has every section, in order",
      );
      assert.deepStrictEqual(sectionIds(state, (s) => s.shown && s.open), DRAWER_SECTIONS.map((s) => s.id), "all shown, open");
    } finally {
      slice.close();
    }
    // A GRIB panel has no variable, axes or steppers, and no Animate row; the
    // Data section holds only the message picker of a changed file, hidden.
    const grib = await gribPanel("regular_latlon_surface.grib2");
    try {
      const state = await settled(grib.panel);
      const shown = sectionIds(state, (s) => s.shown);
      assert.ok(!shown.includes("data"), `no Data section: ${shown}`);
      assert.ok(!shown.includes("animate"), `no Animate section: ${shown}`);
      assert.ok(shown.includes("color") && shown.includes("projection") && shown.includes("overlays"), `${shown}`);
      assert.strictEqual(shown.includes("compare"), grib.panel.html().includes('id="compare-fieldset"'),
        "Compare shows when the file has a second message to compare");
    } finally {
      grib.close();
    }
  });

  test("no control change, readout or plot moves the map", async function () {
    this.timeout(180_000);
    const { panel, close } = await slicePanel();
    try {
      const first = await settled(panel);
      const steps: Array<[string, Parameters<RealPanel["drive"]>[0], boolean | string]> = [
        ["Color Range Manual", [{ selector: 'input[name="range-mode"][value="manual"]', checked: true }], false],
        ["Color Range Auto", [{ selector: 'input[name="range-mode"][value="auto"]', checked: true }], true],
        ["a colormap", [{ id: "picker-colormap", value: "plasma" }], true],
        ["Mollweide", [{ id: "picker-projection", value: "mollweide" }], true],
        ["Coastlines and Graticule", [{ id: "overlay-coastlines", checked: true }, { id: "overlay-graticule", checked: true }], false],
        ["Contours", [{ id: "overlay-contours", checked: true }], "contourReady"],
        ["Compare", [{ id: "compare-op", value: "a_minus_b" }], true],
        ["Compare off", [{ id: "compare-op", value: "" }], true],
        ["Bounds Manual", [{ selector: 'input[name="bounds-mode"][value="manual"]', checked: true }], false],
        ["the source view", [{ id: "picker-projection", value: "source" }], true],
      ];
      let before = first;
      for (const [what, actions, renders] of steps) {
        const state = await change(panel, actions, renders);
        assertMapStays(state, first, what);
        before = state;
      }
      // A step along time, with the stepper's own number field.
      assert.ok((before.steppers?.main.length ?? 0) > 0, "the panel has a stepper");
      const stepped = await change(panel, [{ selector: '#slice-dims input[type="number"]', value: 1 }], true);
      assertMapStays(stepped, first, "a step along time");
      // A readout, the line plot through it, and the zonal average.
      await panel.drive([{ id: "line-toggle", checked: true }]);
      const from = panel.sent.length;
      await panel.probe();
      await panel.waitSent(isType("probeResult"), from, "the readout");
      await panel.waitSent(isType("lineResult"), from, "the line");
      const plotted = await settled(panel);
      assert.ok(plotted.layout.linePanel, "the line plot is open");
      assert.ok(plotted.layout.renderArea && plotted.layout.linePanel.top >= plotted.layout.renderArea.bottom - 0.5,
        "the line plot opens under the map");
      assert.ok(plotted.probe.trim() !== "", "the readout shows");
      assertMapStays(plotted, first, "a readout and its line plot");
      const zonal = await change(panel, [{ id: "zonal-toggle", checked: true }], false);
      assertMapStays(zonal, first, "the zonal average");
      // The drawer's own controls move nothing up or down either.
      const shut = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      assertMapStays(shut, first, "the drawer closed");
      const reopened = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      assertMapStays(reopened, first, "the drawer opened");
      const folded = await change(panel, [{ selector: "#section-color > summary", click: true }], false);
      assertMapStays(folded, first, "a section closed");
    } finally {
      close();
    }
  });

  test("the drawer toggles, gives the map its room, and is kept as it was left", async function () {
    this.timeout(120_000);
    const { panel, close } = await slicePanel();
    try {
      const open = await settled(panel);
      const shut = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      const drawer = shut.layout.drawer;
      assert.ok(drawer, "the page has a drawer");
      assert.strictEqual(drawer.expanded, "false", "the toggle says the drawer is closed");
      assert.strictEqual(drawer.toggleTitle, "Show the controls");
      assert.ok(!drawer.bodyShown, "the controls are put away");
      assert.ok(shut.layout.renderArea && open.layout.renderArea, "the map is on screen");
      assert.ok(shut.layout.renderArea.right > open.layout.renderArea.right + 50, "the map takes the drawer's room");
      assert.deepStrictEqual(shut.layout.saved, { open: false, closed: [], last: null }, "saved closed");
      // Shown again from its HTML, the page keeps the drawer closed.
      await panel.hideShow();
      await panel.waitSent(isType("gridReady"), 0, "the render after the show");
      const back = await settled(panel);
      assert.strictEqual(back.layout.drawer?.expanded, "false", "still closed after a hide and show");
      assert.ok(!back.layout.drawer?.bodyShown, "with the controls put away");
      const reopened = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      assert.strictEqual(reopened.layout.drawer?.expanded, "true");
      assert.strictEqual(reopened.layout.drawer?.toggleTitle, "Hide the controls");
      assert.ok(reopened.layout.drawer?.bodyShown, "the controls are back");
      assert.strictEqual(reopened.layout.saved?.open, true, "saved open");
    } finally {
      close();
    }
  });

  test("sections are kept as closed, and the drawer reopens on the section last used", async function () {
    this.timeout(120_000);
    const { panel, close } = await slicePanel();
    try {
      // Overlays closed; a colormap picked makes Color the section last used.
      await change(panel, [{ selector: "#section-overlays > summary", click: true }], false);
      const used = await change(panel, [{ id: "picker-colormap", value: "plasma" }], true);
      assert.deepStrictEqual(used.layout.saved, { open: true, closed: ["overlays"], last: "color" });
      // The selections and the drawer are saved together.
      assert.ok(used.layout.savedKeys.includes("colormap") && used.layout.savedKeys.includes("drawer"), `${used.layout.savedKeys}`);
      await panel.hideShow();
      const back = await settled(panel);
      assert.deepStrictEqual(sectionIds(back, (s) => s.shown && !s.open), ["overlays"], "Overlays is still closed");
      assert.deepStrictEqual(back.layout.saved, { open: true, closed: ["overlays"], last: "color" });
      assert.strictEqual((await panel.controls())["picker-colormap"], "plasma", "and the colormap is kept");
      // Color closed too, then the drawer closed and opened: it opens on
      // Color, the section last used, and leaves Overlays closed.
      const folded = await change(panel, [{ selector: "#section-color > summary", click: true }], false);
      assert.deepStrictEqual(sectionIds(folded, (s) => s.shown && !s.open).sort(), ["color", "overlays"]);
      await change(panel, [{ id: "drawer-toggle", click: true }], false);
      const reopened = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      assert.deepStrictEqual(sectionIds(reopened, (s) => s.shown && !s.open), ["overlays"], "Color is open again");
      assert.deepStrictEqual(reopened.layout.saved, { open: true, closed: ["overlays"], last: "color" });
      // Opening a section makes it the one last used, and a page written
      // again (an imported colormap rewrites every panel) keeps all of it.
      await change(panel, [{ selector: "#section-overlays > summary", click: true }], false);
      const from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridReady"), from, "the render after the rebuild");
      const rebuilt = await settled(panel);
      assert.deepStrictEqual(rebuilt.layout.saved, { open: true, closed: [], last: "overlays" });
      assert.deepStrictEqual(sectionIds(rebuilt, (s) => s.shown && !s.open), [], "every section open");
    } finally {
      close();
    }
  });

  test("a narrow panel puts the drawer under the map, as a sheet at the bottom", async function () {
    this.timeout(120_000);
    const { panel, close } = await slicePanel();
    try {
      const wide = await settled(panel);
      const narrow = await change(panel, [{ id: "body", width: "440px" }], false);
      const drawer = narrow.layout.drawer;
      assert.ok(drawer && narrow.layout.renderArea, "the page has a drawer and a map");
      assert.ok(drawer.box.top >= narrow.layout.renderArea.bottom - 0.5, "the drawer is under the map");
      assert.ok(drawer.box.right - drawer.box.left > 400, "across the panel's width");
      assert.ok(near(narrow.image.top, wide.image.top), "the map stays at the top");
      assert.deepStrictEqual(narrow.toolbar.outside, [], "no control runs past the sheet's edge");
      // Closed, the sheet is its button.
      const shut = await change(panel, [{ id: "drawer-toggle", click: true }], false);
      assert.ok(shut.layout.drawer && !shut.layout.drawer.bodyShown, "the controls are put away");
      assertMapStays(shut, narrow, "the sheet closed");
      await change(panel, [{ id: "drawer-toggle", click: true }, { id: "body", width: "" }], false);
    } finally {
      close();
    }
  });
});
