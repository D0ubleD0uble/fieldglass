// Small layout faults from the 2026-11-04 manual pass (#1677), each checked
// where the owner saw it:
//   - a collapsed long attribute value stays inside its column, so the Global
//     attributes table does not scroll sideways for it;
//   - the contour key wraps only between words, in the narrow key column;
//   - a constant field's exported color bar writes its one value once.
// (The missing final period of the budget sentence is Rust's, tested there.)

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { colorbarLabels, joinNotes } from "../../render-panel";
import { renderHtml } from "../../provider";
import {
  classicNetcdf,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const HUGE = path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_huge_attributes.nc",
);

/** Shows the page `renderHtml` writes for `file` in a webview with its body
 *  `width` wide, runs `body` there and resolves with what it returns. */
async function onPage<T>(file: string, width: string, body: string): Promise<T> {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const meta = native.open(fs.readFileSync(file)).metadata();
  const panel = vscode.window.createWebviewPanel("fieldglass.test1677", "layout", vscode.ViewColumn.One, {
    enableScripts: true,
  });
  try {
    let got: T | undefined;
    let done: () => void = () => undefined;
    const finished = new Promise<void>((r) => (done = r));
    panel.webview.onDidReceiveMessage((m) => {
      if (m && m.type === "test:page") {
        got = m.result;
        done();
      }
    });
    const page = renderHtml(panel.webview, "netcdf", file, undefined, meta, undefined, false, undefined);
    const nonce = /<script nonce="([^"]+)">/.exec(page)?.[1];
    assert.ok(nonce, "the page has a nonce'd script");
    const memo = `<script nonce="${nonce}">(function(){const a=window.acquireVsCodeApi;let api;window.acquireVsCodeApi=()=>api||(api=a());})();</script>`;
    const reader = `<script nonce="${nonce}">
      const api = acquireVsCodeApi();
      window.addEventListener('message', (ev) => {
        if (!ev.data || ev.data.type !== 'test:read') return;
        document.body.style.width = ${JSON.stringify(width)};
        api.postMessage({ type: 'test:page', result: (${body})() });
      });
    </script>`;
    panel.webview.html = page.replace("<head>", `<head>${memo}`).replace("</body>", `${reader}</body>`);
    const ask = setInterval(() => void panel.webview.postMessage({ type: "test:read" }), 200);
    const give = setTimeout(() => done(), 15_000);
    await finished;
    clearInterval(ask);
    clearTimeout(give);
    assert.ok(got !== undefined, "the page answered");
    return got;
  } finally {
    panel.dispose();
  }
}

interface Fit {
  /** The Global attributes table's scroll box: how wide its content is and
   *  how wide the box. */
  scrollWidth: number;
  clientWidth: number;
  /** Closed previews found, and those running past their cell's right edge. */
  previews: number;
  outside: string[];
}

suite("A collapsed long attribute value stays in its column (#1677)", function () {
  this.timeout(60_000);

  for (const width of ["900px", "500px", "320px"]) {
    test(`at ${width} the Global attributes table does not scroll sideways for it`, async () => {
      const fit = await onPage<Fit>(HUGE, width, `() => {
        const h2 = Array.from(document.querySelectorAll('h2')).find((h) => h.textContent === 'Global attributes');
        const wrap = h2.nextElementSibling;
        const outside = [];
        let previews = 0;
        for (const d of Array.from(wrap.querySelectorAll('details.long-value:not([open])'))) {
          previews++;
          const cell = d.closest('td').getBoundingClientRect();
          const text = d.querySelector('.long-value-preview').getBoundingClientRect();
          if (text.right > cell.right + 1) outside.push(Math.round(text.right) + ' > ' + Math.round(cell.right));
        }
        return { scrollWidth: wrap.scrollWidth, clientWidth: wrap.clientWidth, previews, outside };
      }`);
      assert.ok(fit.previews > 0, "the fixture has a long global attribute, collapsed");
      assert.deepStrictEqual(fit.outside, [], "no preview runs past its cell");
      assert.ok(
        fit.scrollWidth <= fit.clientWidth + 1,
        `the table's content (${fit.scrollWidth}) fits its box (${fit.clientWidth})`,
      );
    });
  }
});


/** Click Export PNG with the save dialog answered "cancel". */
async function exportPng(panel: RealPanel): Promise<void> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    await panel.drive([{ id: "export-png", click: true }]);
    await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

/** A real panel on `variable` of the classic NetCDF `bytes`, once drawn. */
async function fieldPanel(
  bytes: Buffer,
  variable: string,
): Promise<{ panel: RealPanel; drawn: Msg; close: () => void }> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1677-"));
  const file = path.join(dir, "field.nc");
  fs.writeFileSync(file, bytes);
  const native = loadNative();
  assert.ok(native, "native binding required");
  const found = native.open(bytes).variables().find((v) => v.name === variable);
  assert.ok(found, `the file has ${variable}`);
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: found.index }));
  const drawn = await panel.waitSent(isType("gridReady"), 0, `${variable} drawn`);
  return {
    panel,
    drawn,
    close: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const grid = (name: string, values: number[], units = "K") =>
  classicNetcdf(
    [["lat", 4], ["lon", 6]],
    [
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: [-30, -10, 10, 30] },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: [0, 20, 40, 60, 80, 100] },
      { name, dims: ["lat", "lon"], attrs: { units }, values },
    ],
  );

suite("The contour key wraps only between words (#1677)", function () {
  this.timeout(120_000);

  test("a 5 K interval keeps Contours, every and 5 K whole at every panel width", async () => {
    // 210 to 250 K: about eight levels, so the automatic spacing is 5 K.
    const values = Array.from({ length: 24 }, (_, k) => 210 + (40 * k) / 23);
    const { panel, close } = await fieldPanel(grid("t", values), "t");
    try {
      await panel.drive([{ id: "overlay-contours", checked: true }, { id: "contours-only", checked: true }]);
      let state = await panel.drive([]);
      const key = String(state.contourKey);
      assert.ok(key && key.startsWith("Contours every 5 K"), `a 5 K interval: ${key}`);
      const words = key.replace(" \u00b7 ", " ").split(/\s+/);
      for (const width of ["1400px", "420px", "300px", "280px", "200px", "130px", "120px", "110px"]) {
        await panel.drive([{ id: "body", width }]);
        state = await panel.drive([]);
        const lines: string[] = state.contourKeyLines ?? [];
        // A word cut across two lines reads as two words once the lines are
        // joined: "Contour" / "s every".
        assert.deepStrictEqual(lines.join(" ").split(/\s+/), words, `no word is cut at ${width}: ${lines.join(" | ")}`);
      }
      await panel.drive([{ id: "body", width: "" }]);
    } finally {
      close();
    }
  });
});

suite("A constant field's exported color bar writes its value once (#1677)", function () {
  this.timeout(120_000);

  test("the labels of a flat range are the one value", () => {
    assert.deepStrictEqual(colorbarLabels(4, 4, false), ["4.000"]);
    assert.deepStrictEqual(colorbarLabels(0, 0, false), ["0.000"]);
    assert.deepStrictEqual(colorbarLabels(7, 7, true), ["7.000"]);
    // Ends that differ but read alike at every figure the bar has: one label.
    assert.deepStrictEqual(colorbarLabels(1e22, 1e22 * (1 + 1e-15), false), ["1.000e+22"]);
    // A range with two ends still has its three.
    assert.deepStrictEqual(colorbarLabels(0, 5, false), ["5.000", "2.500", "0.000"]);
  });

  test("the PNG a constant field exports has one label, and a field with a range keeps three", async () => {
    const flat = await fieldPanel(grid("t", new Array(24).fill(0)), "t");
    try {
      await exportPng(flat.panel);
      assert.deepStrictEqual((await flat.panel.drive([])).exportedColorbarLabels, ["0.000"]);
    } finally {
      flat.close();
    }
    const ramp = await fieldPanel(grid("t", Array.from({ length: 24 }, (_, k) => k)), "t");
    try {
      await exportPng(ramp.panel);
      assert.strictEqual((await ramp.panel.drive([])).exportedColorbarLabels?.length, 3);
    } finally {
      ramp.close();
    }
  });
});

suite("The variable note joins its sentences without doubling a full stop (#1677)", function () {
  test("a note's own full stop is taken off before the join, and the last one is kept", () => {
    const fine = "Attribute \"a\" not read: the file is fine.";
    assert.strictEqual(joinNotes([fine, "\"lat\" is read as latitude from its name"]), "Attribute \"a\" not read: the file is fine. \"lat\" is read as latitude from its name");
    assert.strictEqual(joinNotes([fine, "b is placed by name."]), "Attribute \"a\" not read: the file is fine. B is placed by name.");
    assert.strictEqual(joinNotes([null, fine, undefined, ""]), fine);
    assert.strictEqual(joinNotes([null, undefined]), "");
    assert.ok(!joinNotes([fine, fine, "c."]).includes(".."));
  });
});
