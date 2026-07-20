import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";

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
