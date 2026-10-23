// Opening a long attribute value does not move the table's columns (#1390).
//
// The open value used to be a wrapped block of its own width in place of the
// one-line preview, so the Value column changed width and the columns beside
// it moved sideways. The preview now keeps its place in the layout, unseen,
// while the value is open. Measured in a real webview on the page
// `renderHtml` writes for the committed huge-attributes fixture: every
// header cell's left edge and width, before and after opening a value.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { renderHtml } from "../../provider";
import { extensionPath } from "./real-panel";

const HUGE = path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_huge_attributes.nc",
);

interface Columns {
  title: string;
  before: number[][];
  after: number[][];
  opened: boolean;
}

suite("Opening a long attribute value moves no column (#1390)", function () {
  this.timeout(60_000);

  test("the Global attributes and Variables tables keep their columns", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(HUGE)).metadata();
    const panel = vscode.window.createWebviewPanel("fieldglass.test1390", "columns", vscode.ViewColumn.One, {
      enableScripts: true,
    });
    try {
      const got: Columns[] = [];
      let done: () => void = () => undefined;
      const finished = new Promise<void>((r) => (done = r));
      panel.webview.onDidReceiveMessage((m) => {
        if (m && m.type === "test:columns") {
          got.push(...m.tables);
          done();
        }
      });
      const page = renderHtml(panel.webview, "netcdf", HUGE, undefined, meta, undefined, false, undefined);
      const nonce = /<script nonce="([^"]+)">/.exec(page)?.[1];
      assert.ok(nonce, "the page has a nonce'd script");
      const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
      const reader = `<script nonce="${nonce}">
        const api = acquireVsCodeApi();
        const edges = (t) => Array.from(t.querySelectorAll('thead th')).map((th) => {
          const r = th.getBoundingClientRect();
          return [Math.round(r.left * 10) / 10, Math.round(r.width * 10) / 10];
        });
        window.addEventListener('message', (ev) => {
          if (!ev.data || ev.data.type !== 'test:read') return;
          const tables = [];
          for (const h2 of Array.from(document.querySelectorAll('h2'))) {
            const wrap = h2.nextElementSibling;
            const table = wrap && wrap.querySelector('table');
            const d = table && table.querySelector('details.long-value');
            if (!d) continue;
            const before = edges(table);
            d.open = true;
            const after = edges(table);
            tables.push({ title: h2.textContent, before, after, opened: d.open });
            d.open = false;
          }
          api.postMessage({ type: 'test:columns', tables });
        });
      </script>`;
      panel.webview.html = page.replace("<head>", `<head>${memo}`).replace("</body>", `${reader}</body>`);
      const ask = setInterval(() => void panel.webview.postMessage({ type: "test:read" }), 200);
      const give = setTimeout(() => done(), 15_000);
      await finished;
      clearInterval(ask);
      clearTimeout(give);

      const titles = got.map((t) => t.title);
      assert.deepStrictEqual(titles, ["Global attributes", "Variables"], "both tables hold a long value");
      for (const t of got) {
        assert.ok(t.opened, `${t.title}: a value opened`);
        // The last column runs to the container's edge, so a scrollbar that
        // the taller page brings narrows it without moving any column: it is
        // compared by its left edge only.
        const last = t.before.length - 1;
        assert.deepStrictEqual(
          t.after.map(([left, width], k) => [left, k === last ? 0 : width]),
          t.before.map(([left, width], k) => [left, k === last ? 0 : width]),
          `${t.title}: opening a long value moves no column`,
        );
      }
    } finally {
      panel.dispose();
    }
  });
});

// Every other cell that expands on the file pages moves no column either
// (#1428): a variable's "+N more" attributes (`details.more-attrs`, which
// holds a long value of its own) and a GRIB message's expansion row
// (`tr.expand-row`). They are read in a real webview like the test above.

interface Probe {
  before: number[][];
  after: number[][];
  opened: boolean;
  overflow: string[];
}

// Shows the page `render` writes in a webview, runs `body` (a function of the
// page's `edges` reader, returning a Probe) there, and resolves with the answer.
async function probe(render: (webview: vscode.Webview) => string, body: string): Promise<Probe> {
  const panel = vscode.window.createWebviewPanel("fieldglass.test1428", "expanding cells", vscode.ViewColumn.One, {
    enableScripts: true,
  });
  try {
    let got: Probe | undefined;
    let done: () => void = () => undefined;
    const finished = new Promise<void>((r) => (done = r));
    panel.webview.onDidReceiveMessage((m) => {
      if (m && m.type === "test:probe") {
        got = m.probe;
        done();
      }
    });
    const page = render(panel.webview);
    const nonce = /<script nonce="([^"]+)">/.exec(page)?.[1];
    assert.ok(nonce, "the page has a nonce'd script");
    const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
    const reader = `<script nonce="${nonce}">
      const api = acquireVsCodeApi();
      const edges = (t) => Array.from(t.querySelectorAll('thead th')).map((th) => {
        const r = th.getBoundingClientRect();
        return [Math.round(r.left * 10) / 10, Math.round(r.width * 10) / 10];
      });
      window.addEventListener('message', (ev) => {
        if (!ev.data || ev.data.type !== 'test:read') return;
        const probe = (${body})(edges);
        api.postMessage({ type: 'test:probe', probe });
      });
    </script>`;
    panel.webview.html = page.replace("<head>", `<head>${memo}`).replace("</body>", `${reader}</body>`);
    const ask = setInterval(() => void panel.webview.postMessage({ type: "test:read" }), 200);
    const give = setTimeout(() => done(), 15_000);
    await finished;
    clearInterval(ask);
    clearTimeout(give);
    assert.ok(got, "the page answered");
    return got;
  } finally {
    panel.dispose();
  }
}

// The last column runs to the container's edge, so a scrollbar that a taller
// page brings narrows it without moving any column: it is compared by its left
// edge only.
function sameColumns(p: Probe, what: string): void {
  const last = p.before.length - 1;
  assert.deepStrictEqual(
    p.after.map(([left, width], k) => [left, k === last ? 0 : width]),
    p.before.map(([left, width], k) => [left, k === last ? 0 : width]),
    what,
  );
}

suite("Opening any expanding cell moves no column (#1428)", function () {
  this.timeout(60_000);

  test("t's \"+8 more\" attributes keep the Variables table's columns and wrap in their cell", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(HUGE)).metadata();
    const p = await probe(
      (w) => renderHtml(w, "netcdf", HUGE, undefined, meta, undefined, false, undefined),
      `(edges) => {
        const row = Array.from(document.querySelectorAll('tr')).find(
          (r) => r.cells[0] && r.cells[0].textContent === 't' && r.querySelector('details.more-attrs'));
        const table = row.closest('table');
        const more = row.querySelector('details.more-attrs');
        const cell = more.closest('td');
        const before = edges(table);
        const sum = more.querySelector('summary');
        const sumBefore = sum.getBoundingClientRect();
        more.open = true;
        const nested = more.querySelector('details.long-value');
        nested.open = true;
        const after = edges(table);
        const c = cell.getBoundingClientRect();
        const overflow = [];
        for (const el of cell.querySelectorAll('*')) {
          const r = el.getBoundingClientRect();
          if (r.width > 0 && r.right > c.right + 0.5) overflow.push(el.className || el.tagName);
        }
        // The "+8 more" label stays where it was, at the end of the first line.
        const sumAfter = sum.getBoundingClientRect();
        if (Math.abs(sumAfter.left - sumBefore.left) > 1 || Math.abs(sumAfter.top - sumBefore.top) > 1) overflow.push('summary moved');
        if (cell.scrollWidth > cell.clientWidth + 1) overflow.push('cell scrolls');
        return { before, after, opened: more.open && nested.open, overflow };
      }`,
    );
    assert.ok(p.opened, "+8 more and the comment= value inside it both opened");
    sameColumns(p, "opening +8 more and its comment= value moves no column");
    assert.deepStrictEqual(p.overflow, [], "everything inside the opened cell stays within the Attributes column");
  });

  test("a GRIB message's expansion row keeps the table's columns", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const grib = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
    const messages = [native.open(fs.readFileSync(grib)).message(0)];
    const p = await probe(
      (w) => renderHtml(w, "grib2", grib, messages, undefined, undefined, false),
      `(edges) => {
        const table = document.querySelector('table');
        const before = edges(table);
        document.querySelector('tr.msg-row').click();
        const row = document.querySelector('tr.expand-row');
        const after = edges(table);
        return { before, after, opened: !row.hasAttribute('hidden'), overflow: [] };
      }`,
    );
    assert.ok(p.opened, "the expansion row opened");
    sameColumns(p, "opening a message's expansion row moves no column");
  });
});
