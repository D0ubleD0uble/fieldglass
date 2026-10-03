// A slice panel's projection picker follows the slice on screen (#822).
//
// The picker used to be written once, from the slice the panel opened on. A
// panel opened on a variable with no coordinates then offered only the source
// view for every variable it moved to, and one opened on a placed variable kept
// offering map targets that an unplaceable one refused in `renderSlice`.
//
// These drive the provider's real message handler through a stand-in panel,
// over the real native handles, and read back what it posts: each render now
// carries the drawn slice's own answer, and the panel script builds its options
// from that answer with the same function the first HTML is written with.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import { loadNative } from "../../native";
import type { FieldglassDocument, FieldglassEditorProvider, GridReadyMessage } from "../../provider";
import { MAP_PROJECTIONS, projectionOptionsHtml, reprojectionNote } from "../../render-panel";

const EXT_ID = "fieldglass.fieldglass";

/** The note beside the picker for a slice with no coordinates. */
const UNPLACED_NOTE = "Reprojection isn't available for source grids yet.";

function extensionPath(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed");
  return ext.extensionPath;
}

async function provider(): Promise<FieldglassEditorProvider> {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  assert.ok(ext, "extension is installed");
  return (await ext.activate()).provider;
}

/** The option values a projection `<select>`'s inner HTML offers. */
function offered(optionsHtml: string): string[] {
  return [...optionsHtml.matchAll(/<option value="([^"]+)"/g)].map((m) => m[1]);
}

/** The options of the panel's own projection picker, as written in its HTML. */
function pickerIn(html: string): string[] {
  const select = /<select id="picker-projection">([\s\S]*?)<\/select>/.exec(html);
  assert.ok(select, "the projection picker is in the panel");
  return offered(select[1]);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Posted = any;

interface FakePanel {
  /** Send the provider a message as the panel script would. */
  send(m: object): void;
  /** Everything the provider has posted to the panel, oldest first. */
  posted: Posted[];
  /** The panel's HTML as last written. */
  html(): string;
  /** Rebuild the panel the way a theme or colormap change does. */
  rebuild(): void;
}

/** Open a slice panel through `open`, against a stand-in for the webview panel
 *  that records what the provider registers and posts. */
function withSlicePanel(p: FieldglassEditorProvider, open: () => void): FakePanel {
  let onMessage: ((m: object) => void) | undefined;
  const posted: Posted[] = [];
  const fake = {
    webview: {
      html: "",
      cspSource: "vscode-webview:",
      onDidReceiveMessage: (cb: (m: object) => void) => {
        onMessage = cb;
        return { dispose: () => undefined };
      },
      postMessage: (m: Posted) => {
        posted.push(m);
        return Promise.resolve(true);
      },
    },
    onDidDispose: () => ({ dispose: () => undefined }),
    reveal: () => undefined,
    dispose: () => undefined,
  };
  const origCreate = vscode.window.createWebviewPanel;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).createWebviewPanel = () => fake;
  try {
    open();
  } finally {
    vscode.window.createWebviewPanel = origCreate;
  }
  assert.ok(onMessage, "the slice panel registers a message handler");
  const handler = onMessage;
  // The builder `trackRenderPanel` keeps for this panel: what a theme change
  // or an imported colormap runs to write the panel again.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const builders = (p as any)._renderPanelBuilders as Map<unknown, () => void>;
  const build = builders.get(fake);
  assert.ok(build, "the panel is tracked for rebuilds");
  return {
    send: (m) => handler(m),
    posted,
    html: () => fake.webview.html,
    rebuild: build,
  };
}

/** Ask for one slice in one projection and return the reply, which must be a
 *  render — not an error. */
function render(
  panel: FakePanel,
  slice: { variableIndex: number; yDim: number; xDim: number; sliceIndices: number[] },
  projection: string,
): GridReadyMessage {
  const before = panel.posted.length;
  panel.send({ type: "rerenderRequest", projection, resampling: "nearest", flipY: false, slice });
  const replies = panel.posted.slice(before);
  assert.strictEqual(replies.length, 1, "one reply per render request");
  const reply = replies[0];
  assert.strictEqual(
    reply.type,
    "gridReady",
    `${projection} on variable ${slice.variableIndex}: ${reply.error ?? "not a render"}`,
  );
  return reply as GridReadyMessage;
}

/** What the panel script's picker offers after it takes this render. */
function pickerAfter(reply: GridReadyMessage): string[] {
  assert.ok(reply.sliceGrid, "a slice render carries the slice's own answer");
  return offered(projectionOptionsHtml(MAP_PROJECTIONS, reply.sliceGrid.reprojectable));
}

suite("Slice panel projection picker", () => {
  test("the picker's targets are the ones the provider accepts", () => {
    // `MAP_PROJECTIONS` is what the panel offers, and the provider snaps
    // anything it does not list back to `source`; a target offered here and
    // missing there would be a picker entry that silently does nothing.
    const values = offered(projectionOptionsHtml(MAP_PROJECTIONS, true));
    assert.deepStrictEqual(values, [
      "source",
      "equirectangular",
      "web_mercator",
      "orthographic",
      "polar_stereographic",
      "mollweide",
      "robinson",
      "equal_earth",
    ]);
    assert.deepStrictEqual(offered(projectionOptionsHtml(MAP_PROJECTIONS, false)), ["source"]);
    assert.strictEqual(reprojectionNote(true, "latlon"), "");
    assert.strictEqual(reprojectionNote(false, "source"), UNPLACED_NOTE);
    assert.strictEqual(reprojectionNote(false, null), "Reprojection isn't available for this grids yet.");
  });

  test("a NetCDF panel opened on an unplaceable variable offers the map targets for a placed one", async () => {
    const p = await provider();
    const uri = vscode.Uri.file(path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc"));
    const doc = (await p.openCustomDocument(
      uri,
      {} as vscode.CustomDocumentOpenContext,
      new vscode.CancellationTokenSource().token,
    )) as FieldglassDocument;
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.NetcdfHandle.fromBytes(fs.readFileSync(uri.fsPath)).variables();
    const byName = (name: string) => {
      const v = variables.find((x) => x.name === name);
      assert.ok(v, `the fixture has ${name}`);
      return v;
    };
    const latBnds = byName("lat_bnds");
    const temperature = byName("temperature");
    // The slice the panel script picks for a variable: its detected axes.
    const latBndsSlice = { variableIndex: latBnds.variableIndex, yDim: 0, xDim: 1, sliceIndices: [0, 0] };
    const temperatureSlice = {
      variableIndex: temperature.variableIndex,
      yDim: 1,
      xDim: 2,
      sliceIndices: [0, 0, 0],
    };

    // The editor opens a slice panel with a private call; this is that call.
    const panel = withSlicePanel(p, () =>
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (p as any).openNetcdfRenderPanel(doc, latBnds.variableIndex),
    );
    assert.deepStrictEqual(pickerIn(panel.html()), ["source"], "opened on lat_bnds");
    assert.match(panel.html(), /Reprojection isn't available for source grids yet\./);

    // The first paint: lat_bnds, source only.
    panel.send({ type: "ready", projection: "source", resampling: "nearest", flipY: false, slice: latBndsSlice });
    const first = panel.posted[panel.posted.length - 1] as GridReadyMessage;
    assert.strictEqual(first.type, "gridReady");
    assert.deepStrictEqual(first.sliceGrid, { label: "source", reprojectable: false, note: UNPLACED_NOTE });

    // Move the same panel onto temperature. The render says it can be
    // reprojected, so the picker offers the map targets…
    const onTemperature = render(panel, temperatureSlice, "source");
    assert.deepStrictEqual(onTemperature.sliceGrid, { label: "latlon", reprojectable: true, note: "" });
    assert.ok(pickerAfter(onTemperature).includes("equirectangular"));
    // The export is named for the variable drawn, not the one the panel opened
    // on — the same frozen-at-open shape as the picker.
    assert.match(panel.html(), /DEFAULT_PNG_NAME = "lat_bnds-message-1\.png"/);
    assert.strictEqual(onTemperature.defaultPngName, `temperature-message-${temperature.variableIndex}.png`);

    // …and they draw.
    const mapped = render(panel, temperatureSlice, "equirectangular");
    assert.strictEqual(mapped.options.projection, "equirectangular");
    assert.ok(mapped.usedLatMin !== null, "an equirectangular render reports its extent");

    // A rebuild now writes the picker for the slice on screen, so the saved
    // selection the panel restores is one it still offers.
    panel.rebuild();
    assert.ok(pickerIn(panel.html()).includes("equirectangular"), "a rebuild keeps the map targets");
    assert.doesNotMatch(panel.html(), /Reprojection isn't available/);

    // Back to lat_bnds with equirectangular still selected. It is drawn in the
    // source view rather than failing, and the picker goes back to source only,
    // with the note.
    const back = render(panel, latBndsSlice, "equirectangular");
    assert.strictEqual(back.options.projection, "source", "drawn in the source view");
    assert.strictEqual(back.usedLatMin, null, "with no geographic extent");
    assert.deepStrictEqual(back.sliceGrid, { label: "source", reprojectable: false, note: UNPLACED_NOTE });
    assert.deepStrictEqual(pickerAfter(back), ["source"]);
  });

  // One `openSliceRenderPanel` serves both containers, so the NetCDF test above
  // covers the Zarr panel's code as well. This pins the Zarr entry point into
  // it, with the only unplaceable slice the committed CF store has: its axes
  // transposed, which a placed store refuses to map.
  test("a Zarr store panel follows the slice too", async () => {
    const p = await provider();
    const store = path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores", "cf_v2");
    assert.ok(fs.existsSync(store), `fixture store missing: ${store}`);
    const panel = withSlicePanel(p, () => p.openZarrStore(vscode.Uri.file(store)));
    assert.ok(pickerIn(panel.html()).includes("equirectangular"), "cf_v2 opens placed");

    panel.send({ type: "ready", projection: "source", resampling: "nearest", flipY: false });
    const first = panel.posted[panel.posted.length - 1] as GridReadyMessage;
    assert.strictEqual(first.type, "gridReady");
    assert.deepStrictEqual(first.sliceGrid, { label: "latlon", reprojectable: true, note: "" });
    const variableIndex = first.messageIndex;

    const mapped = render(panel, { variableIndex, yDim: 0, xDim: 1, sliceIndices: [0, 0] }, "equirectangular");
    assert.strictEqual(mapped.options.projection, "equirectangular");

    const transposed = render(panel, { variableIndex, yDim: 1, xDim: 0, sliceIndices: [0, 0] }, "equirectangular");
    assert.strictEqual(transposed.options.projection, "source", "drawn in the source view, not refused");
    assert.deepStrictEqual(transposed.sliceGrid, { label: "source", reprojectable: false, note: UNPLACED_NOTE });
    assert.deepStrictEqual(pickerAfter(transposed), ["source"]);
  });
});
