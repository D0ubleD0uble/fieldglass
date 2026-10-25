// A message that cannot be drawn says why (#1465).
//
// The table used to answer every such message with "grid dimensions unknown",
// even for a Bi-Fourier message whose Size column showed `N4 M4` on the same
// row. Each case below goes through the real addon and the real editor: the
// table's note and the refusal a Render request gets must name the same cause.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { type Msg, extensionPath, openEditor, provider } from "./real-panel";

const CRATES = path.join(extensionPath(), "..", "crates");

function grib2(name: string): Buffer {
  return Buffer.from(fs.readFileSync(path.join(CRATES, "fieldglass-grib2", "tests", "fixtures", name)));
}

/** The §3 grid definition section of a one-message GRIB2 file. */
function gridSection(b: Buffer): number {
  let at = 16;
  while (b[at + 4] !== 3) at += b.readUInt32BE(at);
  return at;
}

/** A GRIB1 message with no grid description section, naming grid `number`. */
function grib1Naming(number: number): Buffer {
  const bds = 12;
  const total = 8 + 28 + bds + 4;
  const m = Buffer.alloc(total);
  m.write("GRIB");
  m[6] = total;
  m[7] = 1;
  m[8 + 2] = 28; // PDS length
  m[8 + 6] = number; // octet 7: predefined grid number
  m[8 + 7] = 0; // octet 8: no GDS, no BMS
  m.write("7777", total - 4);
  return m;
}

interface Case {
  name: string;
  bytes: () => Buffer;
  placement: string;
  /** What the message says about why, as a user reads it. */
  reason: RegExp;
}

const CASES: Case[] = [
  {
    name: "Bi-Fourier coefficients",
    bytes: () => grib2("bifourier_ellipse_ieee32.grib2"),
    placement: "no_raster",
    reason: /^Bi-Fourier synthesis isn't supported, so these coefficients can't be drawn\.$/,
  },
  {
    name: "a GRIB2 grid template this build does not model (3.4)",
    bytes: () => {
      const b = grib2("regular_latlon_surface.grib2");
      b.writeUInt16BE(4, gridSection(b) + 12);
      return b;
    },
    placement: "unsupported",
    reason: /^grid template 3\.4 isn't supported yet\.$/,
  },
  {
    name: "a template this build does not model that states no points",
    bytes: () => {
      const b = grib2("regular_latlon_surface.grib2");
      const at = gridSection(b);
      b.writeUInt16BE(4, at + 12);
      b.writeUInt32BE(0, at + 6); // octets 7-10: number of data points
      return b;
    },
    placement: "no_raster",
    reason: /^grid template 3\.4 isn't supported yet\.$/,
  },
  {
    name: "a GRIB1 data representation type this build does not model (90)",
    bytes: () => {
      const b = Buffer.from(
        fs.readFileSync(path.join(CRATES, "fieldglass-grib1", "tests", "fixtures", "ieee32_cmc_wind.grib1")),
      );
      b[8 + b.readUIntBE(8, 3) + 5] = 90;
      return b;
    },
    placement: "unsupported",
    reason: /^grid type 90 isn't supported yet\.$/,
  },
  {
    name: "a grid that declares no columns",
    bytes: () => {
      const b = grib2("regular_latlon_surface.grib2");
      b.writeUInt32BE(0, gridSection(b) + 30);
      return b;
    },
    placement: "no_raster",
    reason: /^the grid declares no columns or rows\.$/,
  },
  {
    name: "a GRIB1 message that states no grid (255)",
    bytes: () => grib1Naming(255),
    placement: "no_raster",
    reason: /^the message states no grid\.$/,
  },
  {
    name: "a GRIB1 message naming a predefined grid this build lacks",
    bytes: () => grib1Naming(7),
    placement: "predefined_unresolved",
    reason: /^the message names a predefined grid that Fieldglass doesn't have\.$/,
  },
];

/** The note in place of the Render button, as text. */
function noteIn(html: string): string {
  const raw = /<div class="render-na">([^<]*)<\/div>/.exec(html)?.[1] ?? "";
  return raw.replace(/&#39;/g, "'").replace(/&amp;/g, "&");
}

suite("an unrenderable message names its reason (#1465)", () => {
  for (const c of CASES) {
    test(c.name, async () => {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1465-"));
      const file = path.join(dir, "unrenderable.grib");
      fs.writeFileSync(file, c.bytes());
      const editor = await openEditor(await provider(), file);
      try {
        const note = noteIn(editor.html());
        assert.ok(note.startsWith("Render not available: "), `the table says: ${note}`);
        assert.ok(!/dimensions unknown/.test(note), `no stale wording: ${note}`);
        const reason = note.slice("Render not available: ".length);
        assert.match(reason, c.reason, `${c.name}: ${note}`);
        assert.ok(!editor.html().includes('class="render-btn"'), "and no Render button");

        // Asking for the render anyway is refused for the same reason.
        editor.send({ type: "decodeGrid", messageIndex: 0 });
        const refusal: Msg | undefined = editor.sent.find((m) => m.type === "gridError");
        assert.ok(refusal, "the request is refused");
        assert.strictEqual(refusal.error, reason, "the refusal and the table agree");
      } finally {
        editor.close();
        fs.rmSync(dir, { recursive: true, force: true });
      }
    });
  }
});
