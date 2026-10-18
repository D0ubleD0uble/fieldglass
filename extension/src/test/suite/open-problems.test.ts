// What the user is told when a file or store will not open (#1040).
//
// Each case goes through the real entry point (the Open Zarr Store command's
// `openZarrStore`, or `resolveCustomEditor` for a file) over committed bytes,
// and reads what reached the toast. The toast must be plain: no metadata file
// names, no reader error text, no codec or dtype tokens. The technical text
// must still be one click away, behind "Details", so a bug report can carry it.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";
import { DETAILS } from "../../problems";
import { extensionPath, provider } from "./real-panel";

/** The repo's Zarr fixtures folder: about forty one-array stores, and `stores/`. */
function zarrFixtures(...parts: string[]): string {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures", ...parts);
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** One call to `showErrorMessage` or `showWarningMessage`. */
interface Shown {
  severity: "error" | "warning";
  message: string;
  /** The modal dialog's `detail`, for the call Details led to. */
  detail?: string;
  modal: boolean;
}

/** Record every toast while `run` goes, answering each with `answer`. VS Code's
 *  own toasts would wait for a click that never comes. */
async function capture(answer: string | undefined, run: () => Promise<void> | void): Promise<Shown[]> {
  const shown: Shown[] = [];
  const w = vscode.window as unknown as Record<string, unknown>;
  const real = { error: w.showErrorMessage, warning: w.showWarningMessage };
  const stub = (severity: Shown["severity"]) => (message: string, ...rest: unknown[]) => {
    const options = typeof rest[0] === "object" && rest[0] !== null ? (rest[0] as vscode.MessageOptions) : undefined;
    shown.push({ severity, message, detail: options?.detail, modal: options?.modal === true });
    // A modal dialog has only the button VS Code adds; a toast gets `answer`.
    return Promise.resolve(options?.modal ? undefined : answer);
  };
  w.showErrorMessage = stub("error");
  w.showWarningMessage = stub("warning");
  try {
    await run();
    // A file's open reports without waiting for the answer, so let the
    // Details dialog that answer leads to be shown before reading.
    for (let i = 0; i < 40 && answer === DETAILS && shown.length === 1; i += 1) {
      await new Promise((r) => setTimeout(r, 25));
    }
  } finally {
    w.showErrorMessage = real.error;
    w.showWarningMessage = real.warning;
  }
  return shown;
}

/** The folders directly in `folder` that are stores, found here from the folder
 *  itself rather than taken from the code under test. */
function storesIn(folder: string): string[] {
  const native = loadNative();
  assert.ok(native, "native binding required");
  return fs
    .readdirSync(folder, { withFileTypes: true })
    .filter((e) => e.isDirectory() && native.ZarrHandle.isStore(path.join(folder, e.name)))
    .map((e) => e.name)
    .sort();
}

function tabCount(): number {
  return vscode.window.tabGroups.all.flatMap((g) => g.tabs).length;
}

/** Toast text that only someone who knows the format can read. */
const TECHNICAL = [/zarr\.json/, /\.zmetadata/, /\.zgroup/, /\.zarray/, /convention/, /<U4/, /dtype/, /parse error/, /offset/];

function assertPlain(message: string): void {
  for (const t of TECHNICAL) assert.doesNotMatch(message, t, `technical text in the toast: ${message}`);
}

suite("Open problems (#1040)", () => {
  test("a folder of stores names the stores in it", async () => {
    const p = await provider();
    const before = tabCount();
    const shown = await capture(DETAILS, () => p.openZarrStore(vscode.Uri.file(zarrFixtures("stores"))));

    assert.strictEqual(shown.length, 2, JSON.stringify(shown));
    const [toast, dialog] = shown;
    assert.strictEqual(toast.severity, "error");
    // The eight committed when this was written. Read from the folder, so a
    // store added later changes the count here, not the test.
    const stores = storesIn(zarrFixtures("stores"));
    for (const name of ["cf_v2", "cf_v3", "v2_nested", "v2_problems", "v2_slash", "v3_nested", "v3_sharded", "v3_v2keys"]) {
      assert.ok(stores.includes(name), `${name} should be found as a store`);
    }
    const listed = stores.length <= 8 ? stores.join(", ") : `${stores.slice(0, 8).join(", ")} and ${stores.length - 8} more`;
    assert.strictEqual(
      toast.message,
      `Fieldglass: "stores" isn't a Zarr store. It contains ${stores.length} Zarr stores: ${listed}. Pick one of those.`,
    );
    assertPlain(toast.message);
    // Details keeps what the toast left out: the path and what was looked for.
    assert.ok(dialog.modal, "Details opens a dialog");
    assert.ok(dialog.detail?.includes(zarrFixtures("stores")), String(dialog.detail));
    assert.match(dialog.detail ?? "", /\.zgroup/);
    assert.strictEqual(tabCount(), before, "nothing should have opened");
  });

  test("a long list of stores is cut short", async () => {
    const folder = zarrFixtures();
    const stores = storesIn(folder);
    assert.ok(stores.length > 8, `expected more than 8 stores in ${folder}`);
    assert.ok(!stores.includes("stores"), "a folder of stores is not itself a store");

    const p = await provider();
    const shown = await capture(undefined, () => p.openZarrStore(vscode.Uri.file(folder)));
    assert.strictEqual(shown.length, 1);
    assert.strictEqual(
      shown[0].message,
      `Fieldglass: "fixtures" isn't a Zarr store. It contains ${stores.length} Zarr stores: ` +
        `${stores.slice(0, 8).join(", ")} and ${stores.length - 8} more. Pick one of those.`,
    );
  });

  test("a folder with no stores in it says so plainly", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-notastore-"));
    fs.mkdirSync(path.join(dir, "notes"));
    fs.writeFileSync(path.join(dir, "readme.txt"), "not a store");
    const p = await provider();
    const shown = await capture(undefined, () => p.openZarrStore(vscode.Uri.file(dir)));
    assert.strictEqual(shown.length, 1);
    assert.strictEqual(
      shown[0].message,
      `Fieldglass: "${path.basename(dir)}" isn't a Zarr store. Pick the folder that holds ` +
        "the store's data, often one whose name ends in .zarr.",
    );
    assertPlain(shown[0].message);
  });

  test("a store with nothing to draw leads with the plain sentence", async () => {
    const p = await provider();
    const before = tabCount();
    const store = zarrFixtures("stores", "v2_problems");
    const shown = await capture(DETAILS, () => p.openZarrStore(vscode.Uri.file(store)));

    assert.strictEqual(shown.length, 2, JSON.stringify(shown));
    const [toast, dialog] = shown;
    assert.strictEqual(toast.severity, "warning");
    assert.ok(
      toast.message.startsWith("Fieldglass: Nothing in this store can be drawn as a map."),
      toast.message,
    );
    // The arrays it could not read are named, and nothing about why.
    assert.match(toast.message, /couldn't read 2 of its arrays: names, x2\.$/);
    assertPlain(toast.message);
    // The reader's reasons are behind Details, word for word.
    assert.ok(dialog.detail?.includes(store), String(dialog.detail));
    assert.match(dialog.detail ?? "", /- names: .*"<U4"/);
    assert.match(dialog.detail ?? "", /- x2: dimension "x" has length 5/);
    assert.strictEqual(tabCount(), before, "no empty panel should have opened");
  });

  // A GRIB or NetCDF file the reader refuses, opened as a user opens it.
  for (const [fixture, what] of [
    ["regular_latlon_surface.grib2", "GRIB file"],
    ["cmc_wind_300_2010052400_p012.grib", "GRIB file"],
    ["netcdf_classic_dummy.nc", "NetCDF or HDF5 file"],
  ] as const) {
    test(`a damaged ${path.extname(fixture)} file is reported plainly`, async () => {
      // Cut short: the header names more bytes than there are.
      const bytes = fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", fixture));
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-damaged-"));
      const file = path.join(dir, `cut_${fixture}`);
      fs.writeFileSync(file, bytes.subarray(0, 60));

      const p: FieldglassEditorProvider = await provider();
      const doc = (await p.openCustomDocument(
        vscode.Uri.file(file),
        {} as vscode.CustomDocumentOpenContext,
        new vscode.CancellationTokenSource().token,
      )) as FieldglassDocument;
      const panel = vscode.window.createWebviewPanel("fieldglass.viewer", fixture, vscode.ViewColumn.One, {});
      try {
        const shown = await capture(DETAILS, () => p.resolveCustomEditor(doc, panel));
        assert.strictEqual(shown.length, 2, JSON.stringify(shown));
        const [toast, dialog] = shown;
        assert.strictEqual(
          toast.message,
          `Fieldglass: couldn't open "cut_${fixture}" as a ${what}. It may be damaged or ` +
            "incomplete, or use a feature Fieldglass doesn't read yet.",
        );
        assertPlain(toast.message);
        assert.ok(dialog.detail?.includes(file), String(dialog.detail));
        assert.match(dialog.detail ?? "", /parse error: .*(claims length|truncated)/);
      } finally {
        panel.dispose();
      }
    });
  }
});
