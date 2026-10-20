// The fields that show only while their toggle is on keep their room but are
// invisible while it is off (#1260). `spacing-hidden` was styled for the
// graticule spacing and the Bounds fields only, so the contour interval box
// showed with Contours unticked. Read through the real panel's computed style.

import * as assert from "assert";
import * as path from "path";

import { extensionPath, isType, openEditor, openReal, provider } from "./real-panel";

const FIXTURE = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");

suite("Toggle-dependent fields hide with their toggle (#1260)", () => {
  test("contour interval, graticule spacing and the Bounds fields follow their toggles", async () => {
    const p = await provider();
    const editor = await openEditor(p, FIXTURE());
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.deepStrictEqual(state.fieldVisibility, {
        "graticule-spacing-label": "hidden",
        "contour-interval-label": "hidden",
        "bounds-manual-fields": "hidden",
      });

      state = await panel.drive([{ id: "overlay-contours", checked: true }]);
      assert.strictEqual(state.fieldVisibility["contour-interval-label"], "visible", "Contours on");
      assert.strictEqual(state.fieldVisibility["graticule-spacing-label"], "hidden");
      state = await panel.drive([{ id: "overlay-contours", checked: false }]);
      assert.strictEqual(state.fieldVisibility["contour-interval-label"], "hidden", "Contours off again");

      state = await panel.drive([{ id: "overlay-graticule", checked: true }]);
      assert.strictEqual(state.fieldVisibility["graticule-spacing-label"], "visible", "Graticule on");
      state = await panel.drive([{ selector: 'input[name="bounds-mode"][value="manual"]', checked: true }]);
      assert.strictEqual(state.fieldVisibility["bounds-manual-fields"], "visible", "Bounds Manual");
      state = await panel.drive([{ selector: 'input[name="bounds-mode"][value="auto"]', checked: true }]);
      assert.strictEqual(state.fieldVisibility["bounds-manual-fields"], "hidden", "Bounds Auto");
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});
