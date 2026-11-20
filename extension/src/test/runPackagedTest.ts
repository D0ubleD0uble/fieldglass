// Run the smoke test against an unzipped `.vsix` rather than the development
// layout (#1869).
//
//   node out/test/runPackagedTest.js <unzipped .vsix>
//
// The argument is the folder the `.vsix` was unzipped into, or the `extension/`
// folder inside it. The test host loads the extension from there, with a fresh
// user-data folder and a fresh, empty extensions folder, so nothing from this
// checkout or from an earlier install can stand in for a file the package
// lacks.
//
// The compiled smoke test and its helper are copied into the package for the
// run, and removed after. VS Code gives each extension its own `vscode` API
// object, chosen by the path of the file that asks for it, and the test
// replaces calls on that object (the webview it records, the folder dialog it
// answers). A test file outside the package would get a different object, and
// its replacements would never reach the extension. Only test files are
// added: the package's own files are left as they are.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { runTests } from "@vscode/test-electron";

import { checkCacheOutside, downloadVSCodeWithRetry } from "./vscode-download";

/** The compiled files the smoke test needs, from `out/test/suite/`. */
const SMOKE_FILES = ["packaged-smoke.test.js", "real-panel.js"];

/** Where in the package they are copied for the run. */
const SMOKE_DIR = path.join("out", "test", "packaged-smoke");

function packagedExtension(arg: string | undefined): string {
  if (!arg) {
    console.error("usage: node out/test/runPackagedTest.js <unzipped .vsix folder>");
    process.exit(2);
  }
  const given = path.resolve(arg);
  const inner = path.join(given, "extension");
  const dir = fs.existsSync(path.join(inner, "package.json")) ? inner : given;
  if (!fs.existsSync(path.join(dir, "package.json"))) {
    console.error(`no package.json in ${given} or ${inner}: is this an unzipped .vsix?`);
    process.exit(2);
  }
  return fs.realpathSync(dir);
}

async function main(): Promise<void> {
  // This checkout's `extension/` folder: the fixtures, and the compiled tests.
  const checkout = path.resolve(__dirname, "..", "..");
  const extensionDevelopmentPath = packagedExtension(process.argv[2]);
  if (extensionDevelopmentPath === fs.realpathSync(checkout)) {
    console.error("that is the development layout, not an unzipped .vsix");
    process.exit(2);
  }

  const smokeDir = path.join(extensionDevelopmentPath, SMOKE_DIR);
  fs.rmSync(smokeDir, { recursive: true, force: true });
  fs.mkdirSync(smokeDir, { recursive: true });
  for (const name of SMOKE_FILES) {
    fs.copyFileSync(path.join(__dirname, "suite", name), path.join(smokeDir, name));
  }

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-packaged-"));
  const userDataDir = path.join(scratch, "user-data");
  const extensionsDir = path.join(scratch, "extensions");
  fs.mkdirSync(extensionsDir);

  checkCacheOutside(extensionDevelopmentPath);

  let vscodeExecutablePath: string;
  try {
    vscodeExecutablePath = await downloadVSCodeWithRetry();
  } catch (err) {
    console.error("Failed to download VS Code (network, not a test failure):", err);
    process.exit(1);
  }

  let failed = false;
  try {
    await runTests({
      vscodeExecutablePath,
      extensionDevelopmentPath,
      extensionTestsPath: path.resolve(__dirname, "packaged", "index.js"),
      extensionTestsEnv: {
        FIELDGLASS_PACKAGED_EXTENSION: extensionDevelopmentPath,
        FIELDGLASS_SMOKE_CHECKOUT: checkout,
        FIELDGLASS_SMOKE_TESTS: smokeDir,
      },
      launchArgs: [
        "--disable-extensions",
        "--no-sandbox",
        "--user-data-dir",
        userDataDir,
        "--extensions-dir",
        extensionsDir,
      ],
    });
  } catch (err) {
    console.error("Failed to run tests:", err);
    failed = true;
  } finally {
    fs.rmSync(smokeDir, { recursive: true, force: true });
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  if (failed) process.exit(1);
}

main();
