// The V component and Field B pickers tell an ensemble's mean, spread and a
// derived field of unstated kind apart (#1850).
//
// From the 0.6.2 manual pass (§6, noted on #1843). On `uv_derived.grib2`, V
// options #1, #3 and #4 all read `VGRD · 2 · +0h`: the mean, the spread, and a
// derived forecast whose kind is coded missing. Each now names what it is, in
// Rust's words (`MessageInfo.derivedForecast`, from Code Table 4.7), and a
// message that is no derived forecast reads as it did.
//
// Driven through the real panel and the real provider, over the committed
// fixture (built by `tools/build_conformance_uv_fixtures.py`: 0 the mean's u,
// 1 the mean's v, 2 member 7's v, 3 the spread's v, 4 a v whose kind is
// missing, 5 a 4.12 mean's v, 6 member 1's u).

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative, type MessageListing } from "../../native";
import { extensionPath, isType, openEditor, openReal, provider, type RealPanel } from "./real-panel";

const DERIVED = path.join(extensionPath(), "..", "crates", "fieldglass", "tests", "fixtures", "uv_derived.grib2");
const GFS = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");

/** The options of the page's `<select id>`, as `index → text`. */
function options(html: string, id: string): Map<number, string> {
  const start = html.indexOf(`<select id="${id}">`);
  assert.ok(start >= 0, `the page has #${id}`);
  const body = html.slice(start, html.indexOf("</select>", start));
  const out = new Map<number, string>();
  for (const m of body.matchAll(/<option value="(\d+)"[^>]*>([^<]*)<\/option>/g)) {
    out.set(Number(m[1]), m[2].replace(/&amp;/g, "&"));
  }
  return out;
}

/** The label a message read before #1850: index, parameter, level, forecast
 *  and member, from the listing, independent of `gribFieldLabel`. */
function before(m: MessageListing): string {
  return [`#${m.index}`, m.abbreviation || m.parameter, m.level, m.forecast,
    m.ensembleMember != null ? `member ${m.ensembleMember}` : null]
    .filter((s) => !!s)
    .join(" · ");
}

function listing(file: string): MessageListing[] {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const h = native.open(fs.readFileSync(file));
  return Array.from({ length: h.count() }, (_, i) => h.messageListing(i));
}

async function open(file: string, message: number): Promise<{ panel: RealPanel; editorHtml: string; close: () => void }> {
  const p = await provider();
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: message }));
  const close = () => {
    panel.dispose();
    editor.close();
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
  } catch (e) {
    close();
    throw e;
  }
  return { panel, editorHtml: editor.html(), close };
}

suite("The pickers name a derived forecast (#1850)", function () {
  this.timeout(180_000);

  test("uv_derived: V and Field B options #1, #3 and #4 differ, and a member reads as before", async () => {
    assert.ok(fs.existsSync(DERIVED), `fixture missing: ${DERIVED}`);
    const listed = listing(DERIVED);
    const { panel, editorHtml, close } = await open(DERIVED, 0);
    try {
      const html = panel.html();
      for (const id of ["vector-field-v", "compare-field-b"]) {
        const opts = options(html, id);
        assert.strictEqual(opts.size, 7, `${id}: every message is offered`);
        const base = (i: number) => before(listed[i]);
        assert.strictEqual(opts.get(1), `${base(1)} · Unweighted mean of all members`, `${id}: #1, the mean`);
        assert.strictEqual(opts.get(3), `${base(3)} · Spread of all members`, `${id}: #3, the spread`);
        assert.strictEqual(opts.get(4), `${base(4)} · Derived forecast, kind missing`, `${id}: #4, kind missing`);
        assert.strictEqual(new Set([opts.get(1), opts.get(3), opts.get(4)]).size, 3, `${id}: #1, #3 and #4 read apart`);
        // Members are no derived forecast: they read exactly as before.
        assert.strictEqual(opts.get(2), base(2), `${id}: #2, member 7`);
        assert.strictEqual(opts.get(6), base(6), `${id}: #6, member 1`);
        assert.ok(opts.get(2)?.endsWith("member 7"));
      }
      // The panel's heading names it too, as it names a member.
      const state = await panel.drive([]);
      assert.ok(state.subtitle.includes("Unweighted mean of all members"), `the heading: ${state.subtitle}`);
      // And the message list has a Derived column for this file.
      assert.ok(editorHtml.includes("<th>Derived</th>"), "the message list has a Derived column");
      assert.ok(editorHtml.includes("<td>Spread of all members</td>"), "the spread's row names it");
    } finally {
      close();
    }
  });

  test("a file with no derived forecast: every option and the message list read as before", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1850-"));
    // Two messages, so the pickers are written.
    const gfs = fs.readFileSync(GFS);
    const file = path.join(dir, "gfs_twice.grib2");
    fs.writeFileSync(file, Buffer.concat([gfs, gfs]));
    try {
      const listed = listing(file);
      assert.ok(listed.every((m) => m.derivedForecast === null), "no message is a derived forecast");
      const { panel, editorHtml, close } = await open(file, 0);
      try {
        const html = panel.html();
        for (const id of ["vector-field-v", "compare-field-b"]) {
          const opts = options(html, id);
          assert.deepStrictEqual([...opts.values()], listed.map(before), `${id}: unchanged`);
        }
        assert.ok(!editorHtml.includes("<th>Derived</th>"), "no Derived column");
      } finally {
        close();
      }
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
