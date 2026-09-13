// Imported colour tables (#236).
//
// A GMT colour palette table (`.cpt`) is read once, on import, by the Rust
// parser, which compiles it to the 768-byte lookup table every colormap paints
// through. What is kept — in `globalState`, so an import outlives the window —
// is that table and the legend stops, not the file: a render sends the table as
// `RenderOptions.colormapTable`, and nothing re-reads the `.cpt`.

import * as path from "path";
import * as vscode from "vscode";

import { loadNative, nativeBinaryName, type ColormapInfo } from "./native";

/** The command id, in one place so `package.json` and the registration cannot
 *  disagree. */
export const IMPORT_COLOR_TABLE_COMMAND = "fieldglass.importColorTable";

/** The `globalState` key imported tables are kept under. */
export const IMPORTED_COLOR_TABLES_KEY = "fieldglass.importedColorTables";

/** Imported names carry this prefix, so one can never shadow a Rust colormap
 *  of the same name. */
const IMPORTED_PREFIX = "imported:";

const TABLE_LEN = 256 * 3;

/** A colormap the picker offers: one of the Rust registry's, or an import. */
export interface PickerColormap {
  name: string;
  label: string;
  kind: ColormapInfo["kind"] | "imported";
  stops: string[];
}

/** An imported colour table, as it is stored. */
export interface ImportedColorTable {
  name: string;
  label: string;
  stops: string[];
  table: number[];
}

let store: vscode.Memento | undefined;

/** Where imports are kept. Set once on activation; a test hands in its own. */
export function useColorTableStore(memento: vscode.Memento): void {
  store = memento;
}

/** Whether a stored value is a table this build can paint. `globalState` is
 *  JSON the extension wrote, but it outlives versions, so anything malformed is
 *  dropped rather than sent to Rust to be refused on every render. */
function isImportedColorTable(v: unknown): v is ImportedColorTable {
  const t = v as Partial<ImportedColorTable> | undefined;
  return (
    !!t &&
    typeof t.name === "string" &&
    t.name.startsWith(IMPORTED_PREFIX) &&
    typeof t.label === "string" &&
    Array.isArray(t.stops) &&
    t.stops.every((s) => typeof s === "string") &&
    Array.isArray(t.table) &&
    t.table.length === TABLE_LEN &&
    t.table.every((b) => Number.isInteger(b) && b >= 0 && b <= 255)
  );
}

/** Every imported table, in import order. */
export function importedColorTables(): ImportedColorTable[] {
  const stored = store?.get<unknown[]>(IMPORTED_COLOR_TABLES_KEY, []) ?? [];
  return Array.isArray(stored) ? stored.filter(isImportedColorTable) : [];
}

/** The imported table under `name`, if there is one. */
export function importedColorTable(name: string): ImportedColorTable | undefined {
  return importedColorTables().find((t) => t.name === name);
}

/** The imports as picker entries, after the Rust registry's. */
export function importedPickerColormaps(): PickerColormap[] {
  return importedColorTables().map((t) => ({
    name: t.name,
    label: t.label,
    kind: "imported",
    stops: t.stops,
  }));
}

/** Read a `.cpt` file's text into a table to keep, named after the file.
 *  Throws with the parser's line and reason when it cannot be imported. */
export function compileColorTable(
  text: string,
  fileName: string,
): ImportedColorTable & { slices: number } {
  const native = loadNative();
  if (!native) {
    throw new Error(`native module ${nativeBinaryName()} not loaded`);
  }
  const parsed = native.parseColorTable(text);
  const label = path.basename(fileName).replace(/\.cpt$/i, "") || "color table";
  return {
    name: IMPORTED_PREFIX + label,
    label,
    stops: parsed.stops,
    table: parsed.table,
    slices: parsed.slices,
  };
}

/** Keep `entry`, replacing an earlier import of the same name. */
export async function saveImportedColorTable(entry: ImportedColorTable): Promise<void> {
  if (!store) {
    throw new Error("the color table store is not set up");
  }
  const kept: ImportedColorTable = {
    name: entry.name,
    label: entry.label,
    stops: entry.stops,
    table: entry.table,
  };
  const others = importedColorTables().filter((t) => t.name !== entry.name);
  await store.update(IMPORTED_COLOR_TABLES_KEY, [...others, kept]);
}

/** Import the `.cpt` at `uri`: parse it, keep it, and say what happened.
 *  Resolves to the kept table, or `undefined` when the file was refused — the
 *  reason is shown to the user rather than thrown. */
export async function importColorTableFile(uri: vscode.Uri): Promise<ImportedColorTable | undefined> {
  const fileName = path.basename(uri.fsPath);
  let entry: ImportedColorTable & { slices: number };
  try {
    const bytes = await vscode.workspace.fs.readFile(uri);
    entry = compileColorTable(new TextDecoder("utf-8").decode(bytes), fileName);
  } catch (err) {
    void vscode.window.showErrorMessage(
      `Fieldglass: could not import ${fileName}: ${err instanceof Error ? err.message : String(err)}`,
    );
    return undefined;
  }
  await saveImportedColorTable(entry);
  void vscode.window.showInformationMessage(
    `Fieldglass: imported color table "${entry.label}" (${entry.slices} slices). ` +
      "Pick it under Imported in a render panel's Colormap menu.",
  );
  return entry;
}
