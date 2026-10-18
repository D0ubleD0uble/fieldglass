// What the user is told when a file or store will not open (#1040).
//
// A toast says what happened in plain words and what to do next. The technical
// text (a reader's own error, a path, a metadata file name) is for whoever
// reports the problem, so it goes behind a "Details" button and into the
// extension host log, never into the toast itself.

import * as vscode from "vscode";

import type { LeftOutArray, LeftOutCause } from "./api.generated";

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

/** `a, b, c`, or `a, b and 3 more` past {@link MAX_NAMES}. With `and`, a
 *  list that fits ends `b and c`, as in a sentence. */
export function listNames(names: readonly string[], { and = false } = {}): string {
  if (names.length > MAX_NAMES) {
    return `${names.slice(0, MAX_NAMES).join(", ")} and ${names.length - MAX_NAMES} more`;
  }
  if (!and || names.length <= 1) return names.join(", ");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** How many reasons a toast gives before it says there are others. */
const MAX_REASONS = 4;

/** A name from the disk or a store, made safe to put in a toast.
 *
 *  A toast turns `[text](target)` into a link, and a `command:` target runs a
 *  command when clicked. A folder or array name is not ours to trust, so its
 *  brackets and parentheses become the full-width forms, which read the same
 *  and never make a link. Details is shown as plain text and keeps the name
 *  as it is. */
export function toastName(name: string): string {
  return name
    .replace(/\[/g, "\uFF3B")
    .replace(/\]/g, "\uFF3D")
    .replace(/\(/g, "\uFF08")
    .replace(/\)/g, "\uFF09");
}

/** A thrown value as text, without the `Error: ` prefix `String(err)` adds. */
export function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** A folder picked as a Zarr store that is not one. `childStores` are the names
 *  of the folders directly inside it that are stores, in the order to list
 *  them; the toast names them so the user can pick the one they meant.
 *  `partial` says only some of its folders were looked at, so the count is a
 *  floor. */
export function notAStore(
  folderPath: string,
  folderName: string,
  childStores: readonly string[],
  partial = false,
): Problem {
  const lead = `Fieldglass: "${toastName(folderName)}" isn't a Zarr store.`;
  const names = childStores.map(toastName);
  const atLeast = partial ? "at least " : "";
  let summary: string;
  if (names.length === 1) {
    summary = `${lead} It contains ${atLeast}one Zarr store, ${names[0]}. Pick that folder instead.`;
  } else if (names.length > 1) {
    summary =
      `${lead} It contains ${atLeast}${names.length} Zarr stores: ${listNames(names)}. ` +
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

/** Why an array is no map, in plain words: what follows its name, as `[one,
 *  many]` for one array or several. `cause` is the reader's code (#1129); the
 *  words never repeat a dtype or codec spelling, which stay in Details. A code
 *  this build does not know yet gets a plain fallback rather than nothing. */
function plainReason(cause: LeftOutCause): [string, string] {
  switch (cause.code) {
    case "text":
      return ["holds text, not numbers", "hold text, not numbers"];
    case "unsupported_dtype":
      return [
        "holds a kind of number or value Fieldglass doesn't read",
        "hold a kind of number or value Fieldglass doesn't read",
      ];
    case "dimension_clash": {
      const dim = toastName(cause.dimension);
      return [
        `gives dimension "${dim}" a different length from another array`,
        `give dimension "${dim}" a different length from another array`,
      ];
    }
    case "unsupported":
      return [
        "is stored in a way Fieldglass doesn't read yet",
        "are stored in a way Fieldglass doesn't read yet",
      ];
    case "unreadable_metadata":
      return ["has a description Fieldglass couldn't read", "have descriptions Fieldglass couldn't read"];
    case "too_few_dimensions":
      return cause.rank === 0
        ? ["is a single value", "are single values"]
        : ["has only one dimension", "have only one dimension"];
    case "coordinate":
      return [
        "is a coordinate, the positions along another array's axes",
        "are coordinates, the positions along other arrays' axes",
      ];
    default:
      return ["can't be drawn as a map", "can't be drawn as a map"];
  }
}

/** A store that opened but holds nothing a map can be drawn from.
 *
 *  `leftOut` are the arrays the reader did not read, `undrawable` the ones it
 *  read that are no map. Between them they are every array in the store, so
 *  the toast says why for each, grouped by reason in plain words from each
 *  array's code (#1129). The reader's own reasons, with their dtype and codec
 *  spellings, are in Details. */
export function nothingToDraw(
  storePath: string,
  leftOut: readonly LeftOutArray[],
  undrawable: readonly LeftOutArray[] = [],
): Problem {
  const lead =
    "Fieldglass: Nothing in this store can be drawn as a map. A map needs an array " +
    "of numbers with at least two dimensions, such as latitude and longitude";
  // Grouped by the words, in the order a reason first appears.
  const groups = new Map<string, { words: [string, string]; names: string[] }>();
  for (const a of [...leftOut, ...undrawable]) {
    const words = plainReason(a.cause);
    const group = groups.get(words[0]) ?? { words, names: [] };
    group.names.push(toastName(a.name));
    groups.set(words[0], group);
  }
  const reasons = [...groups.values()]
    .slice(0, MAX_REASONS)
    .map(({ words, names }) => `${listNames(names, { and: true })} ${names.length === 1 ? words[0] : words[1]}`);
  // Past the cap, the rest are one clause, and Details has every array.
  if (groups.size > MAX_REASONS) reasons.push("other arrays can't be drawn for other reasons; Details says why");
  if (reasons.length > 1) reasons[reasons.length - 1] = `and ${reasons[reasons.length - 1]}`;
  const summary =
    reasons.length === 0 ? `${lead}, and this store has none.` : `${lead}. In this store, ${reasons.join("; ")}.`;

  const lines = [`Store: ${storePath}`];
  if (leftOut.length > 0) {
    lines.push("Arrays left out, with the reader's reason:");
    for (const l of leftOut) lines.push(`- ${l.name}: ${l.reason}`);
  }
  if (undrawable.length > 0) {
    lines.push("Arrays read but not drawable as a map:");
    for (const l of undrawable) lines.push(`- ${l.name}: ${l.reason}`);
  }
  if (leftOut.length === 0 && undrawable.length === 0) {
    lines.push("The store holds no arrays.");
  }
  return { summary, details: lines.join("\n") };
}

/** A file or store the reader refused. `what` names it the way the user knows
 *  it ("GRIB Edition 2 file", "Zarr store"), and `name` is the file or folder
 *  name. The reader's error goes to Details: it is the one part a bug report
 *  needs, and the one part a user cannot act on. */
export function couldNotOpen(what: string, name: string, path: string, err: unknown): Problem {
  return {
    summary:
      `Fieldglass: couldn't open "${toastName(name)}" as a ${what}. It may be damaged or incomplete, ` +
      "or use a feature Fieldglass doesn't read yet.",
    details: `${path}\n${errorText(err)}`,
  };
}

/** The native module did not load, so nothing can be opened.
 *
 *  Reinstalling helps when the module is missing from the install. When it is
 *  there and still fails (a platform it was not built for, a library the
 *  system lacks) reinstalling changes nothing, so the toast does not offer
 *  it. */
export function nativeNotLoaded(modulePath: string, err: unknown): Problem {
  const missing = (err as { code?: unknown } | null)?.code === "MODULE_NOT_FOUND";
  return {
    summary: missing
      ? "Fieldglass: couldn't find its file reader, so it can't open files. The " +
        "extension may be incompletely installed; reinstalling it may fix this."
      : "Fieldglass: couldn't load its file reader, so it can't open files. Details " +
        "has the error to report.",
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
