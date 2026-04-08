import * as vscode from "vscode";
import * as path from "path";

// Loaded once on first use — avoids requiring at module load time so the
// extension can activate even if the .node file is missing (e.g. wrong platform).
interface MessageMeta {
  messageIndex: number;
  offsetBytes: number;
  parameterName: string;
  parameterUnits: string;
  parameterAbbreviation: string;
  levelType: string;
  levelValue: number;
  referenceTime: string;
  forecastHours: number;
  originatingCentre: string;
  gridType: string | null;
  gridNi: number | null;
  gridNj: number | null;
  latFirst: number | null;
  lonFirst: number | null;
  latLast: number | null;
  lonLast: number | null;
  format: string;
}

let fieldglass: {
  detectBytes: (bytes: Uint8Array) => string;
  openGrib1: (bytes: Uint8Array) => MessageMeta[];
} | undefined;

function nativeBinaryName(): string {
  const platform = process.platform;  // 'linux' | 'win32' | 'darwin'
  const arch = process.arch;          // 'x64' | 'arm64'
  const abi = platform === "linux" ? "-gnu"
            : platform === "win32" ? "-msvc"
            : "";                     // macOS has no ABI suffix
  return `fieldglass.${platform}-${arch}${abi}.node`;
}

function loadNative(): typeof fieldglass {
  if (fieldglass) {
    return fieldglass;
  }
  // Binaries live in extension/bin/ — populated by `napi build --output-dir`
  // during development and bundled into the .vsix for distribution.
  const nodePath = path.join(__dirname, "..", "bin", nativeBinaryName());
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    fieldglass = require(nodePath);
  } catch (err) {
    vscode.window.showErrorMessage(
      `Fieldglass: failed to load native module (${nativeBinaryName()}): ${err}`
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

function renderHtml(format: string, filePath: string, messages?: MessageMeta[], headerBytes?: Uint8Array): string {
  const label = FORMAT_LABELS[format] ?? "Unknown";
  const filename = path.basename(filePath);
  const isKnown = format !== "unknown";

  let bodyContent = "";

  if (messages && messages.length > 0) {
    const fmt1 = (v: number | null) => v !== null ? v.toFixed(3) : "—";
    const rows = messages.map((m) => {
      const gridDims = (m.gridNi !== null && m.gridNj !== null)
        ? `${m.gridNi}×${m.gridNj}` : "—";
      const gridBounds = (m.latFirst !== null && m.lonFirst !== null)
        ? `${fmt1(m.latFirst)},${fmt1(m.lonFirst)} → ${fmt1(m.latLast)},${fmt1(m.lonLast)}` : "—";
      return `
      <tr>
        <td>${m.messageIndex}</td>
        <td>${m.parameterName}</td>
        <td>${m.parameterAbbreviation}</td>
        <td>${m.parameterUnits}</td>
        <td>${m.levelType}</td>
        <td>${m.levelValue}</td>
        <td>${m.referenceTime}</td>
        <td>${m.forecastHours}h</td>
        <td>${m.originatingCentre}</td>
        <td>${m.gridType ?? "—"}</td>
        <td>${gridDims}</td>
        <td>${gridBounds}</td>
      </tr>`;
    }).join("");
    bodyContent = `
    <table>
      <thead>
        <tr>
          <th>#</th><th>Parameter</th><th>Abbrev</th><th>Units</th>
          <th>Level Type</th><th>Level</th><th>Reference Time</th><th>Fcst</th>
          <th>Centre</th><th>Grid</th><th>Size</th><th>Bounds (lat,lon)</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>`;
  } else if (!isKnown && headerBytes && headerBytes.length > 0) {
    const hex = Array.from(headerBytes)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join(" ");
    const ascii = Array.from(headerBytes)
      .map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."))
      .join("");
    bodyContent = `
    <div class="header-dump">
      <div class="dump-label">First ${headerBytes.length} bytes</div>
      <code class="hex">${hex}</code>
      <code class="ascii">${ascii}</code>
    </div>`;
  } else {
    bodyContent = `<div class="status">No messages found.</div>`;
  }

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
    table { border-collapse: collapse; font-size: 0.85rem; width: 100%; }
    th, td { text-align: left; padding: 0.3rem 0.6rem; border-bottom: 1px solid var(--vscode-panel-border); white-space: nowrap; }
    th { color: var(--vscode-descriptionForeground); font-weight: 600; }
    tr:hover td { background: var(--vscode-list-hoverBackground); }
    .header-dump { margin-top: 1rem; }
    .dump-label { font-size: 0.8rem; color: var(--vscode-descriptionForeground); margin-bottom: 0.25rem; }
    code { display: block; font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85rem; }
    .ascii { color: var(--vscode-descriptionForeground); margin-top: 0.2rem; }
  </style>
</head>
<body>
  <h1>Fieldglass</h1>
  <div class="subtitle">${filename}</div>
  <div class="badge">${label}</div>
  ${bodyContent}
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

  public async resolveCustomEditor(
    document: vscode.CustomDocument,
    webviewPanel: vscode.WebviewPanel
  ): Promise<void> {
    const native = loadNative();
    const fileData = await vscode.workspace.fs.readFile(document.uri);
    const header = fileData.slice(0, 32);
    const format = native ? native.detectBytes(header) : "unknown";
    console.log(`[Fieldglass] uri=${document.uri} format=${format} native=${!!native}`);

    let messages: MessageMeta[] | undefined;
    let headerBytes: Uint8Array | undefined;

    if (native && format === "grib1") {
      messages = native.openGrib1(fileData);
    } else if (format === "unknown") {
      headerBytes = header;
    }

    webviewPanel.webview.options = { enableScripts: false };
    webviewPanel.webview.html = renderHtml(format, document.uri.fsPath, messages, headerBytes);
  }
}
