// The VS Code download both test runners share: `runTest.ts`, over the
// development layout, and `runPackagedTest.ts`, over an unzipped `.vsix` (#1869).

import * as path from "path";
import { downloadAndUnzipVSCode } from "@vscode/test-electron";

/**
 * Where the VS Code builds are kept: `.vscode-test/` at the repository root,
 * beside `extension/` rather than inside it (#1877).
 *
 * VS Code 1.85 takes a color theme found under the extension development path
 * for the extension's own and holds the window to the first one. With the
 * builds inside `extension/`, every built-in theme counted: the window stayed
 * on Abyss, and a change of `workbench.colorTheme` did nothing. Newer
 * releases do not do this, which is why the theme tests passed there.
 */
export const VSCODE_CACHE = path.resolve(__dirname, "..", "..", "..", ".vscode-test");

/** Fail unless the VS Code builds are kept outside `extensionDevelopmentPath`. */
export function checkCacheOutside(extensionDevelopmentPath: string): void {
  const rel = path.relative(path.resolve(extensionDevelopmentPath), VSCODE_CACHE);
  if (rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel))) {
    throw new Error(
      `the VS Code builds in ${VSCODE_CACHE} are inside the extension under test, ${extensionDevelopmentPath}, ` +
        "so VS Code would take their themes for the extension's own",
    );
  }
}

/** Attempts at the VS Code download before giving up. */
const DOWNLOAD_ATTEMPTS = 3;

/**
 * The VS Code version the test host runs: `FIELDGLASS_VSCODE_VERSION`, or the
 * latest stable release when it is unset or empty (#1877).
 *
 * It lets a run test the `engines.vscode` floor, 1.85.0, as well as the
 * newest release. Any value `@vscode/test-electron` accepts works: an exact
 * version, `stable` or `insiders`.
 */
export function requestedVSCodeVersion(): string | undefined {
  const version = process.env.FIELDGLASS_VSCODE_VERSION?.trim();
  return version ? version : undefined;
}

/**
 * Fetch the VS Code build the test host runs, retrying the download only.
 *
 * `runTests` would download it implicitly, but then a network fault and a
 * failing test are the same failure: the run reports "Failed to run tests"
 * either way. That is not hypothetical — a CDN timeout here has already left a
 * green PR sitting unmerged, because auto-merge waits on pending checks but
 * never retries a failed one.
 *
 * So the download is pulled out and retried on its own, and the executable is
 * handed to `runTests` explicitly. Retrying *only* this keeps a genuinely
 * failing test failing on the first attempt — a retry around the tests
 * themselves would paper over real flakiness. The download is idempotent and
 * skips itself once `.vscode-test/vscode-<version>` exists, so a retry after a
 * partial failure costs nothing.
 */
export async function downloadVSCodeWithRetry(): Promise<string> {
  const version = requestedVSCodeVersion();
  console.log(`Testing on VS Code ${version ?? "stable"}`);
  let lastErr: unknown;
  for (let attempt = 1; attempt <= DOWNLOAD_ATTEMPTS; attempt++) {
    try {
      return await downloadAndUnzipVSCode({ version, cachePath: VSCODE_CACHE });
    } catch (err) {
      lastErr = err;
      if (attempt < DOWNLOAD_ATTEMPTS) {
        // Linear backoff: these faults are registry/CDN timeouts lasting
        // seconds, not rate limits needing a long cool-off.
        const delayMs = attempt * 5000;
        console.warn(
          `VS Code download failed (attempt ${attempt}/${DOWNLOAD_ATTEMPTS}), ` +
            `retrying in ${delayMs / 1000}s:`,
          err,
        );
        await new Promise((resolve) => setTimeout(resolve, delayMs));
      }
    }
  }
  throw new Error(
    `VS Code download failed after ${DOWNLOAD_ATTEMPTS} attempts: ${String(lastErr)}`,
  );
}
