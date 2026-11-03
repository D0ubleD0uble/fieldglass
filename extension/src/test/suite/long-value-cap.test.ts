// A very large attribute value is cut in the metadata page (#1572).
//
// NetCDF-4 string attributes are read up to 16 MiB a file (#1549), and a 9 MiB
// value written whole into the page made it slow to open and scroll. The page
// now holds the first ATTR_FULL_LIMIT characters of a value and a note of its
// full size. These tests call the HTML builder with attribute values made
// here, so they cover every place the page writes one: the Global attributes
// table, a variable's attributes (the first three and the "+N more" ones) and
// the reason in Variables not decoded.

import * as assert from "assert";
import type * as vscode from "vscode";

import { escapeHtml } from "../../html";
import type { DatasetMeta } from "../../native";
import { ATTR_FULL_LIMIT, cutAtCharacter, formatByteSize, renderHtml } from "../../provider";

function dataset(over: Partial<DatasetMeta>): DatasetMeta {
  return {
    backing: "netcdf4", backingLabel: "NetCDF-4", fullyParsed: true, note: null,
    dimensions: [], globalAttributes: [], variables: [], hdf5SuperblockVersion: 0,
    unsupportedVariables: [], ...over,
  };
}

function page(d: DatasetMeta): string {
  return renderHtml({ cspSource: "" } as unknown as vscode.Webview, "netcdf", "x.nc", undefined, d, undefined, false);
}

/** The `<div class="long-value-full">` bodies on the page, as written. */
function fullBodies(html: string): string[] {
  return [...html.matchAll(/<div class="long-value-full">([\s\S]*?)<\/div>/g)].map((m) => m[1]);
}

const global = (value: string): DatasetMeta =>
  dataset({ globalAttributes: [{ name: "blob", ncType: "NC_STRING", value }] });

suite("A very large attribute value is cut in the page (#1572)", () => {
  test("a 9 MiB value is not in the page in full, and the note gives its size", () => {
    const html = page(global("x".repeat(9 * 1024 * 1024)));
    assert.ok(html.length < 200_000, `the page is small (${html.length} characters)`);
    const [body] = fullBodies(html);
    assert.ok(body.startsWith("x".repeat(ATTR_FULL_LIMIT) + "<span"), "it opens to the first part");
    assert.ok(body.endsWith("… (9.0 MiB in all)</span>"), body.slice(ATTR_FULL_LIMIT));
  });

  test("a short value and a long value under the limit are unchanged", () => {
    const short = page(global("value 0"));
    assert.ok(short.includes(`<td title="value 0">value 0</td>`));
    const under = "y".repeat(ATTR_FULL_LIMIT);
    const [body] = fullBodies(page(global(under)));
    assert.strictEqual(body, under, "exactly at the limit is whole, with no note");
  });

  test("a value one character over the limit is cut, with its size", () => {
    const [body] = fullBodies(page(global("y".repeat(ATTR_FULL_LIMIT + 1))));
    assert.strictEqual(body, "y".repeat(ATTR_FULL_LIMIT) + `<span class="long-value-cut">… (16.0 KiB in all)</span>`);
  });

  test("the size is in UTF-8 bytes, so multi-byte text is counted as stored", () => {
    // 20,000 two-byte characters: 40,000 bytes, 39.1 KiB.
    const [body] = fullBodies(page(global("é".repeat(20_000))));
    assert.ok(body.startsWith("é".repeat(ATTR_FULL_LIMIT) + "<span"));
    assert.ok(body.endsWith("… (39.1 KiB in all)</span>"));
  });

  test("a surrogate pair at the cut is kept whole or left out, never split", () => {
    const emoji = "😀"; // two UTF-16 code units
    // The pair straddles the limit: its first half is unit LIMIT-1.
    const straddle = "a".repeat(ATTR_FULL_LIMIT - 1) + emoji + "tail";
    const cut = cutAtCharacter(straddle, ATTR_FULL_LIMIT);
    assert.strictEqual(cut, "a".repeat(ATTR_FULL_LIMIT - 1), "the lone first half is dropped");
    // The pair ends exactly at the limit: it stays.
    const fits = "a".repeat(ATTR_FULL_LIMIT - 2) + emoji + "tail";
    assert.strictEqual(cutAtCharacter(fits, ATTR_FULL_LIMIT), "a".repeat(ATTR_FULL_LIMIT - 2) + emoji);
    // And on the page no lone surrogate is written.
    const [body] = fullBodies(page(global(straddle)));
    assert.ok(!/[\ud800-\udbff](?![\udc00-\udfff])/.test(body), "no lone high surrogate");
    assert.ok(!/(?<![\ud800-\udbff])[\udc00-\udfff]/.test(body), "no lone low surrogate");
    // The 120-character preview takes the same care.
    const previewStraddle = "b".repeat(119) + emoji + "z".repeat(50);
    assert.strictEqual(cutAtCharacter(previewStraddle, 120), "b".repeat(119));
  });

  test("HTML-special characters at the cut are escaped whole, never cut mid-entity", () => {
    for (const ch of ["&", "<", ">", '"', "'"]) {
      const value = "a".repeat(ATTR_FULL_LIMIT - 1) + ch + "tail";
      const [body] = fullBodies(page(global(value)));
      const text = body.slice(0, body.indexOf('<span class="long-value-cut">'));
      assert.strictEqual(text, "a".repeat(ATTR_FULL_LIMIT - 1) + escapeHtml(ch), `${ch} is whole`);
      // Every ampersand starts a complete entity.
      assert.ok(!/&(?!(amp|lt|gt|quot|#39);)/.test(text), `${ch} leaves no broken entity`);
    }
    // One past the cut leaves the character out, and nothing of its entity.
    const past = "a".repeat(ATTR_FULL_LIMIT) + "&<";
    const [body] = fullBodies(page(global(past)));
    assert.ok(body.startsWith("a".repeat(ATTR_FULL_LIMIT) + `<span class="long-value-cut">`));
  });

  test("a variable's attributes and a not-decoded reason are cut the same way", () => {
    const big = "z".repeat(ATTR_FULL_LIMIT + 5000);
    const attrs = ["a", "b", "c", "d"].map((name, i) => ({ name, ncType: "NC_STRING", value: i === 0 || i === 3 ? big : "v" }));
    const html = page(dataset({
      variables: [{ name: "t", ncType: "NC_FLOAT", dimensions: [], units: null, attributes: attrs }],
      unsupportedVariables: [{ name: "u", reason: big }],
    }));
    const bodies = fullBodies(html);
    assert.strictEqual(bodies.length, 3, "a first-three value, a '+N more' value and a reason");
    for (const b of bodies) {
      assert.ok(b.endsWith("in all)</span>") && b.length < ATTR_FULL_LIMIT + 100);
    }
    assert.ok(html.length < 3 * (ATTR_FULL_LIMIT + 1000) + 20_000, "and nothing else carries the value");
  });

  test("sizes read plainly", () => {
    assert.strictEqual(formatByteSize(900), "900 bytes");
    assert.strictEqual(formatByteSize(16384), "16.0 KiB");
    assert.strictEqual(formatByteSize(9 * 1024 * 1024), "9.0 MiB");
  });
});
