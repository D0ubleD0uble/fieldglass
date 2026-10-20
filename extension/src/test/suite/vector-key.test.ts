// The arrow key is an arrow (#1046).
//
// From the 0.6.0 manual pass (§6): the Vectors row's key read `⟶ 18.1 m s⁻¹`,
// a glyph of fixed width, while a full-length arrow on the map is as long as
// the spacing between sampled cells on that map. Now the key is an arrow drawn
// on the map at the length a full-length arrow is drawn there, in the arrows'
// colour and weight, labelled with the speed it stands for, and it goes out
// with the arrows in an exported PNG. Arrows are drawn at one on-screen length
// per speed (#1159), so the key is also the longest arrow on the map, on
// Equirectangular as on a globe.
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
import { composeVectorScale, composeVectorScaleNote, VECTOR_KEY_LEFT_OFF } from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  type ExportedKey,
  type VectorKeyState,
  waitUntil,
} from "./real-panel";
import { expectedImageWidth } from "./row-width";

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
function wholeArrows(result: Msg): Array<{ tail: [number, number]; tip: [number, number]; length: number }> {
  const xy: ArrayLike<number> = result.xy;
  const segs: ArrayLike<number> = result.segLengths;
  const out: Array<{ tail: [number, number]; tip: [number, number]; length: number }> = [];
  let at = 0;
  for (let s = 0; s < segs.length; s++) {
    if (segs[s] === ARROW_VERTICES) {
      const [x0, y0, x1, y1] = [xy[at * 2], xy[at * 2 + 1], xy[at * 2 + 2], xy[at * 2 + 3]];
      out.push({ tail: [x0, y0], tip: [x1, y1], length: Math.hypot(x1 - x0, y1 - y0) });
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
  const handle = native.open(fs.readFileSync(file));
  const values = handle.decode(0);
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
      const at = handle.projectOverlay(0, options, { latlon: new Float64Array([lat, lon, lat, lon]), ringLengths: new Uint32Array([2]) });
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
  rasterHeight: number;
}

/** Wait for the arrows the page asks for after `from`, until it has stopped
 *  asking, then read the page. */
async function arrowsAfter(
  panel: RealPanel,
  from: { sent: number; received: number },
  what: string,
  keyed = true,
): Promise<Drawn> {
  await panel.waitSent(isType("vectorResult"), from.sent, `the arrows for ${what}`);
  for (;;) {
    // The newest request the page has made, and the reply to it, if the
    // provider has sent one. The page is read only after that: a round trip
    // through it handles everything sent before it, so the state read holds
    // the arrows this reply drew. Read before the reply was looked for, the
    // state could miss an overlay draw for a reply sent in between (#1178).
    const request = panel.received.filter(isType("vectorRequest")).pop();
    const result = panel.sent.find((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq);
    if (!result) {
      await panel.waitSent((m: Msg) => isType("vectorResult")(m) && m.seq === request.seq, from.sent, `the arrows for ${what}`);
      continue;
    }
    const state = await panel.drive([]);
    // Another request may have gone out while the page was read.
    if (panel.received.filter(isType("vectorRequest")).pop() !== request) continue;
    assert.ok(!result.error, `${what}: ${result.error}`);
    const grid = panel.sent.filter(isType("gridReady")).pop();
    if (keyed) assert.ok(state.vectorKey, `${what}: a key is drawn`);
    return {
      state,
      key: state.vectorKey as VectorKeyState,
      request,
      result,
      rasterWidth: grid.width,
      rasterHeight: grid.height,
    };
  }
}

/** Wait for the render the page asks for after `from`, then for its arrows. */
async function renderAfter(
  panel: RealPanel,
  from: { sent: number; received: number },
  what: string,
  keyed = true,
): Promise<Drawn> {
  const grid = await panel.waitSent(isType("gridReady"), from.sent, `the ${what} render`);
  return arrowsAfter(panel, { sent: panel.sent.indexOf(grid) + 1, received: from.received }, what, keyed);
}

const mark = (panel: RealPanel) => ({ sent: panel.sent.length, received: panel.received.length });

/** The key's length, measured off the canvas, against what Rust measured off
 *  the map's arrows, both in canvas pixels. */
function assertKeyMatches(d: Drawn, file: string, what: string): void {
  const scale = d.key.canvas.width / d.rasterWidth;
  const reference = d.result.referenceLength;
  assert.ok(typeof reference === "number" && reference > 0, `${what}: a reference length, got ${reference}`);
  // The page drew what it was given, scaled as the arrows are.
  const [bx, by, bw, bh] = d.key.drawn.box;
  assert.ok(bx + bw <= d.key.canvas.width && by + bh <= d.key.canvas.height, `${what}: the key fits the map`);
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
  // And the longest arrow on the map, whatever the projection does to
  // distance: one on-screen length per speed (#1159).
  assert.ok(
    Math.abs(longestDrawn(d) - d.key.drawn.length) <= 1,
    `${what}: the longest arrow is ${longestDrawn(d)} px, the key ${d.key.drawn.length} px`,
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

/** A CSS colour as the page gives it: `#rgb`, `#rrggbb(aa)` or `rgb(a)(…)`. */
function rgbOf(css: string): [number, number, number] {
  const c = css.trim();
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(c);
  if (hex) {
    const h = hex[1].length === 3 ? hex[1].replace(/./g, (d) => d + d) : hex[1];
    return [0, 2, 4].map((k) => parseInt(h.slice(k, k + 2), 16)) as [number, number, number];
  }
  const fn = /^rgba?\(([^)]+)\)$/i.exec(c);
  assert.ok(fn, `a colour: ${css}`);
  const [r, g, b] = fn[1].split(/[ ,/]+/).map(Number);
  return [r, g, b];
}

/** How many rows of the exported key's backing hold its label: pixels near
 *  the label's colour, right of the arrow's tip. */
function labelHeight(image: { width: number; height: number; rgba: Uint8Array }, key: ExportedKey): number {
  const ink = rgbOf(key.labelColour);
  const [bx, by, bw, bh] = key.box;
  let top = Infinity;
  let bottom = -Infinity;
  // Inside the backing only: its edge rows and columns are part-covered,
  // and show the lighter image under it.
  for (let y = Math.ceil(by) + 1; y < Math.floor(by + bh) - 1; y++) {
    for (let x = Math.ceil(key.tip + key.lineWidth); x < Math.floor(bx + bw) - 1; x++) {
      const i = (y * image.width + x) * 4;
      const d = Math.hypot(image.rgba[i] - ink[0], image.rgba[i + 1] - ink[1], image.rgba[i + 2] - ink[2]);
      if (d < 90) {
        top = Math.min(top, y);
        bottom = Math.max(bottom, y);
      }
    }
  }
  return bottom >= top ? bottom - top + 1 : 0;
}

/** Click Export PNG with the save dialog cancelled, and read back the PNG the
 *  page made and the key it says it drew in it. */
async function exportPng(panel: RealPanel): Promise<{ image: ReturnType<typeof decodePng>; key: ExportedKey }> {
  const saved = vscode.window.showSaveDialog;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).showSaveDialog = () => Promise.resolve(undefined);
  try {
    const from = panel.received.length;
    await panel.drive([{ id: "export-png", click: true }]);
    const exported = await waitUntil(() => panel.received.slice(from).find(isType("exportPng")), "the exported PNG");
    const png = Buffer.from(String(exported.dataUrl).replace(/^data:image\/png;base64,/, ""), "base64");
    // Kept for a reviewer to look at, when asked for: FIELDGLASS_SHOTS_DIR.
    const shots = process.env.FIELDGLASS_SHOTS_DIR;
    if (shots) {
      fs.mkdirSync(shots, { recursive: true });
      fs.writeFileSync(path.join(shots, `vector-key-${Date.now()}.png`), png);
    }
    const state = await panel.drive([]);
    assert.ok(state.exportedKey, "the export drew a key");
    return { image: decodePng(png), key: state.exportedKey };
  } finally {
    vscode.window.showSaveDialog = saved;
  }
}

suite("Vector key (#1046)", () => {
  test("the key matches the longest arrow, follows spacing and projection, and is in the PNG", async function () {
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

      // Zoomed in, the window is a few sampled cells across. A full-length
      // arrow lengthens with the zoom until it is a quarter of the map's
      // shorter side, and holds there (#1168): the arrows spread apart
      // rather than run off the map, and the key stays on it.
      let zoomed: Drawn | undefined;
      const lengths: number[] = [sparse.result.referenceLength];
      for (let k = 0; k < 6; k++) {
        from = mark(panel);
        await panel.drive([{ id: "zoom-in", click: true }]);
        zoomed = await renderAfter(panel, from, `zoom ${k + 1}`);
        const length = zoomed.result.referenceLength;
        assert.ok(
          length >= lengths[lengths.length - 1] - 1e-9,
          `zooming in never shortens it: ${lengths.join(", ")}, ${length}`,
        );
        lengths.push(length);
        const [bx, , bw] = zoomed.key.drawn.box;
        assert.ok(bx + bw <= zoomed.key.canvas.width, `zoom ${k + 1}: the key is on the map`);
      }
      assert.ok(zoomed);
      const capped = 0.25 * Math.min(zoomed.rasterWidth, zoomed.rasterHeight);
      assert.ok(
        Math.abs(zoomed.result.referenceLength - capped) < 1e-9,
        `held at a quarter of the ${zoomed.rasterWidth} × ${zoomed.rasterHeight} map: ${lengths.join(", ")}`,
      );
      assert.ok(longestDrawn(zoomed) <= zoomed.key.drawn.length + 1, "and no arrow is longer than the key");

      // A panel narrowed step by step: the key stays a full-length arrow
      // labelled with the reference speed, on the map, until the map is too
      // small for it and its label; then there is no key and the Vectors row
      // says why. (The overlay is redrawn when arrows arrive, so each width
      // asks for them again, at a spacing whose arrows are all held to the
      // same full length at this zoom.)
      let leftOff = false;
      for (let px = 300; px >= 60 && !leftOff; px -= 8) {
        await panel.drive([{ id: "body", width: `${px}px` }]);
        from = mark(panel);
        await panel.drive([{ id: "vector-spacing", value: px % 16 === 0 ? 6 : 7 }]);
        const { state, result } = await arrowsAfter(panel, from, `a ${px}px panel`, false);
        assert.ok(Math.abs(result.referenceLength - capped) < 1e-9, `${px}px: still held at a quarter of the map`);
        const k = state.vectorKey;
        if (!k) {
          assert.ok(state.text.includes(VECTOR_KEY_LEFT_OFF), `${px}px: no key, and the row says why`);
          leftOff = true;
          continue;
        }
        assert.ok(!state.text.includes(VECTOR_KEY_LEFT_OFF), `${px}px: a key, and no note`);
        const [bx, by, bw, bh] = k.drawn.box;
        assert.ok(bx + bw <= k.canvas.width && by + bh <= k.canvas.height, `${px}px: the key is on the map`);
        assert.strictEqual(k.drawn.label, composeVectorScale(result.referenceSpeed, "Pa"));
        assert.ok(
          Math.abs(k.drawn.length - result.referenceLength * (k.canvas.width / zoomed.rasterWidth)) < 1e-6,
          `${px}px: the key is a full-length arrow`,
        );
      }
      assert.ok(leftOff, "a panel too small for the key leaves it off");
      await panel.drive([{ id: "body", width: "" }]);
      from = mark(panel);
      await panel.drive([
        { id: "zoom-reset", click: true },
        { id: "vector-spacing", value: 12 },
      ]);
      const wide = await renderAfter(panel, from, "Reset view");
      assertKeyMatches(wide, file, "Reset view");
      assert.ok(!wide.state.text.includes(VECTOR_KEY_LEFT_OFF), "and the note goes with it");

      // The export carries the key, drawn at the PNG's own resolution: the
      // map is the raster's size there, so a full-length arrow is
      // `referenceLength` PNG pixels, beside arrows exported at that scale.
      const exported = await exportPng(panel);
      assert.ok(
        Math.abs(exported.key.length - wide.result.referenceLength) < 1e-6,
        `the exported key is ${exported.key.length} px for ${wide.result.referenceLength} raster px`,
      );
      const run = longestInkRun(exported.image);
      const expected = exported.key.length + exported.key.lineWidth / 2;
      assert.ok(Math.abs(run - expected) <= 1.5, `the key is ${run} px in the PNG, expected ${expected}`);
      assert.strictEqual(exported.key.label, wide.key.drawn.label);
      assert.ok(labelHeight(exported.image, exported.key) >= 7, "its label is legible in the PNG");

      // A small raster shown large: the panel's canvas is much wider than the
      // raster the PNG is made at. The label is still drawn at a size the PNG
      // can show, not shrunk with the overlay. (The overlay is redrawn on a
      // window resize, which a widened body does not fire, so the arrows are
      // asked for again.)
      await panel.drive([{ id: "body", width: "2400px" }]);
      from = mark(panel);
      await panel.drive([{ id: "vector-spacing", value: 12 }]);
      const large = await arrowsAfter(panel, from, "a large panel");
      // The image is drawn no taller than 80vh (#1252), so however wide the
      // panel, this 2:1 map is no wider than the row or 80% of the page's
      // height times its aspect, whichever is less: about 1,120 px on a 702 px
      // page. The bar follows that width rather than the window's, with a
      // tenth of it spare, so it holds in a short window too. It only says the
      // canvas is drawn well over the raster; the guard that matters is the
      // label's size in the PNG below (`fontPx` 11).
      const { image, plotRow, viewportHeight } = large.state;
      const room = image && plotRow ? plotRow.right - image.left : Infinity;
      const held = expectedImageWidth(viewportHeight, large.rasterWidth / large.rasterHeight, room);
      const bar = Math.max(large.rasterWidth, Math.min(1.5 * large.rasterWidth, 0.9 * held));
      assert.ok(
        large.key.canvas.width > bar,
        `the canvas (${large.key.canvas.width} px) is well over the raster (${large.rasterWidth} px)`,
      );
      const shrunk = await exportPng(panel);
      const height = labelHeight(shrunk.image, shrunk.key);
      assert.ok(height >= 7, `the label is ${height} px tall in the PNG`);
      assert.strictEqual(shrunk.key.fontPx, 11, "the label is the colour bar's size in the PNG");
      assert.ok(
        Math.abs(shrunk.key.length - large.result.referenceLength) < 1e-6,
        "and the key is still a full-length arrow at the PNG's scale",
      );
      await panel.drive([{ id: "body", width: "" }]);

      // Orthographic, centred on the equator: the same rule, and the key is
      // the longest arrow there too, as on Equirectangular, though a globe
      // squeezes distance towards its limb.
      from = mark(panel);
      await panel.drive([
        { id: "vector-spacing", value: "" },
        { id: "picker-projection", value: "orthographic" },
      ]);
      const globe = await renderAfter(panel, from, "Orthographic");
      assert.strictEqual(globe.request.options.projection, "orthographic");
      assertKeyMatches(globe, file, "Orthographic");

      // Arrows off: no key.
      const off = await panel.drive([{ id: "overlay-vectors", checked: false }]);
      assert.strictEqual(off.vectorKey, null, "no arrows, no key");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });

  // #1168: a full-length arrow is the spacing between arrows on screen, which
  // a zoom spreads without limit. At the deepest zoom (a hundredth of a
  // degree) on this 2.5° grid it was some 1,500 map widths: a headless line
  // across the map, and a key whose halving stopped at 1/64 and ran its label
  // off the map. Now a full-length arrow is held to a quarter of the map's
  // shorter side, the arrows stay on it with their heads, and the key is a
  // whole full-length arrow beside its speed.
  test("at the deepest zoom the arrows keep their heads and the key fits, in the panel and the PNG", async function () {
    this.timeout(240_000);
    const p = await provider();
    const file = pairFile();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
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
      await arrowsAfter(panel, from, "Equirectangular");

      // Zoom in with + to the panel's floor of 0.01°: 360° halves to it in
      // sixteen steps. A click at the floor renders nothing new. + zooms about
      // the map's centre, which is a sampled cell (the equator, at a column
      // the automatic spacing of 6 samples), so one arrow starts in view.
      let deep: Drawn | undefined;
      let span = Infinity;
      for (let k = 0; k < 24 && span > 0.01 + 1e-9; k++) {
        from = mark(panel);
        await panel.drive([{ id: "zoom-in", click: true }]);
        deep = await renderAfter(panel, from, `zoom ${k + 1}`, false);
        const o = deep.request.options;
        span = o.boundsLonMax - o.boundsLonMin;
      }
      assert.ok(deep);
      assert.ok(Math.abs(span - 0.01) < 1e-9, `the window is at the floor: ${span}°`);
      // Exported before anything is checked, so a failure still leaves the
      // PNG for FIELDGLASS_SHOTS_DIR.
      const exported = await exportPng(panel);

      // The arrows: a full-length one fits on the map, and an arrow drawn from
      // a cell in view is there whole, head and all.
      const [w, h] = [deep.rasterWidth, deep.rasterHeight];
      const full = deep.result.referenceLength;
      assert.ok(typeof full === "number" && full > 0, `a full-length arrow, got ${full}`);
      assert.ok(full <= Math.min(w, h), `a full-length arrow is ${full} px on a ${w} × ${h} map`);
      const onMap = (x: number, y: number) => x >= -0.5 && x <= w - 0.5 && y >= -0.5 && y <= h - 0.5;
      const whole = wholeArrows(deep.result).filter((a) => onMap(...a.tail) && onMap(...a.tip));
      assert.ok(whole.length >= 1, `an arrow is on the map with its head: ${wholeArrows(deep.result).length} drawn`);

      // The key inside the map, and true to its label.
      const inside = (box: number[], region: number[], what: string) => {
        const [bx, by, bw, bh] = box;
        const [rx, ry, rw, rh] = region;
        assert.ok(
          bx >= rx && by >= ry && bx + bw <= rx + rw + 1e-6 && by + bh <= ry + rh + 1e-6,
          `${what}: the key [${box.map((v) => v.toFixed(1))}] is inside the map [${region.map((v) => v.toFixed(1))}]`,
        );
      };
      const key = deep.state.vectorKey;
      assert.ok(key, "a key is drawn");
      {
        inside(key.drawn.box, key.drawn.region, "the panel");
        const scale = key.canvas.width / w;
        assert.ok(
          Math.abs(key.drawn.length - full * scale) < 1e-6,
          "the key is a full-length arrow",
        );
        assert.strictEqual(key.drawn.label, composeVectorScale(deep.result.referenceSpeed, "Pa"));
        assert.ok(key.measured, "the shaft is on the canvas");
        assert.ok(!deep.state.text.includes(VECTOR_KEY_LEFT_OFF), "and the row does not say it was left off");
      }

      // The PNG, decoded: the key's shaft and label are in it, inside the map.
      {
        inside(exported.key.box, exported.key.region, "the PNG");
        assert.ok(
          Math.abs(exported.key.length - full) < 1e-6,
          `the exported key is ${exported.key.length} px for a full-length arrow of ${full} px`,
        );
        assert.strictEqual(
          exported.key.label,
          composeVectorScale(deep.result.referenceSpeed, "Pa"),
        );
        const [bx, by, bw, bh] = exported.key.box;
        assert.ok(bx + bw <= exported.image.width && by + bh <= exported.image.height, "the key is inside the image");
        const run = longestInkRun(exported.image);
        assert.ok(run >= exported.key.length, `the key's shaft is ${run} px in the PNG, for ${exported.key.length}`);
        assert.ok(labelHeight(exported.image, exported.key) >= 7, "its label is drawn in the PNG");
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });

  // #1178: arrows that arrive before the map has a size draw no key, and the
  // page said it would draw one once the map had a size. Nothing did, so the
  // key stayed off until some unrelated redraw. The map is taken out of the layout
  // here, the arrows asked for, and put back with nothing else
  // sent: the key has to come on its own.
  test("a key whose arrows arrived before the map was laid out is drawn once it is", async function () {
    this.timeout(120_000);
    const p = await provider();
    const file = pairFile();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
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
      const laid = await arrowsAfter(panel, from, "Equirectangular");
      assert.ok(laid.key.canvas.width > 1);

      // No layout for the map: the arrows arrive, and there is nothing to draw
      // a key on, nor a map to call too small.
      await panel.drive([{ selector: ".canvas-wrap", display: "none" }]);
      from = mark(panel);
      await panel.drive([{ id: "vector-spacing", value: 12 }]);
      const hidden = await arrowsAfter(panel, from, "a map out of the layout", false);
      assert.strictEqual(hidden.state.vectorKey, null, "no key on a map out of the layout");
      assert.ok(!hidden.state.text.includes(VECTOR_KEY_LEFT_OFF), "and no note about one");

      // Laid out again, and no window resize, control or reply: the page redraws.
      await panel.drive([{ selector: ".canvas-wrap", display: "" }]);
      let back = await panel.drive([]);
      for (let k = 0; k < 150 && !back.vectorKey; k++) {
        await new Promise((r) => setTimeout(r, 20));
        back = await panel.drive([]);
      }
      assert.ok(back.vectorKey, "the key is drawn once the map is laid out");
      assert.ok(back.vectorKey.canvas.width > 1);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });
});
