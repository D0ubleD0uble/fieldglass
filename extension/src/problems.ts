// What the user is told when a file or store will not open (#1040).
//
// A toast says what happened in plain words and what to do next. The technical
// text (a reader's own error, a path, a metadata file name) is for whoever
// reports the problem, so it goes behind a "Details" button and into the
// extension host log, never into the toast itself.

import * as vscode from "vscode";

/** One thing to tell the user: a plain toast, and the technical text behind it. */
export interface Problem {
  /** The toast. Plain words; no reader error text, no file-format terms. */
  summary: string;
  /** What "Details" shows and the log keeps. */
  details: string;
}

/** The button that shows {@link Problem.details}. */
export const DETAILS = "Details";

/** How many names a toast lists before it says "and N more". */
const MAX_NAMES = 8;

/** `a, b, c`, or `a, b and 3 more` past {@link MAX_NAMES}. */
export function listNames(names: readonly string[]): string {
  if (names.length <= MAX_NAMES) return names.join(", ");
  return `${names.slice(0, MAX_NAMES).join(", ")} and ${names.length - MAX_NAMES} more`;
}

/** A thrown value as text, without the `Error: ` prefix `String(err)` adds. */
export function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** A folder picked as a Zarr store that is not one. `childStores` are the names
 *  of the folders directly inside it that are stores, in the order to list
 *  them; the toast names them so the user can pick the one they meant. */
export function notAStore(folderPath: string, folderName: string, childStores: readonly string[]): Problem {
  const lead = `Fieldglass: "${folderName}" isn't a Zarr store.`;
  let summary: string;
  if (childStores.length === 1) {
    summary = `${lead} It contains one Zarr store, ${childStores[0]}. Pick that folder instead.`;
  } else if (childStores.length > 1) {
    summary =
      `${lead} It contains ${childStores.length} Zarr stores: ${listNames(childStores)}. ` +
      "Pick one of those.";
  } else {
    summary =
      `${lead} Pick the folder that holds the store's data, often one whose name ends in .zarr.`;
  }
  return {
    summary,
    details:
      `${folderPath} holds none of the files that start a Zarr store: zarr.json, ` +
      ".zmetadata, .zgroup or .zarray. Fieldglass looks for these, not for a .zarr " +
      "name, so a store without the suffix still opens.",
  };
}

/** One array a store holds and this build will not read, as the reader
 *  reported it. */
export interface LeftOutArray {
  name: string;
  reason: string;
}

/** A store that opened but holds nothing a map can be drawn from.
 *
 *  The reasons the reader gives are technical text with no code beside them,
 *  so the toast names the arrays it could not read and leaves the reasons to
 *  Details rather than guessing at what each string means. */
export function nothingToDraw(storePath: string, leftOut: readonly LeftOutArray[]): Problem {
  const lead =
    "Fieldglass: Nothing in this store can be drawn as a map. A map needs an array " +
    "of numbers with at least two dimensions, such as latitude and longitude";
  const names = leftOut.map((l) => l.name);
  const summary =
    names.length === 0
      ? `${lead}, and this store has none.`
      : names.length === 1
        ? `${lead}. Fieldglass couldn't read one of its arrays, ${names[0]}.`
        : `${lead}. Fieldglass couldn't read ${names.length} of its arrays: ${listNames(names)}.`;
  const lines = [`Store: ${storePath}`];
  if (leftOut.length > 0) {
    lines.push("Arrays Fieldglass couldn't read:");
    for (const l of leftOut) lines.push(`- ${l.name}: ${l.reason}`);
  }
  lines.push(
    "No other array in the store can be drawn as a map: each has fewer than two " +
      "dimensions, does not hold numbers, or is the coordinates of another array.",
  );
  return { summary, details: lines.join("\n") };
}

/** A file or store the reader refused. `what` names it the way the user knows
 *  it ("GRIB Edition 2 file", "Zarr store"), and `name` is the file or folder
 *  name. The reader's error goes to Details: it is the one part a bug report
 *  needs, and the one part a user cannot act on. */
export function couldNotOpen(what: string, name: string, path: string, err: unknown): Problem {
  return {
    summary:
      `Fieldglass: couldn't open "${name}" as a ${what}. It may be damaged or incomplete, ` +
      "or use a feature Fieldglass doesn't read yet.",
    details: `${path}\n${errorText(err)}`,
  };
}

/** The native module did not load, so nothing can be opened. */
export function nativeNotLoaded(modulePath: string, err: unknown): Problem {
  return {
    summary:
      "Fieldglass: couldn't load its file reader, so it can't open files. " +
      "Reinstalling the extension may fix this.",
    details: `${modulePath}\n${errorText(err)}`,
  };
}

/** Show `problem`: its summary as a toast with a Details button, which opens
 *  the technical text in a dialog. The text is logged as well, so it survives
 *  a dismissed toast. Resolves once the user has dismissed the toast, or the
 *  dialog it led to. */
export async function reportProblem(severity: "error" | "warning", problem: Problem): Promise<void> {
  console.error(`[Fieldglass] ${problem.summary}\n${problem.details}`);
  const w = vscode.window;
  const choice =
    severity === "error"
      ? await w.showErrorMessage(problem.summary, DETAILS)
      : await w.showWarningMessage(problem.summary, DETAILS);
  if (choice !== DETAILS) return;
  const dialog = { modal: true, detail: problem.details };
  if (severity === "error") await w.showErrorMessage(problem.summary, dialog);
  else await w.showWarningMessage(problem.summary, dialog);
}
