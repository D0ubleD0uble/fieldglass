// Tests for the render-panel PNG export (#243). The compositing itself is
// canvas work in the webview (not reachable headlessly), so these pin the
// host-side pieces: the filename sanitiser, the provider handler that decodes
// the data URL and writes exactly those bytes, and — the seam that had no
// coverage at all — that each render panel actually *routes* the webview's
// `exportPng` message to that handler. Calling the handler directly passes
// whether or not any panel is wired to it, which is how the GRIB panel shipped
// with a dead button.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";
import { loadNative, type MessageMeta } from "../../native";
import { exportCanvasWidth, sanitizePngName } from "../../render-panel";

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
  });

  test("sanitizePngName caps a very long parameter name", () => {
    // The WMO master tables (#415) brought names up to 127 characters, which
    // the default export name is built from.
    const longest = "Shape factor with respect to temperature profile in thermocline";
    const name = sanitizePngName(`${longest}-message-3.png`);
    assert.ok(name.endsWith(".png"));
    assert.ok(
      name.length <= 100,
      `expected a capped name, got ${name.length} characters: ${name}`,
    );
    assert.ok(name.startsWith("shape-factor-with-respect-to-temperature"));
    // The cut must not leave a dangling separator before the extension.
    assert.ok(!/[-.]\.png$/.test(name), `dangling separator in ${name}`);
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

  // The export canvas was sized from the map block alone (raster + colorbar +
  // labels), so a field whose raster is narrower than its own header had the
  // title and reference time clipped at the right edge. The synthesized
  // spectral grid is 256 px wide and its subtitle ~456 px, which is where this
  // showed up; a 1440-px GFS raster hides it completely.
  const EXPORT_MARGIN = 14;
  const mapBlock = (rasterW: number) =>
    EXPORT_MARGIN + rasterW + 16 + 18 + 64 + EXPORT_MARGIN;

  test("exportCanvasWidth widens the canvas when the header outruns the map block", () => {
    const block = mapBlock(256);
    assert.strictEqual(block, 382, "spectral T63 map block");
    const width = exportCanvasWidth(block, 456, EXPORT_MARGIN);
    assert.ok(width > block, "a header wider than the map block must widen the canvas");
    assert.strictEqual(width, EXPORT_MARGIN + 456 + EXPORT_MARGIN, "and fit it with both margins");
  });

  test("exportCanvasWidth leaves a map block that already fits its header", () => {
    const block = mapBlock(1440); // GFS 0.25°
    assert.strictEqual(exportCanvasWidth(block, 456, EXPORT_MARGIN), block);
  });

  // The message-routing seam. Every test above calls `handleExportPng`
  // directly, so all six passed while the GRIB panel dropped the message on
  // the floor: the button composited an image, posted it, and nothing
  // listened — no save dialog, and no `exportPngDone` to clear the panel's
  // "Exporting PNG…" status. Drive the registered handler instead.
  test("the GRIB render panel routes exportPng to the handler", async () => {
    const { provider, doc } = await providerAndDoc();
    const native = loadNative();
    assert.ok(native, "native binding required");
    const bytes = fs.readFileSync(fixturePath("regular_latlon_surface.grib2"));
    const meta: MessageMeta = native.Grib2Handle.fromBytes(bytes).messages()[0];

    // A stand-in panel that records what the provider registers and posts.
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

    const dest = vscode.Uri.file(path.join(os.tmpdir(), "fieldglass-png-route-test.png"));
    const origCreate = vscode.window.createWebviewPanel;
    const origSave = vscode.window.showSaveDialog;
    const origInfo = vscode.window.showInformationMessage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).createWebviewPanel = () => fakePanel;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(dest);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showInformationMessage = () => Promise.resolve(undefined);
    try {
      provider.openRenderPanel(doc, meta);
      assert.ok(onMessage, "the render panel registers a message handler");

      onMessage({
        type: "exportPng",
        dataUrl: "data:image/png;base64," + PNG_B64,
        defaultName: "field.png",
      });
      // The handler is async; wait for the reply the panel needs to clear its
      // status rather than for a fixed delay.
      for (let i = 0; i < 200 && !posted.some((m) => m.type === "exportPngDone"); i++) {
        await new Promise((r) => setTimeout(r, 10));
      }
    } finally {
      vscode.window.createWebviewPanel = origCreate;
      vscode.window.showSaveDialog = origSave;
      vscode.window.showInformationMessage = origInfo;
    }

    const done = posted.find((m) => m.type === "exportPngDone");
    assert.ok(done, "the panel is told how the export ended");
    assert.strictEqual(done.outcome.status, "saved");
    assert.strictEqual(done.outcome.path, dest.fsPath);
    assert.ok(fs.existsSync(dest.fsPath), "the PNG was written");
    assert.deepStrictEqual(
      fs.readFileSync(dest.fsPath),
      Buffer.from(PNG_B64, "base64"),
      "the bytes the webview composited are the bytes on disk",
    );
    fs.unlinkSync(dest.fsPath);
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
