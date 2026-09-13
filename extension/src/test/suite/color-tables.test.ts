// Importing a GMT colour palette table (#236).
//
// What the Rust gates cannot see: that the command is registered under the id
// the manifest contributes, that an import survives into the picker and into
// the options a render sends, that a table actually paints its own colours
// through the addon, and that a file which cannot be imported is refused without
// touching what is stored.
//
// The `.cpt` files are the committed MIT-licensed ones `fieldglass-core`'s
// parser tests read (provenance in their `NOTICE.md`), reached by path rather
// than copied, so there is one set.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import {
  IMPORT_COLOR_TABLE_COMMAND,
  IMPORTED_COLOR_TABLES_KEY,
  compileColorTable,
  importedColorTables,
  importedPickerColormaps,
  useColorTableStore,
} from "../../color-tables";
import type { FieldglassApi } from "../../extension";
import { loadNative, type MessageMeta } from "../../native";
import { resolveRerenderOptions } from "../../provider";
import { renderImagePanelHtml } from "../../render-panel";

const EXT_ID = "fieldglass.fieldglass";

function extensionPath(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed in the test host");
  return ext.extensionPath;
}

function cptPath(name: string): string {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-core", "tests", "fixtures", "cpt", name);
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** A `globalState` stand-in, so the tests neither read nor leave behind the
 *  host's real imports. */
class MemoryMemento implements vscode.Memento {
  private readonly values = new Map<string, unknown>();
  keys(): readonly string[] {
    return [...this.values.keys()];
  }
  get<T>(key: string, defaultValue?: T): T {
    return (this.values.has(key) ? this.values.get(key) : defaultValue) as T;
  }
  update(key: string, value: unknown): Thenable<void> {
    this.values.set(key, value);
    return Promise.resolve();
  }
}

async function provider() {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  assert.ok(ext, "extension is installed");
  return (await ext.activate()).provider;
}

/** Write `text` to a temporary `.cpt` and hand back its URI. */
function tempCpt(name: string, text: string): vscode.Uri {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-cpt-"));
  const file = path.join(dir, name);
  fs.writeFileSync(file, text, "utf8");
  return vscode.Uri.file(file);
}

suite("Imported color tables", () => {
  let memento: MemoryMemento;
  setup(async () => {
    await provider();
    memento = new MemoryMemento();
    useColorTableStore(memento);
  });

  test("the command package.json contributes is the one registered", async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.includes(IMPORT_COLOR_TABLE_COMMAND), "the import command is not registered");
    const manifest = JSON.parse(
      fs.readFileSync(path.join(extensionPath(), "package.json"), "utf8"),
    ) as { contributes?: { commands?: { command: string; title: string }[] } };
    const entry = manifest.contributes?.commands?.find((c) => c.command === IMPORT_COLOR_TABLE_COMMAND);
    assert.ok(entry, "the manifest does not contribute the import command");
    assert.match(entry.title, /Color Table/);
  });

  test("a real table compiles to what the picker and the painter need", () => {
    const entry = compileColorTable(fs.readFileSync(cptPath("batlow.cpt"), "utf8"), "batlow.cpt");
    assert.strictEqual(entry.name, "imported:batlow");
    assert.strictEqual(entry.label, "batlow");
    assert.strictEqual(entry.slices, 255);
    assert.strictEqual(entry.table.length, 768);
    assert.strictEqual(entry.stops[0], "#011959", "the legend starts at the table's first colour");
  });

  test("a file that cannot be imported is refused and nothing is kept", async () => {
    // A real categorical table: refused with its line and the reason.
    assert.throws(
      () => compileColorTable(fs.readFileSync(cptPath("batlowS.cpt"), "utf8"), "batlowS.cpt"),
      /line \d+: .*categorical/,
    );
    const p = await provider();
    const imported = await p.importColorTable(tempCpt("broken.cpt", "0 red 1 blue\n2 red 3 blue\n"));
    assert.strictEqual(imported, false, "a table with a gap must not import");
    assert.deepStrictEqual(importedColorTables(), [], "a refused file must leave nothing behind");
  });

  test("an import reaches the picker and the options a render sends", async () => {
    const p = await provider();
    const uri = vscode.Uri.file(cptPath("balance.cpt"));
    assert.strictEqual(await p.importColorTable(uri), true);
    // Importing the same file again replaces it rather than adding a second.
    assert.strictEqual(await p.importColorTable(uri), true);
    const kept = importedColorTables();
    assert.strictEqual(kept.length, 1);
    assert.strictEqual(kept[0].name, "imported:balance");

    const native = loadNative();
    assert.ok(native, "native binding required");
    const html = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      { gridType: "latlon", reprojectable: true } as unknown as MessageMeta,
      "summary",
      [...native.colormaps(), ...importedPickerColormaps()],
      native.combineOps(),
    );
    assert.match(
      html,
      /<optgroup label="Imported">\s*<option value="imported:balance">balance<\/option>/,
      "the picker must offer the import under Imported",
    );

    // Picking it sends the table, not a name Rust would refuse.
    const options = resolveRerenderOptions({ colormap: "imported:balance", reverseColormap: true });
    assert.strictEqual(options.colormap, undefined);
    assert.deepStrictEqual(options.colormapTable, kept[0].table);
    assert.strictEqual(options.reverseColormap, true);
    // A registered name still goes by name, with no table beside it.
    const named = resolveRerenderOptions({ colormap: "plasma" });
    assert.strictEqual(named.colormap, "plasma");
    assert.strictEqual(named.colormapTable, undefined);
  });

  test("an import rebuilds a render panel that is already open, until it closes", async () => {
    const p = await provider();
    const native = loadNative();
    assert.ok(native, "native binding required");
    const fixture = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const doc = await p.openCustomDocument(
      vscode.Uri.file(fixture),
      {} as vscode.CustomDocumentOpenContext,
      new vscode.CancellationTokenSource().token,
    );
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(fixture)).messages()[0];

    // A stand-in panel: the HTML the provider writes is the thing under test.
    const onDispose: (() => void)[] = [];
    const fakePanel = {
      webview: {
        html: "",
        cspSource: "vscode-webview:",
        onDidReceiveMessage: () => ({ dispose: () => undefined }),
        postMessage: () => Promise.resolve(true),
      },
      visible: false,
      onDidDispose: (cb: () => void) => {
        onDispose.push(cb);
        return { dispose: () => undefined };
      },
      dispose: () => undefined,
    };
    const origCreate = vscode.window.createWebviewPanel;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).createWebviewPanel = () => fakePanel;
    try {
      p.openRenderPanel(doc, meta);
    } finally {
      vscode.window.createWebviewPanel = origCreate;
    }
    assert.ok(fakePanel.webview.html.includes('id="picker-colormap"'), "the panel was built");
    assert.ok(!fakePanel.webview.html.includes("imported:vik"));

    assert.strictEqual(await p.importColorTable(vscode.Uri.file(cptPath("vik.cpt"))), true);
    assert.ok(
      fakePanel.webview.html.includes('<option value="imported:vik">vik</option>'),
      "the open panel's picker must offer the new import",
    );

    // Once closed, a panel is not rebuilt again.
    assert.ok(onDispose.length > 0, "the provider watches for the panel closing");
    onDispose.forEach((cb) => cb());
    fakePanel.webview.html = "closed";
    assert.strictEqual(await p.importColorTable(vscode.Uri.file(cptPath("batlow.cpt"))), true);
    assert.strictEqual(fakePanel.webview.html, "closed");
  });

  test("a table paints its own colours through the addon", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const bytes = fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"));
    const handle = native.Grib2Handle.fromBytes(bytes);
    // One colour for the low half of the ramp and another for the high half, so
    // a field with any spread paints both and nothing else.
    const table = Array.from({ length: 256 }, (_, i) => (i < 128 ? [200, 10, 20] : [30, 40, 250])).flat();
    const rendered = handle.renderGrid(0, {
      projection: "source",
      resampling: "nearest",
      flipY: false,
      colormapTable: table,
    });
    const seen = new Set<string>();
    for (let k = 0; k < rendered.rgba.length; k += 4) {
      if (rendered.rgba[k + 3] === 0) continue;
      seen.add(`${rendered.rgba[k]},${rendered.rgba[k + 1]},${rendered.rgba[k + 2]}`);
    }
    assert.deepStrictEqual([...seen].sort(), ["200,10,20", "30,40,250"]);

    // And sending a name beside a table is an error, not a guess.
    assert.throws(
      () =>
        handle.renderGrid(0, {
          projection: "source",
          resampling: "nearest",
          flipY: false,
          colormap: "viridis",
          colormapTable: table,
        }),
      /send one/,
    );
  });

  test("a stored entry this build cannot paint is ignored", async () => {
    await memento.update(IMPORTED_COLOR_TABLES_KEY, [
      { name: "imported:short", label: "short", stops: [], table: [1, 2, 3] },
      { name: "viridis", label: "shadow", stops: [], table: new Array(768).fill(0) },
      "not an object",
    ]);
    assert.deepStrictEqual(importedColorTables(), []);
    assert.strictEqual(resolveRerenderOptions({ colormap: "imported:short" }).colormapTable, undefined);
  });
});
