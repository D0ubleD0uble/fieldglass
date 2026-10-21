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
      assert.ok(bgOnly / total > 0.5, `background with only on: ${bgOnly}/${total} pixels`);
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
          // The overlay is drawn at the panel's size and scaled down onto the
          // PNG's map, so a line is a faint blend of its colour and the
          // background. It still leaves the background in the direction of the
          // theme's foreground: lighter in a dark theme, darker in a light one.
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
