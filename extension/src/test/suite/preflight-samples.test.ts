// `tools/preflight_samples.js` (#1209): the headless pre-flight the release
// pass runs over the sample corpus. It is plain JavaScript that tsc and CI
// never see, so it read a renamed addon field (`variableIndex`, now `index`)
// and reported RENDER FAILED for every NetCDF sample. Run its real code over
// committed fixtures, through the real addon, so a rename fails here.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

const EXT_ID = "fieldglass.fieldglass";

interface Preflight {
  preflightFile(bytes: Uint8Array, label: string): string[];
}

function run(name: string): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  const script = path.join(ext.extensionPath, "..", "tools", "preflight_samples.js");
  // eslint-disable-next-line security/detect-non-literal-require, @typescript-eslint/no-require-imports
  const { preflightFile } = require(script) as Preflight;
  const bytes = fs.readFileSync(path.join(ext.extensionPath, "src", "test", "fixtures", name));
  return preflightFile(bytes, name).join("\n");
}

suite("preflight_samples.js", () => {
  for (const name of ["netcdf4_dimscale.nc", "ersst_v5_187001_cdf1.nc", "regular_latlon_surface.grib2"]) {
    test(`renders ${name}`, () => {
      const out = run(name);
      assert.ok(!/FAILED/.test(out), out);
      assert.match(out, /render: \d+x\d+/);
    });
  }
});
