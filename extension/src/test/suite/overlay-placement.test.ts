// The render panel offers its overlays only for a field placed on the Earth
// (#840).
//
// Coastlines, the other map layers, the graticule, contours and arrows all
// project through the field's geometry. A field nothing places (a NetCDF slice
// with no coordinates, a cross-section, a GRIB grid whose projection cannot
// place it) still draws in its own grid coordinates, but every one of those
// requests is refused, and the panel used to send them anyway and show the
// refusal as an error.
//
// These run the panel's own script in a real webview. The panel is opened the
// way a user opens it, through the editor's message handler, and a small
// driver script added to its page flips the controls a user would and reports
// what the page shows. The test then reads what crossed the boundary in both
// directions: the requests the panel sent, and the provider's answers.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import { loadNative } from "../../native";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";
import { UNPLACED_OVERLAY_NOTE, UNPLACED_VECTORS_NOTE } from "../../render-panel";

const EXT_ID = "fieldglass.fieldglass";

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

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Msg = any;

/** What the driver reports of the page. */
interface PageState {
  overlayDisabled: boolean;
  overlayHidden: boolean;
  /** The Overlay row's note when shown, `null` when hidden. */
  overlayNote: string | null;
  vectorDisabled: boolean | null;
  vectorNote: string | null;
  coastlines: boolean;
  graticule: boolean;
  contours: boolean;
  projection: string;
  canvasVisibility: string;
}

/** Added to the panel's page ahead of its own script. It keeps the webview API
 *  the panel acquires, so it can answer on the same channel, and on each
 *  `test:drive` sets the named controls, fires their `change` like a user's
 *  edit, and reports the page. A message is handled after every one posted
 *  before it, so the report follows whatever the page did with those. */
const DRIVER = `
  (function () {
    const acquire = window.acquireVsCodeApi;
    let api = null;
    window.acquireVsCodeApi = function () {
      if (!api) api = acquire();
      return api;
    };
    const byId = (id) => document.getElementById(id);
    const shownText = (el) => (el && !el.hasAttribute('hidden') ? el.textContent : null);
    function report() {
      const overlays = byId('overlay-fieldset');
      const vectors = byId('vector-fieldset');
      return {
        overlayDisabled: !!(overlays && overlays.disabled),
        overlayHidden: !!(overlays && overlays.hasAttribute('hidden')),
        overlayNote: shownText(byId('overlay-note')),
        vectorDisabled: vectors ? vectors.disabled : null,
        vectorNote: vectors ? byId('vector-note').textContent : null,
        coastlines: byId('overlay-coastlines').checked,
        graticule: byId('overlay-graticule').checked,
        contours: byId('overlay-contours').checked,
        projection: byId('picker-projection').value,
        canvasVisibility: byId('canvas').style.visibility,
      };
    }
    window.addEventListener('message', (ev) => {
      const m = ev.data;
      if (!m || m.type !== 'test:drive') return;
      for (const a of m.actions) {
        const el = byId(a.id);
        if (!el) throw new Error('no #' + a.id);
        if ('checked' in a) el.checked = a.checked; else el.value = String(a.value);
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      api.postMessage({ type: 'test:state', tag: m.tag, state: report() });
    });
  })();
`;

/** The page with the driver ahead of the panel's script, under its nonce. */
function withDriver(html: string): string {
  const nonce = /<script nonce="([^"]+)">/.exec(html);
  assert.ok(nonce, "the panel script carries a nonce");
  return html.replace("</head>", `<script nonce="${nonce[1]}">${DRIVER}</script>\n</head>`);
}

interface RealPanel {
  /** What the page sent the provider, oldest first. */
  received: Msg[];
  /** What the provider posted to the page, oldest first. */
  sent: Msg[];
  /** Set controls as a user would, then read the page back. */
  drive(actions: Array<{ id: string; value?: string | number; checked?: boolean }>): Promise<PageState>;
  /** Wait for the provider to post a message matching `pred`, after `from`. */
  waitSent(pred: (m: Msg) => boolean, from: number, what: string): Promise<Msg>;
  dispose(): void;
}

async function waitUntil<T>(get: () => T | undefined, what: string, ms = 15_000): Promise<T> {
  const until = Date.now() + ms;
  for (;;) {
    const got = get();
    if (got !== undefined) return got;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** Run `open`, which makes the provider create one render panel, and hand back
 *  that real panel with the driver on its page and both directions recorded. */
async function openReal(open: () => void): Promise<RealPanel> {
  const received: Msg[] = [];
  const sent: Msg[] = [];
  let made: vscode.WebviewPanel | undefined;
  const origCreate = vscode.window.createWebviewPanel;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).createWebviewPanel = (...args: Parameters<typeof origCreate>) => {
    const panel = origCreate.apply(vscode.window, args);
    const webview = panel.webview;
    // The provider writes the page through `html`; the driver goes in on the way.
    let proto = Object.getPrototypeOf(webview);
    let html: PropertyDescriptor | undefined;
    while (proto && !(html = Object.getOwnPropertyDescriptor(proto, "html"))) {
      proto = Object.getPrototypeOf(proto);
    }
    assert.ok(html?.get && html.set, "the webview's html is an accessor");
    const { get, set } = html;
    Object.defineProperty(webview, "html", {
      configurable: true,
      get: () => get.call(webview),
      set: (v: string) => set.call(webview, withDriver(v)),
    });
    const post = webview.postMessage.bind(webview);
    webview.postMessage = (m: Msg) => {
      sent.push(m);
      return post(m);
    };
    webview.onDidReceiveMessage((m) => received.push(m));
    made = panel;
    return panel;
  };
  try {
    open();
  } finally {
    vscode.window.createWebviewPanel = origCreate;
  }
  const panel = made;
  assert.ok(panel, "a render panel was created");
  let tag = 0;
  return {
    received,
    sent,
    drive: async (actions) => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:drive", tag: mine, actions });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:state" && m.tag === mine),
        `the page's report ${mine}`,
      );
      return reply.state as PageState;
    },
    waitSent: (pred, from, what) => waitUntil(() => sent.slice(from).find(pred), what),
    dispose: () => panel.dispose(),
  };
}

/** Open `file` in the editor, through a stand-in editor webview whose message
 *  handler the test calls the way the table's page does. */
async function openEditor(
  p: FieldglassEditorProvider,
  file: string,
): Promise<(m: object) => void> {
  const doc = (await p.openCustomDocument(
    vscode.Uri.file(file),
    {} as vscode.CustomDocumentOpenContext,
    new vscode.CancellationTokenSource().token,
  )) as FieldglassDocument;
  let handler: ((m: object) => void) | undefined;
  const editor = {
    webview: {
      html: "",
      options: {},
      cspSource: "vscode-webview:",
      onDidReceiveMessage: (cb: (m: object) => void) => {
        handler = cb;
        return { dispose: () => undefined };
      },
      postMessage: () => Promise.resolve(true),
    },
    onDidDispose: () => ({ dispose: () => undefined }),
    onDidChangeViewState: () => ({ dispose: () => undefined }),
    dispose: () => undefined,
  };
  await p.resolveCustomEditor(doc, editor as unknown as vscode.WebviewPanel);
  assert.ok(handler, "the editor registers a message handler");
  return handler;
}

const isType = (type: string) => (m: Msg) => m && m.type === type;
const overlayTraffic = (m: Msg) =>
  m && ["overlayRequest", "contourRequest", "vectorRequest"].includes(m.type);
const refusals = (m: Msg) =>
  m && ["overlayError", "contourError", "gridError"].includes(m.type);

suite("Overlays follow placement (#840)", function () {
  this.timeout(60_000);

  test("a NetCDF slice panel drops the overlays on a slice it cannot place, and takes them back", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const variables = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables();
    const index = (name: string) => {
      const v = variables.find((x) => x.name === name);
      assert.ok(v, `the fixture has ${name}`);
      return v.variableIndex;
    };
    const temperature = index("temperature");
    const latBnds = index("lat_bnds");

    const editor = await openEditor(p, file);
    const panel = await openReal(() => editor({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // temperature is placed: the row is offered, and coastlines and
      // contours on an equirectangular map are drawn.
      let from = panel.sent.length;
      let state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      from = panel.sent.length;
      await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-graticule", checked: true },
        { id: "overlay-contours", checked: true },
      ]);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");
      await panel.waitSent(isType("contourReady"), from, "the contours");

      // Onto lat_bnds, which has no coordinates. The provider draws it in the
      // source view and the picker follows; the panel asks for no overlay and
      // so is refused nothing.
      const before = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      const drawn = await panel.waitSent(
        (m) => m.type === "gridReady" && m.messageIndex === latBnds,
        before.sent,
        "lat_bnds drawn",
      );
      assert.strictEqual(drawn.sliceGrid.placed, false, "the render says the slice is not placed");
      assert.strictEqual(drawn.options.projection, "source");
      state = await panel.drive([]);
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(state.overlayDisabled, true, "the Overlay row is disabled");
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE, "with the reason");
      assert.deepStrictEqual(
        panel.received.slice(before.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay, contour or arrow request for lat_bnds",
      );
      assert.deepStrictEqual(
        panel.sent.slice(before.sent).filter(refusals).map((m) => `${m.type}: ${m.error}`),
        [],
        "and no refusal",
      );
      // The toggles keep what the user chose, for the next field that takes them.
      assert.deepStrictEqual([state.coastlines, state.graticule, state.contours], [true, true, true]);

      // "Contours only" hides the raster. With no contours to show, lat_bnds
      // would be a blank panel, so the raster stays.
      state = await panel.drive([{ id: "contours-only", checked: true }]);
      assert.strictEqual(state.canvasVisibility, "visible", "the raster stays without contours");
      state = await panel.drive([{ id: "contours-only", checked: false }]);

      // Back onto temperature: the row comes back, and the overlays are asked
      // for and drawn again without the user touching them.
      const back = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      const placed = await panel.waitSent(
        (m) => m.type === "gridReady" && m.messageIndex === temperature,
        back,
        "temperature drawn",
      );
      assert.strictEqual(placed.sliceGrid.placed, true);
      await panel.waitSent(isType("overlayReady"), back, "the coastlines again");
      await panel.waitSent(isType("contourReady"), back, "the contours again");
      state = await panel.drive([]);
      assert.strictEqual(state.overlayDisabled, false);
      assert.strictEqual(state.overlayNote, null);

      // A cross-section is not placed either: time down the rows of
      // temperature. Its axes are not a map, so nothing is asked for.
      const cross = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([{ id: "slice-y", value: 0 }]);
      const plane = await panel.waitSent(isType("gridReady"), cross.sent, "the cross-section");
      assert.strictEqual(plane.sliceGrid.placed, false, "a cross-section is not placed");
      state = await panel.drive([]);
      assert.strictEqual(state.overlayHidden, true, "the Overlay row is put away, as before");
      assert.deepStrictEqual(
        panel.received.slice(cross.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay request for a cross-section",
      );
      assert.deepStrictEqual(panel.sent.slice(cross.sent).filter(refusals).map((m) => m.error), []);
    } finally {
      panel.dispose();
    }
  });

  test("a NetCDF slice panel opened on an unplaceable slice starts with the overlays disabled", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const latBnds = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
      .variables()
      .find((v) => v.name === "lat_bnds");
    assert.ok(latBnds, "the fixture has lat_bnds");

    const editor = await openEditor(p, file);
    const panel = await openReal(() => editor({ type: "renderVariable", variableIndex: latBnds.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([{ id: "overlay-coastlines", checked: true }]);
      assert.strictEqual(state.overlayDisabled, true);
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE);
      assert.deepStrictEqual(panel.received.filter(overlayTraffic).map((m) => m.type), []);
      assert.deepStrictEqual(panel.sent.filter(refusals).map((m) => m.error), []);
    } finally {
      panel.dispose();
    }
  });

  test("a GRIB panel on a grid nothing places offers no overlays or arrows", async () => {
    const p = await provider();
    // Two messages, so the Vectors row is written. The committed polar
    // stereographic fixture declares degenerate projection parameters, so its
    // grid is unplaceable (#776), and copied twice it is its own u/v pair.
    const bytes = fs.readFileSync(
      path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "polar_stereographic_surface.grib2"),
    );
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-840-"));
    const file = path.join(dir, "unplaced_pair.grib2");
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.placement, "unplaceable");

    const editor = await openEditor(p, file);
    const panel = await openReal(() => editor({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-contours", checked: true },
        { id: "overlay-vectors", checked: true },
      ]);
      assert.strictEqual(state.overlayDisabled, true);
      assert.strictEqual(state.overlayNote, UNPLACED_OVERLAY_NOTE);
      assert.strictEqual(state.vectorDisabled, true);
      assert.strictEqual(state.vectorNote, UNPLACED_VECTORS_NOTE);
      assert.deepStrictEqual(panel.received.filter(overlayTraffic).map((m) => m.type), []);
      assert.deepStrictEqual(panel.sent.filter((m: Msg) => m && /Error$|vectorResult/.test(m.type)), []);
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
