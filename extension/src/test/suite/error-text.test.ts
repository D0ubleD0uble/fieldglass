// What a refused render says, and what is left on the panel (#1053).
//
// An error used to reach the status line as "Error: render failed: Error: …":
// the provider wrote the thrown value with its own "Error: " and the panel
// put another in front. A first render that failed left the colour bar up,
// labelled "—", beside an empty map. A colour range refused against the
// field's own end named that end unrounded (`311.0986328125`) where the colour
// bar said `311.1`. And the file page described the grid as drawn with "no map
// reprojection", and said "No messages found." of files that are not GRIB.
//
// Driven through the real panel, whose own script runs in a real webview.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { nothingListed } from "../../provider";
import { formatReading } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const FIXTURE = path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const NETCDF_FIXTURES = path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures");

const MANUAL = { selector: 'input[name="range-mode"][value="manual"]', checked: true };

/** How many times `text` says "Error:". */
const errorPrefixes = (text: string) => (text.match(/Error:/g) ?? []).length;

suite("A refused render says why once, and leaves no colour bar over nothing (#1053)", function () {
  this.timeout(120_000);

  test("a NetCDF slice refused on its first render: one prefix, no colour bar", async () => {
    // States 60,000,000 records and holds 2: the read is refused before any
    // data is drawn (§32 of the release test plan).
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1053-"));
    const file = path.join(dir, "numrecs_huge_cdf1.nc");
    fs.copyFileSync(path.join(NETCDF_FIXTURES, "numrecs_huge_cdf1.nc"), file);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const v = native.open(fs.readFileSync(file)).variables().find((x) => x.name === "v");
    assert.ok(v, "v is offered for rendering");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: v.index }));
    try {
      const err = await panel.waitSent(isType("gridError"), 0, "the refused first render");
      assert.ok(String(err.error).startsWith("render failed: parse error: "), String(err.error));
      const state = await panel.drive([]);
      assert.ok(state.status.startsWith('Error: render failed: parse error: variable "v": '), state.status);
      assert.strictEqual(errorPrefixes(state.status), 1, state.status);
      assert.strictEqual(state.canvasPainted, false, "nothing is drawn");
      assert.strictEqual(state.colorbarShown, false, "so there is no colour bar");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a GRIB render error after a picture: one prefix, and the picture keeps its bar", async () => {
    // Field B on another grid: the combine fails before the range is read.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1053-"));
    const file = path.join(dir, "two-grids.grib2");
    const other = path.join(extensionPath(), "src", "test", "fixtures", "eta_lambert_msg0.grib2");
    fs.writeFileSync(file, Buffer.concat([fs.readFileSync(FIXTURE), fs.readFileSync(other)]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      const err = await panel.waitSent(
        (m: Msg) => m.type === "gridError" && !m.rangeRefusal,
        from,
        "the different-grid error",
      );
      const reason = String(err.error);
      assert.ok(reason.startsWith("render failed: ") && !reason.includes("Error:"), reason);
      const state = await panel.drive([]);
      assert.strictEqual(state.status, `Error: ${reason}`);
      assert.strictEqual(errorPrefixes(state.status), 1, state.status);
      // The last picture stays (§45), and its colour bar with it.
      assert.strictEqual(state.canvasPainted, true);
      assert.strictEqual(state.colorbarShown, true);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a range refused against the field's maximum names it as the colour bar does", async () => {
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      const shown = await panel.drive([]);
      const barMax = shown.colorbarLabels[1];
      assert.strictEqual(barMax, formatReading(Number(first.usedMax), 4));
      // Not four figures already, or the test would not tell the two apart.
      assert.notStrictEqual(String(first.usedMax), barMax, "the field's maximum has more figures than the bar shows");

      const from = panel.sent.length;
      await panel.drive([MANUAL, { id: "range-max", value: "" }, { id: "range-min", value: 400 }]);
      await panel.waitSent(
        (m: Msg) => m.type === "gridError" && m.rangeRefusal?.end === "min" && m.rangeRefusal?.given === 400,
        from,
        "the minimum above the data refused",
      );
      const state = await panel.drive([]);
      assert.strictEqual(
        state.rangeNote,
        `minimum 400 is above the field's maximum ${barMax} (taken from the data); set a maximum too`,
      );
    } finally {
      panel.dispose();
      editor.close();
    }
  });
});

suite("The file page in each format's own words (#1053)", function () {
  this.timeout(60_000);

  test("a GRIB message's legend does not say the grid cannot be reprojected", async () => {
    const p = await provider();
    const editor = await openEditor(p, FIXTURE);
    try {
      const html = editor.html();
      assert.ok(html.includes('class="render-legend"'), "the message has a Render legend");
      assert.ok(!html.includes("no map reprojection"), "no claim that there is no reprojection");
      assert.ok(html.includes("on a map projection where the grid allows one"));
    } finally {
      editor.close();
    }
  });

  test("a file that lists nothing is not said to have no messages unless it is GRIB", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1053-"));
    const file = path.join(dir, "empty.nc");
    fs.writeFileSync(file, Buffer.alloc(0));
    const editor = await openEditor(p, file);
    try {
      const html = editor.html();
      assert.ok(html.includes("Nothing to list in this file."), "the empty file's line");
      assert.ok(!html.includes("No messages"), "no GRIB word for a file that is not GRIB");
    } finally {
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
    // A NetCDF file whose metadata could not be read takes the same line.
    assert.strictEqual(nothingListed("netcdf"), "Nothing to list in this file.");
    assert.strictEqual(nothingListed("grib2"), "No messages found.");
    assert.strictEqual(nothingListed("grib1"), "No messages found.");
  });
});
