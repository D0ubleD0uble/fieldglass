// Run inside the test host by both Mocha entries, `suite/index.ts` and
// `packaged/index.ts` (#1877).

import * as vscode from "vscode";

/**
 * Fail when the test host is not the VS Code version the run asked for.
 *
 * `FIELDGLASS_VSCODE_VERSION` picks the build `vscode-download.ts` fetches, and
 * the test host inherits it. When it names an exact version, the running VS
 * Code must be that version, so a run meant for the `engines` floor that
 * quietly got the latest release fails instead of passing as a floor run.
 * `stable`, `insiders` or unset name no fixed version and are not compared.
 */
export function checkHostVersion(): void {
  const want = process.env.FIELDGLASS_VSCODE_VERSION?.trim();
  console.log(`Test host: VS Code ${vscode.version}`);
  if (want && /^\d+\.\d+\.\d+$/.test(want) && vscode.version !== want) {
    throw new Error(`the test host is VS Code ${vscode.version}, not the requested ${want}`);
  }
}
