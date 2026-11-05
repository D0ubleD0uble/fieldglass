// A GRIB message's row has its own Render button and does nothing else
// (#1628, #1702).
//
// A first-time user could not tell that a message can be drawn: Render was
// only inside an expanded row. #1628 put a Render button in every drawable
// row, and said why in the row of one that can't be drawn. It also made rows
// open to an Export CSV button. The owner judged that the render tab already
// has Export CSV, so #1702 took the opening away: no chevron, toggle or
// detail row, and nothing about a row looks or acts clickable. One line above
// the table, which stays on screen as the table scrolls, says what Render
// does.
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
import { classicNetcdf, extensionPath, interceptHtml, isType, openReal, provider, waitUntil } from "./real-panel";
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
  cursor: string;
  /** The row's own Render button, `null` for none. */
  render: { shown: boolean; type: string; disabled: boolean; text: string; label: string | null } | null;
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
  /** The order Tab moves through the table, as `render N` or the element. */
  tabOrder: string[];
  /** What would make a row look or act like it opens: any of these found is a
   *  defect. Elements with `aria-expanded` or `aria-controls`, a chevron, a
   *  detail row or an Export CSV button, and any style rule that names a
   *  message row (a pointer cursor or a hover highlight would be one). */
  opening: string[];
  /** The table body's markup, to show a click changed nothing. */
  tbody: string | null;
  focused: string | null;
  docHasFocus: boolean;
  /** The focused element's outline style and color, and the color
   *  `--vscode-focusBorder` resolves to. */
  ring: { style: string; color: string; want: string } | null;
  /** Whether the last click hit what it aimed at. */
  hit: boolean | null;
  /** Where the last scroll put its element, and whether something is drawn
   *  over its center. */
  scrolled: { top: number; inView: boolean; covered: boolean } | null;
  /** The focused element's top, and what is drawn at its center: itself
   *  (`true`), or something covering it, named. */
  focusedTop: number | null;
  atFocusedCenter: true | string | null;
  /** The sticky head's height and the page's scroll position. */
  headHeight: number | null;
  scrollY: number;
  /** The table's own header cells, as `[left, width]` and text, and the
   *  header row's top (#1716). */
  cols: { top: number; cells: number[][]; texts: string[] } | null;
  /** The copy of the header row that stays at the top while the table
   *  scrolls, `null` while none is shown: where it is, its cells, what is
   *  drawn at the middle of each cell (`true` for the copy itself), and how
   *  screen readers see it (#1716). */
  stuck: {
    top: number;
    bottom: number;
    background: string;
    ariaHidden: string | null;
    cells: number[][];
    texts: string[];
    onTop: Array<true | string>;
  } | null;
  /** The bottom of everything that stays at the top: the head, or the
   *  stuck header row under it. */
  stuckBottom: number;
}

type Action =
  | { focus: string }
  /** Focus without scrolling, then scroll the way Tab does: just far enough
   *  to bring the element into view, clear of `scroll-padding-top`. */
  | { focusNearest: string }
  | { click: string }
  /** Scroll the page so an element's top is this many pixels below the top
   *  of the view. */
  | { scrollTo: string; top: number }
  /** Press the focused element, as Enter or Space on a button does. */
  | { pressFocused: true }
  /** Scroll the table's box sideways to this many pixels. */
  | { sideways: number };

// The reader. It runs after the page's own script, under its nonce.
const READER = `
  const api = acquireVsCodeApi();
  const label = (el) => {
    if (el.matches('button.render-btn')) return 'render ' + el.getAttribute('data-message-index');
    return el.tagName.toLowerCase() + (el.className ? '.' + el.className : '');
  };
  const shown = (el) => !!el && el.getClientRects().length > 0;
  const probe = document.createElement('div');
  probe.style.color = 'var(--vscode-focusBorder)';
  document.body.appendChild(probe);
  // Every selector in the page's own style sheets that names a message row.
  function rowRules() {
    const found = [];
    for (const sheet of Array.from(document.styleSheets)) {
      let rules;
      try { rules = Array.from(sheet.cssRules); } catch (e) { continue; }
      for (const rule of rules) {
        if (rule.selectorText && rule.selectorText.includes('msg-row')) found.push(rule.selectorText);
      }
    }
    return found;
  }
  // The table whose header row \`cols\` reports: the first unless asked.
  let colsOf = null;
  function read() {
    const rows = Array.from(document.querySelectorAll('tr.msg-row')).map((row) => {
      const idx = Number(row.getAttribute('data-message-index'));
      const button = row.querySelector('button.render-btn');
      const note = row.querySelector('.render-na');
      return {
        index: idx,
        cursor: getComputedStyle(row).cursor,
        render: button
          ? { shown: shown(button), type: button.type, disabled: button.disabled, text: button.textContent.trim(), label: button.getAttribute('aria-label') }
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
    const opening = [];
    for (const sel of ['[aria-expanded]', '[aria-controls]', '.chevron', '.chevron-cell', '.row-toggle', '.expand-row', 'button.export-csv-btn', '[data-opens]', '.selected']) {
      if (document.querySelector(sel)) opening.push(sel);
    }
    for (const rule of rowRules()) opening.push('rule ' + rule);
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
    const tbody = document.querySelector('tbody');
    const edges = (cells) => cells.map((c) => {
      const r = c.getBoundingClientRect();
      return [Math.round(r.left * 10) / 10, Math.round(r.width * 10) / 10];
    });
    const colsTable = colsOf ? document.querySelector(colsOf) : table;
    const realHead = colsTable ? colsTable.querySelector('thead') : null;
    const realCells = realHead ? Array.from(realHead.querySelectorAll('th')) : [];
    const cols = realHead
      ? { top: realHead.getBoundingClientRect().top, cells: edges(realCells), texts: realCells.map((c) => c.textContent.trim()) }
      : null;
    // The one copy shown: tables pass under the top one at a time.
    const box = Array.from(document.querySelectorAll('.stuck-cols')).find(shown) || null;
    let stuck = null;
    if (box) {
      const b = box.getBoundingClientRect();
      const cells = Array.from(box.querySelectorAll('th'));
      stuck = {
        top: b.top,
        bottom: b.bottom,
        background: getComputedStyle(box).backgroundColor,
        ariaHidden: box.getAttribute('aria-hidden'),
        cells: edges(cells),
        texts: cells.map((c) => c.textContent.trim()),
        onTop: cells.map((c) => {
          const r = c.getBoundingClientRect();
          const x = Math.min(Math.max(r.left + r.width / 2, b.left + 1), b.right - 1);
          const at = document.elementFromPoint(x, r.top + r.height / 2);
          return at && box.contains(at) ? true : at ? at.tagName.toLowerCase() + (at.className ? '.' + at.className : '') : 'nothing';
        }),
      };
    }
    const headBottom = head ? head.getBoundingClientRect().bottom : 0;
    return {
      cols,
      stuck,
      stuckBottom: Math.max(headBottom, stuck ? stuck.bottom : 0),
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
      opening,
      tbody: tbody ? tbody.innerHTML : null,
      focused: active && active !== document.body ? label(active) : null,
      docHasFocus: document.hasFocus(),
      ring: s ? { style: s.outlineStyle, color: s.outlineColor, want: getComputedStyle(probe).color } : null,
    };
  }
  window.addEventListener('message', (ev) => {
    const m = ev.data;
    if (!m || m.type !== 'test:table') return;
    colsOf = m.colsOf || null;
    let hit = null;
    let scrolled = null;
    let error = null;
    try {
      for (const a of m.actions) {
        if (a.focus) {
          window.focus();
          document.querySelector(a.focus).focus({ focusVisible: true });
        } else if (a.focusNearest) {
          window.focus();
          const el = document.querySelector(a.focusNearest);
          el.focus({ focusVisible: true, preventScroll: true });
          el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        } else if (a.scrollTo) {
          const el = document.querySelector(a.scrollTo);
          window.scrollBy(0, el.getBoundingClientRect().top - a.top);
          const r = el.getBoundingClientRect();
          const x = r.left + r.width / 2;
          const y = r.top + r.height / 2;
          const inView = x >= 0 && y >= 0 && x < window.innerWidth && y < window.innerHeight;
          const top = inView ? document.elementFromPoint(x, y) : null;
          scrolled = { top: r.top, inView, covered: !!top && top !== el && !el.contains(top) };
        } else if (a.click) {
          // Click where a user would: the middle of the element, on whatever
          // is drawn there. Bring it into view first, as a user would scroll
          // to it: CI's window is small, and the table is wider than it.
          const el = document.querySelector(a.click);
          el.scrollIntoView({ block: 'center', inline: 'center' });
          const r = el.getBoundingClientRect();
          const x = r.left + r.width / 2;
          const y = r.top + r.height / 2;
          const inView = x >= 0 && y >= 0 && x < window.innerWidth && y < window.innerHeight;
          const top = inView ? document.elementFromPoint(x, y) : null;
          hit = !!top && (top === el || el.contains(top));
          for (const type of ['mousedown', 'mouseup', 'click']) {
            (top || el).dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, detail: 1, clientX: x, clientY: y }));
          }
        } else if (a.pressFocused) {
          document.activeElement.click();
        } else if (typeof a.sideways === 'number') {
          document.querySelector('.table-scroll').scrollLeft = a.sideways;
        }
      }
    } catch (e) {
      error = String(e);
    }
    api.postMessage({ type: 'test:table', tag: m.tag, error, state: Object.assign(read(), { hit, scrolled }) });
  });
`;

interface Table {
  /** Do `actions`, then read the page. `cols` is the header row of the table
   *  `colsOf` selects, the page's first table if none. */
  act(actions: Action[], colsOf?: string): Promise<TableState>;
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
    act: async (actions, colsOf) => {
      const mine = ++tag;
      // The page may not be up yet: ask until it answers.
      const ask = () => void panel.webview.postMessage({ type: "test:table", tag: mine, actions, colsOf });
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

/** Each header cell's `[left, width]` within half a pixel of its column's. */
function assertOver(got: number[][], want: number[][], what: string): void {
  const lined = got.length === want.length && got.every((g, k) => Math.abs(g[0] - want[k][0]) <= 0.5 && Math.abs(g[1] - want[k][1]) <= 0.5);
  assert.ok(lined, `${what}: ${JSON.stringify(got)} over ${JSON.stringify(want)}`);
}

const row = (i: number) => `tr.msg-row[data-message-index="${i}"]`;
const renderIn = (i: number) => `${row(i)} button.render-btn`;
const HINT = "Render draws the message in a new tab, on a map projection where the grid allows one.";

suite("A GRIB message's row has its own Render and does nothing else (#1628, #1702)", function () {
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

  test("Render in a row draws the message in a panel", async () => {
    const p = await provider();
    table = await openTable(file);
    const before = await table.act([]);
    const first = before.rows[0];
    assert.ok(first.render, "message 0 has a Render button in its own row");
    assert.ok(first.render.shown, "and it is on screen");

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
      assert.strictEqual(after.status, "Opened render of message 0 in a new tab.");
    } finally {
      panel.dispose();
    }
  });

  test("one line says what Render does, and no row opens or looks as if it does", async () => {
    table = await openTable(file);
    const state = await table.act([]);
    assert.strictEqual(state.hint, HINT);
    assert.ok(state.hintShown, "the hint is on screen");
    assert.ok(state.head, "the hint and the status line share a block");
    assert.strictEqual(state.head.position, "sticky", "which stays on screen as the table scrolls");
    assert.strictEqual(state.head.top, "0px");
    assert.notStrictEqual(state.head.background, "rgba(0, 0, 0, 0)", "over a background, so rows don't show through");
    assert.deepStrictEqual(state.opening, [], "no chevron, toggle, detail row, Export CSV, or row style");
    for (const r of state.rows) assert.strictEqual(r.cursor, "auto", `row ${r.index} has no pointer cursor`);
    for (const i of [0, 2]) {
      assert.deepStrictEqual(
        state.rows[i].render,
        { shown: true, type: "button", disabled: false, text: "Render", label: `Render message ${i}` },
        `row ${i} has an enabled Render button`,
      );
    }
  });

  test("a click on a row, drawable or not, changes nothing", async () => {
    table = await openTable(file);
    const before = await table.act([]);
    // The last cell of each row, which is data and not a control.
    for (const i of [0, 1, 2]) {
      const clicked = await table.act([{ click: `${row(i)} td:last-child` }]);
      assert.ok(clicked.hit, `the click landed on row ${i}`);
      assert.strictEqual(clicked.tbody, before.tbody, `row ${i} is as it was`);
      assert.deepStrictEqual(clicked.opening, [], `and nothing opened for row ${i}`);
      assert.strictEqual(clicked.status, before.status, "and the status line says nothing new");
    }
  });

  test("a message that can't be drawn says why in its row, with no button", async () => {
    table = await openTable(file);
    const state = await table.act([]);
    const r = state.rows[1];
    assert.strictEqual(r.reason, BIFOURIER, "the row says why, in full");
    assert.ok(r.reasonShown, "on screen");
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
    assert.ok(!state.tabOrder.some((s) => s.endsWith(" 1")), `and Tab passes it: ${state.tabOrder.join(", ")}`);
  });

  test("Tab reaches each drawable row's Render and nothing else, and the keyboard draws", async () => {
    const p = await provider();
    table = await openTable(file);
    const t = table;
    const state = await t.act([]);
    assert.deepStrictEqual(state.tabOrder, ["render 0", "render 2"], "Tab order through the table");

    // A focus ring shows only on a page that has focus.
    let focused = await t.act([{ focus: renderIn(0) }]);
    await pageHasFocus(
      async () => (focused = await t.act([{ focus: renderIn(0) }])).docHasFocus,
      () => t.reveal(),
      "the table",
    );
    assert.strictEqual(focused.focused, "render 0");
    assert.ok(
      focused.ring && focused.ring.style !== "none",
      `the focused Render has a ring: ${JSON.stringify(focused.ring)}`,
    );
    assert.strictEqual(focused.ring.color, focused.ring.want, "the ring is --vscode-focusBorder");

    // Enter or Space on a focused button presses it: the browser's own
    // activation, which a test cannot send as a real key. Pressing the focused
    // element is what that activation does.
    const panel = await openReal(p, async () => {
      await t.act([{ focus: renderIn(2) }, { pressFocused: true }]);
    });
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the render of message 2");
      assert.strictEqual(drawn.messageIndex, 2, "the panel draws message 2");
      const after = await t.act([]);
      assert.deepStrictEqual(after.opening, [], "pressing Render opens no row");
    } finally {
      panel.dispose();
    }
  });

  test("Tab never stops on a control hidden under the hint that stays at the top", async () => {
    table = await openTable(mixedFile(dir, 60));
    const t = table;
    const start = await t.act([]);
    assert.ok(start.headHeight && start.headHeight > 0, "the hint block has a height");
    // Row 20's Render just below the top of the view, where the hint and
    // status, which stay there, are drawn over it.
    const under = await t.act([{ scrollTo: renderIn(20), top: 4 }]);
    assert.ok(under.scrolled, "the page scrolled");
    assert.ok(
      Math.abs(under.scrolled.top - 4) < 1 && under.scrolled.inView && under.scrolled.covered,
      `the button sits under the hint before it takes focus: ${JSON.stringify(under.scrolled)}`,
    );
    await t.act([{ focus: renderIn(20) }]);
    // Read once the page has placed what stays at the top for the new scroll.
    const focused = await t.act([]);
    assert.strictEqual(focused.focused, "render 20");
    assert.ok(focused.stuck, "the column headers stay at the top too (#1716)");
    assert.strictEqual(
      focused.atFocusedCenter,
      true,
      `the focused button is not covered: its top is at ${focused.focusedTop}px, the head and headers end at ${focused.stuckBottom}px`,
    );
    // Tab scrolls a control just into view, so it lands exactly at the scroll
    // padding. The focus ring is a 1px outline offset 1px, reaching 2px above
    // the button's box: that line must clear the head and the column headers
    // under it.
    await t.act([{ scrollTo: renderIn(30), top: 4 }]);
    await t.act([{ focusNearest: renderIn(30) }]);
    const tabbed = await t.act([]);
    assert.strictEqual(tabbed.focused, "render 30");
    assert.ok(tabbed.stuck, "with the column headers at the top");
    assert.ok(
      tabbed.focusedTop !== null && tabbed.focusedTop - 2 >= tabbed.stuckBottom,
      `the focus ring's top line is not under the head or the headers: the button's top is ${tabbed.focusedTop}px, they end at ${tabbed.stuckBottom}px`,
    );
    assert.strictEqual(tabbed.atFocusedCenter, true, "and nothing is drawn over the button");
  });

  test("the column headers stay in view under the hint as the table scrolls (#1716)", async () => {
    table = await openTable(mixedFile(dir, 60));
    const t = table;
    const start = await t.act([]);
    assert.ok(start.cols, "the table has a header row");
    assert.strictEqual(start.stuck, null, "at the top of the page the table's own headers show, and no copy");
    assert.ok(start.headHeight !== null);

    // Row 30 a little below the head: the table's own header row has gone
    // up under the head, far above.
    await t.act([{ scrollTo: renderIn(30), top: 200 }]);
    const mid = await t.act([]);
    assert.ok(mid.cols && mid.cols.top < 0, `the table's own header row has scrolled away: ${mid.cols?.top}`);
    const s = mid.stuck;
    assert.ok(s, "a header row stays in view");
    assert.deepStrictEqual(s.texts, mid.cols.texts, "it is the table's header row");
    assert.ok(
      Math.abs(s.top - (mid.headHeight ?? 0)) < 1,
      `right under the hint and status: it starts at ${s.top}px, the head is ${mid.headHeight}px tall`,
    );
    assert.strictEqual(s.ariaHidden, "true", "screen readers read the table's own headers, not the copy");
    assert.notStrictEqual(s.background, "rgba(0, 0, 0, 0)", "over a background, so rows don't show through");
    assertOver(s.cells, mid.cols.cells, "each header sits over its own column");
    const drawn = s.onTop.filter((v) => v !== true);
    assert.deepStrictEqual(drawn, [], "and nothing is drawn over a header that is in the window");

    // The table is wider than CI's window: scrolled sideways, the headers
    // move with their columns.
    await t.act([{ sideways: 150 }]);
    const side = await t.act([]);
    assert.ok(side.stuck && side.cols, "the headers still stay at the top");
    assert.notDeepStrictEqual(side.cols.cells[0], mid.cols.cells[0], "the table moved sideways");
    assertOver(side.stuck.cells, side.cols.cells, "and the headers moved with it");

    // Back at the top, the copy goes and the table's own row shows.
    await t.act([{ scrollTo: row(0), top: 600 }]);
    const back = await t.act([]);
    assert.strictEqual(back.stuck, null, "no copy once the table's own header row is in view");
  });

  test("a click in a row's P1 box changes nothing", async () => {
    // The P1 edit box is dormant: the editor writes its page with `editable`
    // false. The page `renderHtml` writes with it on is checked by writing
    // that page directly, on a GRIB1 message given a one-octet P1 as
    // render.test.ts's dormant-path test does.
    const native = loadNative();
    assert.ok(native, "native binding required");
    const grib1 = path.join(extensionPath(), "src", "test", "fixtures", "cmc_wind_300_2010052400_p012.grib");
    const messages = listMessages(native.open(fs.readFileSync(grib1))).map((m) => ({
      ...m,
      identification: { edition: "grib1" as const, p1Octet: 4 },
    }));
    table = openPage((w) => renderHtml(w, "grib1", grib1, messages, undefined, undefined, true, undefined));
    const before = await table.act([]);
    assert.ok(before.rows[0].render, "the GRIB1 row has its Render button");
    const input = `${row(0)} input.p1-input`;
    const clicked = await table.act([{ click: input }]);
    assert.ok(clicked.hit, "the click landed in the P1 box");
    assert.strictEqual(clicked.tbody, before.tbody, "and the table is as it was");
    assert.deepStrictEqual(clicked.opening, []);
    assert.ok(clicked.tabOrder.includes("input.p1-input"), "the box is still in the tab order");
  });
});

// The NetCDF page's tables keep their column headers in view the same way
// (#1716). A file of many variables makes the Variables table long, and its
// "+N more" attributes are the controls Tab stops at in it.
suite("A long NetCDF table keeps its column headers in view (#1716)", function () {
  this.timeout(90_000);
  let dir: string;
  let table: Table | undefined;

  setup(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1716-"));
  });
  teardown(() => {
    table?.close();
    table = undefined;
    fs.rmSync(dir, { recursive: true, force: true });
  });

  test("the Variables headers stay at the top, and Tab lands below them", async () => {
    const file = path.join(dir, "many.nc");
    const attrs = { a: "1", b: "2", c: "3", d: "4", units: "K" };
    fs.writeFileSync(
      file,
      classicNetcdf(
        [["n", 2]],
        Array.from({ length: 60 }, (_, k) => ({ name: `v${k}`, dims: ["n"], attrs, values: [k, k] })),
      ),
    );
    table = await openTable(file);
    const t = table;
    // The Variables table is the one with five columns.
    const vars = ".table-scroll:has(th:nth-child(5)) > table";
    const more = (i: number) => `${vars} tbody tr:nth-child(${i + 1}) details.more-attrs > summary`;
    const start = await t.act([], vars);
    assert.ok(start.cols, "the Variables table has a header row");
    assert.deepStrictEqual(start.cols.texts, ["Name", "Type", "Units", "Dimensions", "Attributes"]);

    await t.act([{ scrollTo: more(40), top: 200 }]);
    const mid = await t.act([], vars);
    assert.ok(mid.cols && mid.cols.top < 0, `the table's own header row has scrolled away: ${mid.cols?.top}`);
    assert.ok(mid.stuck, "a header row stays in view");
    assert.deepStrictEqual(mid.stuck.texts, mid.cols.texts, "it is the Variables table's");
    assert.ok(Math.abs(mid.stuck.top) < 1, `at the top of the window: ${mid.stuck.top}`);
    assertOver(mid.stuck.cells, mid.cols.cells, "each header sits over its own column");
    assert.deepStrictEqual(mid.stuck.onTop.filter((v) => v !== true), [], "nothing is drawn over it");

    await t.act([{ scrollTo: more(41), top: 4 }]);
    await t.act([{ focusNearest: more(41) }]);
    const tabbed = await t.act([], vars);
    assert.strictEqual(tabbed.focused, "summary");
    assert.ok(tabbed.stuck, "with the headers at the top");
    assert.ok(
      tabbed.focusedTop !== null && tabbed.focusedTop - 2 >= tabbed.stuckBottom,
      `the focus ring's top line is not under the headers: the control's top is ${tabbed.focusedTop}px, they end at ${tabbed.stuckBottom}px`,
    );
    assert.strictEqual(tabbed.atFocusedCenter, true, "and nothing is drawn over it");
  });
});
