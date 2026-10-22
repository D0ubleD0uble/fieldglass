// What Export PNG draws and when it refuses (#1270, #1187), through the real
// panel: the PNG is read back from the page's own canvas.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { waitForRead } from "./themed-panel";
import {
  decodePng,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const FIXTURES = path.join(extensionPath(), "src", "test", "fixtures");
const FIXTURE = path.join(FIXTURES, "regular_latlon_surface.grib2");

type Image = ReturnType<typeof decodePng>;

/** Click Export PNG with the save dialog answered "cancel", and read back the
 *  PNG the page made. */
async function exportedPng(panel: RealPanel): Promise<Image> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    const sentFrom = panel.sent.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    await panel.waitSent(isType("exportPngDone"), sentFrom, "the PNG export's outcome");
    return decodePng(Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

/** The most common colour in the map area: the background, under "only". */
function modeColour(img: Image, top: number, drawn: { width: number; height: number }): number[] {
  const counts = new Map<number, number>();
  for (let y = top; y < top + drawn.height; y++) {
    for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
      const i = (y * img.width + x) * 4;
      const k = (img.rgba[i] << 16) | (img.rgba[i + 1] << 8) | img.rgba[i + 2];
      counts.set(k, (counts.get(k) ?? 0) + 1);
    }
  }
  const [k] = [...counts.entries()].sort((a, c) => c[1] - a[1])[0];
  return [k >> 16, (k >> 8) & 255, k & 255];
}

// The raster is drawn at its own size, 14 px in from the left, below the title
// block (the PNG's height less both 14 px margins and the raster's height).
const MARGIN = 14;

suite("Export PNG keeps what the screen shows, and refuses a stale picture", () => {
  test("Contours + only leaves the raster out of the PNG; Contours off with only ticked keeps it (#1270)", async function () {
    this.timeout(120_000);
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Contours off, only ticked (the #1265 state): the raster is drawn on
      // screen, so it is in the PNG.
      await panel.drive([{ id: "contours-only", checked: true }]);
      let state = await panel.drive([]);
      assert.strictEqual(state.canvasVisibility, "visible", "the raster stays with Contours off");
      const off = await exportedPng(panel);

      const from = panel.sent.length;
      await panel.drive([{ id: "overlay-contours", checked: true }]);
      await panel.waitSent(isType("contourReady"), from, "the contours");
      state = await panel.drive([]);
      assert.strictEqual(state.canvasVisibility, "hidden", "the screen leaves the raster out");
      const only = await exportedPng(panel);

      assert.deepStrictEqual([only.width, only.height], [off.width, off.height]);
      // With only on the map area is one colour (the editor's background) plus
      // the strokes; with it off it is the raster's colours.
      const top = only.height - MARGIN - drawn.height;
      assert.ok(top >= MARGIN, `the map sits below the title block (${top})`);
      const bg = modeColour(only, top, drawn);
      const same = (img: Image, x: number, y: number): boolean => {
        const i = (y * img.width + x) * 4;
        return img.rgba[i] === bg[0] && img.rgba[i + 1] === bg[1] && img.rgba[i + 2] === bg[2];
      };
      let bgOnly = 0;
      let bgOff = 0;
      let total = 0;
      let witness: [number, number] | null = null;
      for (let y = top; y < top + drawn.height; y++) {
        for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
          total += 1;
          if (same(only, x, y)) {
            bgOnly += 1;
            // A pixel with no isoline or coastline: background with only on.
            if (!witness && !same(off, x, y)) witness = [x, y];
          }
          if (same(off, x, y)) bgOff += 1;
        }
      }
      // With only on the map is the background plus strokes; with it off it is
      // the raster. The gap between the two shares does not depend on how wide
      // the strokes are (they are at least 1.5 px in the PNG, #1366, which
      // covers much of a map this small).
      assert.ok(bgOnly - bgOff > 0.1 * total, `background with only on ${bgOnly}, off ${bgOff}, of ${total} pixels`);
      assert.ok(bgOff / total < 0.1, `the raster's colours with only off: ${bgOff}/${total} are the background`);
      assert.ok(witness, "a pixel that is background with only on and the raster's colour with it off");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("Export PNG after a render that failed for another reason posts nothing and says why (#1187)", async function () {
    this.timeout(120_000);
    const p = await provider();
    // Two grids, so a Compare of them fails before the range is read.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1187-"));
    const file = path.join(dir, "two-grids.grib2");
    fs.writeFileSync(
      file,
      Buffer.concat([fs.readFileSync(FIXTURE), fs.readFileSync(path.join(FIXTURES, "eta_lambert_msg0.grib2"))]),
    );
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    // A refusal asks for no dialog, but never leave a real one open.
    const savedDialog = vscode.window.showSaveDialog;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      await panel.waitSent((m: Msg) => m.type === "gridError" && !m.rangeRefusal, from, "the different-grid error");
      const asked = panel.received.length;
      let state = await panel.drive([{ id: "export-png", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportPng")), "no PNG is written of the old picture");
      assert.ok(state.exportStatus.startsWith("Export failed:"), state.exportStatus);
      assert.ok(state.exportStatus.includes("last render failed"), state.exportStatus);

      // A render that draws clears it, and the export goes ahead.
      from = panel.sent.length;
      await panel.drive([{ id: "compare-field-b", value: 0 }]);
      await panel.waitSent(isType("gridReady"), from, "the redraw");
      const before = panel.received.length;
      const sentBefore = panel.sent.length;
      await panel.drive([{ id: "export-png", click: true }]);
      await waitUntil(() => panel.received.slice(before).find(isType("exportPng")), "export works after a render that drew");
      await panel.waitSent(isType("exportPngDone"), sentBefore, "the PNG export's outcome");
    } finally {
      vscode.window.showSaveDialog = savedDialog;
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

// Brightness of an sRGB byte triple, 0 to 255.
const bright = (c: number[]): number => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];

/** Run `body` with the workbench in `theme`. The panel is opened inside `body`,
 *  so it is drawn in that theme from its first paint: the layer colour inputs
 *  are seeded once, from the theme the page loads in. `body` gets a `restore`
 *  to call while its panel is still open: `config.update` returns before the
 *  workbench repaints, so the page is read until it shows the original theme
 *  again, and only then is the panel closed (the next suite starts from it). */
async function inTheme(
  theme: string,
  body: (restore: (panel: RealPanel) => Promise<void>) => Promise<void>,
): Promise<void> {
  const config = vscode.workspace.getConfiguration("workbench");
  const original = config.inspect<string>("colorTheme")?.globalValue;
  const startKind = {
    [vscode.ColorThemeKind.Light]: "vscode-light",
    [vscode.ColorThemeKind.Dark]: "vscode-dark",
    [vscode.ColorThemeKind.HighContrast]: "vscode-high-contrast",
    [vscode.ColorThemeKind.HighContrastLight]: "vscode-high-contrast-light",
  }[vscode.window.activeColorTheme.kind];
  let restored = false;
  try {
    await config.update("colorTheme", theme, vscode.ConfigurationTarget.Global);
    await body(async (panel) => {
      restored = true;
      await config.update("colorTheme", original, vscode.ConfigurationTarget.Global);
      await waitForRead(() => panel.drive([]), (s) => s.themeKind === startKind, "the restored theme");
    });
  } finally {
    if (!restored) await config.update("colorTheme", original, vscode.ConfigurationTarget.Global);
  }
}

/** `#rrggbb` or `rgb(r, g, b)` as lower-case `#rrggbb`. */
function hex(c: string): string {
  const m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/.exec(c);
  return m ? "#" + [m[1], m[2], m[3]].map((v) => Number(v).toString(16).padStart(2, "0")).join("") : c.toLowerCase();
}

const THEMES = [
  ["Default Dark Modern", "vscode-dark"],
  ["Default Light Modern", "vscode-light"],
] as const;

suite("Export PNG draws on the editor's background (#1300)", () => {
  test("Contours + only: strokes leave the map background toward the theme's foreground, in a dark and a light theme", async function () {
    this.timeout(180_000);
    for (const [theme, kind] of THEMES) {
      await inTheme(theme, async (restore) => {
        const p = await provider();
        const editor = await openEditor(p, path.join(FIXTURES, "eta_lambert_msg0.grib2"));
        const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
        try {
          const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
          const loaded = await waitForRead(() => panel.drive([]), (s) => s.themeKind === kind, `the page in ${theme}`);
          // The Contours colour was seeded from this theme's foreground, not
          // from the theme before it, so the strokes below are this theme's.
          assert.ok(loaded.contoursColour, `${theme}: the Contours colour input is there`);
          assert.strictEqual(hex(loaded.contoursColour), hex(loaded.foreground), `${theme}: Contours are the theme's foreground`);
          const from = panel.sent.length;
          await panel.drive([
            { id: "overlay-contours", checked: true },
            { id: "contours-only", checked: true },
          ]);
          await panel.waitSent(isType("contourReady"), from, "the contours");
          const png = await exportedPng(panel);
          const top = png.height - MARGIN - drawn.height;
          const bg = modeColour(png, top, drawn);
          let lighter = 0;
          let darker = 0;
          let strokes = 0;
          for (let y = top; y < top + drawn.height; y++) {
            for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
              const i = (y * png.width + x) * 4;
              const c = [png.rgba[i], png.rgba[i + 1], png.rgba[i + 2]];
              if (c[0] === bg[0] && c[1] === bg[1] && c[2] === bg[2]) continue;
              strokes += 1;
              lighter = Math.max(lighter, bright(c) - bright(bg));
              darker = Math.max(darker, bright(bg) - bright(c));
            }
          }
          const dark = kind === "vscode-dark";
          assert.ok(strokes >= 10, `${theme}: isolines and coastlines are drawn (${strokes} px)`);
          assert.strictEqual(bright(bg) < 80, dark, `${theme}: the map background is the editor's (${bg})`);
          // The strokes leave the background in the direction of the theme's
          // foreground: lighter in a dark theme, darker in a light one. That
          // they reach the layer's colour is the #1366 suite's check.
          const seen = dark ? lighter : darker;
          assert.ok(
            seen >= 30,
            `${theme}: strokes reach ${seen.toFixed(0)}/255 ${dark ? "lighter" : "darker"} than the background ${bg} ` +
              `(${drawn.width}x${drawn.height} map)`,
          );
          await restore(panel);
        } finally {
          panel.dispose();
          editor.close();
        }
      });
    }
  });

  test("with the raster on, cells the raster leaves transparent show the editor's background, not white", async function () {
    this.timeout(180_000);
    await inTheme("Default Dark Modern", async (restore) => {
      const p = await provider();
      const editor = await openEditor(p, FIXTURE);
      const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
      try {
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        await waitForRead(() => panel.drive([]), (s) => s.themeKind === "vscode-dark", "the page in a dark theme");
        const from = panel.sent.length;
        // Mollweide leaves the corners of its rectangle off the disc.
        await panel.drive([{ id: "picker-projection", value: "mollweide" }]);
        const drawn = await panel.waitSent(
          (m: Msg) => m.type === "gridReady" && m.options.projection === "mollweide",
          from,
          "the Mollweide map",
        );
        const png = await exportedPng(panel);
        const top = png.height - MARGIN - drawn.height;
        const i = (top * png.width + MARGIN) * 4;
        const corner = [png.rgba[i], png.rgba[i + 1], png.rgba[i + 2]];
        assert.ok(bright(corner) < 80, `the map's off-disc corner is the editor's dark background, got ${corner}`);
        await restore(panel);
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  });
});

// ---- Strokes keep their colour on a small raster (#1366) -------------------

/** Each overlay layer, in a colour no other layer or the theme uses, with the
 *  opacity the panel draws it at. */
const LAYERS = [
  { flag: "coastlines", colour: "coastline", hex: "#ff0000", alpha: 1 },
  { flag: "borders", colour: "borders", hex: "#0000ff", alpha: 0.75 },
  { flag: "lakes", colour: "lakes", hex: "#ff00ff", alpha: 1 },
  { flag: "rivers", colour: "rivers", hex: "#00ffff", alpha: 1 },
  { flag: "graticule", colour: "graticule", hex: "#ffff00", alpha: 0.35 },
] as const;
const CONTOUR_HEX = "#00ff00";

const rgb = (h: string): number[] => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

suite("Export PNG strokes reach their layer's colour on a small raster (#1366)", () => {
  test("every overlay layer and the contours, in a dark and a light theme", async function () {
    this.timeout(240_000);
    for (const [theme, kind] of THEMES) {
      await inTheme(theme, async (restore) => {
        const p = await provider();
        const editor = await openEditor(p, path.join(FIXTURES, "eta_lambert_msg0.grib2"));
        const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
        try {
          const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
          assert.ok(drawn.width < 200, `a small raster (${drawn.width}x${drawn.height})`);
          await waitForRead(() => panel.drive([]), (s) => s.themeKind === kind, `the page in ${theme}`);
          const from = panel.sent.length;
          await panel.drive([
            ...LAYERS.flatMap((l) => [
              { id: "overlay-" + l.flag, checked: true },
              { id: "color-" + l.colour, value: l.hex },
            ]),
            { id: "color-contours", value: CONTOUR_HEX },
            { id: "overlay-contours", checked: true },
            { id: "contours-only", checked: true },
          ]);
          await panel.waitSent(isType("contourReady"), from, "the contours");
          const png = await exportedPng(panel);
          const top = png.height - MARGIN - drawn.height;
          const bg = modeColour(png, top, drawn);
          // How far the nearest pixel of the map is from the colour a stroke
          // at `alpha` makes over the background.
          const nearest = (hexColour: string, alpha: number): number => {
            const want = rgb(hexColour).map((v, k) => bg[k] + alpha * (v - bg[k]));
            let best = Infinity;
            for (let y = top; y < top + drawn.height; y++) {
              for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
                const i = (y * png.width + x) * 4;
                const d = Math.max(...want.map((v, k) => Math.abs(png.rgba[i + k] - v)));
                best = Math.min(best, d);
              }
            }
            return best;
          };
          const missed: string[] = [];
          for (const l of [...LAYERS, { flag: "contours", hex: CONTOUR_HEX, alpha: 0.9 }]) {
            const d = nearest(l.hex, l.alpha);
            if (d > 30) missed.push(`${l.flag} (${d.toFixed(0)}/255 off)`);
          }
          assert.deepStrictEqual(missed, [], `${theme}: layers that never reach their colour on a ${drawn.width}x${drawn.height} map`);
          await restore(panel);
        } finally {
          panel.dispose();
          editor.close();
        }
      });
    }
  });
});

// ---- Layer colours follow a theme change (#1340) ---------------------------

/** The GFS fixture twice over: a u/v pair on one grid, for the arrows. */
function pairFile(): string {
  const gfs = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  const one = fs.readFileSync(gfs);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1340-"));
  const file = path.join(dir, "gfs_pair.grib2");
  fs.writeFileSync(file, Buffer.concat([one, one]));
  return file;
}

/** Switch the workbench to `theme` and wait until the page shows `kind`. */
async function switchTheme(panel: RealPanel, theme: string, kind: string): Promise<void> {
  await vscode.workspace.getConfiguration("workbench").update("colorTheme", theme, vscode.ConfigurationTarget.Global);
  await waitForRead(() => panel.drive([]), (s) => s.themeKind === kind, `the page in ${theme}`);
}

/** Wait until every themed colour input, and the arrow key on the map, show the
 *  page's own foreground. */
async function expectFollowing(panel: RealPanel, kind: string, what: string): Promise<void> {
  await waitForRead(
    () => panel.drive([]),
    (s) =>
      s.themeKind === kind &&
      [s.coastlineColour, s.contoursColour, s.vectorsColour].every((c) => c !== null && hex(c) === hex(s.foreground)) &&
      s.vectorKey !== null &&
      hex(s.vectorKey.drawn.colour) === hex(s.foreground),
    `${what}: the coastline, contour and arrow colours follow ${kind}`,
  );
}

/** Export a PNG and check the strokes and the arrow key leave the map's
 *  background toward the foreground: lighter in a dark theme, darker in a light
 *  one. The Contours-only view leaves the map as one colour plus strokes. */
async function expectExported(
  panel: RealPanel,
  drawn: { width: number; height: number },
  dark: boolean,
  what: string,
): Promise<void> {
  const png = await exportedPng(panel);
  const top = png.height - MARGIN - drawn.height;
  const bg = modeColour(png, top, drawn);
  assert.strictEqual(bright(bg) < 80, dark, `${what}: the PNG's map background is the editor's (${bg})`);
  let reach = 0;
  let strokes = 0;
  for (let y = top; y < top + drawn.height; y++) {
    for (let x = MARGIN; x < MARGIN + drawn.width; x++) {
      const i = (y * png.width + x) * 4;
      const c = [png.rgba[i], png.rgba[i + 1], png.rgba[i + 2]];
      if (c[0] === bg[0] && c[1] === bg[1] && c[2] === bg[2]) continue;
      strokes += 1;
      reach = Math.max(reach, dark ? bright(c) - bright(bg) : bright(bg) - bright(c));
    }
  }
  assert.ok(strokes >= 10, `${what}: strokes are drawn (${strokes} px)`);
  assert.ok(reach >= 30, `${what}: strokes reach ${reach.toFixed(0)}/255 toward the foreground`);
  // The arrow key's shaft, in the PNG's own pixels.
  const state = await panel.drive([]);
  const key = state.exportedKey;
  assert.ok(key, `${what}: the export drew the arrow key`);
  // The shaft is a thin line: take the strongest pixel within a pixel or two
  // of its middle, toward the foreground.
  const mid = Math.round((key.tail + key.tip) / 2);
  let shaft = dark ? 0 : 255;
  for (let y = Math.round(key.y) - 2; y <= Math.round(key.y) + 2; y++) {
    for (let x = mid - 2; x <= mid + 2; x++) {
      const i = (y * png.width + x) * 4;
      const b = bright([png.rgba[i], png.rgba[i + 1], png.rgba[i + 2]]);
      shaft = dark ? Math.max(shaft, b) : Math.min(shaft, b);
    }
  }
  const keyReach = dark ? shaft - bright(bg) : bright(bg) - shaft;
  assert.ok(keyReach >= 30, `${what}: the key's shaft reaches ${keyReach.toFixed(0)}/255 toward the foreground in the PNG`);
}

suite("Layer colours follow a theme change (#1340)", () => {
  test("seeded colours follow a switch with the panel visible and again after a hide and show, on screen and in the PNG", async function () {
    this.timeout(300_000);
    await inTheme("Default Dark Modern", async (restore) => {
      const p = await provider();
      const editor = await openEditor(p, pairFile());
      const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
      try {
        const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
        await waitForRead(() => panel.drive([]), (s) => s.themeKind === "vscode-dark", "the page in a dark theme");
        let from = panel.sent.length;
        await panel.drive([
          { id: "overlay-coastlines", checked: true },
          { id: "vector-field-v", value: 1 },
          { id: "overlay-vectors", checked: true },
          { id: "overlay-contours", checked: true },
          { id: "contours-only", checked: true },
        ]);
        await panel.waitSent(isType("contourReady"), from, "the contours");
        await expectFollowing(panel, "vscode-dark", "dark at the start");

        // A switch with the panel visible.
        await switchTheme(panel, "Default Light Modern", "vscode-light");
        await expectFollowing(panel, "vscode-light", "light, panel visible");
        await expectExported(panel, drawn, false, "light, panel visible");

        // A hide and show rebuilds the page from its saved state; the seeded
        // colours must still count as the theme's, not the user's.
        from = panel.sent.length;
        await panel.hideShow();
        await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
        await expectFollowing(panel, "vscode-light", "light, after a hide and show");

        await switchTheme(panel, "Default Dark Modern", "vscode-dark");
        await expectFollowing(panel, "vscode-dark", "dark, after a hide and show");
        await expectExported(panel, drawn, true, "dark, after a hide and show");
        await restore(panel);
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  });

  test("a colour the user picked is kept across a theme switch and a hide and show; the rest follow", async function () {
    this.timeout(300_000);
    const PICKED = "#ff00ff";
    await inTheme("Default Dark Modern", async (restore) => {
      const p = await provider();
      const editor = await openEditor(p, pairFile());
      const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
      try {
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        await waitForRead(() => panel.drive([]), (s) => s.themeKind === "vscode-dark", "the page in a dark theme");
        let from = panel.sent.length;
        await panel.drive([
          { id: "overlay-coastlines", checked: true },
          { id: "vector-field-v", value: 1 },
          { id: "overlay-vectors", checked: true },
          { id: "overlay-contours", checked: true },
          { id: "color-contours", value: PICKED },
        ]);
        await panel.waitSent(isType("contourReady"), from, "the contours");
        const others = (s: { coastlineColour: string | null; vectorsColour: string | null; foreground: string }): boolean =>
          hex(s.coastlineColour ?? "") === hex(s.foreground) && hex(s.vectorsColour ?? "") === hex(s.foreground);
        const keptAnd = async (kind: string, what: string): Promise<void> => {
          const s = await waitForRead(() => panel.drive([]), (x) => x.themeKind === kind && others(x), `${what}: the others follow`);
          assert.strictEqual(hex(s.contoursColour ?? ""), PICKED, `${what}: the picked Contours colour is kept`);
        };
        await switchTheme(panel, "Default Light Modern", "vscode-light");
        await keptAnd("vscode-light", "light, visible");
        from = panel.sent.length;
        await panel.hideShow();
        await panel.waitSent(isType("gridReady"), from, "the render after a hide and show");
        await keptAnd("vscode-light", "light, after a hide and show");
        await switchTheme(panel, "Default Dark Modern", "vscode-dark");
        await keptAnd("vscode-dark", "dark, after a hide and show");
        await restore(panel);
      } finally {
        panel.dispose();
        editor.close();
      }
    });
  });
});

// ---- An export that waited on a scene change waits for its lines (#1379) ---

suite("Export PNG clicked during a projection change saves the new lines (#1379)", () => {
  test("the PNG equals one taken after everything settled, strokes included", async function () {
    this.timeout(180_000);
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    const savedDialog = vscode.window.showSaveDialog;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-contours", checked: true },
      ]);
      await panel.waitSent(isType("contourReady"), from, "the contours");
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");

      // One script, as a click that follows the change with no wait: the page
      // has asked for the render but not heard back when the export runs.
      from = panel.sent.length;
      const asked = panel.received.length;
      await panel.drive([
        { id: "picker-projection", value: "mollweide" },
        { id: "export-png", click: true },
      ]);
      const early = await waitUntil(() => panel.received.slice(asked).find(isType("exportPng")), "the early PNG");
      await panel.waitSent(isType("exportPngDone"), from, "the early export's outcome");

      const settledFrom = panel.received.length;
      const sentFrom = panel.sent.length;
      await panel.drive([{ id: "export-png", click: true }]);
      const settled = await waitUntil(() => panel.received.slice(settledFrom).find(isType("exportPng")), "the settled PNG");
      await panel.waitSent(isType("exportPngDone"), sentFrom, "the settled export's outcome");
      const a = decodePng(Buffer.from(String(early.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
      const b = decodePng(Buffer.from(String(settled.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64"));
      assert.deepStrictEqual([a.width, a.height], [b.width, b.height]);
      let differing = 0;
      for (let i = 0; i < a.rgba.length; i++) if (a.rgba[i] !== b.rgba[i]) differing++;
      assert.strictEqual(differing, 0, `${differing} channel values differ from the settled export`);
    } finally {
      vscode.window.showSaveDialog = savedDialog;
      panel.dispose();
      editor.close();
    }
  });
});
