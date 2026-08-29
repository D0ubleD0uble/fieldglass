// The file associations the viewer claims, pinned against the manifest.
//
// Nothing else in the suite covers this. Every other custom-editor test calls
// `provider.resolveCustomEditor` directly, which proves the provider builds a
// panel but skips the layer that decides *whether a given file reaches the
// provider at all*. A pattern dropped from `contributes.customEditors` would
// leave all of those tests green while the file it names stopped opening in
// Fieldglass — the user would get the binary-file notice and have to reach for
// "Reopen Editor With…" instead.
//
// So this asserts the contribution itself: the extensions the reader decodes
// are claimed at `default` priority, and the catch-all stays at `option` so it
// offers Fieldglass for anything else without hijacking every file in the
// editor.

import * as assert from "assert";
import * as vscode from "vscode";

const EXT_ID = "fieldglass.fieldglass";

/** Every extension the reader decodes and the viewer should own outright.
 *  GRIB in both spellings and both editions, NetCDF in all three. */
const CLAIMED = [
  "*.grb",
  "*.grib",
  "*.grib1",
  "*.grb1",
  "*.grb2",
  "*.grib2",
  "*.nc",
  "*.nc4",
  "*.netcdf",
];

interface CustomEditorContribution {
  viewType: string;
  priority?: string;
  selector: { filenamePattern?: string }[];
}

function customEditors(): CustomEditorContribution[] {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  const editors = ext.packageJSON?.contributes?.customEditors;
  assert.ok(Array.isArray(editors), "the manifest must contribute custom editors");
  return editors as CustomEditorContribution[];
}

suite("File associations", () => {
  test("every decodable extension is claimed at default priority", () => {
    const primary = customEditors().find((e) => e.viewType === "fieldglass.viewer");
    assert.ok(primary, "the primary viewer contribution must exist");
    assert.strictEqual(
      primary.priority ?? "default",
      "default",
      "at option priority these files open as binary text until the user reopens them",
    );

    const patterns = primary.selector.map((s) => s.filenamePattern);
    for (const want of CLAIMED) {
      assert.ok(
        patterns.includes(want),
        `${want} is no longer claimed; files matching it will not open in Fieldglass`,
      );
    }
  });

  // The catch-all is how a file the reader can decode but does not name — an
  // HDF5 `.h5`, or a GRIB file with no extension at all — is still openable,
  // via "Reopen Editor With…". It must stay `option`: at `default` it would
  // take over every file in the workspace, source included.
  test("the catch-all viewer stays opt-in", () => {
    const any = customEditors().find((e) => e.viewType === "fieldglass.viewer.any");
    assert.ok(any, "the catch-all viewer contribution must exist");
    assert.strictEqual(any.priority, "option", "a default-priority '*' would hijack every file");
    assert.deepStrictEqual(
      any.selector.map((s) => s.filenamePattern),
      ["*"],
      "the catch-all must match everything, and only via the reopen picker",
    );
  });

  test("the viewer registers both contributed view types", async () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, "extension is installed in the test host");
    await ext.activate();
    assert.ok(ext.isActive, "the extension must activate before an editor can resolve");
    for (const editor of customEditors()) {
      assert.ok(
        typeof editor.viewType === "string" && editor.viewType.startsWith("fieldglass."),
        `unexpected view type ${editor.viewType}`,
      );
    }
  });
});
