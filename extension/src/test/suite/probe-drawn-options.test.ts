// A click reads the image on screen, not the pickers' state (#1339).
//
// Between a change of the pickers and the render it asks for, or after that
// render was refused, the old image is still shown. The probe sends the pixel
// under the click and the options to map it through, so those options must be
// the ones the image was drawn with. #1114 moved the map geometry onto the
// drawn image's; this pins every option that places a pixel, through the real
// panel, with the render held back or refused.

import * as assert from "assert";
import * as path from "path";

import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel, waitUntil } from "./real-panel";

/** The options that put an image's pixels on the field. */
const GEOMETRY = [
  "projection", "projectionPreset", "centerLat", "centerLon", "flipY",
  "boundsLatMin", "boundsLatMax", "boundsLonMin", "boundsLonMax",
] as const;

function lastDrawn(panel: RealPanel): Msg {
  const drew = panel.sent.filter(isType("gridReady"));
  assert.ok(drew.length > 0, "a render was drawn");
  return drew[drew.length - 1];
}

/** Click the point `fy` down the image and return the request and the cell it read. */
async function click(panel: RealPanel, what: string): Promise<{ asked: Msg; read: Msg }> {
  const received = panel.received.length;
  const sent = panel.sent.length;
  await panel.probe(0.3, 0.4);
  const asked = await waitUntil(() => panel.received.slice(received).find(isType("probeRequest")), `the click (${what})`);
  const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
  return { asked, read };
}

/** The click's geometry must be the image on screen's, key by key. */
function assertDrawnGeometry(asked: Msg, drawn: Msg, what: string): void {
  for (const key of GEOMETRY) {
    assert.deepStrictEqual(asked.options[key] ?? null, drawn.options[key] ?? null, `${what}: ${key} is the image's`);
  }
}

suite("A click reads the image drawn, not the pickers (#1339)", function () {
  this.timeout(120_000);

  async function onMap(projection: string, body: (panel: RealPanel) => Promise<void>): Promise<void> {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: projection }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === projection, from, "the map");
      await body(panel);
    } finally {
      panel.dispose();
      editor.close();
    }
  }

  const changes: Array<[string, string, Array<{ id: string; value: string | number }>]> = [
    ["a new projection", "equirectangular", [{ id: "picker-projection", value: "mollweide" }]],
    ["a new central meridian", "mollweide", [{ id: "picker-world-meridian", value: 90 }]],
    ["a new orthographic centre", "orthographic", [
      { id: "picker-center-lat", value: 40 },
      { id: "picker-center-lon", value: 100 },
    ]],
    ["a new polar pole and meridian", "polar_stereographic", [
      { id: "picker-preset-polar", value: "south" },
      { id: "picker-central-meridian", value: 45 },
    ]],
    ["typed bounds", "equirectangular", [
      { id: "bounds-lat-min", value: -20 },
      { id: "bounds-lat-max", value: 20 },
      { id: "bounds-lon-min", value: 10 },
      { id: "bounds-lon-max", value: 50 },
    ]],
  ];

  for (const [what, from, actions] of changes) {
    test(`${what}, with the render held back, is not what a click asks through`, async () => {
      await onMap(from, async (panel) => {
        if (what === "typed bounds") {
          const sent = panel.sent.length;
          await panel.drive([{ selector: 'input[name="bounds-mode"][value="manual"]', checked: true }]);
          await panel.waitSent(isType("gridReady"), sent, "the render on Manual");
        }
        const before = await click(panel, "before the change");
        assert.ok(before.read, "the click lands on the grid");
        const gate = panel.hold(isType("gridReady"));
        try {
          const received = panel.received.length;
          await panel.drive(actions);
          await waitUntil(
            () => panel.received.slice(received).filter(isType("rerenderRequest")).pop(),
            "the render the pickers ask for",
          );
          await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
          const drawn = lastDrawn(panel);
          const waiting = await click(panel, "while the render is on its way");
          assertDrawnGeometry(waiting.asked, drawn, what);
          assert.deepStrictEqual(
            [waiting.read?.gridI, waiting.read?.gridJ, waiting.read?.value],
            [before.read.gridI, before.read.gridJ, before.read.value],
            `${what}: and it reads the cell on screen`,
          );
        } finally {
          await gate.release();
        }
      });
    });
  }

  test("a refused Web Mercator band leaves clicks on the image still shown", async () => {
    await onMap("web_mercator", async (panel) => {
      const sent0 = panel.sent.length;
      await panel.drive([{ selector: 'input[name="bounds-mode"][value="manual"]', checked: true }]);
      await panel.waitSent(isType("gridReady"), sent0, "the render on Manual");
      const before = await click(panel, "before the band");
      assert.ok(before.read, "the click lands on the grid");
      const sent = panel.sent.length;
      await panel.drive([
        { id: "bounds-lat-min", value: 86 },
        { id: "bounds-lat-max", value: 89 },
        { id: "bounds-lon-min", value: -20 },
        { id: "bounds-lon-max", value: 20 },
      ]);
      const refusal = await panel.waitSent(isType("gridError"), sent, "the refusal of the band");
      assert.ok(refusal.error, "the band is refused in words");
      const drawn = lastDrawn(panel);
      assert.notStrictEqual(drawn.options.boundsLatMin, 86, "the band asked for was not drawn");
      const after = await click(panel, "after the refusal");
      assertDrawnGeometry(after.asked, drawn, "after the refusal");
      assert.deepStrictEqual(
        [after.read?.gridI, after.read?.gridJ, after.read?.value],
        [before.read.gridI, before.read.gridJ, before.read.value],
        "and it reads the cell on screen",
      );
    });
  });
});
