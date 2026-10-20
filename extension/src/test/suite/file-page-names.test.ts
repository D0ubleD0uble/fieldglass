// The NetCDF file page names a variable or dimension in a nested group the way
// the render panel does: `PRODUCT/qa_value`, with no leading `/` (#1231).
//
// #704 dropped the `/` from the names the panel and the page's Render buttons
// read, but the page's tables came from the dataset metadata, which still
// spelled the HDF5 path, so one file showed two spellings of each name. This
// opens the committed grouped file in the real editor and reads every table on
// the page it wrote.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import { loadNative } from "../../native";
import { extensionPath, openEditor, provider } from "./real-panel";

const GROUPED = path.join(
  extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_grouped.nc",
);

/** The rows of the table under the `<h2>` titled `title`, each as its cells'
 *  text. */
function tableRows(html: string, title: string): string[][] {
  const at = html.indexOf(`<h2>${title}</h2>`);
  assert.ok(at >= 0, `the page has a ${title} table`);
  const body = /<tbody>([\s\S]*?)<\/tbody>/.exec(html.slice(at));
  assert.ok(body, `the ${title} table has a body`);
  return [...body[1].matchAll(/<tr>([\s\S]*?)<\/tr>/g)].map((row) =>
    [...row[1].matchAll(/<td[^>]*>([\s\S]*?)<\/td>/g)].map((cell) => cell[1].trim()),
  );
}

suite("The NetCDF file page names group members as the panel does (#1231)", function () {
  this.timeout(60_000);

  test("netcdf4_grouped.nc: Variables, Dimensions and each variable's dimensions", async () => {
    const native = loadNative();
    assert.ok(native, "native binding required");
    // What the panel and its variable picker read.
    const panel = native.open(fs.readFileSync(GROUPED)).variables();
    assert.deepStrictEqual(
      panel.map((v) => v.name).sort(),
      ["PRODUCT/SUPPORT_DATA/surface_altitude", "PRODUCT/latitude", "PRODUCT/longitude", "PRODUCT/qa_value"],
      "the panel's names, as #704 spells them",
    );

    const p = await provider();
    const editor = await openEditor(p, GROUPED);
    try {
      const html = editor.html();
      const buttons = [...html.matchAll(/class="netcdf-render-btn"[^>]*>Render ([^<]*)</g)].map((m) => m[1]);
      assert.deepStrictEqual(buttons.sort(), panel.map((v) => v.name).sort(), "a Render button per variable");

      const variables = tableRows(html, "Variables");
      const named = new Map(variables.map((cells) => [cells[0], cells[3]]));
      assert.deepStrictEqual(
        [...named.keys()].sort(),
        [...panel.map((v) => v.name), "time"].sort(),
        "the Variables table names each variable as its Render button does",
      );
      // Each variable's Dimensions column lists the axes the panel steps through.
      for (const v of panel) {
        assert.strictEqual(
          named.get(v.name),
          v.dims.map((d) => d.name).join(", "),
          `${v.name}'s Dimensions column`,
        );
      }

      const dimensions = tableRows(html, "Dimensions").map((cells) => cells[0]);
      assert.deepStrictEqual(
        dimensions,
        ["time", "PRODUCT/scanline", "PRODUCT/ground_pixel"],
        "the Dimensions table names a group's dimensions as the panel's axes do",
      );

      // Nothing anywhere in the tables spells the HDF5 path.
      assert.ok(!/<td[^>]*>\/PRODUCT/.test(html), "no cell starts with `/PRODUCT`");
    } finally {
      editor.close();
    }
  });
});
