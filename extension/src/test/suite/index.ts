import * as fs from "fs";
import * as path from "path";
import Mocha from "mocha";

import { checkHostVersion } from "../host-version";

/**
 * Every compiled `*.test.js` under `dir`, at any depth, in a stable order.
 *
 * A plain walk rather than the `glob` package, whose current releases need a
 * newer Node than the one VS Code 1.85 runs extensions on (18.15), and 1.85
 * is the `engines.vscode` floor the release tests on (#1877).
 */
function testFiles(dir: string): string[] {
  const found: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) found.push(...testFiles(full));
    else if (entry.isFile() && entry.name.endsWith(".test.js")) found.push(full);
  }
  return found.sort();
}

export async function run(): Promise<void> {
  checkHostVersion();
  const mocha = new Mocha({
    ui: "tdd",
    color: true,
    timeout: 20_000,
    // `MOCHA_GREP="#1247" xvfb-run -a npm test` runs only the tests whose
    // title matches; unset, every test runs.
    grep: process.env.MOCHA_GREP,
  });

  for (const file of testFiles(path.resolve(__dirname))) {
    mocha.addFile(file);
  }

  return new Promise((resolve, reject) => {
    try {
      mocha.run((failures) => {
        if (failures > 0) {
          reject(new Error(`${failures} test(s) failed`));
        } else {
          resolve();
        }
      });
    } catch (err) {
      reject(err);
    }
  });
}
