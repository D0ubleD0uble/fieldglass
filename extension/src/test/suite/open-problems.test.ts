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
import { MAX_FOLDERS_EXAMINED, type FieldglassDocument, type FieldglassEditorProvider } from "../../provider";
import type { LeftOutCause } from "../../api.generated";
import { couldNotOpen, DETAILS, nativeNotLoaded, nothingToDraw, notAStore, toastName } from "../../problems";
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

function tabs(): vscode.Tab[] {
  return vscode.window.tabGroups.all.flatMap((g) => g.tabs);
}

/** Fail if a tab is open now that was not in `before`. Compared by identity
 *  rather than by count: a tab an earlier test left closing can go while this
 *  one runs, and a count would read that as nothing opening. */
function assertNoNewTab(before: readonly vscode.Tab[], why: string): void {
  const added = tabs().filter((t) => !before.includes(t));
  assert.deepStrictEqual(added.map((t) => t.label), [], why);
}

/** Toast text that only someone who knows the format can read. */
const TECHNICAL = [
  /zarr\.json/,
  /\.zmetadata/,
  /\.zgroup/,
  /\.zarray/,
  /convention/,
  /<U4/,
  /dtype/,
  /parse error/,
  /offset/,
  // Codec and type names a reader reason carries (#1129).
  /bz2/,
  /blosc/,
  /compound/,
  /unsupported/,
];

function assertPlain(message: string): void {
  for (const t of TECHNICAL) assert.doesNotMatch(message, t, `technical text in the toast: ${message}`);
}

suite("Open problems (#1040)", () => {
  test("a folder of stores names the stores in it", async () => {
    const p = await provider();
    const before = tabs();
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
    assertNoNewTab(before, "nothing should have opened");
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
    const before = tabs();
    const store = zarrFixtures("stores", "v2_problems");
    const shown = await capture(DETAILS, () => p.openZarrStore(vscode.Uri.file(store)));

    assert.strictEqual(shown.length, 2, JSON.stringify(shown));
    const [toast, dialog] = shown;
    assert.strictEqual(toast.severity, "warning");
    assert.ok(
      toast.message.startsWith("Fieldglass: Nothing in this store can be drawn as a map."),
      toast.message,
    );
    // Every array is named with why, in plain words from its code (#1129):
    // the two the reader left out, and the three it read that are lines.
    // x2 is readable and only gives `x` a second length, so "couldn't read"
    // would be wrong for it.
    assert.strictEqual(
      toast.message,
      "Fieldglass: Nothing in this store can be drawn as a map. A map needs an array of numbers " +
        "with at least two dimensions, such as latitude and longitude. In this store, names holds " +
        'text, not numbers; x2 gives dimension "x" a different length from another array; and ' +
        "bz, ok and x1 have only one dimension.",
    );
    assert.doesNotMatch(toast.message, /couldn't read/);
    assertPlain(toast.message);
    // The reader's reasons are behind Details, word for word.
    assert.ok(dialog.detail?.includes(store), String(dialog.detail));
    assert.match(dialog.detail ?? "", /- names: .*"<U4"/);
    assert.match(dialog.detail ?? "", /- x2: dimension "x" has length 5/);
    assert.match(dialog.detail ?? "", /Arrays read but not drawable as a map:\n- bz: has 1 dimension/);
    assertNoNewTab(before, "no empty panel should have opened");
  });

  test("every reason code has plain words", () => {
    const cases: [LeftOutCause, string][] = [
      [{ code: "unsupported_dtype", dtype: "<f2" }, "holds a kind of number or value Fieldglass doesn't read"],
      [{ code: "text" }, "holds text, not numbers"],
      [{ code: "dimension_clash", dimension: "time" }, 'gives dimension "time" a different length from another array'],
      [{ code: "unsupported" }, "is stored in a way Fieldglass doesn't read yet"],
      [{ code: "unreadable_metadata" }, "has a description Fieldglass couldn't read"],
      [{ code: "too_few_dimensions", rank: 0 }, "is a single value"],
      [{ code: "too_few_dimensions", rank: 1 }, "has only one dimension"],
      [{ code: "coordinate" }, "is a coordinate, the positions along another array's axes"],
      // A code a later build adds still reads as a sentence.
      [{ code: "from_the_future" } as unknown as LeftOutCause, "can't be drawn as a map"],
    ];
    for (const [cause, words] of cases) {
      const { summary } = nothingToDraw("/x", [{ name: "v", reason: "the reader's <f2 prose", cause }]);
      assert.ok(summary.endsWith(`In this store, v ${words}.`), summary);
      assertPlain(summary);
      assert.doesNotMatch(summary, /<f2|prose/);
    }
    // Two arrays with one reason share a sentence and its plural.
    const two = nothingToDraw("/x", [], [
      { name: "lat", reason: "r", cause: { code: "coordinate" } },
      { name: "lon", reason: "r", cause: { code: "coordinate" } },
    ]).summary;
    assert.match(two, /In this store, lat and lon are coordinates, /);
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

  test("past the folder limit, the count of stores is a floor", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-manyfolders-"));
    // Two stores that sort first, so they fall inside the folders looked at,
    // then more plain folders than the limit.
    for (const store of ["a1", "a2"]) {
      fs.mkdirSync(path.join(dir, store));
      fs.writeFileSync(path.join(dir, store, ".zgroup"), '{"zarr_format": 2}');
      assert.ok(native.ZarrHandle.isStore(path.join(dir, store)), `${store} should be a store`);
    }
    for (let i = 0; i < MAX_FOLDERS_EXAMINED + 10; i += 1) {
      fs.mkdirSync(path.join(dir, `z${String(i).padStart(4, "0")}`));
    }
    // And one store past the limit, which is not looked at.
    fs.mkdirSync(path.join(dir, "zzzz"));
    fs.writeFileSync(path.join(dir, "zzzz", ".zgroup"), '{"zarr_format": 2}');

    const p = await provider();
    const shown = await capture(undefined, () => p.openZarrStore(vscode.Uri.file(dir)));
    assert.strictEqual(shown.length, 1);
    assert.strictEqual(
      shown[0].message,
      `Fieldglass: "${path.basename(dir)}" isn't a Zarr store. It contains at least 2 Zarr ` +
        "stores: a1, a2. Pick one of those.",
    );
  });

  test("a throw while opening reaches the command", async () => {
    const p = await provider();
    const w = vscode.window as unknown as Record<string, unknown>;
    const realDialog = w.showOpenDialog;
    const target = p as unknown as Record<string, unknown>;
    const realOpen = target.openZarrRenderPanel;
    w.showOpenDialog = () => Promise.resolve([vscode.Uri.file(zarrFixtures("stores", "cf_v3"))]);
    // A store that opens, then a failure past `fromDirectory`.
    target.openZarrRenderPanel = () => {
      throw new Error("panel failed");
    };
    try {
      await assert.rejects(p.promptOpenZarrStore(), /panel failed/);
    } finally {
      w.showOpenDialog = realDialog;
      target.openZarrRenderPanel = realOpen;
    }
  });

  test("a name cannot make a link in the toast", () => {
    const hostile = "[click](" + "command" + ":workbench.action.quit)";
    assert.strictEqual(
      toastName(hostile),
      "［click］（" + "command" + ":workbench.action.quit）",
    );
    const leftOut = [
      { name: hostile, reason: "r", cause: { code: "text" } as const },
      // A dimension name comes from the store too, and reaches the toast.
      { name: "a", reason: "r", cause: { code: "dimension_clash", dimension: hostile } as const },
    ];
    const summaries = [
      notAStore("/x", hostile, [hostile, "ok"]).summary,
      notAStore("/x", hostile, []).summary,
      nothingToDraw("/x", leftOut).summary,
      nothingToDraw("/x", [], leftOut).summary,
      couldNotOpen("GRIB file", hostile, "/x", new Error("e")).summary,
    ];
    for (const summary of summaries) assert.doesNotMatch(summary, /[[\]()]/, summary);
    // Details is plain text, and keeps the name as written.
    assert.ok(nothingToDraw("/x", leftOut).details.includes(hostile));
  });

  test("reinstalling is offered only when the module is missing", () => {
    const missing = Object.assign(new Error("Cannot find module"), { code: "MODULE_NOT_FOUND" });
    assert.match(nativeNotLoaded("/m.node", missing).summary, /reinstalling it may fix this/);
    const broken = new Error("invalid ELF header");
    const problem = nativeNotLoaded("/m.node", broken);
    assert.doesNotMatch(problem.summary, /reinstall/i);
    assert.ok(problem.details.includes("invalid ELF header"));
  });
});
