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
