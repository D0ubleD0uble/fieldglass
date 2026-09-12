// The "Open Zarr Store…" command, and the panel behind it (#659).
//
// A Zarr store is a **directory**, and `registerCustomEditorProvider` binds to
// file patterns — so a folder has no editor path to arrive by and the entry point
// has to be a command. That is the design decision this file pins, along with the
// two behaviours the Rust gates cannot see: that the command is actually
// registered under the id `package.json` contributes, and that a folder which is
// not a store is *reported* rather than opened as an empty editor.
//
// The store used is the committed oracle one — the same directory
// `fieldglass-zarr`'s own tests read, written by `tools/build_zarr_fixtures.py`
// from zarr-python. Reached by a path relative to the extension rather than
// copied in, because a CF store hand-built here would be a second, unverified
// implementation of that generator.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { FieldglassEditorProvider } from "../../provider";
import type { FieldglassApi } from "../../extension";

const EXT_ID = "fieldglass.fieldglass";

/** The repo root, from the extension's own directory. */
function repoRoot(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return path.join(ext.extensionPath, "..");
}

/** A committed Zarr store. Asserted to exist, so a moved fixture fails loudly
 *  rather than quietly skipping the only test of this path. */
function storePath(name: string): string {
  const p = path.join(repoRoot(), "crates", "fieldglass-zarr", "tests", "fixtures", "stores", name);
  assert.ok(fs.existsSync(p), `fixture store missing: ${p}`);
  return p;
}

async function provider(): Promise<FieldglassEditorProvider> {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  assert.ok(ext, "extension is installed");
  const api = await ext.activate();
  return api.provider;
}

suite("Zarr store", () => {
  test("the command package.json contributes is the one registered", async () => {
    await provider();
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes(FieldglassEditorProvider.openStoreCommand),
      `${FieldglassEditorProvider.openStoreCommand} is not registered`,
    );

    // And it is the id the manifest promises. A mismatch would leave the palette
    // entry pointing at nothing — which is the failure mode a `package.json`
    // string and a `registerCommand` string can have between them.
    const manifest = JSON.parse(
      fs.readFileSync(path.join(repoRoot(), "extension", "package.json"), "utf8"),
    ) as { contributes?: { commands?: { command: string; title: string }[] } };
    const contributed = manifest.contributes?.commands ?? [];
    assert.deepStrictEqual(
      contributed.map((c) => c.command),
      [FieldglassEditorProvider.openStoreCommand],
      "the manifest and the registration disagree",
    );
    assert.match(contributed[0].title, /Zarr/, "the palette entry should say Zarr");
  });

  test("detection is by metadata, not by the .zarr suffix", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");

    // Every committed store, none of which is named `*.zarr`.
    for (const name of ["v2_nested", "v2_slash", "v3_nested", "cf_v2", "cf_v3"]) {
      assert.ok(native.ZarrHandle.isStore(storePath(name)), `${name} is a store`);
    }
    // The directory that holds them is not one, and neither is a file.
    assert.ok(
      !native.ZarrHandle.isStore(path.join(repoRoot(), "crates", "fieldglass-zarr", "tests", "fixtures")),
      "a directory of stores is not a store",
    );
    assert.ok(!native.ZarrHandle.isStore(path.join(repoRoot(), "README.md")));
  });

  test("a store lists its variables and renders a slice", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.ZarrHandle.fromDirectory(storePath("cf_v3"));

    const variables = handle.variables();
    assert.ok(variables.length > 0, "the store lists variables");
    const t = variables.find((v) => v.name === "t");
    assert.ok(t, "the packed array is listed");
    // `undefined`, not `null` — napi maps Rust `None` that way, and a strict
    // `!== null` guard on these fails *open* (#288). Nullish is the rule.
    assert.ok(t.detectedYDim != null, "a latitude axis was detected");
    assert.ok(t.detectedXDim != null, "a longitude axis was detected");

    const grid = handle.renderSlice(
      t.variableIndex,
      t.detectedYDim,
      t.detectedXDim,
      t.dims.map(() => 0),
      {
        projection: "equirectangular",
        resampling: "nearest",
        flipY: false,
        colormap: "viridis",
      },
    );
    assert.ok(grid.width > 0 && grid.height > 0, "a raster was produced");
    assert.strictEqual(grid.rgba.length, grid.width * grid.height * 4);
    assert.ok(grid.usedMax > grid.usedMin, "a real range was resolved");
    // Physical units, so the CF scale/offset ran: the fixture is packed int16
    // around 240-290 K.
    assert.ok(grid.usedMin > 100 && grid.usedMax < 400, `range looks unscaled: ${grid.usedMin}..${grid.usedMax}`);
  });

  test("a folder that is not a store is reported, not opened", async () => {
    const p = await provider();
    const notAStore = vscode.Uri.file(path.join(repoRoot(), "crates", "fieldglass-zarr", "tests", "fixtures"));

    let shown: string | undefined;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const realError = (vscode.window as any).showErrorMessage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showErrorMessage = (msg: string) => {
      shown = msg;
      return Promise.resolve(undefined);
    };
    const before = vscode.window.tabGroups.all.flatMap((g) => g.tabs).length;
    try {
      p.openZarrStore(notAStore);
    } finally {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (vscode.window as any).showErrorMessage = realError;
    }

    assert.ok(shown, "the user was told");
    assert.match(shown, /not a Zarr store/, shown);
    assert.match(shown, /\.zgroup/, "the message names what it looked for");
    assert.match(shown, /convention/, "and that the suffix is not the test");
    assert.strictEqual(
      vscode.window.tabGroups.all.flatMap((g) => g.tabs).length,
      before,
      "no editor should have opened",
    );
  });

  test("opening a store opens a render panel", async () => {
    const p = await provider();
    const before = vscode.window.tabGroups.all.flatMap((g) => g.tabs).length;
    p.openZarrStore(vscode.Uri.file(storePath("cf_v3")));

    // `createWebviewPanel` returns synchronously, but the tab model catches up
    // on a later tick — so this polls rather than asserting immediately. The
    // tab list is the oracle on purpose: it is what a user would actually see,
    // where a count kept by the provider would only say the code ran.
    const tabs = async () => vscode.window.tabGroups.all.flatMap((g) => g.tabs);
    let after = await tabs();
    for (let i = 0; i < 40 && after.length === before; i += 1) {
      await new Promise((r) => setTimeout(r, 25));
      after = await tabs();
    }
    assert.strictEqual(after.length, before + 1, "a panel opened");
    const opened = after[after.length - 1];
    assert.match(opened.label, /Render:/, `unexpected panel title: ${opened.label}`);
  });
});
