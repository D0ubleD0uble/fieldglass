// One file of every format opened and drawn, the way a user does it (#1869).
//
// The rest of the suite runs the development layout: everything under
// `extension/`, including what `.vscodeignore` leaves out of the `.vsix`. So a
// runtime file the package drops, or a release-built native module that
// differs from a local one, passed every test. The release workflow unzips the
// linux-x64 `.vsix` and runs this file against it (`runPackagedTest.ts`), so
// the extension, its native module and its pages all come from the package.
// In the ordinary suite it runs against the development layout, which keeps
// it from going stale between releases.
//
// It reaches the extension only through what the extension itself exposes:
// its activation API, its commands, and the pages it writes. Nothing here
// imports the extension's own modules, which would load the checkout's copy.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { extensionPath, isType, type Msg, openEditor, openReal, provider, type RealPanel } from "./real-panel";

/** The unzipped package's `extension/` folder, set by `runPackagedTest.ts`;
 *  unset in the ordinary suite. */
const PACKAGED = process.env.FIELDGLASS_PACKAGED_EXTENSION;

/** The checkout's `extension/` folder, which holds the fixtures. A packaged run
 *  copies this file next to the package, so the runner says where it is. */
const CHECKOUT = process.env.FIELDGLASS_SMOKE_CHECKOUT ?? path.resolve(__dirname, "..", "..", "..");

function fixture(...parts: string[]): string {
  const p = path.join(CHECKOUT, ...parts);
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** One file per format the editor opens. `mapLayers` also draws the
 *  coastlines, borders, lakes and rivers over it, from the package's `media/`. */
const FILES: Array<{ format: string; file: () => string; mapLayers?: boolean }> = [
  { format: "GRIB1", file: () => fixture("src", "test", "fixtures", "cmc_wind_300_2010052400_p012.grib") },
  {
    format: "GRIB2",
    file: () => fixture("src", "test", "fixtures", "regular_latlon_surface.grib2"),
    mapLayers: true,
  },
  { format: "NetCDF classic", file: () => fixture("src", "test", "fixtures", "ersst_v5_187001_cdf1.nc") },
  { format: "NetCDF-4", file: () => fixture("src", "test", "fixtures", "netcdf4_dimscale.nc") },
];

const ZARR_STORE = () => fixture("..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores", "cf_v2");

/** The first Render button the editor's page shows, as the message its click
 *  sends: a GRIB message's `decodeGrid` or a NetCDF variable's `renderVariable`. */
function firstRender(html: string): Msg {
  const grib = /class="render-btn" data-message-index="(\d+)"/.exec(html);
  if (grib) return { type: "decodeGrid", messageIndex: Number(grib[1]) };
  const netcdf = /class="netcdf-render-btn" data-variable-index="(\d+)"/.exec(html);
  if (netcdf) return { type: "renderVariable", variableIndex: Number(netcdf[1]) };
  assert.fail("the editor's page offers no Render button");
}

/** The panel received a picture and its page painted it, with no page error. */
async function assertDrawn(panel: RealPanel, what: string): Promise<void> {
  const ready = await panel.waitSent(isType("gridReady"), 0, `${what}: the first render`);
  assert.ok(ready.width > 0 && ready.height > 0, `${what}: an empty picture, ${ready.width}x${ready.height}`);
  assert.strictEqual(ready.rgba.length, ready.width * ready.height * 4, `${what}: the picture's size`);
  // The page paints after the picture arrives, so ask until it has.
  let page = await panel.drive([]);
  const until = Date.now() + 15_000;
  while (!page.canvasPainted && page.pageErrors.length === 0 && Date.now() < until) {
    await new Promise((r) => setTimeout(r, 100));
    page = await panel.drive([]);
  }
  assert.deepStrictEqual(page.pageErrors, [], `${what}: the page reported errors`);
  assert.ok(page.canvasPainted, `${what}: the page never painted its image`);
}

/** Each map layer's checkbox, and the name its layer has in `overlayReady`.
 *  Each reads its own file under the package's `media/`. */
const MAP_LAYERS = [
  { id: "overlay-coastlines", layer: "coastline" },
  { id: "overlay-borders", layer: "borders" },
  { id: "overlay-lakes", layer: "lakes" },
  { id: "overlay-rivers", layer: "rivers" },
] as const;

/** Ticking each map layer in turn draws some of that layer over the map. */
async function assertMapLayers(panel: RealPanel, what: string): Promise<void> {
  for (const { id, layer } of MAP_LAYERS) {
    const from = panel.sent.length;
    await panel.drive([{ id, checked: true }]);
    const reply = await panel.waitSent(
      (m: Msg) =>
        m &&
        (m.type === "overlayError" ||
          (m.type === "overlayReady" && (m.layers as Array<{ name: string }>).some((l) => l.name === layer))),
      from,
      `${what}: the ${layer} layer`,
    );
    assert.strictEqual(reply.type, "overlayReady", `${what}: the ${layer} layer was refused: ${reply.error}`);
    const drawn = (reply.layers as Array<{ name: string; segLengths: Uint32Array }>).find((l) => l.name === layer);
    assert.ok(drawn && drawn.segLengths.length > 0, `${what}: no ${layer} drawn`);
  }
}

suite("The packaged extension opens and draws every format (#1869)", function () {
  this.timeout(120_000);

  if (PACKAGED) {
    test("the extension under test is the unzipped package", () => {
      assert.strictEqual(fs.realpathSync(extensionPath()), fs.realpathSync(PACKAGED));
      assert.notStrictEqual(fs.realpathSync(extensionPath()), fs.realpathSync(CHECKOUT), "the checkout is not the package");
    });
  }

  for (const { format, file, mapLayers } of FILES) {
    test(`${format}: the first Render draws`, async () => {
      const p = await provider();
      const editor = await openEditor(p, file());
      try {
        const panel = await openReal(p, () => editor.send(firstRender(editor.html())));
        try {
          await assertDrawn(panel, format);
          if (mapLayers) await assertMapLayers(panel, format);
        } finally {
          panel.dispose();
        }
      } finally {
        editor.close();
      }
    });
  }

  test("Zarr: Open Zarr Store draws the store", async () => {
    const p = await provider();
    const store = vscode.Uri.file(ZARR_STORE());
    // The command a user runs, with only its folder dialog answered.
    const w = vscode.window as unknown as Record<string, unknown>;
    const showOpenDialog = w.showOpenDialog;
    w.showOpenDialog = () => Promise.resolve([store]);
    try {
      const panel = await openReal(p, async () => {
        await vscode.commands.executeCommand("fieldglass.openZarrStore");
      });
      try {
        await assertDrawn(panel, "Zarr");
      } finally {
        panel.dispose();
      }
    } finally {
      w.showOpenDialog = showOpenDialog;
    }
  });

  if (PACKAGED) {
    test("the native module loaded is the package's own", () => {
      // VS Code loads native modules of its own, so only Fieldglass's count.
      const bin = path.join(fs.realpathSync(PACKAGED), "bin") + path.sep;
      const loaded = Object.keys(require.cache)
        .filter((k) => /^fieldglass\..+\.node$/.test(path.basename(k)))
        .map((k) => fs.realpathSync(k));
      assert.ok(loaded.length > 0, "no Fieldglass native module is loaded");
      for (const k of loaded) {
        assert.ok(k.startsWith(bin), `a Fieldglass native module from outside the package's bin/: ${k}`);
      }
    });
  }
});
