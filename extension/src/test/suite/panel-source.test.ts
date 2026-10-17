// A render panel says which file or store it draws, and a NetCDF or Zarr panel
// never calls its variable a "message" (#1038).
//
// With the v2 store, the v3 store and the NetCDF twin of the same data open on
// one variable, the three tabs and headings used to read the same, and every
// heading said "Message 0", a GRIB word. The slice panel's tab also kept the
// variable it opened on after the picker moved to another.
//
// These open committed fixtures the way a user does (the editor's message for a
// file, the store command for a store) and read the real tab and the page the
// panel's own script draws, through the driver in `real-panel.ts`.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { sourceName } from "../../provider";
import {
  composeDefaultPngName,
  composeTabTitle,
  composeTitleLine,
  type PanelField,
  renderImagePanelHtml,
} from "../../render-panel";
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

const ZARR_FIXTURES = () => path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures");

/** A committed fixture, asserted to exist so a moved one fails loudly. */
function fixture(p: string): string {
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** The tab VS Code shows for `panel`: what a user reads, not what the
 *  provider set. The tab model catches up on a later tick. */
async function tabLabel(panel: RealPanel): Promise<string> {
  const want = panel.title();
  return waitUntil(
    () => vscode.window.tabGroups.all.flatMap((g) => g.tabs).map((t) => t.label).find((l) => l === want),
    `a tab labelled "${want}"`,
  );
}

/** Every NetCDF or Zarr panel's page, checked for the GRIB word. */
function assertNoMessage(state: PageState, what: string): void {
  assert.ok(state.text.includes("Export PNG"), `${what}: the page's text was read`);
  const hit = /\bmessages?\b/i.exec(state.text);
  assert.strictEqual(
    hit,
    null,
    `${what} says "${hit?.[0]}": ${state.text.slice(Math.max(0, (hit?.index ?? 0) - 60), (hit?.index ?? 0) + 60)}`,
  );
}

const drawn = (variableIndex: number) => (m: Msg) => m && m.type === "gridReady" && m.messageIndex === variableIndex;

suite("A render panel names its file or store (#1038)", function () {
  this.timeout(120_000);

  test("a GRIB panel's tab and heading name the message and the file", async () => {
    const p = await provider();
    const file = fixture(path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(file)).message(0);
    assert.ok(meta.abbreviation, "the fixture's message has an abbreviation");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const ready = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(panel.title(), `msg 0 ${meta.abbreviation} · regular_latlon_surface.grib2`);
      assert.strictEqual(await tabLabel(panel), panel.title(), "the tab a user sees");
      const state = await panel.drive([]);
      // GRIB keeps its own word.
      assert.strictEqual(state.title, composeTitleLine(meta, "message"));
      assert.match(state.title, /^Message 0 — /);
      assert.ok(
        state.subtitle.startsWith("regular_latlon_surface.grib2 · "),
        `the line under the heading names the file, then the level: "${state.subtitle}"`,
      );
      assert.ok(
        ready.defaultPngName.startsWith("regular_latlon_surface-") && ready.defaultPngName.endsWith("-message-0.png"),
        `the PNG is named for the file: ${ready.defaultPngName}`,
      );
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("the v2 store, the v3 store and their NetCDF twin each say which they are, and never say message", async () => {
    const p = await provider();
    const twin = fixture(path.join(ZARR_FIXTURES(), "cf_twin.nc"));
    const stores = ["cf_v2", "cf_v3"].map((s) => fixture(path.join(ZARR_FIXTURES(), "stores", s)));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.NetcdfHandle.fromBytes(fs.readFileSync(twin)).variables();
    const a = variables.find((v) => v.name === "a");
    const t = variables.find((v) => v.name === "t");
    assert.ok(a && t, `the twin has a and t, got ${variables.map((v) => v.name).join(", ")}`);
    // The stores open on their first variable; the twin is opened on the same.
    // Each numbers its variables its own way, so t is found in each.
    const tIn = new Map<string, number>([["cf_twin.nc", t.variableIndex]]);
    for (const store of stores) {
      const vars = native.ZarrHandle.fromDirectory(store).variables();
      assert.strictEqual(vars[0].name, "a", `${store} opens on a`);
      const tv = vars.find((v) => v.name === "t");
      assert.ok(tv, `${store} has t`);
      tIn.set(path.basename(store), tv.variableIndex);
    }

    const editor = await openEditor(p, twin);
    const panels: Array<{ source: string; panel: RealPanel }> = [];
    try {
      panels.push({
        source: "cf_twin.nc",
        panel: await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: a.variableIndex })),
      });
      for (const store of stores) {
        panels.push({ source: path.basename(store), panel: await openReal(p, () => p.openZarrStore(vscode.Uri.file(store))) });
      }

      const titles: string[] = [];
      for (const { source, panel } of panels) {
        const ready = await panel.waitSent(isType("gridReady"), 0, `${source}'s first render`);
        assert.strictEqual(panel.title(), `a · ${source}`, "the tab names the variable, then the source");
        assert.strictEqual(await tabLabel(panel), panel.title(), "the tab a user sees");
        titles.push(panel.title());
        const state = await panel.drive([]);
        assert.strictEqual(state.title, ready.titleLine, "the page shows the heading the render sent");
        assert.ok(state.title === "a" || state.title.startsWith("a ("), `the heading names the variable alone: "${state.title}"`);
        assert.strictEqual(state.subtitle, source, "the line under the heading names the source");
        assert.strictEqual(ready.defaultPngName, `${source.replace(/\.nc$/, "")}-a.png`);
        assertNoMessage(state, `the ${source} panel`);
      }
      assert.strictEqual(new Set(titles).size, 3, `three tabs, three titles: ${titles.join(" | ")}`);

      // Onto t: the tab, the heading and the export name follow, in each.
      for (const { source, panel } of panels) {
        const from = panel.sent.length;
        const ti = tIn.get(source) ?? -1;
        await panel.drive([{ id: "slice-variable", value: ti }]);
        const ready = await panel.waitSent(drawn(ti), from, `${source} drawing t`);
        assert.strictEqual(panel.title(), `t · ${source}`, "the tab follows the variable");
        assert.strictEqual(await tabLabel(panel), panel.title(), "the tab a user sees");
        assert.strictEqual(ready.defaultPngName, `${source.replace(/\.nc$/, "")}-t.png`);
        const state = await panel.drive([]);
        assert.match(state.title, /^t\b/, `the heading follows: "${state.title}"`);
        assert.strictEqual(state.subtitle, source);
        assertNoMessage(state, `the ${source} panel on t`);
      }

      // A rebuild (an imported colour table, a reload) restores the panel on
      // t (#1027), and the tab and heading still say so.
      const { source, panel } = panels[2];
      const from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(drawn(tIn.get(source) ?? -1), from, "the restored render of t");
      assert.strictEqual(panel.title(), `t · ${source}`);
      const state = await panel.drive([]);
      assert.match(state.title, /^t\b/);
      assert.strictEqual(state.subtitle, source);
      assertNoMessage(state, `the rebuilt ${source} panel`);
    } finally {
      for (const { panel } of panels) panel.dispose();
      editor.close();
    }
  });

  test("a tab title names the field first, and a hostile name cannot garble it", () => {
    assert.strictEqual(composeTabTitle("msg 3 TMP", "gfs.grib2"), "msg 3 TMP · gfs.grib2");
    assert.strictEqual(composeTabTitle("air", "air_v2.zarr"), "air · air_v2.zarr");
    assert.strictEqual(composeTabTitle("a\u0007b", "x\ny.nc"), "ab · xy.nc");
    assert.strictEqual(composeTabTitle("v", "n".repeat(100)), `v · ${"n".repeat(64)}`);
    assert.strictEqual(composeTabTitle("v", ""), "v", "no source, no separator");
  });

  test("a heading says Message only for GRIB", () => {
    const meta = { index: 2, parameter: "air", units: "degK" };
    assert.strictEqual(composeTitleLine(meta, "message"), "Message 2 — air (degK)");
    assert.strictEqual(composeTitleLine(meta, "variable"), "air (degK)");
    assert.strictEqual(composeTitleLine({ index: 2, parameter: "air", units: "" }, "variable"), "air");
  });

  test("an export is named for its source, and only a GRIB one for its message", () => {
    const meta = { index: 4, parameter: "Temperature" };
    assert.strictEqual(
      composeDefaultPngName(meta, { kind: "message", name: "gfs.grib2" }),
      "gfs-temperature-message-4.png",
    );
    assert.strictEqual(
      composeDefaultPngName({ index: 0, parameter: "air" }, { kind: "variable", name: "air_v2.zarr" }),
      "air_v2-air.png",
    );
    assert.strictEqual(
      composeDefaultPngName({ index: 0, parameter: "air" }, { kind: "variable", name: "ncep_air_daily_2024.nc" }),
      "ncep_air_daily_2024-air.png",
    );
    // Only an extension Fieldglass opens by is dropped: this one's last dot
    // is the forecast hour.
    assert.strictEqual(
      composeDefaultPngName(meta, { kind: "message", name: "gfs.t00z.pgrb2.0p25.f000" }),
      "gfs.t00z.pgrb2.0p25.f000-temperature-message-4.png",
    );
  });

  test("a store is named by its folder, with or without a trailing separator", () => {
    assert.strictEqual(sourceName(vscode.Uri.file("/data/air_v2.zarr")), "air_v2.zarr");
    assert.strictEqual(sourceName(vscode.Uri.parse("file:///data/air_v2.zarr/")), "air_v2.zarr");
    assert.strictEqual(sourceName(vscode.Uri.file("/data/gfs.grib2")), "gfs.grib2");
  });

  test("a file's own name and a variable's units cannot close the panel script", () => {
    const hostile = 'x</script><b id="injected">';
    const meta: PanelField = {
      index: 0,
      parameter: hostile,
      units: hostile,
      level: null,
      levelType: null,
      referenceTime: null,
      forecast: null,
      uvRelativeToGrid: null,
      reprojectable: false,
      placement: "placed",
      supports: { contours: false, vectors: false, longCsv: false, zonalMean: false },
      truncation: null,
      grid: null,
    };
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      meta,
      "summary",
      [],
      [],
      undefined,
      undefined,
      undefined,
      `${hostile}.grib2`,
    );
    assert.ok(!html.includes(hostile), "every copy of the name is escaped");
    assert.ok(html.includes("x\\u003c/script>"), "and inlined in its script-safe form");
  });
});
