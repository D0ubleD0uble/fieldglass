// The Mocha entry the test host calls for a packaged run (#1869). It runs only
// the smoke test `runPackagedTest.ts` copied next to the package, named by
// `FIELDGLASS_SMOKE_TESTS`, and nothing from the development layout.

import * as path from "path";
import Mocha from "mocha";

import { checkHostVersion } from "../host-version";

export async function run(): Promise<void> {
  checkHostVersion();
  const dir = process.env.FIELDGLASS_SMOKE_TESTS;
  if (!dir) throw new Error("FIELDGLASS_SMOKE_TESTS is not set: run this through runPackagedTest.js");
  const mocha = new Mocha({ ui: "tdd", color: true, timeout: 20_000 });
  mocha.addFile(path.join(dir, "packaged-smoke.test.js"));
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
