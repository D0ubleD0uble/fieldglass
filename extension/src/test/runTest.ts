import * as path from "path";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
  // The folder containing the extension's `package.json`. `@vscode/test-electron`
  // launches a fresh VS Code with `--extensionDevelopmentPath` pointing here so
  // the extension under test gets activated like a real install.
  const extensionDevelopmentPath = path.resolve(__dirname, "..", "..");

  // Path to the Mocha test runner (compiled). VS Code calls its `run()` export.
  const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");

  try {
    await runTests({
      extensionDevelopmentPath,
      extensionTestsPath,
      // `--disable-extensions` keeps user-installed extensions out of the test
      // host so we exercise just the one under test. `--no-sandbox` avoids
      // namespace issues on minimal CI containers.
      launchArgs: ["--disable-extensions", "--no-sandbox"],
    });
  } catch (err) {
    console.error("Failed to run tests:", err);
    process.exit(1);
  }
}

main();
