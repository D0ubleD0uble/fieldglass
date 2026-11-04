// A GRIB message's row shows that it opens, and has its own Render button
// (#1628).
//
// A first-time user could not tell that a row in the message table opens, or
// that a message can be drawn: Render was only inside the expanded row. Each
// row that opens now has a pointer cursor, a focus ring, a chevron and
// `aria-expanded`, and Tab reaches it; Enter or Space opens it. A message that
// can be drawn has a Render button in its row, and one that can't shows why
// there instead. One line above the table says so.
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

import type { FieldglassDocument } from "../../provider";
import { extensionPath, interceptHtml, isType, openReal, provider, waitUntil } from "./real-panel";
import { pageHasFocus } from "./themed-panel";

const CRATES = path.join(extensionPath(), "..", "crates");

/** A drawable message, a Bi-Fourier one that can't be drawn, and another
 *  drawable one. */
function mixedFile(dir: string): string {
  const fixture = (crate: string, name: string) => fs.readFileSync(path.join(CRATES, crate, "tests", "fixtures", name));
  const file = path.join(dir, "rows.grib2");
  fs.writeFileSync(
    file,
    Buffer.concat([
      fixture("fieldglass-grib2", "regular_latlon_surface.grib2"),
      fixture("fieldglass-grib2", "bifourier_ellipse_ieee32.grib2"),
      fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "eta_lambert_msg0.grib2")),
    ]),
  );
  return file;
}

const BIFOURIER = "Render not available: Bi-Fourier synthesis isn't supported, so these coefficients can't be drawn.";

interface Row {
  index: number;
  /** `aria-expanded`, `null` for a row that does not open. */
  expanded: string | null;
  /** Whether the row's expansion is on screen. */
  expansionShown: boolean;
  tabIndex: number;
  cursor: string;
  /** The chevron's drawn glyph, `null` for a row with none. */
  chevron: string | null;
  /** The row's own Render button, `null` for none. */
  render: { shown: boolean; type: string; disabled: boolean; text: string } | null;
  /** The note in the row in place of a Render button. */
  reason: string | null;
  reasonShown: boolean;
}

interface TableState {
  rows: Row[];
  hint: string | null;
  hintShown: boolean;
  /** The page's status line for a GRIB render. */
  status: string | null;
  /** The order Tab moves through the table, as `row N`, `render N` and `csv N`. */
  tabOrder: string[];
  focused: string | null;
  docHasFocus: boolean;
  /** The focused element's outline style and color, and the color
   *  `--vscode-focusBorder` resolves to. */
  ring: { style: string; color: string; want: string } | null;
  /** What the last action reported: whether a key's default was prevented,
   *  and whether a click hit what it aimed at. */
  prevented: boolean | null;
  hit: boolean | null;
}

type Action =
  | { focus: string }
  | { key: "Enter" | " "; at?: string }
  | { click: string }
  /** Press the focused element, as Enter or Space on a button does. */
  | { pressFocused: true };

// The reader. It runs after the page's own script, under its nonce.
const READER = `
  const api = acquireVsCodeApi();
  const label = (el) => {
    const row = el.closest('tr.msg-row');
    const idx = row ? row.getAttribute('data-message-index') : null;
    if (el.matches('tr.msg-row')) return 'row ' + idx;
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
      return {
        index: idx,
        expanded: row.getAttribute('aria-expanded'),
        expansionShown: shown(document.getElementById('expand-' + idx)),
        tabIndex: row.tabIndex,
        cursor: getComputedStyle(row).cursor,
        chevron: content && content !== 'none' && shown(chevron) ? content.replace(/^"|"$/g, '') : null,
        render: button
          ? { shown: shown(button), type: button.type, disabled: button.disabled, text: button.textContent.trim() }
          : null,
        reason: note ? note.textContent.trim() : null,
        reasonShown: shown(note),
      };
    });
    const hint = document.querySelector('.table-hint');
    const status = document.getElementById('grib-render-status');
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
    return {
      rows,
      hint: hint ? hint.textContent.trim() : null,
      hintShown: shown(hint),
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
    let prevented = null;
    let hit = null;
    let error = null;
    try {
      for (const a of m.actions) {
        if (a.focus) {
          window.focus();
          document.querySelector(a.focus).focus({ focusVisible: true });
        } else if (a.key) {
          const el = a.at ? document.querySelector(a.at) : document.activeElement;
          const ev = new KeyboardEvent('keydown', { key: a.key, code: a.key === ' ' ? 'Space' : a.key, bubbles: true, cancelable: true });
          el.dispatchEvent(ev);
          prevented = ev.defaultPrevented;
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
    api.postMessage({ type: 'test:table', tag: m.tag, error, state: Object.assign(read(), { prevented, hit }) });
  });
`;

interface Table {
  act(actions: Action[]): Promise<TableState>;
  reveal(): void;
  close(): void;
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
  const replies: Array<{ tag: number; error: string | null; state: TableState }> = [];
  panel.webview.onDidReceiveMessage((m) => {
    if (m && m.type === "test:table") replies.push(m);
  });
  interceptHtml(panel.webview, (html) => {
    const nonce = /<script nonce="([^"]+)">/.exec(html)?.[1];
    assert.ok(nonce, "the page has a nonce'd script");
    // The page's script asks for the VS Code API once; hand the same one to
    // the reader.
    const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
    return html.replace("<head>", `<head>${memo}`).replace("</body>", `<script nonce="${nonce}">${READER}</script></body>`);
  });
  await p.resolveCustomEditor(doc, panel);
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
    assert.strictEqual(state.hint, "Click a row for its details. Render draws the message.");
    assert.ok(state.hintShown, "the hint is on screen");
    for (const i of [0, 2]) {
      const r = state.rows[i];
      assert.strictEqual(r.expanded, "false", `row ${i} says it is closed`);
      assert.strictEqual(r.cursor, "pointer", `row ${i} has a pointer cursor`);
      assert.strictEqual(r.chevron, "▸", `row ${i} has a closed chevron`);
      assert.strictEqual(r.tabIndex, 0, `row ${i} is in the tab order`);
      assert.deepStrictEqual(
        r.render,
        { shown: true, type: "button", disabled: false, text: "Render" },
        `row ${i} has an enabled Render button`,
      );
    }
    const opened = await table.act([{ click: `${row(0)} td:nth-child(5)` }]);
    assert.strictEqual(opened.rows[0].expanded, "true", "a click opens the row");
    assert.ok(opened.rows[0].expansionShown, "and shows its details");
    assert.strictEqual(opened.rows[0].chevron, "▾", "and the chevron turns");
  });

  test("a message that can't be drawn says why in its row, with no button", async () => {
    table = await openTable(file);
    const state = await table.act([]);
    const r = state.rows[1];
    assert.strictEqual(r.reason, BIFOURIER, "the row says why");
    assert.ok(r.reasonShown, "without being opened");
    assert.strictEqual(r.render, null, "and has no Render button");
    assert.strictEqual(r.expanded, null, "it does not open, having nothing more to show");
    assert.strictEqual(r.chevron, null, "so it has no chevron");
    assert.strictEqual(r.cursor, "auto", "and no pointer cursor");
    assert.ok(!state.tabOrder.includes("row 1"), "and Tab passes it");
    const clicked = await table.act([{ click: `${row(1)} td:nth-child(5)` }]);
    assert.strictEqual(clicked.rows[1].expansionShown, false, "a click on it opens nothing");
  });

  test("Tab reaches each row and its Render button, Enter and Space open a row, and the focused Render draws", async () => {
    const p = await provider();
    table = await openTable(file);
    const t = table;
    const state = await t.act([]);
    assert.deepStrictEqual(state.tabOrder, ["row 0", "render 0", "row 2", "render 2"], "Tab order through the table");

    // A focus ring shows only on a page that has focus.
    let focused = await t.act([{ focus: row(0) }]);
    await pageHasFocus(
      async () => (focused = await t.act([{ focus: row(0) }])).docHasFocus,
      () => t.reveal(),
      "the table",
    );
    assert.strictEqual(focused.focused, "row 0");
    assert.ok(focused.ring && focused.ring.style !== "none", `the focused row has a ring: ${JSON.stringify(focused.ring)}`);
    assert.strictEqual(focused.ring.color, focused.ring.want, "the ring is --vscode-focusBorder");

    const entered = await t.act([{ key: "Enter" }]);
    assert.strictEqual(entered.rows[0].expanded, "true", "Enter opens the focused row");
    assert.ok(entered.rows[0].expansionShown);
    assert.ok(entered.prevented, "Enter is taken by the row");
    assert.deepStrictEqual(
      entered.tabOrder,
      ["row 0", "render 0", "csv 0", "row 2", "render 2"],
      "the open row's Export CSV comes next in the tab order",
    );
    const spaced = await t.act([{ key: " " }]);
    assert.strictEqual(spaced.rows[0].expanded, "false", "Space closes it again");
    assert.ok(spaced.prevented, "Space does not scroll the page");
    const other = await t.act([{ focus: row(2) }, { key: " " }]);
    assert.strictEqual(other.rows[2].expanded, "true", "Space opens a row");

    // A key on the Render button is the button's, not the row's.
    const onButton = await t.act([{ focus: renderIn(0) }, { key: "Enter" }]);
    assert.strictEqual(onButton.focused, "render 0");
    assert.strictEqual(onButton.rows[0].expanded, "false", "Enter on Render does not open its row");
    assert.ok(!onButton.prevented, "and leaves the button its own Enter");

    // Enter or Space on a focused button presses it: the browser's own
    // activation, which a test cannot send as a real key. Pressing the focused
    // element is what that activation does.
    const panel = await openReal(p, async () => {
      await t.act([{ focus: renderIn(2) }, { pressFocused: true }]);
    });
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the render of message 2");
      assert.strictEqual(drawn.messageIndex, 2, "the panel draws message 2");
    } finally {
      panel.dispose();
    }
  });
});
