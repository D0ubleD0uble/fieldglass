// The arrow key is an arrow (#1046).
//
// From the 0.6.0 manual pass (§6): the Vectors row's key read `⟶ 18.1 m s⁻¹`,
// a glyph of fixed width, while a full-length arrow on the map is as long as
// the spacing between sampled cells on that map. Now the key is an arrow drawn
// on the map at the length a full-length arrow is drawn there, in the arrows'
// colour and weight, labelled with the speed it stands for, and it goes out
// with the arrows in an exported PNG.
//
// Driven through the real panel: the page asks for the arrows, the provider
// answers through the real addon, and the key is read back off the overlay
// canvas's own pixels and out of the PNG the page exports. The oracle for
// "the fastest arrow" is the field itself: each drawn arrow's tail is matched
// to the cell it starts from, through the same projection, and the cell's
// decoded value gives its speed.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as zlib from "zlib";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { resolveRerenderOptions } from "../../provider";
import { composeVectorScale, composeVectorScaleNote } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  type VectorKeyState,
  waitUntil,
} from "./real-panel";

/** One arrow is one run of five vertices: tail, tip, barb, tip, barb. */
const ARROW_VERTICES = 5;
/** A colour nothing else on the page is drawn in. */
const INK = "#ff00ff";
/** Thick enough that the shaft fills the row through its middle. */
const WEIGHT = 3;

/** `gfs_c255_latlon.grib2`: a 2.5° global GFS message, 144 × 73, rows from
 *  lat 90 down to −90, columns from lon 0 east. */
const NI = 144;
const NJ = 73;

function gfsFixture(): string {
  const p = path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "gfs_c255_latlon.grib2");
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** The fixture twice over, as a file of two messages: a u/v pair on one grid,
 *  the field against itself. Every arrow points north-east and its speed is
 *  √2 times the field's value at its cell. */
function pairFile(): string {
  const one = fs.readFileSync(gfsFixture());
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-vector-key-"));
  const file = path.join(dir, "gfs_pair.grib2");
  fs.writeFileSync(file, Buffer.concat([one, one]));
  return file;
}

/** The tail and tip of every arrow the result drew whole, in raster pixels. */
function wholeArrows(result: Msg): Array<{ tail: [number, number]; length: number }> {
  const xy: ArrayLike<number> = result.xy;
  const segs: ArrayLike<number> = result.segLengths;
  const out: Array<{ tail: [number, number]; length: number }> = [];
  let at = 0;
  for (let s = 0; s < segs.length; s++) {
    if (segs[s] === ARROW_VERTICES) {
      const [x0, y0, x1, y1] = [xy[at * 2], xy[at * 2 + 1], xy[at * 2 + 2], xy[at * 2 + 3]];
      out.push({ tail: [x0, y0], length: Math.hypot(x1 - x0, y1 - y0) });
    }
    at += segs[s];
  }
  return out;
}

/** The drawn arrow that starts from the fastest cell, found from the field:
 *  every sampled cell is projected where the panel's arrows were, and the
 *  arrow whose tail lands on it takes that cell's speed. */
function fastestDrawn(
  file: string,
  request: Msg,
  result: Msg,
): { length: number; speed: number; matched: number; drawn: number } {
  const native = loadNative();
  assert.ok(native, "native module must load");
  const handle = native.Grib2Handle.fromBytes(fs.readFileSync(file));
  const values = handle.decodeGrid(0);
  const options = resolveRerenderOptions(request.options ?? {});
  const step = request.spacing ?? Math.max(1, Math.floor(Math.max(NI, NJ) / 24));
  const arrows = wholeArrows(result);
  let best: { length: number; speed: number } | undefined;
  let matched = 0;
  for (let j = 0; j < NJ; j += step) {
    for (let i = 0; i < NI; i += step) {
      const k = j * NI + i;
      if (values.mask[k] !== 1) continue;
      const lat = 90 - 2.5 * j;
      const lon = 2.5 * i;
      const at = handle.projectOverlay(0, options, new Float64Array([lat, lon, lat, lon]), new Uint32Array([2]));
      if (at.xy.length === 0) continue;
      const arrow = arrows.find((a) => Math.hypot(a.tail[0] - at.xy[0], a.tail[1] - at.xy[1]) < 1e-6);
      if (!arrow) continue;
      matched += 1;
      const speed = Math.SQRT2 * Math.abs(values.values[k]);
      if (!best || speed > best.speed) best = { length: arrow.length, speed };
    }
  }
  assert.ok(best, "some drawn arrow starts from a sampled cell");
  return { ...best, matched, drawn: arrows.length };
}

interface Drawn {
  state: PageState;
  key: VectorKeyState;
  request: Msg;
  result: Msg;
  /** The raster's width, which the overlay is scaled from. */
  rasterWidth: number;
}

/** Wait for the arrows the page asks for after `from`, until it has stopped
 *  asking, then read the page. */
async function arrowsAfter(panel: RealPanel, from: { sent: number; received: number }, what: string): Promise<Drawn> {
  await panel.waitSent(isType("vectorResult"), from.sent, `the arrows for ${what}`);
  for (;;) {
    // A round trip through the page: everything sent before it is handled.
    const state = await panel.drive([]);
    const request = panel.received.filter(isType("vectorRequest")).pop();
    const result = panel.sent.find((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq);
    if (!result) {
      await panel.waitSent((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq, from.sent, `the arrows for ${what}`);
      continue;
    }
    assert.ok(!result.error, `${what}: ${result.error}`);
    const grid = panel.sent.filter(isType("gridReady")).pop();
    assert.ok(state.vectorKey, `${what}: a key is drawn`);
    return { state, key: state.vectorKey, request, result, rasterWidth: grid.width };
  }
}

/** Wait for the render the page asks for after `from`, then for its arrows. */
async function renderAfter(panel: RealPanel, from: { sent: number; received: number }, what: string): Promise<Drawn> {
  const grid = await panel.waitSent(isType("gridReady"), from.sent, `the ${what} render`);
  return arrowsAfter(panel, { sent: panel.sent.indexOf(grid) + 1, received: from.received }, what);
}

const mark = (panel: RealPanel) => ({ sent: panel.sent.length, received: panel.received.length });

/** The key's length, measured off the canvas, against what Rust measured off
 *  the map's arrows, both in canvas pixels. */
function assertKeyMatches(d: Drawn, file: string, what: string): void {
  const scale = d.key.canvas.width / d.rasterWidth;
  const reference = d.result.referenceLength;
  assert.ok(typeof reference === "number" && reference > 0, `${what}: a reference length, got ${reference}`);
  // The page drew what it was given, scaled as the arrows are.
  assert.strictEqual(d.key.drawn.share, 1, `${what}: the key fits the map at full length`);
  assert.ok(
    Math.abs(d.key.drawn.length - reference * scale) < 1e-6,
    `${what}: drawn ${d.key.drawn.length} px for ${reference} raster px × ${scale}`,
  );
  // And the canvas holds it: tail to tip, plus the round join past the tip.
  assert.ok(d.key.measured, `${what}: the shaft is on the canvas`);
  const measured = d.key.measured.to - d.key.measured.from;
  const expected = d.key.drawn.length + d.key.drawn.lineWidth / 2;
  assert.ok(Math.abs(measured - expected) <= 1, `${what}: the shaft is ${measured} px on the canvas, expected ${expected}`);
  // The length is the fastest drawn arrow's, as the map draws it.
  const fastest = fastestDrawn(file, d.request, d.result);
  assert.ok(fastest.matched >= fastest.drawn * 0.9, `${what}: ${fastest.matched} of ${fastest.drawn} arrows placed`);
  assert.ok(
    Math.abs(fastest.length * scale - d.key.drawn.length) <= 1,
    `${what}: the fastest arrow is ${fastest.length * scale} px, the key ${d.key.drawn.length} px`,
  );
  assert.ok(
    Math.abs(fastest.speed - d.result.referenceSpeed) < 1e-6 * fastest.speed,
    `${what}: the fastest cell drawn is ${fastest.speed}, the key says ${d.result.referenceSpeed}`,
  );
  // In the arrows' colour and weight.
  assert.strictEqual(d.key.drawn.colour.toLowerCase(), INK);
  assert.strictEqual(d.key.drawn.lineWidth, WEIGHT * d.key.dpr);
}

/** The longest drawn arrow, in canvas pixels. */
function longestDrawn(d: Drawn): number {
  const scale = d.key.canvas.width / d.rasterWidth;
  return Math.max(...wholeArrows(d.result).map((a) => a.length)) * scale;
}

/** An 8-bit RGB or RGBA PNG, non-interlaced, as canvas `toDataURL` writes. */
function decodePng(png: Buffer): { width: number; height: number; rgba: Uint8Array } {
  assert.strictEqual(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", "a PNG signature");
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat: Buffer[] = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at);
    const type = png.toString("ascii", at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      assert.strictEqual(data[8], 8, "8-bit samples");
      assert.ok(data[9] === 6 || data[9] === 2, `RGB or RGBA, got colour type ${data[9]}`);
      channels = data[9] === 6 ? 4 : 3;
      assert.strictEqual(data[12], 0, "not interlaced");
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    at += 12 + length;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const rows = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? rows[y * stride + x - channels] : 0;
      const b = y > 0 ? rows[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? rows[(y - 1) * stride + x - channels] : 0;
      let v = line[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += Math.floor((a + b) / 2);
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      rows[y * stride + x] = v & 0xff;
    }
  }
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    for (let k = 0; k < 3; k++) rgba[i * 4 + k] = rows[i * channels + k];
    rgba[i * 4 + 3] = channels === 4 ? rows[i * channels + 3] : 255;
  }
  return { width, height, rgba };
}

/** The longest horizontal run of pixels at least half magenta, in any row. */
function longestInkRun(image: { width: number; height: number; rgba: Uint8Array }): number {
  let longest = 0;
  for (let y = 0; y < image.height; y++) {
    let run = 0;
    for (let x = 0; x < image.width; x++) {
      const i = (y * image.width + x) * 4;
      const [r, g, b] = [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]];
      if (r - g >= 127 && b - g >= 127) {
        run += 1;
        longest = Math.max(longest, run);
      } else {
        run = 0;
      }
    }
  }
  return longest;
}

/** When `FIELDGLASS_SHOTS` names a folder, the screen as it is now, saved
 *  there as `name`: the before-and-after pictures for the owner. */
async function shot(name: string): Promise<void> {
  const dir = process.env.FIELDGLASS_SHOTS;
  if (!dir) return;
  // Long enough for the panel to paint what the test just read.
  await new Promise((r) => setTimeout(r, 400));
  const { execFileSync } = await import("child_process");
  try {
    execFileSync("import", ["-window", "root", path.join(dir, name)]);
  } catch (e) {
    console.warn(`no screenshot ${name}: ${e}`);
  }
}

suite("Vector key (#1046)", () => {
  test("the key is the fastest arrow as drawn, follows spacing and projection, and is in the PNG", async function () {
    this.timeout(120_000);
    const p = await provider();
    const file = pairFile();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // Equirectangular, the arrows in a colour of their own, V = message 1.
      let from = mark(panel);
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent(isType("gridReady"), from.sent, "the equirectangular render");
      from = mark(panel);
      await panel.drive([
        { id: "color-vectors", value: INK },
        { id: "width-vectors", value: WEIGHT },
        { id: "vector-field-v", value: 1 },
        { id: "overlay-vectors", checked: true },
      ]);
      const equirect = await arrowsAfter(panel, from, "Equirectangular");
      await shot("after-equirectangular.png");
      assertKeyMatches(equirect, file, "Equirectangular");

      // The label: the speed and units the arrows were drawn to, on the map
      // and, in words, in the Vectors row.
      const label = composeVectorScale(equirect.result.referenceSpeed, "Pa");
      assert.strictEqual(label, `${equirect.result.referenceSpeed.toPrecision(3)} Pa`);
      assert.strictEqual(equirect.key.drawn.label, label);
      assert.ok(
        equirect.state.text.includes(composeVectorScaleNote(label)),
        `the row reads "${composeVectorScaleNote(label)}"`,
      );
      assert.ok(!equirect.state.text.includes("⟶"), "no fixed glyph stands in for the arrow");

      // A sparser spacing: longer arrows, and the key with them.
      from = mark(panel);
      await panel.drive([{ id: "vector-spacing", value: 12 }]);
      const sparse = await arrowsAfter(panel, from, "every 12 cells");
      assert.strictEqual(sparse.request.spacing, 12);
      assertKeyMatches(sparse, file, "every 12 cells");
      assert.ok(
        sparse.key.drawn.length > equirect.key.drawn.length * 1.5,
        `the key lengthens with the spacing: ${equirect.key.drawn.length} → ${sparse.key.drawn.length} px`,
      );
      assert.ok(longestDrawn(sparse) > longestDrawn(equirect), "and so do the arrows");

      // Zoomed in, the window is a few sampled cells across, and a
      // full-length arrow would run off the map: the key is halved, speed and
      // all, until it fits.
      let zoomed: Drawn | undefined;
      const lengths: string[] = [];
      for (let k = 0; k < 6 && !(zoomed && zoomed.key.drawn.share < 1); k++) {
        from = mark(panel);
        await panel.drive([{ id: "zoom-in", click: true }]);
        zoomed = await renderAfter(panel, from, `zoom ${k + 1}`);
        lengths.push(`${zoomed.key.drawn.length.toFixed(1)} of ${zoomed.key.canvas.width}`);
      }
      assert.ok(zoomed);
      const half = zoomed.key;
      assert.ok(half.drawn.share < 1, `the key is cut to fit: ${lengths.join(", ")}`);
      assert.ok(half.drawn.tip < half.canvas.width, "and its arrow is on the map");
      assert.strictEqual(half.drawn.label, composeVectorScale(zoomed.result.referenceSpeed * half.drawn.share, "Pa"));
      assert.ok(
        Math.abs(
          half.drawn.length -
            zoomed.result.referenceLength * (half.canvas.width / zoomed.rasterWidth) * half.drawn.share,
        ) < 1e-6,
        `the cut key is that share of a full-length arrow: ${half.drawn.length} px`,
      );
      assert.ok(
        zoomed.result.referenceLength > sparse.result.referenceLength * 4,
        `zooming in lengthens a full-length arrow: ${sparse.result.referenceLength} → ${zoomed.result.referenceLength} px`,
      );
      from = mark(panel);
      await panel.drive([{ id: "zoom-reset", click: true }]);
      const wide = await renderAfter(panel, from, "Reset view");
      assert.strictEqual(wide.key.drawn.share, 1, "and full length again on the whole map");

      // The export carries the key, at the length the map's arrows are
      // exported at.
      const saved = vscode.window.showSaveDialog;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
      try {
        from = mark(panel);
        await panel.drive([{ id: "export-png", click: true }]);
        const exported = await waitUntil(
          () => panel.received.slice(from.received).find(isType("exportPng")),
          "the exported PNG",
        );
        const png = Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64");
        const image = decodePng(png);
        const dir = process.env.FIELDGLASS_SHOTS;
        if (dir) fs.writeFileSync(path.join(dir, "after-export.png"), png);
        // The map is exported at the raster's own size, so the overlay, key
        // and all, is scaled by raster over canvas.
        const toExport = wide.rasterWidth / wide.key.canvas.width;
        const expected = (wide.key.drawn.length + wide.key.drawn.lineWidth / 2) * toExport;
        const run = longestInkRun(image);
        assert.ok(Math.abs(run - expected) <= 1.5, `the key is ${run} px in the PNG, expected ${expected}`);
      } finally {
        vscode.window.showSaveDialog = saved;
      }

      // Orthographic, centred on the equator: the same rule, and here, where
      // a globe only shortens what is off-centre, the key is also the longest
      // arrow on the map to within a pixel. (On Equirectangular it need not
      // be: that map stretches a parallel by 1 / cos(lat), so a slower arrow
      // near a pole can be drawn longer than the fastest.)
      from = mark(panel);
      await panel.drive([
        { id: "vector-spacing", value: "" },
        { id: "picker-projection", value: "orthographic" },
      ]);
      const globe = await renderAfter(panel, from, "Orthographic");
      await shot("after-orthographic.png");
      assert.strictEqual(globe.request.options.projection, "orthographic");
      assertKeyMatches(globe, file, "Orthographic");
      assert.ok(
        Math.abs(longestDrawn(globe) - globe.key.drawn.length) <= 1,
        `Orthographic: the longest arrow is ${longestDrawn(globe)} px, the key ${globe.key.drawn.length} px`,
      );

      // Arrows off: no key.
      from = mark(panel);
      const off = await panel.drive([{ id: "overlay-vectors", checked: false }]);
      assert.strictEqual(off.vectorKey, null, "no arrows, no key");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });
});
