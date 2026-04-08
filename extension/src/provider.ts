import * as vscode from "vscode";
import * as path from "path";

// Loaded once on first use — avoids requiring at module load time so the
// extension can activate even if the .node file is missing (e.g. wrong platform).
let fieldglass: { detect: (filePath: string) => string } | undefined;

function loadNative(): typeof fieldglass {
  if (fieldglass) {
    return fieldglass;
  }
  // During development the .node lives next to the napi crate.
  // The path here will be updated when packaging for distribution.
  const nodePath = path.join(
    __dirname,
    "..",
    "..",
    "crates",
    "fieldglass-napi",
    "fieldglass.linux-x64-gnu.node"
  );
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    fieldglass = require(nodePath);
  } catch (err) {
    vscode.window.showErrorMessage(
      `Fieldglass: failed to load native module from ${nodePath}: ${err}`
    );
  }
  return fieldglass;
}

const FORMAT_LABELS: Record<string, string> = {
  grib1: "GRIB Edition 1",
  grib2: "GRIB Edition 2",
  netcdf: "NetCDF",
  unknown: "Unknown",
};

function renderHtml(format: string, filePath: string): string {
  const label = FORMAT_LABELS[format] ?? "Unknown";
  const filename = path.basename(filePath);
  const isKnown = format !== "unknown";

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Fieldglass</title>
  <style>
    body {
      font-family: var(--vscode-font-family);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 2rem;
      margin: 0;
    }
    h1 { font-size: 1.4rem; margin-bottom: 0.25rem; }
    .subtitle { color: var(--vscode-descriptionForeground); font-size: 0.9rem; margin-bottom: 2rem; }
    .card {
      border: 1px solid var(--vscode-panel-border);
      border-radius: 4px;
      padding: 1.25rem 1.5rem;
      max-width: 480px;
    }
    .badge {
      display: inline-block;
      padding: 0.2rem 0.6rem;
      border-radius: 3px;
      font-size: 0.8rem;
      font-weight: bold;
      margin-bottom: 1rem;
      background: ${isKnown ? "var(--vscode-badge-background)" : "var(--vscode-inputValidation-warningBackground)"};
      color: ${isKnown ? "var(--vscode-badge-foreground)" : "var(--vscode-inputValidation-warningForeground)"};
    }
    .status { font-size: 0.95rem; color: var(--vscode-descriptionForeground); }
  </style>
</head>
<body>
  <h1>Fieldglass</h1>
  <div class="subtitle">${filename}</div>
  <div class="card">
    <div class="badge">${label}</div>
    <div class="status">Parsing not yet implemented.</div>
  </div>
</body>
</html>`;
}

export class FieldglassEditorProvider
  implements vscode.CustomReadonlyEditorProvider
{
  public static readonly viewType = "fieldglass.viewer";
  public static readonly viewTypeAny = "fieldglass.viewer.any";

  public static register(context: vscode.ExtensionContext): vscode.Disposable[] {
    const provider = new FieldglassEditorProvider();
    const opts = { supportsMultipleEditorsPerDocument: true };
    return [
      vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewType, provider, opts),
      vscode.window.registerCustomEditorProvider(FieldglassEditorProvider.viewTypeAny, provider, opts),
    ];
  }

  public openCustomDocument(uri: vscode.Uri): vscode.CustomDocument {
    return { uri, dispose: () => {} };
  }

  public resolveCustomEditor(
    document: vscode.CustomDocument,
    webviewPanel: vscode.WebviewPanel
  ): void {
    const native = loadNative();
    const format = native ? native.detect(document.uri.fsPath) : "unknown";
    webviewPanel.webview.options = { enableScripts: false };
    webviewPanel.webview.html = renderHtml(format, document.uri.fsPath);
  }
}
