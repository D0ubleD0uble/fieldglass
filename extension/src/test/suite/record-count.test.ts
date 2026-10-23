// The NetCDF file page's Dimensions table gives a record dimension's count
// beside `unlimited`, as `3 (unlimited)` (#1394).
//
// Since #1014 the napi `DimensionMeta.length` carries the resolved record count,
// but the page printed only `unlimited` for a record dimension. This opens the
// committed classic files in the real editor and reads the table it wrote.

import * as assert from "assert";
import * as path from "path";

import { extensionPath, openEditor, provider } from "./real-panel";

const FIXTURES = path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures");

/** The Dimensions table's rows, each as its cells' text. */
function dimensionRows(html: string): string[][] {
  const at = html.indexOf("<h2>Dimensions</h2>");
  assert.ok(at >= 0, "the page has a Dimensions table");
  const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html.slice(at));
  assert.ok(body, "the Dimensions table has a body");
  return [...body[1].matchAll(/<tr>([\s\S]*?)<\/tr>/g)].map((row) =>
    [...row[1].matchAll(/<td[^>]*>([\s\S]*?)<\/td>/g)].map((cell) => cell[1].trim()),
  );
}

async function pageRows(file: string): Promise<string[][]> {
  const p = await provider();
  const editor = await openEditor(p, path.join(FIXTURES, file));
  try {
    return dimensionRows(editor.html());
  } finally {
    editor.close();
  }
}

suite("The Dimensions table gives a record dimension's count (#1394)", function () {
  this.timeout(60_000);

  test("record_single_short_cdf1.nc: the record dimension reads `3 (unlimited)`", async () => {
    const rows = await pageRows("record_single_short_cdf1.nc");
    const record = rows.filter((cells) => cells[2] === "record");
    assert.strictEqual(record.length, 1, "one record dimension");
    assert.strictEqual(record[0][1], "3 (unlimited)");
    for (const cells of rows.filter((c) => c[2] === "fixed")) {
      assert.ok(/^\d+$/.test(cells[1]), `a fixed dimension is a bare count: ${cells[1]}`);
    }
  });

  test("each record file: the Dimensions column reads the count the file holds or states", async () => {
    // Pinned by hand from the fixtures' NOTICE.md, not rebuilt from the binding.
    const expected: Array<[string, string[]]> = [
      ["record_single_short_cdf2.nc", ["3 (unlimited)", "3"]],
      ["record_single_short_cdf5.nc", ["3 (unlimited)", "3"]],
      ["record_mixed_cdf1.nc", ["3 (unlimited)", "3", "1"]],
      ["numrecs_streaming_cdf1.nc", ["2 (unlimited)", "1"]],
      ["numrecs_streaming_partial_cdf1.nc", ["1 (unlimited)", "1"]],
      // States 60,000,000 records and holds 2: the page gives the header's count.
      ["numrecs_huge_cdf1.nc", ["60000000 (unlimited)", "1"]],
      // NetCDF-4: time is (2, unlimited).
      ["netcdf4_dimscale.nc", ["2 (unlimited)", "3", "4", "2"]],
    ];
    for (const [file, lengths] of expected) {
      const rows = await pageRows(file);
      assert.deepStrictEqual(rows.map((c) => c[1]), lengths, file);
    }
  });
});
