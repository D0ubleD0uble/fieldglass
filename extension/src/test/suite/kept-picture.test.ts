// A picture left on screen by a refusal comes back with the page (#1427).
//
// A refused render keeps the last picture: a color range Rust refuses
// (#1155), a Web Mercator window past its latitudes (#1278), a Compare on
// another grid (#1174). The panel keeps no context while hidden, so a tab
// switch, a maximize and restore of the editor group, or an imported color
// table writing the page again loads it afresh, and the controls it restores
// ask for what was refused. The page came back with the note but no picture,
// no color bar and no caption. It now asks for the picture first, from the
// request that drew it, then for the controls' own render, which is refused
// again over it as before.
//
// The maximize is not driven here: in the test host a panel beside a
// maximized group stays visible and is not loaded again (measured on VS Code
// 1.143.1), so it is a manual check (RELEASE-TEST-PLAN §45 at v0.6.0). It
// reaches the same load as the tab switch.
//
// The picture so kept is marked as the last one drawn, faded with a note over
// it, so it is not read as the one the controls now ask for (#1509): a Compare
// A - B whose Field B is on another grid left Field A alone on screen under
// controls that said A - B. The mark goes when a render draws.
//
// Driven through the real panel, whose own script runs in a real webview.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { KEPT_PICTURE_NOTE } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const FIXTURES = path.join(extensionPath(), "src", "test", "fixtures");
const GRIB = path.join(FIXTURES, "regular_latlon_surface.grib2");
const NETCDF = path.join(FIXTURES, "ersst_v5_187001_cdf1.nc");

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };
const INVERTED = "minimum 300 is not below maximum 290; set a minimum below the maximum";

/** What the page shows of the picture: whether it is drawn, its color bar,
 *  and the caption naming its projection. */
function picture(state: PageState) {
  return {
    painted: state.canvasPainted,
    colorbar: state.colorbarShown,
    labels: state.colorbarLabels,
    caption: state.caption,
  };
}

/** Drive `actions`, then wait for the answer to the last render they asked
 *  for. Each control's change asks for one of its own. */
async function answerTo(panel: RealPanel, actions: object[], what: string): Promise<Msg> {
  const asked = panel.received.length;
  const from = panel.sent.length;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  await panel.drive(actions as any);
  const last = await waitUntil(
    () => panel.received.slice(asked).filter(isType("rerenderRequest")).pop(),
    `the render ${what} asks for`,
  );
  return panel.waitSent(
    (m: Msg) => (m.type === "gridReady" || m.type === "gridError") && m.seq === last.seq,
    from,
    `the answer to ${what}`,
  );
}

/** Load the page again by `how`, and wait for what it asks once loaded: the
 *  kept picture in its ready, then the controls' render, and that render's
 *  answer. */
async function reload(panel: RealPanel, how: () => Promise<void> | void, what: string): Promise<Msg> {
  const asked = panel.received.length;
  const from = panel.sent.length;
  await how();
  const ready = await waitUntil(() => panel.received.slice(asked).find(isType("ready")), `the page loaded again by ${what}`);
  const first = await panel.waitSent(
    (m: Msg) => (m.type === "gridReady" || m.type === "gridError") && m.seq === ready.seq,
    from,
    `the answer to the ready after ${what}`,
  );
  // A ready that asked for the controls is the last word.
  if (first.type === "gridError") return first;
  const follow = await waitUntil(
    () => panel.received.slice(asked).find((m: Msg) => m.type === "rerenderRequest" && m.seq > ready.seq),
    `the controls' render after ${what}`,
  );
  return panel.waitSent(
    (m: Msg) => (m.type === "gridReady" || m.type === "gridError") && m.seq === follow.seq,
    from,
    `the answer to the controls' render after ${what}`,
  );
}

suite("A picture kept under a refusal comes back with the page (#1427)", function () {
  this.timeout(180_000);

  test("a refused color range keeps its picture through a tab switch and a rebuild", async () => {
    const p = await provider();
    const editor = await openEditor(p, GRIB);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const map = await answerTo(panel, [{ id: "picker-projection", value: "equirectangular" }], "Equirectangular");
      assert.strictEqual(map.type, "gridReady");
      const refused = await answerTo(
        panel,
        [MANUAL, { id: "range-min", value: 300 }, { id: "range-max", value: 290 }],
        "the inverted range",
      );
      assert.strictEqual(refused.rangeRefusal?.end, "both", "refused naming both ends");
      const before = await panel.drive([]);
      assert.strictEqual(before.canvasPainted, true, "the last picture stays");
      assert.ok(before.caption.includes("equirectangular"), before.caption);
      assert.strictEqual(before.rangeNote, INVERTED);

      // A panel shown again with nothing refused asks for one render only.
      const triggers: Array<[string, () => Promise<void> | void]> = [
        ["a tab switch", () => panel.hideShow()],
        ["a rebuild", () => panel.rebuild()],
        ["a second tab switch", () => panel.hideShow()],
      ];
      for (const [what, how] of triggers) {
        const answer = await reload(panel, how, what);
        assert.strictEqual(answer.type, "gridError", `${what}: the controls' range is still refused`);
        assert.strictEqual(answer.rangeRefusal?.end, "both", what);
        const state = await panel.drive([]);
        assert.deepStrictEqual(picture(state), picture(before), `${what}: the picture, color bar and caption are back`);
        assert.strictEqual(state.rangeNote, INVERTED, `${what}: the note is under the range fields`);
        assert.deepStrictEqual(state.rangeInvalid, ["range-min", "range-max"], what);
        assert.deepStrictEqual(state.rangeFields, ["300", "290"], `${what}: the fields hold what was typed`);
        assert.strictEqual(state.status, "Color range not drawn: see the range fields", what);
      }

      // Fixed, the range draws, and a page loaded again then asks for one
      // render: there is no picture kept apart from the controls.
      const fixed = await answerTo(panel, [{ id: "range-min", value: 280 }], "the fixed range");
      assert.strictEqual(fixed.type, "gridReady");
      assert.deepStrictEqual([fixed.usedMin, fixed.usedMax], [280, 290]);
      // The page has taken that answer before it is hidden. answerTo waits
      // for the provider to send it, not for the page to read it, and a page
      // hidden with the render still on its way keeps the picture before it,
      // by design (#1427): its ready then asks for that one, from 300 up.
      await panel.drive([]);
      const asked = panel.received.length;
      const from = panel.sent.length;
      await panel.hideShow();
      const ready = await waitUntil(() => panel.received.slice(asked).find(isType("ready")), "the ready");
      assert.deepStrictEqual([ready.rangeMin, ready.rangeMax], [280, 290], "the ready asks for the controls");
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.seq === ready.seq, from, "the redraw");
      await panel.pageTimer(200);
      assert.ok(
        !panel.received.slice(asked).some(isType("rerenderRequest")),
        "nothing more is asked for than the ready",
      );
      assert.strictEqual((await panel.drive([])).rangeNote, null);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a tab switched before the refusal arrives still brings the picture back", async () => {
    const p = await provider();
    const editor = await openEditor(p, GRIB);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await answerTo(panel, [{ id: "picker-projection", value: "equirectangular" }], "Equirectangular");
      await answerTo(panel, [MANUAL, { id: "range-min", value: 300 }], "Manual from 300");
      const before = await panel.drive([]);
      assert.strictEqual(before.canvasPainted, true);
      // The refusal is held back, as if still on its way, until the page is
      // hidden: the hidden page never hears it.
      let hiding = false;
      const gate = panel.hold((m: Msg) => !hiding && m.type === "gridError" && m.rangeRefusal !== undefined);
      let answer: Msg;
      try {
        const asked = panel.received.length;
        await panel.drive([{ id: "range-max", value: 290 }]);
        await waitUntil(() => panel.received.slice(asked).find(isType("rerenderRequest")), "the refused render asked");
        await waitUntil(() => gate.held.find(isType("gridError")), "its refusal, held back");
        // Dropped: it was for the page that goes. What the page loaded again
        // is sent passes.
        gate.held.splice(0);
        hiding = true;
        answer = await reload(panel, () => panel.hideShow(), "a tab switch");
      } finally {
        await gate.release();
      }
      assert.strictEqual(answer.rangeRefusal?.end, "both", "the controls' range is refused on the page loaded again");
      const state = await panel.drive([]);
      assert.deepStrictEqual(picture(state), picture(before), "the picture, color bar and caption are back");
      assert.strictEqual(state.rangeNote, INVERTED);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a refused Web Mercator window keeps its picture through a tab switch (#1278)", async () => {
    const p = await provider();
    const editor = await openEditor(p, GRIB);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await answerTo(panel, [{ id: "picker-projection", value: "web_mercator" }], "Web Mercator");
      await answerTo(panel, [{ selector: 'input[name="bounds-mode"][value="manual"]', checked: true }], "Manual bounds");
      const window = await answerTo(
        panel,
        [
          { id: "bounds-lat-max", value: 60 },
          { id: "bounds-lat-min", value: 20 },
          { id: "bounds-lon-min", value: -20 },
          { id: "bounds-lon-max", value: 20 },
        ],
        "a window inside the band",
      );
      assert.strictEqual(window.type, "gridReady", "the window draws");
      const refused = await answerTo(
        panel,
        [
          { id: "bounds-lat-max", value: 89 },
          { id: "bounds-lat-min", value: 86 },
        ],
        "a window past the band",
      );
      assert.strictEqual(refused.type, "gridError");
      assert.ok(/Web Mercator latitude band/.test(refused.error), refused.error);
      const before = await panel.drive([]);
      assert.strictEqual(before.canvasPainted, true, "the last picture stays");

      const answer = await reload(panel, () => panel.hideShow(), "a tab switch");
      assert.strictEqual(answer.type, "gridError", "the window is still refused");
      const state = await panel.drive([]);
      assert.deepStrictEqual(picture(state), picture(before), "the picture, color bar and caption are back");
      assert.ok(state.status.startsWith("Error: ") && state.status.includes("Web Mercator latitude band"), state.status);
      const controls = await panel.controls();
      assert.deepStrictEqual([controls["bounds-lat-min"], controls["bounds-lat-max"]], ["86", "89"], "as typed");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a Compare on another grid keeps its picture and the range note through a tab switch (#1174)", async () => {
    // The fixture beside a message on another grid, so a Compare of them
    // fails before the range is read.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1427-"));
    const file = path.join(dir, "two-grids.grib2");
    fs.writeFileSync(
      file,
      Buffer.concat([fs.readFileSync(GRIB), fs.readFileSync(path.join(FIXTURES, "eta_lambert_msg0.grib2"))]),
    );
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await answerTo(panel, [MANUAL, { id: "range-min", value: 300 }, { id: "range-max", value: 290 }], "the range");
      const err = await answerTo(
        panel,
        [
          { id: "compare-op", value: "a_minus_b" },
          { id: "compare-field-b", value: 1 },
        ],
        "Field B on another grid",
      );
      assert.strictEqual(err.type, "gridError");
      assert.strictEqual(err.rangeRefusal, undefined, "an error about the grids, not the range");
      const before = await panel.drive([]);
      assert.strictEqual(before.canvasPainted, true, "the last picture stays");
      assert.strictEqual(before.rangeNote, INVERTED, "and the note by the fields stays up");

      const answer = await reload(panel, () => panel.hideShow(), "a tab switch");
      assert.strictEqual(answer.type, "gridError", "the Compare still fails");
      assert.strictEqual(answer.rangeRefusal, undefined);
      const state = await panel.drive([]);
      assert.deepStrictEqual(picture(state), picture(before), "the picture, color bar and caption are back");
      assert.strictEqual(state.rangeNote, INVERTED, "the note by the fields is back");
      assert.deepStrictEqual(state.rangeInvalid, ["range-min", "range-max"]);
      assert.strictEqual(state.status, before.status, "the status line says the grid error again");
      assert.strictEqual(state.compareFieldB, "1", "Field B is still the other grid's message");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF slice's refused range keeps its picture through a tab switch", async () => {
    const p = await provider();
    const editor = await openEditor(p, NETCDF);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      const refused = await answerTo(
        panel,
        [MANUAL, { id: "range-min", value: 300 }, { id: "range-max", value: 290 }],
        "the inverted range",
      );
      assert.strictEqual(refused.rangeRefusal?.end, "both");
      const before = await panel.drive([]);
      assert.strictEqual(before.canvasPainted, true, "the last picture stays");

      const answer = await reload(panel, () => panel.hideShow(), "a tab switch");
      assert.strictEqual(answer.rangeRefusal?.end, "both", "still refused");
      const ready = panel.received.filter(isType("ready")).pop();
      assert.deepStrictEqual(ready.slice, first.slice, "the ready asks for the slice drawn");
      const state = await panel.drive([]);
      assert.deepStrictEqual(picture(state), picture(before), "the picture, color bar and caption are back");
      assert.strictEqual(state.subtitle, before.subtitle, "the line under the heading names the slice drawn");
      assert.strictEqual(state.rangeNote, INVERTED);
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});

/** Whether the page marks its picture as the last one drawn (#1509): the note
 *  over it, and the picture faded. */
function marked(state: PageState): { note: string | null; faded: boolean } {
  return { note: state.keptNote, faded: Number(state.canvasOpacity) < 1 };
}
const MARKED = { note: KEPT_PICTURE_NOTE, faded: true };
const UNMARKED = { note: null, faded: false };

suite("A picture left by a failed render is marked as the last one drawn (#1509)", function () {
  this.timeout(180_000);

  test("a Compare on another grid marks the picture of Field A, and a drawn render clears the mark (#1509)", async () => {
    // The fixture beside a message on another grid, as in
    // samples/0.6.0/two_grids.grib2.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1509-"));
    const file = path.join(dir, "two-grids.grib2");
    fs.writeFileSync(
      file,
      Buffer.concat([fs.readFileSync(GRIB), fs.readFileSync(path.join(FIXTURES, "eta_lambert_msg0.grib2"))]),
    );
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const drawn = await panel.drive([]);
      assert.deepStrictEqual(marked(drawn), UNMARKED, "a picture the controls ask for is not marked");

      const err = await answerTo(
        panel,
        [
          { id: "compare-op", value: "a_minus_b" },
          { id: "compare-field-b", value: 1 },
        ],
        "A - B with Field B on another grid",
      );
      assert.strictEqual(err.type, "gridError");
      assert.ok(/different grids/.test(err.error), err.error);
      let state = await panel.drive([]);
      assert.strictEqual(state.canvasPainted, true, "Field A's picture stays (#1174, #1427)");
      assert.deepStrictEqual(marked(state), MARKED, "and is marked as the last picture drawn");
      assert.ok(state.status.startsWith("Error: "), state.status);

      // A page loaded again brings the picture back (#1427), marked.
      const again = await reload(panel, () => panel.hideShow(), "a tab switch");
      assert.strictEqual(again.type, "gridError", "the Compare still fails");
      state = await panel.drive([]);
      assert.strictEqual(state.canvasPainted, true);
      assert.deepStrictEqual(marked(state), MARKED, "still marked after a tab switch");

      // Compare off: the controls ask for Field A again, which draws.
      const off = await answerTo(panel, [{ id: "compare-op", value: "" }], "Compare off");
      assert.strictEqual(off.type, "gridReady", `${off.error ?? ""}`);
      state = await panel.drive([]);
      assert.deepStrictEqual(marked(state), UNMARKED, "a drawn render clears the mark");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a refused color range marks the picture, on a GRIB panel and a slice panel (#1509)", async () => {
    const p = await provider();
    const cases: Array<[string, string, object]> = [
      ["GRIB", GRIB, { type: "decodeGrid", messageIndex: 0 }],
      ["slice", NETCDF, { type: "renderVariable", variableIndex: 0 }],
    ];
    for (const [what, file, open] of cases) {
      const editor = await openEditor(p, file);
      const panel = await openReal(p, () => editor.send(open));
      try {
        await panel.waitSent(isType("gridReady"), 0, `${what}: the first render`);
        const refused = await answerTo(
          panel,
          [MANUAL, { id: "range-min", value: 300 }, { id: "range-max", value: 290 }],
          `${what}: the inverted range`,
        );
        assert.strictEqual(refused.rangeRefusal?.end, "both", what);
        let state = await panel.drive([]);
        assert.strictEqual(state.canvasPainted, true, `${what}: the last picture stays`);
        assert.deepStrictEqual(marked(state), MARKED, `${what}: and is marked`);

        const fixed = await answerTo(panel, [{ id: "range-min", value: 200 }], `${what}: the fixed range`);
        assert.strictEqual(fixed.type, "gridReady", `${what}: ${fixed.error ?? ""}`);
        state = await panel.drive([]);
        assert.deepStrictEqual(marked(state), UNMARKED, `${what}: the drawn range clears the mark`);
      } finally {
        panel.dispose();
        editor.close();
      }
    }
  });
});
