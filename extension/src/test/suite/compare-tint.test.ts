// Field B's stepper box shows a tint in every theme (#1390, #1041).
//
// It sat on `--vscode-editorWidget-background`, the same token as the toolbar
// around it, so only its orange rule showed. The tint is now the rule's own
// orange; the two High Contrast themes, which draw borders rather than fills,
// also get a border. The page resolves its own colors, so this holds no copy
// of any theme's palette.

import * as assert from "assert";

import { inThemes, openAnimated } from "./themed-panel";

const THEMES = [
  ["Default Light Modern", "vscode-light"],
  ["Default Dark Modern", "vscode-dark"],
  ["Default High Contrast", "vscode-high-contrast"],
  ["Default High Contrast Light", "vscode-high-contrast-light"],
] as const;

/** The alpha of a computed color: `rgb()`, `rgba()` or `color(srgb …)`. */
const alpha = (css: string): number => {
  const m = /^(?:rgba?|color)\(([^)]*)\)$/.exec(css);
  assert.ok(m, `a color, got ${css}`);
  const slash = m[1].split("/");
  if (slash.length > 1) return Number(slash[1]);
  const parts = m[1].split(/[ ,]+/).filter(Boolean);
  return parts.length > 3 && !m[1].startsWith("srgb") ? Number(parts[3]) : 1;
};

suite("Field B's stepper box is tinted in every theme (#1390)", function () {
  this.timeout(90_000);

  test("its fill differs from the toolbar's, and High Contrast adds a border", async () => {
    await inThemes(await openAnimated(), THEMES, (state, theme) => {
      const box = state.compareBox;
      assert.ok(box, `the panel has Field B's box in ${theme}`);
      assert.ok(alpha(box.background) > 0, `${theme}: the box has a fill, got ${box.background}`);
      assert.notStrictEqual(box.background, box.toolbarBackground, `${theme}: the fill is not the toolbar's own`);
      if (state.themeKind.startsWith("vscode-high-contrast")) {
        assert.ok(Number.parseFloat(box.borderWidth) >= 1, `${theme}: a border is drawn`);
        assert.ok(alpha(box.border) > 0, `${theme}: the border is visible, got ${box.border}`);
      }
    });
  });
});
