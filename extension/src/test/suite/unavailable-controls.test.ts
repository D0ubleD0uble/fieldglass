// The render panel hides each control the grid can't use, and says which and
// why in one "Not available for this grid" line under the grid caption
// (#1036).
//
// The panel used to grey such a control out with a note beside it, which read
// as broken. A hidden control has to be hidden on screen, not only marked
// `hidden`: an author `display` rule beats the attribute (#1035), so these
// read each control's computed display. The line is a native `<details>`,
// collapsed, naming the controls by their labels and giving one reason per
// group, structural reasons before not-yet ones. It is absent when nothing is
// hidden and follows the slice a panel moves to.
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import {
  NO_BILINEAR_REASON,
  NO_ZONAL_REASON,
  projectionReason,
  SATELLITE_NOT_YET_REASON,
  unavailableControls,
  UNPLACED_REASON,
} from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  overlayTraffic,
  PAGE_CONTROLS,
  type PageControl,
  type PageState,
  provider,
  type RealPanel,
  refusals,
} from "./real-panel";

function crateFixture(crate: string, name: string): string {
  return path.join(extensionPath(), "..", "crates", crate, "tests", "fixtures", name);
}

/** A NetCDF variable's index in `file`, by name. */
function variableIndex(file: string, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(file))
    .variables()
    .find((x) => x.name === name);
  assert.ok(v, `${path.basename(file)} has ${name}`);
  return v.index;
}

/** The committed §3.20 polar stereographic fixture on an Earth of 80 km with
 *  100 km cells, written twice so the panel has a Vectors row: the same file
 *  as RELEASE-TEST-PLAN §39's `samples/0.6.0/tiny_earth_pair.grib2` (#1002). */
function tinyEarthPair(): Buffer {
  const bytes = Buffer.from(fs.readFileSync(crateFixture("fieldglass-grib2", "polar_stereographic_surface.grib2")));
  let at = 16; // past §0
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    const len = bytes.readUInt32BE(at);
    if (bytes[at + 4] === 3) {
      assert.strictEqual(bytes.readUInt16BE(at + 12), 20, "a §3.20 grid");
      bytes[at + 14] = 1; // octet 15: shape of the Earth, radius given
      bytes[at + 15] = 0; // octet 16: scale factor of the radius
      bytes.writeUInt32BE(80_000, at + 16); // octets 17-20: R = 80 km
      bytes.writeUInt32BE(100_000_000, at + 55); // octets 56-59: Dx, 10^-3 m
      bytes.writeUInt32BE(100_000_000, at + 59); // octets 60-63: Dy
      return Buffer.concat([bytes, bytes]);
    }
    at += len;
  }
  throw new Error("no §3 in the fixture");
}

/** Write `bytes` to a fresh temporary file named `name`; returns its path and
 *  a cleanup. */
function tempFile(name: string, bytes: Buffer): { file: string; done: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1036-"));
  const file = path.join(dir, name);
  fs.writeFileSync(file, bytes);
  return { file, done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

/** The page hides exactly `hidden` of the controls it has, and shows the rest.
 *  `absent` are the controls this panel never has. */
function assertHides(
  state: PageState,
  hidden: PageControl[],
  absent: PageControl[] = [],
  covered: PageControl[] = [],
): void {
  for (const key of PAGE_CONTROLS) {
    if (covered.includes(key)) {
      assert.strictEqual(state.controlShown[key], false, `${key} goes with the row around it`);
    } else if (absent.includes(key)) {
      assert.strictEqual(state.controlDisplay[key], null, `${key}: this panel has none`);
    } else if (hidden.includes(key)) {
      assert.strictEqual(state.controlDisplay[key], "none", `${key} is not displayed`);
      assert.strictEqual(state.controlShown[key], false, `${key} takes no room`);
    } else {
      assert.strictEqual(state.controlShown[key], true, `${key} is on screen`);
    }
  }
  assert.deepStrictEqual(state.shownWhileHidden, [], "nothing marked hidden is shown");
}

/** The line names `labels` in its summary and gives `items` when opened. */
function assertLine(state: PageState, labels: string[], items: Array<{ kind: string; text: string }>): void {
  const line = state.unavailable;
  assert.ok(line, "the page has the line");
  assert.notStrictEqual(line.display, "none", "the line is shown");
  assert.strictEqual(line.after, "div#projection-summary", "directly under the grid caption");
  assert.strictEqual(line.summary, `Not available for this grid: ${labels.join(", ")}`);
  assert.deepStrictEqual(line.items, items);
  assert.strictEqual(line.live, false, "static, not a live region");
}

function assertNoLine(state: PageState): void {
  assert.ok(state.unavailable, "the page has the line");
  assert.strictEqual(state.unavailable.display, "none", "the line is absent");
}

/** Check every box, hidden controls' included, the way a restored snapshot can
 *  leave them (#1027), and force a re-render, which asks again for whatever
 *  applies. The caller then reads what was asked for. */
async function checkEveryBox(panel: RealPanel): Promise<PageState> {
  const from = panel.sent.length;
  const boxes = ["overlay-coastlines", "overlay-graticule", "overlay-contours", "contours-only", "zonal-toggle"];
  if (panel.html().includes('id="overlay-vectors"')) boxes.push("overlay-vectors");
  await panel.drive(boxes.map((id) => ({ id, checked: true })));
  await panel.drive([{ id: "flip-y", checked: true }]);
  await panel.waitSent(isType("gridReady"), from, "a re-render");
  return panel.drive([]);
}

/** What the line says on a satellite swath (#1036, #1225): it has no rows of
 *  latitude, and it is a list of cell centres, which the warp draws nearest. */
const SWATH_LINE = ["Resampling", "Zonal average"];
const SWATH_ITEMS = [
  { kind: "structural", text: `Resampling: ${NO_BILINEAR_REASON}` },
  { kind: "structural", text: `Zonal average: ${NO_ZONAL_REASON}` },
];

suite("Controls a grid can't use are hidden, and the panel says why (#1036)", function () {
  this.timeout(120_000);

  test("a satellite swath hides the zonal average and Resampling, and says why", async () => {
    const p = await provider();
    // The committed equivalent of samples/mirs_swath.nc: a MiRS swath placed
    // by its 2-D coordinates.
    const file = crateFixture("fieldglass-netcdf", "mirs_swath_n21.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "TPW") }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(first.sliceGrid.placed, true, "a swath is placed");
      assert.strictEqual(first.sliceGrid.supports.zonalMean, false, "and has no rows of latitude");
      assert.strictEqual(first.sliceGrid.supports.bilinear, false, "and is drawn nearest whatever is asked (#1225)");

      let state = await panel.drive([]);
      assertHides(state, ["zonal", "resampling"], ["arrows"]);
      assertLine(state, SWATH_LINE, SWATH_ITEMS);
      assert.strictEqual(state.unavailable?.open, false, "collapsed");
      assert.strictEqual(state.unavailable?.summaryFocusable, true, "the summary is in the tab order");

      state = await panel.drive([{ id: "unavailable", open: true }]);
      assert.strictEqual(state.unavailable?.open, true, "a click on the summary opens it");

      // Onto another variable of the same swath: the slice changes, the line
      // and its open state stay.
      const from = panel.sent.length;
      const rr = variableIndex(file, "RR");
      await panel.drive([{ id: "slice-variable", value: rr }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === rr, from, "RR drawn");
      state = await panel.drive([]);
      assertLine(state, SWATH_LINE, SWATH_ITEMS);
      assert.strictEqual(state.unavailable?.open, true, "still open after a slice step");

      // A checked but hidden Zonal average asks for nothing, through a
      // re-render, which also keeps the line open: only its contents are
      // rewritten.
      const asked = panel.received.length;
      state = await checkEveryBox(panel);
      assert.strictEqual(state.unavailable?.open, true, "still open");
      assert.deepStrictEqual(
        panel.received.slice(asked).filter((m: Msg) => m && m.type === "zonalRequest").map((m: Msg) => m.type),
        [],
        "a checked but hidden Zonal average asks for nothing",
      );
    } finally {
      panel.dispose();
    }
  });

  test("a GRIB grid nothing places hides the projection, overlays, arrows, zonal average and long CSV, for one reason", async () => {
    const p = await provider();
    const { file, done } = tempFile("tiny_earth_pair.grib2", tinyEarthPair());
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.placement, "unplaceable", "the grid is not placed");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assertHides(state, ["projection", "resampling", "overlay", "arrows", "zonal"], [], ["contours"]);
      assertLine(
        state,
        // A GRIB panel has Export CSV too (#1039), so its long layout is listed.
        ["Projection", "Resampling", "Overlay", "Vectors", "Zonal average", "Long layout (Export CSV)"],
        [
          {
            kind: "structural",
            text: `Projection, Resampling, Overlay, Vectors, Zonal average, Long layout (Export CSV): ${UNPLACED_REASON}`,
          },
        ],
      );
      assert.doesNotMatch(panel.html(), /satellite-view/, "nothing blames a satellite view");

      const checked = await checkEveryBox(panel);
      assert.strictEqual(checked.canvasVisibility, "visible", "'only' hides no raster when there are no contours");
      assert.deepStrictEqual(
        panel.received.filter((m: Msg) => overlayTraffic(m) || (m && m.type === "zonalRequest")).map((m: Msg) => m.type),
        [],
        "the hidden boxes ask for nothing",
      );
      assert.deepStrictEqual(panel.sent.filter(refusals).map((m: Msg) => `${m.type}: ${m.error}`), [], "and nothing is refused");
    } finally {
      panel.dispose();
      done();
    }
  });

  test("a satellite view keeps structural and not-yet reasons apart", async () => {
    const p = await provider();
    const file = crateFixture("fieldglass-netcdf", "goes_geostationary.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "Rad") }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      // Placed: the projection and the coastlines stay; contours go from
      // inside the Overlay row.
      assertHides(state, ["contours", "zonal", "resampling"], ["arrows"]);
      assertLine(
        state,
        ["Zonal average", "Contours", "Long layout (Export CSV)"],
        [
          { kind: "structural", text: `Zonal average: ${NO_ZONAL_REASON}` },
          { kind: "notYet", text: `Contours, Long layout (Export CSV): ${SATELLITE_NOT_YET_REASON}` },
        ],
      );
    } finally {
      panel.dispose();
    }
  });

  test("a regular lat/lon grid shows every control, and no line", async () => {
    const p = await provider();
    // Two messages, so the Vectors row is written as well.
    const bytes = fs.readFileSync(crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
    const { file, done } = tempFile("latlon_pair.grib2", Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Resampling waits for a warp (#1091).
      const state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assertHides(state, []);
      for (const key of PAGE_CONTROLS) assert.strictEqual(state.controlShown[key], true, `${key} is on screen`);
      assertNoLine(state);
    } finally {
      panel.dispose();
      done();
    }
  });

  test("the line follows the variable a slice panel moves to", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const temperature = variableIndex(file, "temperature");
    const latBnds = variableIndex(file, "lat_bnds");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assertHides(state, ["resampling"], ["arrows"]);
      assertNoLine(state);

      let from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from, "lat_bnds drawn");
      state = await panel.drive([]);
      assertHides(state, ["projection", "resampling", "overlay", "zonal"], ["arrows"], ["contours"]);
      assertLine(
        state,
        ["Projection", "Resampling", "Overlay", "Zonal average", "Long layout (Export CSV)"],
        [{ kind: "structural", text: `Projection, Resampling, Overlay, Zonal average, Long layout (Export CSV): ${UNPLACED_REASON}` }],
      );

      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === temperature, from, "temperature drawn");
      state = await panel.drive([]);
      assertHides(state, ["resampling"], ["arrows"]);
      assertNoLine(state);
    } finally {
      panel.dispose();
    }
  });

  test("focus in a control that goes moves to the line's summary", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const latBnds = variableIndex(file, "lat_bnds");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "temperature") }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([{ id: "zonal-toggle", focus: true }]);
      assert.strictEqual(state.focused, "input#zonal-toggle", "the toggle has the focus");
      const from = panel.sent.length;
      // Picking the variable is a change event, not a focus move, so the
      // focus is still on the toggle when lat_bnds' answer hides it.
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from, "lat_bnds drawn");
      state = await panel.drive([]);
      assert.strictEqual(state.controlDisplay.zonal, "none");
      assert.strictEqual(state.focused, "summary in #unavailable", "the focus is not lost with the toggle");
    } finally {
      panel.dispose();
    }
  });

  test("a cross-section hides the projection picker and Resampling rather than greying them out", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "temperature") }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // time down the rows: not a map.
      let from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from, "the cross-section");
      let state = await panel.drive([]);
      assertHides(state, ["projection", "resampling", "overlay", "zonal"], ["arrows"], ["contours"]);
      assert.strictEqual(state.projection, "source");
      assert.match(panel.html(), /id="picker-projection"/);
      assert.strictEqual(state.projectionDisabled, false, "hidden, not disabled");

      // Back to lat down the rows: the map's controls return.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-y", value: 1 }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.sliceGrid && m.sliceGrid.placed, from, "the map again");
      state = await panel.drive([]);
      assertHides(state, ["resampling"], ["arrows"]); // still Source, so nothing to resample
      state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assertHides(state, [], ["arrows"]);
      assertNoLine(state);
    } finally {
      panel.dispose();
    }
  });
});

suite("Resampling is hidden on a grid of cell centres, where Bilinear changes nothing (#1225)", function () {
  this.timeout(120_000);

  // A curvilinear grid is a list of cell centres, and the warp draws the
  // nearest one whatever Resampling says. Both committed kinds: a MiRS
  // satellite swath (like samples/mirs_swath.nc) and an RTOFS tripolar ocean
  // grid (like samples/rtofs_ice.nc).
  for (const [fixture, variable] of [
    ["mirs_swath_n21.nc", "TPW"],
    ["rtofs_tripolar_arctic.nc", "ice_thickness"],
  ] as const) {
    test(`${fixture}: on a map, no Resampling, and the line says why`, async () => {
      const p = await provider();
      const file = crateFixture("fieldglass-netcdf", fixture);
      const editor = await openEditor(p, file);
      const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, variable) }));
      try {
        const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
        assert.strictEqual(first.sliceGrid.reprojectable, true, "the grid is drawn on a map");
        assert.strictEqual(first.sliceGrid.supports.bilinear, false, "Rust says Bilinear changes nothing");

        for (const projection of ["equirectangular", "mollweide"]) {
          const from = panel.sent.length;
          await panel.drive([{ id: "picker-projection", value: projection }]);
          const drawn = await panel.waitSent(
            (m: Msg) => m.type === "gridReady" && m.options.projection === projection,
            from,
            projection,
          );
          assert.match(drawn.projectionSummary, /\(nearest\)$/, "the warp draws nearest");
          const state = await panel.drive([]);
          assert.strictEqual(state.projection, projection);
          assert.strictEqual(state.controlDisplay.resampling, "none", `${projection}: Resampling is not displayed`);
          assert.strictEqual(state.controlShown.resampling, false, `${projection}: and takes no room`);
          assert.deepStrictEqual(state.shownWhileHidden, []);
          const line = state.unavailable;
          assert.ok(line && line.display !== "none", "the line is shown");
          assert.match(line.summary, /^Not available for this grid: .*\bResampling\b/, "it names Resampling");
          assert.ok(
            line.items.some((i) => i.kind === "structural" && i.text === `Resampling: ${NO_BILINEAR_REASON}`),
            `with the reason: ${JSON.stringify(line.items)}`,
          );
        }
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  }
});

suite("Which controls a grid can't use, and why (#1036)", () => {
  const none = { contours: false, vectors: false, longCsv: false, zonalMean: false, bilinear: false };
  const all = { contours: true, vectors: true, longCsv: true, zonalMean: true, bilinear: true };

  test("a template this build does not model is 'not yet', apart from the grid's own reasons", () => {
    const groups = unavailableControls(
      { reprojectable: false, placement: "unsupported", label: "unsupported(3.4)", supports: none },
      { arrows: true, csv: true },
    );
    assert.deepStrictEqual(
      groups.map((g) => [g.kind, g.controls.map((c) => c.label).join(", "), g.reason]),
      [
        ["structural", "Overlay, Vectors, Zonal average, Long layout (Export CSV)", UNPLACED_REASON],
        ["notYet", "Projection, Resampling", "not yet supported for unsupported(3.4) grids."],
      ],
    );
    assert.deepStrictEqual(projectionReason(false, "unsupported", null), {
      kind: "notYet",
      reason: "not yet supported for this grid.",
    });
  });

  test("a placed grid that does not reproject is a limit of this build, and says so once", () => {
    // A lat/lon grid scanned east to west, say: placed, and everything else works.
    const groups = unavailableControls(
      { reprojectable: false, placement: "placed", label: "latlon", supports: all },
      { arrows: true, csv: true },
    );
    assert.deepStrictEqual(groups, [
      {
        kind: "notYet",
        reason: "not yet supported for this grid's layout.",
        controls: [
          { key: "projection", label: "Projection" },
          { key: "resampling", label: "Resampling" },
        ],
      },
    ]);
    assert.doesNotMatch(groups[0].reason, /latlon|not available/, "the reason does not restate the summary");
  });

  test("a grid the warp draws nearest lists Resampling once, for its own reason (#1225)", () => {
    const lookup = {
      reprojectable: true,
      placement: "placed" as const,
      label: "curvilinear",
      supports: { ...all, zonalMean: false, bilinear: false },
    };
    assert.deepStrictEqual(unavailableControls(lookup, { arrows: false, csv: true }), [
      { kind: "structural", reason: NO_BILINEAR_REASON, controls: [{ key: "resampling", label: "Resampling" }] },
      { kind: "structural", reason: NO_ZONAL_REASON, controls: [{ key: "zonal", label: "Zonal average" }] },
    ]);
    // A grid with no map lists Resampling with the projection, not twice.
    const unplaced = unavailableControls(
      { reprojectable: false, placement: "unplaceable", label: "source", supports: none },
      { arrows: false, csv: false },
    );
    assert.deepStrictEqual(
      unplaced.flatMap((g) => g.controls.map((c) => c.key)).filter((k) => k === "resampling"),
      ["resampling"],
    );
    assert.ok(!unplaced.some((g) => g.reason === NO_BILINEAR_REASON), "the projection's reason covers it");
  });

  test("a grid that takes everything lists nothing, and the panel's own rows decide what can be listed", () => {
    const placed = { reprojectable: true, placement: "placed" as const, label: "latlon", supports: all };
    assert.deepStrictEqual(unavailableControls(placed, { arrows: true, csv: true }), []);
    const sat = { ...placed, label: "space_view", supports: { ...none, bilinear: true } };
    const labels = (offers: { arrows: boolean; csv: boolean }) =>
      unavailableControls(sat, offers).flatMap((g) => g.controls.map((c) => c.label));
    assert.deepStrictEqual(labels({ arrows: false, csv: false }), ["Zonal average", "Contours"]);
    assert.deepStrictEqual(labels({ arrows: true, csv: true }), [
      "Zonal average",
      "Contours",
      "Vectors",
      "Long layout (Export CSV)",
    ]);
  });
});

suite("Resampling is shown only while the projection warps (#1091)", function () {
  this.timeout(120_000);

  test("a GRIB panel hides it on Source and brings it back, with its value, on a warp", async () => {
    const p = await provider();
    const editor = await openEditor(p, crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.resampling, "none", "no Resampling on Source");
      assert.strictEqual(state.controlShown.resampling, false);
      assertNoLine(state); // a choice, not a limit of the grid
      assert.deepStrictEqual(state.shownWhileHidden, []);

      let from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent(isType("gridReady"), from, "the warp");
      state = await panel.drive([{ id: "picker-resampling", value: "bilinear" }]);
      assert.strictEqual(state.controlShown.resampling, true, "shown on a warp");
      assert.strictEqual(state.resampling, "bilinear");

      // Source and back: still Bilinear, and the render asks for it.
      from = panel.sent.length;
      state = await panel.drive([{ id: "picker-projection", value: "source" }]);
      assert.strictEqual(state.controlDisplay.resampling, "none");
      assert.strictEqual(state.resampling, "bilinear", "kept while hidden");
      assert.deepStrictEqual(state.shownWhileHidden, []);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "source", from, "Source");
      from = panel.sent.length;
      state = await panel.drive([{ id: "picker-projection", value: "mollweide" }]);
      const drawn = await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "mollweide", from, "Mollweide");
      assert.strictEqual(drawn.options.resampling, "bilinear", "the render uses the kept value");
      assert.strictEqual(state.controlShown.resampling, true);
      assert.strictEqual(state.resampling, "bilinear");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("the saved panel state round-trips it, hidden or shown", async () => {
    const p = await provider();
    const editor = await openEditor(p, crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await panel.drive([
        { id: "picker-projection", value: "equirectangular" },
        { id: "picker-resampling", value: "bilinear" },
      ]);
      // Waited for from here: from 0, the first render would answer at once.
      const sent = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridReady"), sent, "the page again");
      let state = await panel.drive([]);
      assert.strictEqual(state.projection, "equirectangular");
      assert.strictEqual(state.controlShown.resampling, true);
      assert.strictEqual(state.resampling, "bilinear");

      await panel.drive([{ id: "picker-projection", value: "source" }]);
      const before = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridReady"), before, "the page again, on Source");
      state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.controlDisplay.resampling, "none");
      assert.strictEqual(state.resampling, "bilinear", "the value survives a rebuild while hidden");
      state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assert.strictEqual(state.controlShown.resampling, true);
      assert.strictEqual(state.resampling, "bilinear");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a NetCDF slice panel does the same", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, "temperature") }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(first.sliceGrid.placed, true);
      let state = await panel.drive([]);
      assert.strictEqual(state.controlDisplay.resampling, "none", "no Resampling on Source");
      assert.deepStrictEqual(state.shownWhileHidden, []);

      const from = panel.sent.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the warp");
      state = await panel.drive([{ id: "picker-resampling", value: "bilinear" }]);
      assert.strictEqual(state.controlShown.resampling, true);
      state = await panel.drive([{ id: "picker-projection", value: "source" }]);
      assert.strictEqual(state.controlDisplay.resampling, "none");
      assert.strictEqual(state.resampling, "bilinear");
      state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assert.strictEqual(state.controlShown.resampling, true);
      assert.strictEqual(state.resampling, "bilinear");
      assert.deepStrictEqual(state.shownWhileHidden, []);
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("both webviews hide with one global rule, and no per-class override is left", () => {
    for (const name of ["render-panel.ts", "provider.ts"]) {
      const source = fs.readFileSync(path.join(extensionPath(), "src", name), "utf8");
      const global = source.match(/^\s*\[hidden\] \{ display: none !important; \}$/gm) ?? [];
      assert.strictEqual(global.length, 1, `${name} has the global rule once`);
      const overrides = source.match(/[^\s{}][^\n{}]*\[hidden\]\s*\{\s*display:\s*none/g) ?? [];
      assert.deepStrictEqual(overrides, [], `${name} has no per-class [hidden] override`);
    }
  });
});
