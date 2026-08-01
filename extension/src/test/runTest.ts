import * as fs from "fs";
import * as path from "path";
import { downloadAndUnzipVSCode, runTests } from "@vscode/test-electron";

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
async function downloadVSCodeWithRetry(): Promise<string> {
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

async function main(): Promise<void> {
  // The folder containing the extension's `package.json`. `@vscode/test-electron`
  // launches a fresh VS Code with `--extensionDevelopmentPath` pointing here so
  // the extension under test gets activated like a real install.
  const extensionDevelopmentPath = path.resolve(__dirname, "..", "..");

  // Path to the Mocha test runner (compiled). VS Code calls its `run()` export.
  const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");

  // VS Code keeps the test host's profile here between runs. CI always starts
  // from nothing, but a developer machine accumulates it — and a stale profile
  // can stop webviews mounting at all, which shows up as the probe panel never
  // posting `ready` and every webview test timing out. The profile holds no
  // state a test should depend on, so start each run from a clean one and keep
  // local runs honest against CI.
  const userDataDir = path.join(extensionDevelopmentPath, ".vscode-test", "user-data");
  fs.rmSync(userDataDir, { recursive: true, force: true });

  // Downloaded up front so a network fault is reported as one, and retried,
  // instead of surfacing as an opaque test-run failure. Its own catch keeps
  // the two apart in the log: this message means the network, the one below
  // means the tests.
  let vscodeExecutablePath: string;
  try {
    vscodeExecutablePath = await downloadVSCodeWithRetry();
  } catch (err) {
    console.error("Failed to download VS Code (network, not a test failure):", err);
    process.exit(1);
  }

  try {
    await runTests({
      vscodeExecutablePath,
      extensionDevelopmentPath,
      extensionTestsPath,
      // `--disable-extensions` keeps user-installed extensions out of the test
      // host so we exercise just the one under test. `--no-sandbox` avoids
      // namespace issues on minimal CI containers. `--user-data-dir` pins the
      // profile to the directory wiped above.
      launchArgs: [
        "--disable-extensions",
        "--no-sandbox",
        "--user-data-dir",
        userDataDir,
      ],
    });
  } catch (err) {
    console.error("Failed to run tests:", err);
    process.exit(1);
  }
}

main();
