// A long attribute value can be read in full on the file page (#1232).
//
// The page shows the first 120 characters of a long value. The rest used to be
// only in the cell's `title`, and the native tooltip stops near 1,900
// characters, so a 5,600-character `history` could not be read anywhere. A
// variable's attributes past the first three were not on the page at all. Now
// a long value is a `<details>` disclosure: its summary is the preview, and
// opening it (click, or Enter or Space on the focused summary) shows the whole
// value as page text. The test reads the page the real editor writes for the
// committed fixture and finds each whole value in the text of an element.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { escapeHtml } from "../../html";
import { loadNative } from "../../native";
import { renderHtml } from "../../provider";
import { classicNetcdf, extensionPath, openEditor, provider } from "./real-panel";

const HUGE = path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_huge_attributes.nc",
);

/** The text a reader sees for an HTML fragment: tags dropped, entities
 *  decoded. Only the entities `escapeHtml` writes. */
function textOf(html: string): string {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");
}

/** The rows of the table under the `<h2>` titled `title`, each as its cells'
 *  inner HTML. */
function tableRows(html: string, title: string): string[][] {
  const at = html.indexOf(`<h2>${title}</h2>`);
  assert.ok(at >= 0, `the page has a ${title} table`);
  const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html.slice(at));
  assert.ok(body, `the ${title} table has a body`);
  return [...body[1].matchAll(/<tr>([\s\S]*?)<\/tr>/g)].map((row) =>
    [...row[1].matchAll(/<td[^>]*>([\s\S]*?)<\/td>/g)].map((cell) => cell[1].trim()),
  );
}

/** The opening `<td …>` tag of the cell whose content is `inner`. */
function cellTag(html: string, inner: string): string {
  const at = html.indexOf(`>${inner}</td>`);
  assert.ok(at >= 0, "the cell is on the page");
  return html.slice(html.lastIndexOf("<td", at), at + 1);
}

/** Every long-value disclosure in `html`: its summary's text and its body's
 *  text. The summary must be the first child, which is what makes it the
 *  native toggle. A long value's body is text, so it holds no nested
 *  `<details>` and the first `</details>` is its own. */
function longValues(html: string): Array<{ summary: string; body: string }> {
  return [...html.matchAll(
    /<details class="long-value">\s*<summary\b[^>]*>([\s\S]*?)<\/summary>([\s\S]*?)<\/details>/g,
  )].map((m) => ({ summary: textOf(m[1]), body: textOf(m[2]) }));
}

/** The summary text of the "+N more" disclosure in `html`, and the HTML after
 *  that summary, which holds the attributes it opens to. */
function moreAttrs(html: string): { summary: string; rest: string } | undefined {
  const m = /<details class="more-attrs">\s*<summary\b[^>]*>([\s\S]*?)<\/summary>([\s\S]*)$/.exec(html);
  return m ? { summary: textOf(m[1]), rest: m[2] } : undefined;
}

suite("A long attribute value can be read in full on the file page (#1232)", function () {
  this.timeout(60_000);

  test("netcdf4_huge_attributes.nc: all of `history` and all of `t`'s `comment`", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(HUGE)).metadata();
    const history = meta.globalAttributes.find((a) => a.name === "history")?.value;
    assert.ok(history, "the fixture has a global history");
    assert.strictEqual(history.length, 5600, "the fixture's history is 5,600 characters");
    const t = meta.variables.find((v) => v.name === "t");
    assert.ok(t, "the fixture has a variable t");
    const comment = t.attributes.find((a) => a.name === "comment")?.value;
    assert.ok(comment && comment.length > 4000, "t's comment is long");
    assert.ok(
      t.attributes.findIndex((a) => a.name === "comment") >= 3,
      "comment is past the three attributes the table previews",
    );

    const p = await provider();
    const editor = await openEditor(p, HUGE);
    try {
      const html = editor.html();

      // The global `history`: a disclosure whose summary is the preview and
      // whose body is the whole value, as page text rather than a tooltip.
      const globals = tableRows(html, "Global attributes");
      const row = globals.find((cells) => textOf(cells[0]) === "history");
      assert.ok(row, "history is in the Global attributes table");
      const [shown] = longValues(row[2]);
      assert.ok(shown, "the history cell opens to show more");
      assert.strictEqual(shown.summary, history.slice(0, 120) + "…" + "Show less",
        "closed, the cell shows the first 120 characters, as before");
      assert.strictEqual(shown.body, history, "open, it shows all 5,600 characters");

      // A short value is drawn as before: plain text, its title the value.
      const short = globals.find((cells) => textOf(cells[0]) === "global_0");
      assert.ok(short, "global_0 is in the table");
      assert.strictEqual(short[2], "value 0", "a short value is plain text");
      assert.strictEqual(cellTag(html, "value 0"), `<td title="value 0">`, "and keeps its title");

      // `t`'s attributes past the first three are on the page too, and its
      // long comment opens to its whole value.
      const variables = tableRows(html, "Variables");
      const tRow = variables.find((cells) => textOf(cells[0]) === "t");
      assert.ok(tRow, "t is in the Variables table");
      const attrs = tRow[4];
      assert.ok(textOf(attrs).startsWith("attr_0=value 0; attr_1=value 1; attr_2=value 2; "),
        "the first three attributes read as before");
      const more = moreAttrs(attrs);
      assert.ok(more, "the rest of t's attributes are behind a disclosure");
      assert.strictEqual(more.summary, `+${t.attributes.length - 3} more`, "labelled as before");
      for (const a of t.attributes.slice(3).filter((a) => a.value.length <= 120)) {
        assert.ok(textOf(more.rest).includes(`${a.name}=${a.value}`), `${a.name} can be read`);
      }
      assert.ok(textOf(more.rest).includes("comment="), "comment is among them");
      assert.ok(longValues(more.rest).some((d) => d.body === comment), "t's whole comment can be read");
    } finally {
      editor.close();
    }
  });

  // A reader's reason for leaving a variable out goes through the same
  // preview, so a long one opens the same way. No committed file has a reason
  // that long, so this lengthens a real one.
  test("a long reason in Variables not decoded opens to its whole text", () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    const file = path.join(
      extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_unsupported_type.nc",
    );
    const meta = native.open(fs.readFileSync(file)).metadata();
    const [first, ...others] = meta.unsupportedVariables;
    assert.ok(first, "the fixture has a variable left out");
    const reason = (first.reason + " ").repeat(20);
    const html = renderHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      "netcdf", file, undefined,
      { ...meta, unsupportedVariables: [{ ...first, reason }, ...others] },
      undefined, false, undefined,
    );
    const row = tableRows(html, "Variables not decoded").find((cells) => textOf(cells[0]) === first.name);
    assert.ok(row, "the variable is listed");
    assert.ok(longValues(row[1]).some((d) => d.body === reason), "its whole reason can be read");
    const short = others[0];
    assert.ok(short && short.reason.length <= 120, "the other reason is short");
    assert.strictEqual(cellTag(html, escapeHtml(short.reason)), `<td title="${escapeHtml(short.reason)}">`,
      "and is drawn as before");
  });

  test("a long value is escaped, not markup, and needs no script to open", async () => {
    const hostile = "</summary></details><script>alert(1)</script><img src=x onerror=alert(2)> & \"'"
      .padEnd(400, "x");
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-long-"));
    const file = path.join(dir, "hostile.nc");
    fs.writeFileSync(file, classicNetcdf([["x", 2], ["y", 2]], [
      { name: "v", dims: ["y", "x"], attrs: { a: "1", b: "2", c: "3", note: hostile }, values: [1, 2, 3, 4] },
    ]));
    const p = await provider();
    const editor = await openEditor(p, file);
    try {
      const html = editor.html();
      const row = tableRows(html, "Variables").find((cells) => textOf(cells[0]) === "v");
      assert.ok(row, "v is in the Variables table");
      assert.ok(!/<script|<img/i.test(row[4]), "the value is text, not markup");
      assert.ok(longValues(row[4]).some((d) => d.body === hostile), "and reads back whole");
      // The only script on the page is the nonce'd one; the disclosure is
      // native, so no inline handler was added to make it work.
      assert.strictEqual(html.match(/<script\b/g)?.length, 1, "one script on the page");
      assert.ok(!/<[^>]*\son[a-z]+=/i.test(html.replace(/<script[\s\S]*?<\/script>/, "")),
        "no inline event handlers");
    } finally {
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
