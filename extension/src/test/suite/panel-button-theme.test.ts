// The render panel's buttons take the VS Code theme's colors (#1118).
//
// They had no styling, so in a dark theme they drew as the browser's default
// light-gray buttons. Every `<button>` the panel writes (Export PNG, Export
// CSV, the Animate transport row, the zoom buttons) now takes the theme's
// secondary button colors, a focus ring from `--vscode-focusBorder`, and a
// dimmed look when disabled.
//
// Driven through the real panel in a light, a dark and the two
// high-contrast themes. The page resolves the theme's variables itself, with
// a probe element, so the test holds no copy of any theme's colors.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { renderHtml } from "../../provider";
import { PANEL_BUTTONS_SCRIPT, type PanelButton, type PageState, type RealPanel } from "./real-panel";
import { inThemes, inThemesRead, openAnimated } from "./themed-panel";

const THEMES = [
  ["Default Light Modern", "vscode-light"],
  ["Default Dark Modern", "vscode-dark"],
  ["Default High Contrast", "vscode-high-contrast"],
  ["Default High Contrast Light", "vscode-high-contrast-light"],
] as const;

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
/** The browser's own button background, which an unstyled button draws. */
const BROWSER_GRAY = "rgb(239, 239, 239)";

/** A focus ring shows only on a page that has focus. A panel's page does not
 *  have it when it first loads, so ask for it and wait until the page has it.
 *  `take` focuses the page's own tab: focusing the active editor group instead
 *  lands on whatever that group shows, which an earlier suite can leave as
 *  another editor (#1257). */
async function pageHasFocus(has: () => Promise<boolean>, take: () => void, what: string): Promise<void> {
  for (let i = 0; i < 50; i++) {
    if (await has()) return;
    take();
    await new Promise((r) => setTimeout(r, 100));
  }
  const active = vscode.window.tabGroups.activeTabGroup;
  throw new Error(
    `${what} never took focus, so no focus ring can show ` +
      `(window focused: ${vscode.window.state.focused}; active tab: ${active.activeTab?.label ?? "none"} ` +
      `in column ${active.viewColumn} of ${vscode.window.tabGroups.all.length})`,
  );
}

/** Focus each button in turn, as the Tab key would, and read its ring. */
async function assertFocusRing(panel: RealPanel, theme: string): Promise<void> {
  await pageHasFocus(
    async () => (await panel.drive([])).docHasFocus,
    () => panel.takeFocus(),
    `the render panel in ${theme}`,
  );
  // A button that is put away (the zoom buttons on a panel that cannot zoom)
  // cannot take focus; its colors are still checked.
  const shown = (await panel.drive([])).panelButtons.filter((b) => b.shown).map((b) => b.id);
  assert.ok(shown.includes("export-png") && shown.includes("anim-play"), `the toolbar buttons are on screen in ${theme}`);
  for (const id of shown) {
    const state = await panel.drive([{ id, focus: true }]);
    const b = state.panelButtons.find((x) => x.id === id);
    assert.ok(b?.focused, `#${id} takes focus in ${theme}`);
    assert.notStrictEqual(
      b.focusOutlineStyle,
      "none",
      `#${id} has a focus ring in ${theme} (document.hasFocus()=${state.docHasFocus}, activeElement=${state.focused})`,
    );
    assert.strictEqual(b.focusOutlineColor, b.wantFocus, `#${id}'s ring is --vscode-focusBorder in ${theme}`);
  }
}

/** The color checks every button gets, on either page. */
function assertButtons(all: PanelButton[], theme: string, kind: string): void {
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
  // A probe that resolved nothing would make the checks above vacuous. The
  // text color is inherited, so it proves nothing: the background must be
  // really defined in light and dark, and the border in high contrast.
  const [first] = all;
  assert.ok(first, `${theme} has buttons`);
  if (kind === "vscode-light") {
    assert.notStrictEqual(first.wantBackground, TRANSPARENT, `${theme} defines a button background`);
  }
  if (kind === "vscode-dark") {
    // Dark Modern's secondary button is transparent with a hairline border,
    // so what proves the probe resolved is the border, and a background that
    // is not the browser's own gray.
    assert.notStrictEqual(first.wantBorder, TRANSPARENT, `${theme} gives buttons a border`);
    assert.notStrictEqual(first.wantBackground, BROWSER_GRAY, `${theme} is not the browser's gray button`);
  }
  if (kind.startsWith("vscode-high-contrast")) {
    assert.notStrictEqual(first.wantBorder, TRANSPARENT, `${theme} gives buttons a visible border`);
  }
}

function assertThemed(state: PageState, theme: string): void {
  assert.deepStrictEqual(
    state.panelButtons.map((b) => b.id).sort(),
    [...IDS].sort(),
    `the panel's buttons are the ones this test knows (${theme}); a new button must be themed and listed`,
  );
  assertButtons(state.panelButtons, theme, state.themeKind);
}

suite("Render panel buttons follow the theme (#1118)", function () {
  this.timeout(90_000);

  test("every button takes the theme's colors in light, dark and high-contrast themes, and the themes differ", async () => {
    const seen = new Map<string, string>();
    await inThemes(
      await openAnimated(),
      THEMES,
      async (state, theme, panel) => {
        assertThemed(state, theme);
        await assertFocusRing(panel, theme);
        seen.set(theme, state.panelButtons[0].background);
      },
    );
    // Light and dark really are different colors, so following the theme is
    // not the same as drawing one color in both.
    assert.notStrictEqual(seen.get("Default Light Modern"), seen.get("Default Dark Modern"));
  });

  // The dump page's own buttons (Render, Export CSV, the NetCDF render
  // buttons) live in the editor's webview, so they are read from a real
  // webview panel showing the page `renderHtml` writes.
  test("the dump page's buttons take the theme's colors in the same themes", async () => {
    const native = loadNative();
    assert.ok(native, "native module required");
    const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
    assert.ok(ext, "extension installed");
    const fixtures = path.join(ext.extensionPath, "src", "test", "fixtures");
    const grib = path.join(fixtures, "regular_latlon_surface.grib2");
    const nc = path.join(fixtures, "netcdf_classic_dummy.nc");
    const messages = [native.open(fs.readFileSync(grib)).message(0)];
    const ncHandle = native.open(fs.readFileSync(nc));
    const variables = ncHandle.variables();
    assert.ok(variables.length > 0, "the NetCDF fixture has a renderable variable");

    const pages: Array<[string, (w: vscode.Webview) => string, string[]]> = [
      ["GRIB", (w) => renderHtml(w, "grib2", grib, messages, undefined, undefined, false), ["render-btn", "export-csv-btn"]],
      [
        "NetCDF",
        (w) => renderHtml(w, "netcdf", nc, undefined, ncHandle.metadata(), undefined, false, variables),
        ["netcdf-render-btn"],
      ],
    ];
    for (const [what, html, classes] of pages) {
      const panel = vscode.window.createWebviewPanel("fieldglass.test1118", what, vscode.ViewColumn.One, { enableScripts: true });
      const reads: Array<(m: { buttons: PanelButton[]; themeKind: string; docHasFocus?: boolean }) => void> = [];
      panel.webview.onDidReceiveMessage((m) => {
        if (m && m.type === "test:buttons") reads.splice(0).forEach((r) => r(m));
      });
      const page = html(panel.webview);
      const nonce = /<script nonce="([^"]+)">/.exec(page)?.[1];
      assert.ok(nonce, "the dump page has a nonce'd script");
      // The page's script asks for the VS Code API once; hand the same one
      // to the reader.
      const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
      const reader = `<script nonce="${nonce}">
        ${PANEL_BUTTONS_SCRIPT}
        const api = acquireVsCodeApi();
        window.addEventListener('message', (ev) => {
          if (!ev.data || ev.data.type !== 'test:read') return;
          // Open the first message's row, which holds its buttons.
          const row = document.querySelector('tr.msg-row');
          if (row && !document.querySelector('tr.expand-row:not([hidden])')) row.click();
          const buttons = panelButtons();
          const all = Array.from(document.querySelectorAll('button'));
          // Focus each shown button, as the Tab key would, and keep its ring.
          all.forEach((b, i) => {
            if (!b.getClientRects().length) return;
            window.focus();
            b.focus({ focusVisible: true });
            const s = getComputedStyle(b);
            buttons[i].focused = document.activeElement === b;
            buttons[i].focusOutlineColor = s.outlineColor;
            buttons[i].focusOutlineStyle = s.outlineStyle;
            b.blur();
          });
          api.postMessage({ type: 'test:buttons', buttons, docHasFocus: document.hasFocus(), themeKind: document.body.getAttribute('data-vscode-theme-kind') || document.body.className });
        });
      </script>`;
      panel.webview.html = page.replace("<head>", `<head>${memo}`).replace("</body>", `${reader}</body>`);
      const read = () =>
        new Promise<{ buttons: PanelButton[]; themeKind: string; docHasFocus?: boolean }>((resolve, reject) => {
          // The page may not be up yet, so ask again until it answers.
          const ask = setInterval(() => void panel.webview.postMessage({ type: "test:read" }), 200);
          const give = setTimeout(() => {
            clearInterval(ask);
            reject(new Error(`the ${what} dump page did not answer`));
          }, 15_000);
          reads.push((m) => {
            clearTimeout(give);
            clearInterval(ask);
            resolve(m);
          });
        });

      await inThemesRead(read, () => panel.dispose(), THEMES, async (first, theme) => {
        // The ring is read while the page is focused: read again until it is.
        let read1 = first;
        await pageHasFocus(
          async () => !!(read1.docHasFocus || (read1 = await read()).docHasFocus),
          () => panel.reveal(panel.viewColumn, false),
          `the ${what} dump page in ${theme}`,
        );
        const { buttons, themeKind } = read1;
        for (const cls of classes) {
          assert.ok(buttons.some((b) => b.id === cls), `the ${what} dump page has a .${cls} button in ${theme}`);
        }
        assertButtons(buttons, `${theme} (${what} dump page)`, themeKind);
        for (const b of buttons.filter((x) => x.focused)) {
          assert.notStrictEqual(b.focusOutlineStyle, "none", `${b.id} has a focus ring`);
          assert.strictEqual(b.focusOutlineColor, b.wantFocus, `${b.id}'s ring is --vscode-focusBorder`);
        }
        assert.ok(buttons.some((b) => b.focused), `a ${what} dump button took focus in ${theme}`);
      });
    }
  });
});
