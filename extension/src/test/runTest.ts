import * as fs from "fs";
import * as path from "path";
import { runTests } from "@vscode/test-electron";

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

  try {
    await runTests({
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
