import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import { loadNative, type MessageInfo } from "../../native";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";
import { composeSubtitle, renderImagePanelHtml } from "../../render-panel";

const EXT_ID = "fieldglass.fieldglass";

async function activateExtension(): Promise<FieldglassApi> {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  if (!ext) {
    throw new Error(`extension ${EXT_ID} not found`);
  }
  return ext.activate();
}

function copyFixtureToTmp(name: string): vscode.Uri {
  const ext = vscode.extensions.getExtension(EXT_ID);
  if (!ext) {
    throw new Error(`extension ${EXT_ID} not found`);
  }
  const src = path.join(ext.extensionPath, "src", "test", "fixtures", name);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-spectral-"));
  const dest = path.join(dir, name);
  fs.copyFileSync(src, dest);
  return vscode.Uri.file(dest);
}

suite("GRIB1 spectral editor opens", () => {
  let api: FieldglassApi;
  let provider: FieldglassEditorProvider;

  suiteSetup(async () => {
    api = await activateExtension();
    provider = api.provider;
  });

  for (const fixture of ["spectral_complex_t63.grib1", "spectral_simple_t63.grib1"]) {
    test(`resolveCustomEditor populates the table for ${fixture}`, async () => {
      const uri = copyFixtureToTmp(fixture);
      const doc = (await provider.openCustomDocument(
        uri,
        {} as vscode.CustomDocumentOpenContext,
        new vscode.CancellationTokenSource().token,
      )) as FieldglassDocument;

      const panel = vscode.window.createWebviewPanel(
        "fieldglass.viewer",
        fixture,
        vscode.ViewColumn.One,
        {},
      );
      try {
        // Must not throw ("editor could not be opened due to an unexpected error").
        await provider.resolveCustomEditor(doc, panel);
        const html = panel.webview.html;
        assert.ok(html.length > 0, "webview html should be populated");
        assert.ok(
          html.includes("spectral_simple") || html.includes("spectral_complex"),
          "message table should show the spectral packing label",
        );
        assert.ok(html.includes("Temperature"), "message row should render");
        // GRIB1 spectral messages also render via the inverse transform (#303),
        // so they offer a Render button, not the unrenderable fallback.
        assert.ok(
          html.includes('class="render-btn"'),
          "spectral message offers a Render button",
        );
        assert.ok(
          !html.includes("Render not available"),
          "spectral message is not marked unrenderable",
        );
      } finally {
        panel.dispose();
      }
    });
  }
});

suite("GRIB2 spectral editor opens", () => {
  let api: FieldglassApi;
  let provider: FieldglassEditorProvider;

  suiteSetup(async () => {
    api = await activateExtension();
    provider = api.provider;
  });

  // GRIB2 spherical-harmonic messages (§3.50 + §5.50/5.51) carry coefficients,
  // not a grid, so they have no Ni/Nj. Opening one must not crash the editor —
  // the regression class from #288 (a grid-less message reaching an
  // `undefined.toFixed()` through a napi `Option` field that JS sees as
  // `undefined`, not `null`). This is the Electron half of #302.
  for (const fixture of ["spectral_simple_t63.grib2", "spectral_complex_t63.grib2"]) {
    test(`resolveCustomEditor populates the table for ${fixture}`, async () => {
      const uri = copyFixtureToTmp(fixture);
      const doc = (await provider.openCustomDocument(
        uri,
        {} as vscode.CustomDocumentOpenContext,
        new vscode.CancellationTokenSource().token,
      )) as FieldglassDocument;

      const panel = vscode.window.createWebviewPanel(
        "fieldglass.viewer",
        fixture,
        vscode.ViewColumn.One,
        {},
      );
      try {
        // Must not throw ("editor could not be opened due to an unexpected error").
        await provider.resolveCustomEditor(doc, panel);
        const html = panel.webview.html;
        assert.ok(html.length > 0, "webview html should be populated");
        assert.ok(
          html.includes("Spectral"),
          "message table should show the spectral packing label",
        );
        assert.ok(html.includes("Temperature"), "message row should render");
        // A spectral message has no grid, but the inverse-transform synthesis
        // (#303) makes it renderable — the table must offer Render, not the
        // "grid dimensions unknown" fallback.
        assert.ok(
          html.includes('class="render-btn"'),
          "spectral message offers a Render button",
        );
        assert.ok(
          !html.includes("Render not available"),
          "spectral message is not marked unrenderable",
        );
      } finally {
        panel.dispose();
      }
    });
  }
});

// GRIB2 HEALPix messages (§3.150) are the other family with no grid of its
// own: a list of `12·Nside²` equal-area pixels, so no Ni/Nj. Two things have to
// hold, and they pull in opposite directions — the editor must not crash on the
// missing dimensions (the #288 class), and the message must still offer a
// Render button, because #443 resamples it onto a lat/lon grid at decode. A
// bi-Fourier message is the contrast: also grid-less, but genuinely
// unrenderable.
suite("GRIB2 HEALPix editor opens", () => {
  let api: FieldglassApi;
  let provider: FieldglassEditorProvider;

  suiteSetup(async () => {
    api = await activateExtension();
    provider = api.provider;
  });

  test("resolveCustomEditor populates the table for healpix_n4_ring.grib2", async () => {
    const fixture = "healpix_n4_ring.grib2";
    const uri = copyFixtureToTmp(fixture);
    const doc = (await provider.openCustomDocument(
      uri,
      {} as vscode.CustomDocumentOpenContext,
      new vscode.CancellationTokenSource().token,
    )) as FieldglassDocument;

    const panel = vscode.window.createWebviewPanel(
      "fieldglass.viewer",
      fixture,
      vscode.ViewColumn.One,
      {},
    );
    try {
      // Must not throw: a HEALPix message has no gridNi/gridNj, which is the
      // shape that crashed the editor in #288.
      await provider.resolveCustomEditor(doc, panel);
      const html = panel.webview.html;
      assert.ok(html.length > 0, "webview html should be populated");
      assert.ok(html.includes("healpix"), "message table should name the grid type");
      assert.ok(
        html.includes('class="render-btn"'),
        "a HEALPix message offers a Render button (#443 resamples it)",
      );
      assert.ok(
        !html.includes("Render not available"),
        "a HEALPix message is not marked unrenderable",
      );
      // #416: a grid-less message has no Ni×Nj, but it does state its size.
      // Reporting a dash there says "we don't know" about something the file
      // spells out precisely.
      assert.ok(
        html.includes("Nside 4"),
        "the size column states the HEALPix resolution, not a dash",
      );
    } finally {
      panel.dispose();
    }
  });
});

// A Compare map is labelled by whichever operand is band-limited (#814). The
// panel subtitle and the PNG header came from field A's MessageInfo alone, so
// T63 − T383 was drawn smoothed with no "shown at T359" label. Drives the
// render panel's real message routing: the provider's `gridReady` must carry
// the combined map's note, and drop it again when Compare is turned off.
suite("Compare map band-limit label (#814)", () => {
  test("T63 − T383 is labelled shown at T359 of T383, and the label follows Compare", async () => {
    const provider = (await activateExtension()).provider;
    const native = loadNative();
    assert.ok(native, "native binding required");

    // Compare picks field B from the same file, so put both messages in one.
    const fixtures = path.join(
      vscode.extensions.getExtension(EXT_ID)!.extensionPath, "src", "test", "fixtures");
    const bytes = Buffer.concat([
      fs.readFileSync(path.join(fixtures, "spectral_simple_t63.grib2")),
      fs.readFileSync(path.join(fixtures, "spectral_simple_t383.grib2")),
    ]);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-compare-"));
    const file = path.join(dir, "t63_t383.grib2");
    fs.writeFileSync(file, bytes);
    const doc = (await provider.openCustomDocument(
      vscode.Uri.file(file),
      {} as vscode.CustomDocumentOpenContext,
      new vscode.CancellationTokenSource().token,
    )) as FieldglassDocument;
    // Resolving the editor caches the reader handle the render panel paints from.
    const editor = vscode.window.createWebviewPanel("fieldglass.viewer", "t63_t383", vscode.ViewColumn.One, {});
    await provider.resolveCustomEditor(doc, editor);

    const handle = native.Grib2Handle.fromBytes(bytes);
    const [t63, t383] = [handle.message(0), handle.message(1)];
    assert.strictEqual(t63.truncation, null, "T63 fits the 0.5° grid");
    assert.deepStrictEqual(t383.truncation, { declared: 383, truncatedTo: 359 });

    // Open a render panel on `meta` and return what it posts for one request.
    const open = (meta: MessageInfo) => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      let onMessage: ((m: any) => void) | undefined;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const posted: any[] = [];
      const fakePanel = {
        webview: {
          html: "",
          cspSource: "vscode-webview:",
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          onDidReceiveMessage: (cb: (m: any) => void) => {
            onMessage = cb;
            return { dispose: () => undefined };
          },
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          postMessage: (m: any) => {
            posted.push(m);
            return Promise.resolve(true);
          },
        },
        onDidDispose: () => ({ dispose: () => undefined }),
        dispose: () => undefined,
      };
      const origCreate = vscode.window.createWebviewPanel;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (vscode.window as any).createWebviewPanel = () => fakePanel;
      try {
        provider.openRenderPanel(doc, meta);
      } finally {
        vscode.window.createWebviewPanel = origCreate;
      }
      assert.ok(onMessage, "the render panel registers a message handler");
      const send = onMessage;
      return {
        html: () => fakePanel.webview.html,
        note: (type: string, compare?: { op: string; messageIndexB: number }) => {
          posted.length = 0;
          send({ type, projection: "source", ...(compare ? { compare } : {}) });
          const ready = posted.find((m) => m.type === "gridReady");
          assert.ok(ready, `a ${type} paints: ${JSON.stringify(posted.map((m) => m.error ?? m.type))}`);
          return ready.truncationNote;
        },
      };
    };

    try {
      const a = open(t63);
      // Field A alone: no label, and the panel opens with none.
      assert.ok(!a.html().includes("shown at T"), "T63 opens unlabelled");
      assert.strictEqual(a.note("ready"), null);
      // Compare on: field B's label, on the first paint and on a rerender.
      const compare = { op: "a_minus_b", messageIndexB: 1 };
      assert.strictEqual(a.note("ready", compare), "shown at T359 of T383");
      assert.strictEqual(a.note("rerenderRequest", compare), "shown at T359 of T383");
      // Compare off: the label goes with it.
      assert.strictEqual(a.note("rerenderRequest"), null);

      // Field A band-limited, B not: A's label stays through Compare.
      const b = open(t383);
      assert.strictEqual(b.note("ready"), "shown at T359 of T383");
      assert.strictEqual(
        b.note("rerenderRequest", { op: "a_minus_b", messageIndexB: 0 }),
        "shown at T359 of T383",
      );
    } finally {
      editor.dispose();
    }
  });

  // The webview can't run headlessly, so the subtitle the gridReady handler
  // writes, and the PNG export draws (`SUB_LINE`), is pinned through the
  // function the panel script embeds.
  test("the panel script recomposes its subtitle from each render's note", () => {
    assert.strictEqual(composeSubtitle("500 (hPa) Isobaric level · 2024-01-01", null), "500 (hPa) Isobaric level · 2024-01-01");
    assert.strictEqual(
      composeSubtitle("500 (hPa) Isobaric level", "shown at T359 of T383"),
      "500 (hPa) Isobaric level · shown at T359 of T383",
    );
    assert.strictEqual(composeSubtitle("", "shown at T359 of T383"), "shown at T359 of T383");
    assert.strictEqual(composeSubtitle("", null), "");

    const native = loadNative();
    assert.ok(native, "native binding required");
    const fixtures = path.join(
      vscode.extensions.getExtension(EXT_ID)!.extensionPath, "src", "test", "fixtures");
    const meta = native.Grib2Handle.fromBytes(
      fs.readFileSync(path.join(fixtures, "spectral_simple_t383.grib2"))).message(0);
    const html = renderImagePanelHtml({ cspSource: "" } as unknown as vscode.Webview, meta, "summary", [], []);
    assert.ok(html.includes("function composeSubtitle("), "the panel script defines composeSubtitle");
    assert.match(html, /SUB_LINE = composeSubtitle\(SUB_BASE, msg\.truncationNote \?\? null\)/);
    const subtitle = /<div class="subtitle">([^<]*)<\/div>/.exec(html);
    assert.ok(subtitle && subtitle[1].endsWith("shown at T359 of T383"), subtitle?.[1] ?? "no subtitle");
  });
});
