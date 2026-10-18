// The render panel's buttons take the VS Code theme's colours (#1118).
//
// They had no styling, so in a dark theme they drew as the browser's default
// light-grey buttons. Every `<button>` the panel writes (Export PNG, Export
// CSV, the Animate transport row, the zoom buttons) now takes the theme's
// secondary button colours, a focus ring from `--vscode-focusBorder`, and a
// dimmed look when disabled.
//
// Driven through the real panel in a light, a dark and the two
// high-contrast themes. The page resolves the theme's variables itself, with
// a probe element, so the test holds no copy of any theme's colours.

import * as assert from "assert";

import { type PageState, type RealPanel } from "./real-panel";
import { inThemes, openAnimated } from "./themed-panel";

/** Every button the slice panel writes. */
const IDS = [
  "export-png",
  "export-csv",
  "anim-first",
  "anim-back",
  "anim-play",
  "anim-forward",
  "anim-last",
  "zoom-in",
  "zoom-out",
  "zoom-reset",
];

const TRANSPARENT = "rgba(0, 0, 0, 0)";

/** Focus each button in turn, as the Tab key would, and read its ring. */
async function assertFocusRing(panel: RealPanel, theme: string): Promise<void> {
  // A button that is put away (the zoom buttons on a panel that cannot zoom)
  // cannot take focus; its colours are still checked.
  const shown = (await panel.drive([])).panelButtons.filter((b) => b.shown).map((b) => b.id);
  assert.ok(shown.includes("export-png") && shown.includes("anim-play"), `the toolbar buttons are on screen in ${theme}`);
  for (const id of shown) {
    const state = await panel.drive([{ id, focus: true }]);
    const b = state.panelButtons.find((x) => x.id === id);
    assert.ok(b?.focused, `#${id} takes focus in ${theme}`);
    assert.notStrictEqual(b.focusOutlineStyle, "none", `#${id} has a focus ring in ${theme}`);
    assert.strictEqual(b.focusOutlineColor, b.wantFocus, `#${id}'s ring is --vscode-focusBorder in ${theme}`);
  }
}

function assertThemed(state: PageState, theme: string): void {
  const all = state.panelButtons;
  assert.deepStrictEqual(
    all.map((b) => b.id).sort(),
    [...IDS].sort(),
    `the panel's buttons are the ones this test knows (${theme}); a new button must be themed and listed`,
  );
  for (const b of all) {
    const at = `#${b.id} in ${theme}`;
    assert.strictEqual(b.background, b.wantBackground, `${at}: background is the theme's secondary button background`);
    assert.strictEqual(b.color, b.wantColor, `${at}: text is the theme's secondary button foreground`);
    assert.strictEqual(b.borderColor, b.wantBorder, `${at}: border is the theme's button or contrast border`);
    assert.ok(Number.parseFloat(b.opacity) === 1, `${at}: an enabled button is not dimmed, got ${b.opacity}`);
    assert.ok(
      Number.parseFloat(b.disabledOpacity) < 1,
      `${at}: a disabled button is dimmed, got ${b.disabledOpacity}`,
    );
  }
  // A probe that resolved nothing would make the checks above vacuous: the
  // theme must really define the colours the buttons are meant to take.
  assert.notStrictEqual(all[0].wantColor, TRANSPARENT, `${theme} defines a button foreground`);
}

suite("Render panel buttons follow the theme (#1118)", function () {
  this.timeout(90_000);

  test("every button takes the theme's colours in light, dark and high-contrast themes, and the themes differ", async () => {
    const seen = new Map<string, string>();
    await inThemes(
      await openAnimated(),
      [
        ["Default Light Modern", "vscode-light"],
        ["Default Dark Modern", "vscode-dark"],
        ["Default High Contrast", "vscode-high-contrast"],
        ["Default High Contrast Light", "vscode-high-contrast-light"],
      ],
      async (state, theme, panel) => {
        assertThemed(state, theme);
        await assertFocusRing(panel, theme);
        seen.set(theme, state.panelButtons[0].background);
      },
    );
    // Light and dark really are different colours, so following the theme is
    // not the same as drawing one colour in both.
    assert.notStrictEqual(seen.get("Default Light Modern"), seen.get("Default Dark Modern"));
  });
});
