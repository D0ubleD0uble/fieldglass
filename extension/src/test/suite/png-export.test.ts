// Tests for the render-panel PNG export (#243). The compositing itself is
// canvas work in the webview (not reachable headlessly), so these pin the two
// host-side pieces: the filename sanitiser and the provider handler that
// decodes the data URL and writes exactly those bytes.

import * as assert from "assert";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";
import { sanitizePngName } from "../../render-panel";

// A minimal valid 1×1 PNG.
const PNG_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

function fixturePath(name: string): string {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return path.join(ext.extensionPath, "src", "test", "fixtures", name);
}

async function providerAndDoc(): Promise<{
  provider: FieldglassEditorProvider;
  doc: FieldglassDocument;
}> {
  const ext = vscode.extensions.getExtension<FieldglassApi>("fieldglass.fieldglass");
  assert.ok(ext);
  const provider = (await ext.activate()).provider;
  const uri = vscode.Uri.file(fixturePath("regular_latlon_surface.grib2"));
  const doc = await provider.openCustomDocument(
    uri,
    {} as vscode.CustomDocumentOpenContext,
    new vscode.CancellationTokenSource().token,
  );
  return { provider, doc };
}

suite("Export PNG", () => {
  test("sanitizePngName strips paths and forces a lowercase .png basename", () => {
    assert.strictEqual(sanitizePngName("../../etc/passwd"), "passwd.png");
    assert.strictEqual(sanitizePngName("Temp 2m (K).png"), "temp-2m-k.png");
    assert.strictEqual(sanitizePngName("a/b/c/My Field"), "my-field.png");
    assert.strictEqual(sanitizePngName(""), "render.png");
    assert.strictEqual(sanitizePngName("...png"), "render.png");
  });

  test("handleExportPng decodes the data URL and writes exactly those bytes", async () => {
    const { provider, doc } = await providerAndDoc();
    // `vscode.workspace.fs.writeFile` is read-only, so we let the handler write
    // to a real temp file and read it back rather than stubbing the write.
    const dest = vscode.Uri.file(path.join(os.tmpdir(), "fieldglass-png-export-test.png"));

    const origSave = vscode.window.showSaveDialog;
    const origInfo = vscode.window.showInformationMessage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(dest);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showInformationMessage = () => Promise.resolve(undefined);
    try {
      await provider.handleExportPng(doc, {
        dataUrl: "data:image/png;base64," + PNG_B64,
        defaultName: "field.png",
      });
      const written = await vscode.workspace.fs.readFile(dest);
      assert.deepStrictEqual(
        Buffer.from(written),
        Buffer.from(PNG_B64, "base64"),
        "written bytes equal the decoded PNG",
      );
    } finally {
      vscode.window.showSaveDialog = origSave;
      vscode.window.showInformationMessage = origInfo;
      await vscode.workspace.fs.delete(dest).then(undefined, () => undefined);
    }
  });

  // The panel shows "Exporting PNG…" the instant it hands the image over and
  // has no other way to learn what happened — the save dialog and the result
  // notification both live host-side. Every path must therefore report an
  // outcome, or that status sits pending for good and a completed export is
  // indistinguishable from one still running.
  test("handleExportPng reports saved with the path it wrote", async () => {
    const { provider, doc } = await providerAndDoc();
    const dest = vscode.Uri.file(path.join(os.tmpdir(), "fieldglass-png-outcome-saved.png"));
    const origSave = vscode.window.showSaveDialog;
    const origInfo = vscode.window.showInformationMessage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(dest);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showInformationMessage = () => Promise.resolve(undefined);
    try {
      const outcome = await provider.handleExportPng(doc, {
        dataUrl: "data:image/png;base64," + PNG_B64,
        defaultName: "field.png",
      });
      assert.strictEqual(outcome.status, "saved");
      assert.strictEqual(
        outcome.status === "saved" ? outcome.path : "",
        dest.fsPath,
        "the outcome carries the path so the panel can name it",
      );
    } finally {
      vscode.window.showSaveDialog = origSave;
      vscode.window.showInformationMessage = origInfo;
      await vscode.workspace.fs.delete(dest).then(undefined, () => undefined);
    }
  });

  test("handleExportPng reports cancelled when the save dialog is dismissed", async () => {
    const { provider, doc } = await providerAndDoc();
    const origSave = vscode.window.showSaveDialog;
    const origErr = vscode.window.showErrorMessage;
    let erred = false;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showErrorMessage = () => {
      erred = true;
      return Promise.resolve(undefined);
    };
    try {
      const outcome = await provider.handleExportPng(doc, {
        dataUrl: "data:image/png;base64," + PNG_B64,
      });
      assert.strictEqual(outcome.status, "cancelled");
      assert.ok(!erred, "dismissing the dialog is not an error");
    } finally {
      vscode.window.showSaveDialog = origSave;
      vscode.window.showErrorMessage = origErr;
    }
  });

  test("handleExportPng reports failed with a reason for a bad data URL", async () => {
    const { provider, doc } = await providerAndDoc();
    const origErr = vscode.window.showErrorMessage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showErrorMessage = () => Promise.resolve(undefined);
    try {
      const outcome = await provider.handleExportPng(doc, { dataUrl: "not a data url" });
      assert.strictEqual(outcome.status, "failed");
      assert.ok(
        outcome.status === "failed" && outcome.reason.length > 0,
        "a failure must carry a reason the panel can show",
      );
    } finally {
      vscode.window.showErrorMessage = origErr;
    }
  });

  test("handleExportPng rejects a non-PNG data URL without reaching the save dialog", async () => {
    const { provider, doc } = await providerAndDoc();
    const origErr = vscode.window.showErrorMessage;
    const origSave = vscode.window.showSaveDialog;
    let shown = "";
    let savePrompted = false;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showErrorMessage = (msg: string) => {
      shown = msg;
      return Promise.resolve(undefined);
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => {
      savePrompted = true;
      return Promise.resolve(undefined);
    };
    try {
      await provider.handleExportPng(doc, { dataUrl: "not a data url" });
    } finally {
      vscode.window.showErrorMessage = origErr;
      vscode.window.showSaveDialog = origSave;
    }
    assert.match(shown, /PNG/);
    assert.strictEqual(savePrompted, false, "an invalid image never reaches the save dialog");
  });
});
