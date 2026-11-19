// The VS Code download both test runners share: `runTest.ts`, over the
// development layout, and `runPackagedTest.ts`, over an unzipped `.vsix` (#1869).

import { downloadAndUnzipVSCode } from "@vscode/test-electron";

/** Attempts at the VS Code download before giving up. */
const DOWNLOAD_ATTEMPTS = 3;

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
  let lastErr: unknown;
  for (let attempt = 1; attempt <= DOWNLOAD_ATTEMPTS; attempt++) {
    try {
      return await downloadAndUnzipVSCode();
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
