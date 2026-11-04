// A GRIB message's row shows that it opens, and has its own Render button
// (#1628).
//
// A first-time user could not tell that a row in the message table opens, or
// that a message can be drawn: Render was only inside the expanded row. Each
// row that opens now has a pointer cursor and a chevron toggle: a disclosure
// button carrying `aria-expanded`, which Tab reaches and Enter or Space
// presses. A message that can be drawn has a Render button in its row, and one
// that can't shows why there instead, wrapped so it does not widen the table.
// One line above the table, which stays on screen as the table scrolls, says
// so.
//
// The table is the page the real editor writes, in a real webview, wired to
// the provider through its own message handler, so a click on Render opens a
// real render panel. A small reader added to the page reports what it shows
// and does what a user would.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { listMessages, renderHtml, type FieldglassDocument } from "../../provider";
import { extensionPath, interceptHtml, isType, openReal, provider, waitUntil } from "./real-panel";
import { pageHasFocus } from "./themed-panel";

const CRATES = path.join(extensionPath(), "..", "crates");

/** A drawable message, a Bi-Fourier one that can't be drawn, and another
 *  drawable one, then `more` copies of the first, for a page that scrolls. */
function mixedFile(dir: string, more = 0): string {
  const fixture = (crate: string, name: string) => fs.readFileSync(path.join(CRATES, crate, "tests", "fixtures", name));
  const file = path.join(dir, `rows-${more}.grib2`);
  const first = fixture("fieldglass-grib2", "regular_latlon_surface.grib2");
  fs.writeFileSync(
    file,
    Buffer.concat([
      first,
      fixture("fieldglass-grib2", "bifourier_ellipse_ieee32.grib2"),
      fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "eta_lambert_msg0.grib2")),
      ...Array.from({ length: more }, () => first),
    ]),
  );
  return file;
}

const BIFOURIER = "Render not available: Bi-Fourier synthesis isn't supported, so these coefficients can't be drawn.";

interface Row {
  index: number;
  /** The toggle's `aria-expanded`, `null` for a row with no toggle. */
  expanded: string | null;
  /** The row's toggle: its element, label and the id it controls. */
  toggle: { tag: string; type: string; label: string | null; controls: string | null } | null;
  /** Whether the row's expansion is on screen. */
  expansionShown: boolean;
  cursor: string;
  /** The chevron's drawn glyph, `null` for a row with none. */
  chevron: string | null;
  /** The row's own Render button, `null` for none. */
  render: { shown: boolean; type: string; disabled: boolean; text: string } | null;
  /** The note in the row in place of a Render button. */
  reason: string | null;
  reasonShown: boolean;
  /** The note's computed `white-space`, its width and its em, in pixels. */
  reasonBox: { whiteSpace: string; width: number; em: number } | null;
}

interface TableState {
  rows: Row[];
  hint: string | null;
  hintShown: boolean;
  /** The block holding the hint and the status line. */
  head: { position: string; top: string; background: string } | null;
  /** The page's status line for a GRIB render. */
  status: string | null;
  /** The order Tab moves through the table, as `toggle N`, `render N` and `csv N`. */
  tabOrder: string[];
  focused: string | null;
  docHasFocus: boolean;
  /** The focused element's outline style and color, and the color
   *  `--vscode-focusBorder` resolves to. */
  ring: { style: string; color: string; want: string } | null;
  /** Whether the last click hit what it aimed at. */
  hit: boolean | null;
  /** Where the last scroll put its element, and whether something is drawn
   *  over its center. */
  scrolled: { top: number; covered: boolean } | null;
  /** The focused element's top, and what is drawn at its center: itself
   *  (`true`), or something covering it, named. */
  focusedTop: number | null;
  atFocusedCenter: true | string | null;
  /** The sticky head's height and the page's scroll position. */
  headHeight: number | null;
  scrollY: number;
}

type Action =
  | { focus: string }
  | { click: string }
  /** Select an element's text, as a drag across it does. */
  | { select: string }
  /** Scroll the page so an element's top is this many pixels below the top
   *  of the view. */
  | { scrollTo: string; top: number }
  /** Press the focused element, as Enter or Space on a button does. */
  | { pressFocused: true };

// The reader. It runs after the page's own script, under its nonce.
const READER = `
  const api = acquireVsCodeApi();
  const label = (el) => {
    const row = el.closest('tr.msg-row');
    const idx = row ? row.getAttribute('data-message-index') : null;
    if (el.matches('tr.msg-row')) return 'row ' + idx;
    if (el.matches('button.row-toggle')) return 'toggle ' + el.getAttribute('data-message-index');
    if (el.matches('button.render-btn')) return 'render ' + el.getAttribute('data-message-index');
    if (el.matches('button.export-csv-btn')) return 'csv ' + el.getAttribute('data-message-index');
    return el.tagName.toLowerCase() + (el.className ? '.' + el.className : '');
  };
  const shown = (el) => !!el && el.getClientRects().length > 0;
  const probe = document.createElement('div');
  probe.style.color = 'var(--vscode-focusBorder)';
  document.body.appendChild(probe);
  function read() {
    const rows = Array.from(document.querySelectorAll('tr.msg-row')).map((row) => {
      const idx = Number(row.getAttribute('data-message-index'));
      const button = row.querySelector('button.render-btn');
      const note = row.querySelector('.render-na');
      const chevron = row.querySelector('.chevron');
      const content = chevron ? getComputedStyle(chevron, '::before').content : null;
      const toggle = row.querySelector('button.row-toggle');
      return {
        index: idx,
        expanded: toggle ? toggle.getAttribute('aria-expanded') : null,
        toggle: toggle
          ? { tag: toggle.tagName.toLowerCase(), type: toggle.type, label: toggle.getAttribute('aria-label'), controls: toggle.getAttribute('aria-controls') }
          : null,
        expansionShown: shown(document.getElementById('expand-' + idx)),
        cursor: getComputedStyle(row).cursor,
        chevron: content && content !== 'none' && shown(chevron) ? content.replace(/^"|"$/g, '') : null,
        render: button
          ? { shown: shown(button), type: button.type, disabled: button.disabled, text: button.textContent.trim() }
          : null,
        reason: note ? note.textContent.trim() : null,
        reasonShown: shown(note),
        reasonBox: note
          ? { whiteSpace: getComputedStyle(note).whiteSpace, width: note.getBoundingClientRect().width, em: parseFloat(getComputedStyle(note).fontSize) }
          : null,
      };
    });
    const hint = document.querySelector('.table-hint');
    const status = document.getElementById('grib-render-status');
    const head = document.querySelector('.table-head');
    const hs = head ? getComputedStyle(head) : null;
    // Every element in the table Tab stops at: focusable, on screen, not
    // taken out of the order. All are tabindex 0, so the order is the page's.
    const table = document.querySelector('table');
    const tabOrder = table
      ? Array.from(table.querySelectorAll('a[href], button, input, select, textarea, summary, [tabindex]'))
          .filter((el) => el.tabIndex >= 0 && !el.disabled && shown(el))
          .map(label)
      : [];
    const active = document.activeElement;
    const s = active && active !== document.body ? getComputedStyle(active) : null;
    let focusedTop = null;
    let atFocusedCenter = null;
    if (s) {
      const r = active.getBoundingClientRect();
      focusedTop = r.top;
      const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      atFocusedCenter = top && (top === active || active.contains(top))
        ? true
        : top ? top.tagName.toLowerCase() + (top.className ? '.' + top.className : '') : 'nothing';
    }
    return {
      rows,
      hint: hint ? hint.textContent.trim() : null,
      hintShown: shown(hint),
      head: hs ? { position: hs.position, top: hs.top, background: hs.backgroundColor } : null,
      headHeight: head ? head.getBoundingClientRect().height : null,
      scrollY: window.scrollY,
      focusedTop,
      atFocusedCenter,
      status: status ? status.textContent : null,
      tabOrder,
      focused: active && active !== document.body ? label(active) : null,
      docHasFocus: document.hasFocus(),
      ring: s ? { style: s.outlineStyle, color: s.outlineColor, want: getComputedStyle(probe).color } : null,
    };
  }
  window.addEventListener('message', (ev) => {
    const m = ev.data;
    if (!m || m.type !== 'test:table') return;
    let hit = null;
    let scrolled = null;
    let error = null;
    try {
      for (const a of m.actions) {
        if (a.focus) {
          window.focus();
          document.querySelector(a.focus).focus({ focusVisible: true });
        } else if (a.select) {
          const range = document.createRange();
          range.selectNodeContents(document.querySelector(a.select));
          const sel = window.getSelection();
          sel.removeAllRanges();
          sel.addRange(range);
        } else if (a.scrollTo) {
          const el = document.querySelector(a.scrollTo);
          window.scrollBy(0, el.getBoundingClientRect().top - a.top);
          const r = el.getBoundingClientRect();
          const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          scrolled = { top: r.top, covered: !(top && (top === el || el.contains(top))) };
        } else if (a.click) {
          // Click where a user would: the middle of the element, on whatever
          // is drawn there.
          const el = document.querySelector(a.click);
          const r = el.getBoundingClientRect();
          const x = r.left + r.width / 2;
          const y = r.top + r.height / 2;
          const top = document.elementFromPoint(x, y);
          hit = !!top && (top === el || el.contains(top));
          (top || el).dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, clientX: x, clientY: y }));
        } else if (a.pressFocused) {
          document.activeElement.click();
        }
      }
    } catch (e) {
      error = String(e);
    }
    api.postMessage({ type: 'test:table', tag: m.tag, error, state: Object.assign(read(), { hit, scrolled }) });
  });
`;

interface Table {
  act(actions: Action[]): Promise<TableState>;
  reveal(): void;
  close(): void;
}

/** The page with the reader added under its nonce. */
function withReader(html: string): string {
  const nonce = /<script nonce="([^"]+)">/.exec(html)?.[1];
  assert.ok(nonce, "the page has a nonce'd script");
  // The page's script asks for the VS Code API once; hand the same one to
  // the reader.
  const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
  return html.replace("<head>", `<head>${memo}`).replace("</body>", `<script nonce="${nonce}">${READER}</script></body>`);
}

/** `file` in the real editor, its page in a real webview with the reader on it. */
async function openTable(file: string): Promise<Table> {
  const p = await provider();
  const doc = (await p.openCustomDocument(
    vscode.Uri.file(file),
    {} as vscode.CustomDocumentOpenContext,
    new vscode.CancellationTokenSource().token,
  )) as FieldglassDocument;
  const panel = vscode.window.createWebviewPanel("fieldglass.test1628", "rows", vscode.ViewColumn.One, {
    enableScripts: true,
  });
  const table = tableOn(panel);
  interceptHtml(panel.webview, withReader);
  await p.resolveCustomEditor(doc, panel);
  return table;
}

/** A page `renderHtml` writes, in a real webview with the reader on it but
 *  no provider behind it: for a page the editor never writes today. */
function openPage(html: (w: vscode.Webview) => string): Table {
  const panel = vscode.window.createWebviewPanel("fieldglass.test1628", "rows", vscode.ViewColumn.One, {
    enableScripts: true,
  });
  const table = tableOn(panel);
  panel.webview.html = withReader(html(panel.webview));
  return table;
}

function tableOn(panel: vscode.WebviewPanel): Table {
  const replies: Array<{ tag: number; error: string | null; state: TableState }> = [];
  panel.webview.onDidReceiveMessage((m) => {
    if (m && m.type === "test:table") replies.push(m);
  });
  let tag = 0;
  return {
    act: async (actions) => {
      const mine = ++tag;
      // The page may not be up yet: ask until it answers.
      const ask = () => void panel.webview.postMessage({ type: "test:table", tag: mine, actions });
      ask();
      const again = setInterval(() => {
        if (actions.length === 0) ask();
      }, 200);
      try {
        const reply = await waitUntil(() => replies.find((r) => r.tag === mine), `the table's reply ${mine}`);
        if (reply.error) throw new Error(`the table's reply ${mine} failed: ${reply.error}`);
        return reply.state;
      } finally {
        clearInterval(again);
      }
    },
    reveal: () => panel.reveal(panel.viewColumn, false),
    close: () => panel.dispose(),
  };
}

const row = (i: number) => `tr.msg-row[data-message-index="${i}"]`;
const renderIn = (i: number) => `${row(i)} button.render-btn`;
const toggleIn = (i: number) => `${row(i)} button.row-toggle`;
const HINT =
  "Click a row for its details. Render draws the message in a new tab, on a map projection where the grid allows one.";

suite("A GRIB message's row shows that it opens and has its own Render (#1628)", function () {
  this.timeout(90_000);
  let dir: string;
  let file: string;
  let table: Table | undefined;

  setup(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1628-"));
    file = mixedFile(dir);
  });
  teardown(() => {
    table?.close();
    table = undefined;
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("Render in a row that was never opened draws the message in a panel", async () => {
    const p = await provider();
    table = await openTable(file);
    const before = await table.act([]);
    const first = before.rows[0];
    assert.ok(first.render, "message 0 has a Render button in its own row");
    assert.ok(first.render.shown, "and it is on screen without opening the row");
    assert.strictEqual(first.expansionShown, false, "the row is closed");

    let clicked: TableState | undefined;
    const t = table;
    const panel = await openReal(p, async () => {
      clicked = await t.act([{ click: renderIn(0) }]);
    });
    try {
      assert.ok(clicked?.hit, "the click landed on the button");
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the render of message 0");
      assert.strictEqual(drawn.messageIndex, 0, "the panel draws message 0");
      // The page paints after it reads the answer.
      let painted = false;
      for (let i = 0; i < 100 && !painted; i++) {
        painted = (await panel.drive([])).canvasPainted;
        if (!painted) await new Promise((r) => setTimeout(r, 100));
      }
      assert.ok(painted, "the field is drawn");
      const after = await t.act([]);
      assert.strictEqual(after.rows[0].expansionShown, false, "pressing Render did not open the row");
      assert.strictEqual(after.status, "Opened render of message 0 in a new tab.");
    } finally {
      panel.dispose();
    }
  });

  test("rows that open show it, and one line says what a click and Render do", async () => {
    table = await openTable(file);
    const state = await table.act([]);
    assert.strictEqual(state.hint, HINT);
    assert.ok(state.hintShown, "the hint is on screen");
    assert.ok(state.head, "the hint and the status line share a block");
    assert.strictEqual(state.head.position, "sticky", "which stays on screen as the table scrolls");
    assert.strictEqual(state.head.top, "0px");
    assert.notStrictEqual(state.head.background, "rgba(0, 0, 0, 0)", "over a background, so rows don't show through");
    for (const i of [0, 2]) {
      const r = state.rows[i];
      assert.deepStrictEqual(
        r.toggle,
        { tag: "button", type: "button", label: `Details for message ${i}`, controls: `expand-${i}` },
        `row ${i} has a disclosure button for its details`,
      );
      assert.strictEqual(r.expanded, "false", `row ${i}'s toggle says it is closed`);
      assert.strictEqual(r.cursor, "pointer", `row ${i} has a pointer cursor`);
      assert.strictEqual(r.chevron, "▸", `row ${i} has a closed chevron`);
      assert.deepStrictEqual(
        r.render,
        { shown: true, type: "button", disabled: false, text: "Render" },
        `row ${i} has an enabled Render button`,
      );
    }
    const opened = await table.act([{ click: `${row(0)} td:nth-child(5)` }]);
    assert.strictEqual(opened.rows[0].expanded, "true", "a click on the row opens it");
    assert.ok(opened.rows[0].expansionShown, "and shows its details");
    assert.strictEqual(opened.rows[0].chevron, "▾", "and the chevron turns");
    const closed = await table.act([{ click: toggleIn(0) }]);
    assert.ok(closed.hit, "the click landed on the chevron");
    assert.strictEqual(closed.rows[0].expanded, "false", "a click on the chevron closes it");
    assert.strictEqual(closed.rows[0].expansionShown, false);
  });

  test("a click that ends a text selection does not open the row", async () => {
    table = await openTable(file);
    const cell = `${row(0)} td:nth-child(4)`;
    const state = await table.act([{ select: cell }, { click: cell }]);
    assert.strictEqual(state.rows[0].expanded, "false", "selecting a cell's text to copy leaves the row closed");
    assert.strictEqual(state.rows[0].expansionShown, false);
  });

  test("a message that can't be drawn says why in its row, with no button", async () => {
    table = await openTable(file);
    const state = await table.act([]);
    const r = state.rows[1];
    assert.strictEqual(r.reason, BIFOURIER, "the row says why, in full");
    assert.ok(r.reasonShown, "without being opened");
    assert.ok(r.reasonBox, "the note is laid out");
    assert.strictEqual(r.reasonBox.whiteSpace, "normal", "it wraps");
    assert.ok(
      r.reasonBox.width <= 22 * r.reasonBox.em + 1,
      `within 22em, so it does not widen the table: ${JSON.stringify(r.reasonBox)}`,
    );
    assert.ok(
      r.reasonBox.width >= 16 * r.reasonBox.em - 1,
      `but at least 16em, so it is not squeezed to a word a line: ${JSON.stringify(r.reasonBox)}`,
    );
    assert.strictEqual(r.render, null, "and has no Render button");
    assert.strictEqual(r.toggle, null, "it does not open, having nothing more to show");
    assert.strictEqual(r.chevron, null, "so it has no chevron");
    assert.strictEqual(r.cursor, "auto", "and no pointer cursor");
    assert.ok(!state.tabOrder.some((s) => s.endsWith(" 1")), `and Tab passes it: ${state.tabOrder.join(", ")}`);
    const clicked = await table.act([{ click: `${row(1)} td:nth-child(5)` }]);
    assert.strictEqual(clicked.rows[1].expansionShown, false, "a click on it opens nothing");
  });

  test("Tab reaches each row's toggle and its Render button, and the keyboard opens a row and draws", async () => {
    const p = await provider();
    table = await openTable(file);
    const t = table;
    const state = await t.act([]);
    assert.deepStrictEqual(
      state.tabOrder,
      ["toggle 0", "render 0", "toggle 2", "render 2"],
      "Tab order through the table",
    );

    // A focus ring shows only on a page that has focus.
    let focused = await t.act([{ focus: toggleIn(0) }]);
    await pageHasFocus(
      async () => (focused = await t.act([{ focus: toggleIn(0) }])).docHasFocus,
      () => t.reveal(),
      "the table",
    );
    assert.strictEqual(focused.focused, "toggle 0");
    assert.ok(
      focused.ring && focused.ring.style !== "none",
      `the focused toggle has a ring: ${JSON.stringify(focused.ring)}`,
    );
    assert.strictEqual(focused.ring.color, focused.ring.want, "the ring is --vscode-focusBorder");

    // Enter or Space on a focused button presses it: the browser's own
    // activation, which a test cannot send as a real key. Pressing the focused
    // element is what that activation does.
    const opened = await t.act([{ pressFocused: true }]);
    assert.strictEqual(opened.rows[0].expanded, "true", "pressing the toggle opens the row");
    assert.ok(opened.rows[0].expansionShown);
    assert.deepStrictEqual(
      opened.tabOrder,
      ["toggle 0", "render 0", "csv 0", "toggle 2", "render 2"],
      "the open row's Export CSV comes next in the tab order",
    );
    const closed = await t.act([{ pressFocused: true }]);
    assert.strictEqual(closed.rows[0].expanded, "false", "pressing it again closes the row");
    assert.strictEqual(closed.rows[0].expansionShown, false);

    const panel = await openReal(p, async () => {
      await t.act([{ focus: renderIn(2) }, { pressFocused: true }]);
    });
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the render of message 2");
      assert.strictEqual(drawn.messageIndex, 2, "the panel draws message 2");
      const after = await t.act([]);
      assert.strictEqual(after.rows[2].expanded, "false", "pressing Render does not open its row");
    } finally {
      panel.dispose();
    }
  });

  test("Tab never stops on a control hidden under the hint that stays at the top", async () => {
    table = await openTable(mixedFile(dir, 60));
    const t = table;
    const start = await t.act([]);
    assert.ok(start.headHeight && start.headHeight > 0, "the hint block has a height");
    // Row 20's toggle just below the top of the view, where the hint and
    // status, which stay there, are drawn over it.
    const under = await t.act([{ scrollTo: toggleIn(20), top: 4 }]);
    assert.ok(under.scrolled, "the page scrolled");
    assert.ok(
      Math.abs(under.scrolled.top - 4) < 1 && under.scrolled.covered,
      `the toggle sits under the hint before it takes focus: ${JSON.stringify(under.scrolled)}`,
    );
    const focused = await t.act([{ focus: toggleIn(20) }]);
    assert.strictEqual(focused.focused, "toggle 20");
    assert.strictEqual(
      focused.atFocusedCenter,
      true,
      `the focused toggle is not covered: its top is at ${focused.focusedTop}px, the head is ${focused.headHeight}px tall`,
    );
  });

  test("a click in a row's P1 box leaves the row closed", async () => {
    // The P1 edit box is dormant: the editor writes its page with `editable`
    // false. The page `renderHtml` writes with it on still guards the click,
    // so this test writes that page directly, on a GRIB1 message given a
    // one-octet P1 as render.test.ts's dormant-path test does.
    const native = loadNative();
    assert.ok(native, "native binding required");
    const grib1 = path.join(extensionPath(), "src", "test", "fixtures", "cmc_wind_300_2010052400_p012.grib");
    const messages = listMessages(native.open(fs.readFileSync(grib1))).map((m) => ({
      ...m,
      identification: { edition: "grib1" as const, p1Octet: 4 },
    }));
    table = openPage((w) => renderHtml(w, "grib1", grib1, messages, undefined, undefined, true, undefined));
    const state = await table.act([]);
    assert.ok(state.rows[0].toggle, "the GRIB1 row opens, so a click on it could");
    const input = `${row(0)} input.p1-input`;
    const clicked = await table.act([{ click: input }]);
    assert.ok(clicked.hit, "the click landed in the P1 box");
    assert.strictEqual(clicked.rows[0].expanded, "false", "the row stays closed");
    assert.strictEqual(clicked.rows[0].expansionShown, false);
    const elsewhere = await table.act([{ click: `${row(0)} td:nth-child(2)` }]);
    assert.strictEqual(elsewhere.rows[0].expanded, "true", "while a click elsewhere on the row opens it");
  });
});
