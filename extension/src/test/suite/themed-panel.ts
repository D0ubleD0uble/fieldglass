// Shared by the suites that drive the Animate row's real panel in a chosen
// colour theme (#1042, #1118).

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { classicNetcdf, isType, openEditor, openReal, type PageState, provider, type RealPanel } from "./real-panel";

const height = (b: { top: number; bottom: number }) => b.bottom - b.top;

export type Opened = { panel: RealPanel; close: () => void };

/** A classic NetCDF file of `air(time, lat, lon)` over five steps, opened in
 *  a real slice panel, once its Animate row is on screen. */
export async function openAnimated(): Promise<Opened> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-themed-"));
  const file = path.join(dir, "steps.nc");
  fs.writeFileSync(
    file,
    classicNetcdf(
      [["time", 5], ["lat", 3], ["lon", 4]],
      [
        { name: "time", dims: ["time"], attrs: { units: "hours since 2024-01-01 00:00:00" }, values: [0, 6, 12, 18, 24] },
        { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-10, 0, 10] },
        { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 90, 180, 270] },
        {
          name: "air",
          dims: ["time", "lat", "lon"],
          attrs: { units: "K" },
          values: Array.from({ length: 60 }, (_, i) => 250 + Math.floor(i / 12)),
        },
      ],
    ),
  );
  const native = loadNative();
  assert.ok(native, "native binding required");
  const air = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name === "air");
  assert.ok(air, "air is renderable");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: air.variableIndex }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  await shown(panel);
  return {
    panel,
    close: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** Read the page once its Animate row is laid out. */
export function shown(panel: RealPanel): Promise<PageState> {
  return waitFor(
    panel,
    (s) => s.animateRowDisplay !== "none" && !!s.transport && s.transport.every((b) => height(b.box) > 0),
    "the Animate row on screen",
  );
}

/** Read a page through `read` until `ok` holds. */
export async function waitForRead<T extends { themeKind: string }>(
  read: () => Promise<T>,
  ok: (s: T) => boolean,
  what: string,
): Promise<T> {
  const until = Date.now() + 15_000;
  for (;;) {
    const state = await read();
    if (ok(state)) return state;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}; the page is in ${state.themeKind}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

/** Read the page until `ok` holds. */
export function waitFor(panel: RealPanel, ok: (s: PageState) => boolean, what: string): Promise<PageState> {
  return waitForRead(() => panel.drive([]), ok, what);
}

/** Run `check` with the page drawn in each of the named themes (`read` is how
 *  the page is read), then put the theme back and call `close`. `close` sits
 *  in a nested `finally`, so a failed assertion is the error reported, and
 *  the page, editor and temp directory are closed even if the restore never
 *  lands. */
export async function inThemesRead<T extends { themeKind: string }>(
  read: () => Promise<T>,
  close: () => void,
  themes: ReadonlyArray<readonly [name: string, kind: string]>,
  check: (state: T, theme: string) => void | Promise<void>,
): Promise<void> {
  const config = vscode.workspace.getConfiguration("workbench");
  const original = config.inspect<string>("colorTheme")?.globalValue;
  try {
    const startKind = (await read()).themeKind;
    try {
      for (const [theme, kind] of themes) {
        await vscode.workspace.getConfiguration("workbench").update("colorTheme", theme, vscode.ConfigurationTarget.Global);
        const state = await waitForRead(read, (s) => s.themeKind === kind, `the page in ${theme}`);
        await check(state, theme);
      }
    } finally {
      await config.update("colorTheme", original, vscode.ConfigurationTarget.Global);
      // The suites after this one start in the theme this one found.
      await waitForRead(read, (s) => s.themeKind === startKind, "the restored theme");
    }
  } finally {
    close();
  }
}

/** `inThemesRead` for a real render panel. */
export function inThemes(
  opened: Opened,
  themes: ReadonlyArray<readonly [name: string, kind: string]>,
  check: (state: PageState, theme: string, panel: RealPanel) => void | Promise<void>,
): Promise<void> {
  const { panel, close } = opened;
  return inThemesRead(() => panel.drive([]), close, themes, (state, theme) => check(state, theme, panel));
}
