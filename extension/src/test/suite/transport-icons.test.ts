// The Animate row's transport buttons are one set of drawn icons (#1042).
//
// They were text glyphs that did not match: back was `◀` with no bar while
// forward was `▶|`, and first and last (`⏮`, `⏭`) came from another font,
// drew at about half the size and sat above the middle of the button. Now
// each button holds an inline SVG icon on one 16 × 16 grid, in the button's
// text colour, and Play holds both play and pause so it keeps its size as
// they swap.
//
// Driven through the real panel, in a light and a dark theme: what is read
// back is what the page lays out.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import {
  type Box,
  classicNetcdf,
  isType,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  type TransportButton,
} from "./real-panel";

/** The buttons in order, with the labels they have always carried. */
const BUTTONS: Array<[id: string, label: string, icon: string]> = [
  ["anim-first", "First step", "first"],
  ["anim-back", "Step back", "back"],
  ["anim-play", "Play", "play"],
  ["anim-forward", "Step forward", "forward"],
  ["anim-last", "Last step", "last"],
];

/** Within a pixel's hundredth, for boxes laid out from the same rules. */
const SAME = 0.01;
/** Within half a pixel, for a centre rounded to the pixel grid. */
const CENTRED = 0.5;

type Opened = { panel: RealPanel; close: () => void };

/** A classic NetCDF file of `air(time, lat, lon)` over five steps, opened in
 *  a real slice panel, once its Animate row is on screen. */
async function openAnimated(): Promise<Opened> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1042-"));
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
function shown(panel: RealPanel): Promise<PageState> {
  return waitFor(
    panel,
    (s) => s.animateRowDisplay !== "none" && !!s.transport && s.transport.every((b) => height(b.box) > 0),
    "the Animate row on screen",
  );
}

/** Read the page until `ok` holds. */
async function waitFor(panel: RealPanel, ok: (s: PageState) => boolean, what: string): Promise<PageState> {
  const until = Date.now() + 15_000;
  for (;;) {
    const state = await panel.drive([]);
    if (ok(state)) return state;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}; the page is in ${state.themeKind}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

function buttons(state: PageState): TransportButton[] {
  assert.ok(state.transport, "a slice panel has transport buttons");
  return state.transport;
}

function button(state: PageState, id: string): TransportButton {
  const b = buttons(state).find((x) => x.id === id);
  assert.ok(b, `a #${id} button`);
  return b;
}

/** The one icon a button shows. */
function iconOf(b: TransportButton) {
  assert.strictEqual(b.text, "", `#${b.id} draws no text glyph, got ${JSON.stringify(b.text)}`);
  assert.strictEqual(b.icons.length, 1, `#${b.id} shows exactly one icon, got ${b.icons.length}`);
  return b.icons[0];
}

/** An icon's classes. */
const classes = (icon: TransportButton["icons"][number]) => (icon.name ?? "").split(/\s+/);

const height = (b: Box) => b.bottom - b.top;
const width = (b: Box) => b.right - b.left;
const middleY = (b: Box) => (b.top + b.bottom) / 2;
const middleX = (b: Box) => (b.left + b.right) / 2;

/** The box every shape of an icon covers, in its own units. */
function extent(shapes: Array<{ x: number; y: number; width: number; height: number }>) {
  return {
    left: Math.min(...shapes.map((s) => s.x)),
    right: Math.max(...shapes.map((s) => s.x + s.width)),
    top: Math.min(...shapes.map((s) => s.y)),
    bottom: Math.max(...shapes.map((s) => s.y + s.height)),
  };
}

/** `a`'s shapes, mirrored about the middle of the 16-wide grid, are `b`'s. */
function assertMirrors(state: PageState, a: string, b: string): void {
  const key = (s: { x: number; y: number; width: number; height: number }) =>
    [s.x, s.y, s.width, s.height].map((v) => v.toFixed(3)).join(",");
  const mirrored = iconOf(button(state, a))
    .shapes.map((s) => ({ ...s, x: 16 - s.x - s.width }))
    .map(key)
    .sort();
  const theirs = iconOf(button(state, b)).shapes.map(key).sort();
  assert.deepStrictEqual(theirs, mirrored, `#${b} is #${a} mirrored`);
}

/** Every check on the row as laid out in the current theme. */
function assertRow(state: PageState, theme: string): void {
  const all = buttons(state);
  assert.deepStrictEqual(
    all.map((b) => [b.id, b.ariaLabel, b.title]),
    BUTTONS.map(([id, label]) => [id, label, label]),
    "the buttons keep their ids, labels and tooltips, in order",
  );
  const icons = all.map((b) => ({ b, icon: iconOf(b) }));
  for (const { b, icon } of icons) {
    const name = BUTTONS.find(([id]) => id === b.id)?.[2];
    assert.ok(classes(icon).includes(`icon-${name}`), `#${b.id} shows the ${name} icon, got ${icon.name}`);
    assert.strictEqual(icon.viewBox, "0 0 16 16", `#${b.id}'s icon is on the shared grid`);
    // `currentColor`: every shape is filled with the button's own text
    // colour. This proves the icon takes `currentColor`, not that the
    // button's colour follows the theme: the panel's buttons are not themed
    // yet (#1118).
    for (const fill of icon.fills) {
      assert.strictEqual(fill, b.color, `#${b.id}'s icon is filled with currentColor (${theme})`);
    }
    // Each icon's shapes span the same height of the grid and are centred
    // on it, so they draw the same size.
    const e = extent(icon.shapes);
    assert.deepStrictEqual([e.top, e.bottom], [3, 13], `#${b.id}'s shapes span y 3 to 13`);
    assert.ok(Math.abs((e.left + e.right) / 2 - 8) < SAME, `#${b.id}'s shapes are centred on x = 8, got ${e.left}..${e.right}`);
  }

  const [first] = icons;
  for (const { b, icon } of icons) {
    assert.ok(
      Math.abs(height(icon.box) - height(first.icon.box)) < SAME &&
        Math.abs(width(icon.box) - width(first.icon.box)) < SAME,
      `#${b.id}'s icon is drawn at the size of #${first.b.id}'s (${theme}): ` +
        `${width(icon.box)} × ${height(icon.box)} against ${width(first.icon.box)} × ${height(first.icon.box)}`,
    );
    assert.ok(height(icon.box) > 8, `#${b.id}'s icon is drawn at a readable size, got ${height(icon.box)} px`);
    assert.ok(
      Math.abs(middleY(icon.box) - middleY(b.box)) <= CENTRED,
      `#${b.id}'s icon sits on its button's middle (${theme}): icon ${icon.box.top}..${icon.box.bottom}, ` +
        `button ${b.box.top}..${b.box.bottom}`,
    );
    assert.ok(
      Math.abs(middleX(icon.box) - middleX(b.box)) <= CENTRED,
      `#${b.id}'s icon is centred across its button (${theme})`,
    );
    assert.ok(
      Math.abs(height(b.box) - height(first.b.box)) < SAME && Math.abs(b.box.top - first.b.box.top) < SAME,
      `#${b.id} is as tall as #${first.b.id} and level with it (${theme})`,
    );
  }

  assertMirrors(state, "anim-back", "anim-forward");
  assertMirrors(state, "anim-first", "anim-last");
}

suite("Transport icons (#1042)", function () {
  this.timeout(60_000);

  test("every transport button shows one icon from one set, the same size and centred, in light and dark themes", async () => {
    const config = vscode.workspace.getConfiguration("workbench");
    const original = config.inspect<string>("colorTheme")?.globalValue;
    const { panel, close } = await openAnimated();
    const startKind = (await panel.drive([])).themeKind;
    try {
      for (const [theme, kind] of [
        ["Default Light Modern", "vscode-light"],
        ["Default Dark Modern", "vscode-dark"],
      ] as const) {
        await vscode.workspace.getConfiguration("workbench").update("colorTheme", theme, vscode.ConfigurationTarget.Global);
        const state = await waitFor(panel, (s) => s.themeKind === kind, `the page in ${theme}`);
        assertRow(state, theme);
      }
    } finally {
      await config.update("colorTheme", original, vscode.ConfigurationTarget.Global);
      // The suites after this one start in the theme this one found.
      await waitFor(panel, (s) => s.themeKind === startKind, "the restored theme");
      close();
    }
  });

  test("play and pause swap without the button, or the row, changing size", async () => {
    const { panel, close } = await openAnimated();
    try {
      const before = await shown(panel);
      // Measured from the first button, so a frame that lands between reads
      // and moves the whole row (a heading that wraps) does not count.
      const playBox = placed(before, "anim-play");
      const rowBoxes = row(before);
      assert.ok(classes(iconOf(button(before, "anim-play"))).includes("icon-play"));

      const playing = await panel.drive([{ id: "anim-play", click: true }]);
      const pause = button(playing, "anim-play");
      assert.strictEqual(pause.ariaLabel, "Pause");
      assert.strictEqual(pause.title, "Pause");
      assert.ok(classes(iconOf(pause)).includes("icon-pause"), "the playing button shows the pause icon");
      assert.deepStrictEqual(placed(playing, "anim-play"), playBox, "the button keeps its size and place as play becomes pause");
      assert.deepStrictEqual(row(playing), rowBoxes, "the buttons do not move against each other");
      assertOnGrid(iconOf(pause));

      const stopped = await panel.drive([{ id: "anim-play", click: true }]);
      const play = button(stopped, "anim-play");
      assert.strictEqual(play.ariaLabel, "Play");
      assert.strictEqual(play.title, "Play");
      assert.ok(classes(iconOf(play)).includes("icon-play"), "the stopped button shows the play icon");
      assert.deepStrictEqual(placed(stopped, "anim-play"), playBox, "the button keeps its size and place as pause becomes play");
    } finally {
      close();
    }
  });
});

/** Button `id`'s box, measured from the first transport button's top left. */
function placed(state: PageState, id: string): Box {
  const origin = buttons(state)[0].box;
  const b = button(state, id).box;
  return { left: b.left - origin.left, top: b.top - origin.top, right: b.right - origin.left, bottom: b.bottom - origin.top };
}

/** Every transport button's box, measured as `placed` measures it. */
function row(state: PageState): Box[] {
  return buttons(state).map((b) => placed(state, b.id));
}

/** Pause is drawn on the same grid as the rest: the same span of y, centred. */
function assertOnGrid(icon: TransportButton["icons"][number]): void {
  assert.strictEqual(icon.viewBox, "0 0 16 16");
  const e = extent(icon.shapes);
  assert.deepStrictEqual([e.top, e.bottom], [3, 13], "pause spans y 3 to 13");
  assert.ok(Math.abs((e.left + e.right) / 2 - 8) < SAME, "pause is centred on x = 8");
}
