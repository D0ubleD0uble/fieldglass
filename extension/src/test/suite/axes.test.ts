// Line-plot and cross-section axes (#1043).
//
// A vertical axis is drawn the way up CF's `positive` attribute says (§4.3):
// a pressure profile stands upright with the surface at the bottom, on a
// logarithmic scale, a depth profile has the surface at the top, and a
// latitude-height section has the ground at the bottom by default. Every axis
// is ticked at real values (dates stay dates), and its end labels stay inside
// its own label area rather than running into the other axis, the colour bar
// or anything above the image.
//
// Driven through the real panel: the stores are written to disk, opened the
// way a user opens them, and the page reports where it drew each mark.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import {
  classicNetcdf,
  type AxisLabel,
  type Box,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type PageState,
  type RailsState,
  type RealPanel,
  waitUntil,
} from "./real-panel";

/** One array of a Zarr v2 store: its axes, shape, float32 values and the
 *  attributes a CF writer gives it. */
interface StoreArray {
  dims: string[];
  shape: number[];
  values: number[];
  attrs: Record<string, string>;
}

function writeStore(root: string, arrays: Record<string, StoreArray>): void {
  fs.mkdirSync(root);
  fs.writeFileSync(path.join(root, ".zgroup"), JSON.stringify({ zarr_format: 2 }));
  for (const [name, a] of Object.entries(arrays)) {
    const dir = path.join(root, name);
    fs.mkdirSync(dir);
    fs.writeFileSync(
      path.join(dir, ".zarray"),
      JSON.stringify({
        zarr_format: 2,
        shape: a.shape,
        chunks: a.shape,
        dtype: "<f8",
        compressor: null,
        fill_value: null,
        order: "C",
        filters: null,
      }),
    );
    fs.writeFileSync(path.join(dir, ".zattrs"), JSON.stringify({ _ARRAY_DIMENSIONS: a.dims, ...a.attrs }));
    const bytes = Buffer.alloc(a.values.length * 8);
    a.values.forEach((v, i) => bytes.writeDoubleLE(v, i * 8));
    fs.writeFileSync(path.join(dir, a.shape.map(() => "0").join(".")), bytes);
  }
}

// The NCEP/NCAR reanalysis's 17 pressure levels, stored as the file stores
// them, surface first, with its attributes.
const LEVELS = [1000, 925, 850, 700, 600, 500, 400, 300, 250, 200, 150, 100, 70, 50, 30, 20, 10];
// A profile like 30° S 25° E: warm at the surface, coldest near 100 mb.
const AIR = [31, 24, 19, 8, 0, -8, -18, -31, -40, -50, -62, -72, -66, -61, -54, -50, -43];
// An ocean's depths, surface first.
const DEPTHS = [0, 10, 50, 200, 1000];
const LAT3 = [-32.5, -30, -27.5];
const LON3 = [22.5, 25, 27.5];
const each = (n: number, v: number) => Array.from({ length: n }, () => v);

/** A store with `air(level, lat, lon)` on pressure levels and
 *  `temp(depth, lat, lon)` on depths, each the same at every cell. */
function profileStore(root: string): void {
  writeStore(root, {
    level: {
      dims: ["level"],
      shape: [LEVELS.length],
      values: LEVELS,
      attrs: { units: "millibar", positive: "down", axis: "Z", long_name: "Level" },
    },
    depth: { dims: ["depth"], shape: [DEPTHS.length], values: DEPTHS, attrs: { units: "m", positive: "down" } },
    lat: { dims: ["lat"], shape: [3], values: LAT3, attrs: { units: "degrees_north", axis: "Y" } },
    lon: { dims: ["lon"], shape: [3], values: LON3, attrs: { units: "degrees_east", axis: "X" } },
    air: {
      dims: ["level", "lat", "lon"],
      shape: [LEVELS.length, 3, 3],
      values: AIR.flatMap((v) => each(9, v)),
      attrs: { units: "degC" },
    },
    temp: {
      dims: ["depth", "lat", "lon"],
      shape: [DEPTHS.length, 3, 3],
      values: [25, 24, 18, 10, 4].flatMap((v) => each(9, v)),
      attrs: { units: "degC" },
    },
  });
}

// The daily file's time axis: 2024, one value a day, in hours since 1800 with
// the fractional seconds NCEP writes; and its 73 latitudes, north first.
const DAYS = Array.from({ length: 366 }, (_, d) => 1963536 + 24 * d);
const LAT73 = Array.from({ length: 73 }, (_, j) => 90 - 2.5 * j);

/** A store with `air(time, lat, lon)` over 2024 at 73 latitudes. */
function dailyStore(root: string): void {
  const lon = [0, 90, 180, 270];
  const values: number[] = [];
  for (let t = 0; t < DAYS.length; t++) {
    for (let j = 0; j < LAT73.length; j++) {
      const v = 270 + 20 * Math.cos((LAT73[j] * Math.PI) / 180) + 10 * Math.sin((2 * Math.PI * t) / 366);
      for (let i = 0; i < lon.length; i++) values.push(v);
    }
  }
  writeStore(root, {
    time: {
      dims: ["time"],
      shape: [DAYS.length],
      values: DAYS,
      attrs: { units: "hours since 1800-01-01 00:00:0.0", standard_name: "time", axis: "T" },
    },
    lat: { dims: ["lat"], shape: [73], values: LAT73, attrs: { units: "degrees_north", axis: "Y" } },
    lon: { dims: ["lon"], shape: [4], values: lon, attrs: { units: "degrees_east", axis: "X" } },
    air: { dims: ["time", "lat", "lon"], shape: [DAYS.length, 73, 4], values, attrs: { units: "degK" } },
  });
}

async function storePanel(build: (root: string) => void): Promise<{ panel: RealPanel; store: string; close: () => void }> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1043-"));
  const store = path.join(dir, "axes.zarr");
  build(store);
  const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    store,
    close: () => {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

function variableIndex(store: string, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.ZarrHandle.fromDirectory(store).variables().find((x) => x.name === name);
  assert.ok(v, `the store has ${name}`);
  return v.variableIndex;
}

const drawn = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** Set controls, then wait for the render they ask for: the first, or the
 *  first `until` accepts when they ask for more than one. */
async function rerender(
  panel: RealPanel,
  actions: Parameters<RealPanel["drive"]>[0],
  until: (m: Msg) => boolean = () => true,
): Promise<PageState> {
  const from = panel.sent.length;
  await panel.drive(actions);
  const reply = await panel.waitSent((m) => drawn(m) && (m.type === "gridError" || until(m)), from, "the render");
  assert.strictEqual(reply.type, "gridReady", reply.error);
  return panel.drive([]);
}

/** A render `width` × `height` pixels: the plane the axes asked for. */
const sized = (width: number, height: number) => (m: Msg) => m.width === width && m.height === height;

/** Tick Line plot, click the middle of the image, and read the drawn line. */
async function plotted(panel: RealPanel, axis?: number): Promise<PageState> {
  await panel.drive([{ id: "line-toggle", checked: true }]);
  let from = panel.sent.length;
  await panel.probe();
  await panel.waitSent(isType("lineResult"), from, "the line");
  if (axis !== undefined) {
    from = panel.sent.length;
    await panel.drive([{ id: "line-axis", value: axis }]);
    await panel.waitSent(isType("lineResult"), from, "the line along the chosen axis");
  }
  return panel.drive([]);
}

const overlaps = (a: Box, b: Box) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;

/** Fail if any two labels overlap. */
function assertApart(labels: AxisLabel[], what: string): void {
  for (let i = 0; i < labels.length; i++) {
    for (let j = i + 1; j < labels.length; j++) {
      assert.ok(
        !overlaps(labels[i].box, labels[j].box),
        `${what}: "${labels[i].text}" ${JSON.stringify(labels[i].box)} overlaps "${labels[j].text}" ${JSON.stringify(labels[j].box)}`,
      );
    }
  }
}

/** Fail unless every label lies within `area`, give or take half a pixel. */
function assertWithin(labels: AxisLabel[], area: { left?: number; right?: number; top?: number; bottom?: number }, what: string): void {
  for (const l of labels) {
    const b = l.box;
    const ok = (area.left === undefined || b.left >= area.left - 0.5) &&
      (area.right === undefined || b.right <= area.right + 0.5) &&
      (area.top === undefined || b.top >= area.top - 0.5) &&
      (area.bottom === undefined || b.bottom <= area.bottom + 0.5);
    assert.ok(ok, `${what}: "${l.text}" ${JSON.stringify(b)} leaves ${JSON.stringify(area)}`);
  }
}

function pointAt(state: PageState, index: number): { x: number; y: number } {
  const p = state.linePlot?.plot.points.find((q) => q.index === index);
  assert.ok(p, `the plot draws point ${index}`);
  return p;
}

suite("Line-plot and cross-section axes (#1043)", function () {
  this.timeout(120_000);

  test("a pressure profile stands upright, surface at the bottom, on log pressure, ticked in millibar", async () => {
    const { panel, close } = await storePanel(profileStore);
    try {
      const state = await plotted(panel);
      const plot = state.linePlot?.plot;
      assert.ok(plot, "the plot is drawn");
      assert.strictEqual(state.linePlot?.label, "air at this point, across level");
      assert.strictEqual(plot.profile, true, "a Z axis is drawn as a profile");
      const surface = pointAt(state, 0);
      const top = pointAt(state, LEVELS.length - 1);
      assert.ok(surface.y > top.y + 50, `1000 mb (y ${surface.y}) is below 10 mb (y ${top.y})`);
      // Log pressure: 100 mb is the geometric middle of 1000 and 10.
      const hundred = pointAt(state, LEVELS.indexOf(100));
      assert.ok(Math.abs(hundred.y - (surface.y + top.y) / 2) < 2, `100 mb sits halfway up (y ${hundred.y})`);
      // Value across: the surface is the warmest, so the furthest right.
      const coldest = pointAt(state, AIR.indexOf(-72));
      assert.ok(surface.x > coldest.x, "warm to the right of cold");

      const yTicks = plot.y.map((l) => l.text);
      assert.ok(yTicks.includes("1000") && yTicks.includes("10"), `ticks at both ends: ${yTicks}`);
      assert.ok(yTicks.every((t) => LEVELS.includes(Number(t))), `every tick is a real level: ${yTicks}`);
      assert.ok(yTicks.length >= 4, `several levels labelled: ${yTicks}`);
      assert.ok(plot.x.length >= 3, `the values are ticked too: ${plot.x.map((l) => l.text)}`);
      assert.ok(plot.x.some((l) => l.text.startsWith("−")), "a negative value is signed with a true minus");
      assertApart(plot.y.concat(plot.x), "the profile's labels");
      assertWithin(plot.y.concat(plot.x), plot.box, "the profile's labels");

      // The slice on screen is the surface: a rule across the profile there.
      assert.ok(plot.rule, "the slice on screen is marked");
      assert.ok(Math.abs(plot.rule.y1 - plot.rule.y2) < 0.5, "across, not upright");
      assert.ok(Math.abs(plot.rule.y1 - surface.y) < 1, "at the level on screen");
      assert.match(state.linePlot?.caption ?? "", /^air across level \(millibar\) · −72 to 31 degC · 17 of 17 points$/);
    } finally {
      close();
    }
  });

  test("a depth profile has the surface at the top", async () => {
    const { panel, store, close } = await storePanel(profileStore);
    try {
      await rerender(panel, [{ id: "slice-variable", value: variableIndex(store, "temp") }]);
      const state = await plotted(panel);
      assert.strictEqual(state.linePlot?.plot.profile, true);
      const surface = pointAt(state, 0);
      const deep = pointAt(state, DEPTHS.length - 1);
      assert.ok(surface.y < deep.y - 50, `0 m (y ${surface.y}) is above 1000 m (y ${deep.y})`);
    } finally {
      close();
    }
  });

  test("a time series keeps time across, with date ticks that do not collide", async () => {
    const { panel, close } = await storePanel(dailyStore);
    try {
      const state = await plotted(panel);
      const plot = state.linePlot?.plot;
      assert.ok(plot);
      assert.strictEqual(plot.profile, false, "time is not vertical");
      const dates = plot.x.map((l) => l.text);
      assert.strictEqual(dates[0], "2024-01-01");
      assert.ok(dates.includes("2024-12-31"), `the last day is labelled: ${dates}`);
      assert.ok(dates.every((d) => /^\d{4}-\d\d-\d\d$/.test(d)), `dates stay dates: ${dates}`);
      assert.ok(dates.length >= 3, `more than the ends: ${dates}`);
      assertApart(plot.x.concat(plot.y), "the time series' labels");
      assertWithin(plot.x.concat(plot.y), plot.box, "the time series' labels");
      assert.ok(plot.rule && Math.abs(plot.rule.x1 - plot.rule.x2) < 0.5, "the day on screen is an upright rule");

      // Narrow, the labels are fitted again rather than squeezed together.
      const narrow = await panel.drive([{ id: "body", width: "300px" }]);
      const n = narrow.linePlot?.plot;
      assert.ok(n);
      assert.ok(n.box.right - n.box.left < 320, "the plot narrowed");
      assert.strictEqual(n.x[0]?.text, "2024-01-01");
      assertApart(n.x.concat(n.y), "the narrow time series' labels");
      assertWithin(n.x.concat(n.y), n.box, "the narrow time series' labels");
    } finally {
      close();
    }
  });

  test("a level × latitude section has the ground at the bottom, and Flip Y axis turns it over", async () => {
    const { panel, close } = await storePanel(profileStore);
    try {
      // Y = level, X = lat.
      const from = panel.received.length;
      let state = await rerender(panel, [{ id: "slice-y", value: 0 }, { id: "slice-x", value: 1 }], sized(3, LEVELS.length));
      const asked = panel.received.slice(from).filter(isType("rerenderRequest"));
      assert.ok(asked.length > 0, "the axes are drawn");
      assert.strictEqual(asked[asked.length - 1].flipY, true, "rows stored surface first are drawn bottom-up");
      assert.ok(asked.every((m) => m.flipY === true), "never drawn the wrong way up first");
      const rails = state.rails;
      assert.ok(rails?.shown, "a cross-section has its rails");
      assert.strictEqual(rails.y[0].text, "10", "10 mb at the top");
      assert.strictEqual(rails.y[rails.y.length - 1].text, "1000", "1000 mb at the bottom");

      // The pixel near the top of the image reads the 10 mb value, and the one
      // near the bottom the surface's.
      const read = async (fy: number) => {
        const sent = panel.sent.length;
        await panel.probe(fy);
        return (await panel.waitSent(isType("probeResult"), sent, "the probe")).result;
      };
      assert.strictEqual((await read(0.02))?.value, AIR[AIR.length - 1], "the top row is 10 mb");
      assert.strictEqual((await read(0.98))?.value, AIR[0], "the bottom row is the surface");

      // Flip Y axis still turns it over.
      state = await rerender(panel, [{ id: "flip-y", checked: true }]);
      const flipped = panel.received.filter(isType("rerenderRequest")).pop();
      assert.strictEqual(flipped.flipY, false, "the box inverts the default");
      assert.strictEqual(state.rails?.y[0].text, "1000", "1000 mb at the top once flipped");
      assert.strictEqual((await read(0.02))?.value, AIR[0], "and the top row is the surface");
    } finally {
      close();
    }
  });

  test("a time × latitude section keeps every label in its own rail, at any width", async () => {
    const { panel, close } = await storePanel(dailyStore);
    try {
      // Y = lat, X = time. The panel's width is set, not left to the window,
      // so the image is the same height here and in CI: 366 × 73 drawn at
      // the width it is given, so a fifth as tall.
      await panel.drive([{ id: "body", width: "900px" }]);
      const state = await rerender(panel, [{ id: "slice-y", value: 1 }, { id: "slice-x", value: 0 }], sized(DAYS.length, 73));
      const check = (s: PageState, what: string) => {
        const rails = s.rails;
        assert.ok(rails?.shown, `${what}: the rails are shown`);
        const all = rails.y.concat(rails.x);
        assertApart(all, what);
        // Down the side of the image, never above or below it, and under it,
        // never past either edge, so clear of the colour bar and the corner.
        assertWithin(rails.y, { top: rails.image.top, bottom: rails.image.bottom, right: rails.image.left }, `${what}: y`);
        assertWithin(rails.x, { left: rails.image.left, right: rails.image.right, top: rails.image.bottom }, `${what}: x`);
        if (rails.colorbar) {
          for (const l of all) assert.ok(!overlaps(l.box, rails.colorbar), `${what}: "${l.text}" is under the colour bar`);
        }
        assert.strictEqual(rails.x[0].text, "2024-01-01", `${what}: the first day`);
        return rails;
      };
      let rails = check(state, "900 px");
      assert.strictEqual(rails.y[0].text, "90", "90 at the top");
      assert.strictEqual(rails.y[rails.y.length - 1].text, "−90", "−90 at the bottom");
      assert.strictEqual(rails.x[rails.x.length - 1].text, "2024-12-31", "the last day");

      // Narrow: the image is about 37 px tall, room for both end labels but
      // not much else (CI once drew the bottom one past it).
      rails = check(await panel.drive([{ id: "body", width: "360px" }]), "360 px");
      const tall = rails.image.bottom - rails.image.top;
      assert.ok(tall > 30 && tall < 45, `the image is ${tall} px tall`);
      assert.deepStrictEqual([rails.y[0].text, rails.y[rails.y.length - 1].text], ["90", "−90"], "both ends fit");

      // Narrower still, the image is shorter than two labels: one end is
      // dropped rather than either running past the image.
      rails = check(await panel.drive([{ id: "body", width: "300px" }]), "300 px");
      const short = rails.image.bottom - rails.image.top;
      const label = rails.y.length ? rails.y[0].box.bottom - rails.y[0].box.top : 0;
      assert.ok(short < 2 * label + 4, `the image (${short} px) is shorter than two labels (${label} px each)`);
      assert.ok(rails.y.length <= 1, `at most one latitude label: ${rails.y.map((l) => l.text)}`);
    } finally {
      close();
    }
  });

  test("a sparse time series still shows its two points, unjoined", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "netcdf4_large_sparse.nc");
    assert.ok(fs.existsSync(file), `fixture missing: ${file}`);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const t2m = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name === "t2m");
    assert.ok(t2m, "t2m is renderable");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: t2m.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await plotted(panel);
      const plot = state.linePlot?.plot;
      assert.ok(plot);
      assert.deepStrictEqual(plot.points.map((q) => q.index), [7, 119], "the two steps with data");
      assert.strictEqual((plot.path.match(/M/g) ?? []).length, 2, `two runs: ${plot.path}`);
      assert.ok(!/L/.test(plot.path), "and no line between them");
      assertApart(plot.x.concat(plot.y), "the sparse plot's labels");
    } finally {
      panel.dispose();
      editor.close();
    }
  });

  test("a click with no line, then a resize, still says so rather than drawing the line before", async () => {
    const { panel, close } = await storePanel(profileStore);
    try {
      const drawnFirst = await plotted(panel);
      assert.ok((drawnFirst.linePlot?.plot.points.length ?? 0) > 0, "a profile is drawn first");
      // The provider's answer when a cell has no line.
      const seq = panel.received.filter(isType("lineRequest")).pop()?.seq;
      await panel.post({ type: "lineResult", seq, messageIndex: 0, result: null });
      let state = await panel.drive([]);
      assert.strictEqual(state.linePlot?.caption, "No line through this cell.");
      assert.deepStrictEqual(state.linePlot?.plot.points, [], "and nothing is drawn");
      state = await panel.drive([{ id: "body", width: "500px" }]);
      assert.strictEqual(state.linePlot?.caption, "No line through this cell.", "a resize redraws what is on screen");
      assert.deepStrictEqual(state.linePlot?.plot.points, [], "not the profile before it");
    } finally {
      close();
    }
  });

  test("a page written again onto a cross-section draws, after its file is reopened or changed", async () => {
    // A page written again over a reopened or changed file (#839, #959) has
    // everything but its ready dropped until the ready arrives, so a ready
    // held for the Y axis left it blank.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1043-"));
    const file = path.join(dir, "ersst.nc");
    const fixtures = path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures");
    fs.copyFileSync(path.join(fixtures, "ersst_v5_187001_cdf1.nc"), file);
    const native = loadNative();
    assert.ok(native, "native binding required");
    const sst = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name.endsWith("sst"));
    assert.ok(sst, "sst is renderable");
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: sst.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // lev (a depth, positive down) down the rows, lat across: 89 × 1.
      await rerender(panel, [{ id: "slice-y", value: 1 }, { id: "slice-x", value: 2 }], sized(89, 1));
      for (const [what, bytes] of [
        ["reopened", null],
        ["changed", fs.readFileSync(path.join(fixtures, "ersst_v5_187001_cdf2.nc"))],
      ] as const) {
        editor.close();
        if (bytes) fs.writeFileSync(file, bytes);
        editor = await openEditor(p, file);
        const from = panel.sent.length;
        const rewritten = new Promise<void>((resolve) => panel.onNextWrite(resolve));
        // Any request after the handle changed has the page written again.
        await panel.drive([{ id: "flip-y", checked: (await panel.controls())["flip-y"] !== true }]);
        await rewritten;
        const reply = await panel.waitSent(drawn, from, `the rewritten page's render (${what})`);
        assert.strictEqual(reply.type, "gridReady", `${what}: ${reply.error ?? ""}`);
        assert.deepStrictEqual([reply.width, reply.height], [89, 1], `${what}: still the cross-section`);
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

/** The pressure-level profile store, as a classic NetCDF file: `air(level,
 *  lat, lon)` on {@link LEVELS}, stored surface first, `offset` added. */
function levelsFile(offset: number): Buffer {
  return classicNetcdf(
    [["level", LEVELS.length], ["lat", 3], ["lon", 3]],
    [
      { name: "level", dims: ["level"], attrs: { units: "millibar", positive: "down", axis: "Z" }, values: LEVELS },
      { name: "lat", dims: ["lat"], attrs: { units: "degrees_north" }, values: LAT3 },
      { name: "lon", dims: ["lon"], attrs: { units: "degrees_east" }, values: LON3 },
      {
        name: "air",
        dims: ["level", "lat", "lon"],
        attrs: { units: "degC" },
        values: AIR.flatMap((v) => each(9, v + offset)),
      },
    ],
  );
}

suite("A cross-section restored over a reopened or changed file (#1043)", function () {
  this.timeout(120_000);

  test("is painted, then turned the right way up once its Y axis arrives", async () => {
    // The rewritten page's ready cannot wait for the axis (see the test
    // above), so it paints in storage order, 1000 mb at the top, and draws
    // again with the rows turned over once the axis says so.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1043-"));
    const file = path.join(dir, "levels.nc");
    fs.writeFileSync(file, levelsFile(0));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const air = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name === "air");
    assert.ok(air, "air is renderable");
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: air.variableIndex }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      // level down the rows, lat across: 3 × 17, ground at the bottom.
      await rerender(panel, [{ id: "slice-y", value: 0 }, { id: "slice-x", value: 1 }], sized(3, LEVELS.length));
      for (const [what, offset] of [["reopened", 0], ["changed", 5]] as const) {
        editor.close();
        if (offset) fs.writeFileSync(file, levelsFile(offset));
        editor = await openEditor(p, file);
        const sent = panel.sent.length;
        const received = panel.received.length;
        const rewritten = new Promise<void>((resolve) => panel.onNextWrite(resolve));
        // A click is a request too, and leaves Flip Y axis as it is.
        await panel.probe();
        await rewritten;
        const ready = await waitUntil(() => panel.received.slice(received).find(isType("ready")), `the new page's ready (${what})`);
        assert.strictEqual(ready.flipY, false, `${what}: the ready paints in storage order`);
        const again = await waitUntil(
          () => panel.received.slice(received).find(isType("rerenderRequest")),
          `the render once the axis arrives (${what})`,
        );
        assert.strictEqual(again.flipY, true, `${what}: drawn again with the rows turned over`);
        // The last render on screen is that one.
        await waitUntil(
          () => panel.sent.slice(sent).filter((m: Msg) => m.type === "gridReady").length >= 2 || undefined,
          `both renders (${what})`,
        );
        const state = await panel.drive([]);
        const rails = state.rails;
        assert.ok(rails?.shown, `${what}: the rails are shown`);
        assert.strictEqual(rails.y[0].text, "10", `${what}: 10 mb at the top`);
        assert.strictEqual(rails.y[rails.y.length - 1].text, "1000", `${what}: 1000 mb at the bottom`);
        const from = panel.sent.length;
        await panel.probe(0.98);
        const bottom = (await panel.waitSent(isType("probeResult"), from, "the probe")).result;
        assert.strictEqual(bottom?.value, AIR[0] + offset, `${what}: the bottom row is the surface`);
      }
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

/** Open {@link levelsFile} in the editor and draw `air` as a level × latitude
 *  section: 3 × 17, the ground at the bottom. */
async function levelsSection(): Promise<{ panel: RealPanel; close: () => void }> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1114-"));
  const file = path.join(dir, "levels.nc");
  fs.writeFileSync(file, levelsFile(0));
  const native = loadNative();
  assert.ok(native, "native binding required");
  const air = native.NetcdfHandle.fromBytes(fs.readFileSync(file)).variables().find((v) => v.name === "air");
  assert.ok(air, "air is renderable");
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: air.variableIndex }));
  const close = () => {
    panel.dispose();
    editor.close();
    fs.rmSync(dir, { recursive: true, force: true });
  };
  try {
    await panel.waitSent(isType("gridReady"), 0, "the first render");
    await rerender(panel, [{ id: "slice-y", value: 0 }, { id: "slice-x", value: 1 }], sized(3, LEVELS.length));
  } catch (err) {
    close();
    throw err;
  }
  return { panel, close };
}

/** The level a Y rail label names, as an index into {@link LEVELS}. */
function levelOf(label: AxisLabel): number {
  const at = LEVELS.indexOf(Number(label.text));
  assert.ok(at >= 0, `"${label.text}" is a level`);
  return at;
}

/** Click near the top and near the bottom of the image, and require each
 *  click to read the level the rail beside it names, and its request to say
 *  the rows are the way the image on screen has them. Returns the rails. */
async function readsAsLabelled(panel: RealPanel, flipY: boolean, what: string): Promise<RailsState> {
  const state = await panel.drive([]);
  const rails = state.rails;
  assert.ok(rails?.shown, `${what}: the rails are shown`);
  const top = rails.y[0];
  const bottom = rails.y[rails.y.length - 1];
  // The ends are labelled, so a click there has a label to agree with.
  assert.deepStrictEqual(
    [top.text, bottom.text],
    flipY ? ["10", "1000"] : ["1000", "10"],
    `${what}: the rails label the image on screen`,
  );
  for (const [fy, label] of [[0.02, top], [0.98, bottom]] as const) {
    const received = panel.received.length;
    const sent = panel.sent.length;
    await panel.probe(fy);
    const asked = await waitUntil(
      () => panel.received.slice(received).find(isType("probeRequest")),
      `the click at ${fy} (${what})`,
    );
    assert.strictEqual(asked.options.flipY, flipY, `${what}: the click asks for the rows the image has`);
    const read = (await panel.waitSent(isType("probeResult"), sent, `the probe at ${fy} (${what})`)).result;
    assert.strictEqual(read?.value, AIR[levelOf(label)], `${what}: the click by "${label.text}" reads that level`);
  }
  return rails;
}

/** The last render the page drew. */
function lastDrawn(panel: RealPanel): Msg {
  const drew = panel.sent.filter(isType("gridReady"));
  assert.ok(drew.length > 0, "a render was drawn");
  return drew[drew.length - 1];
}

suite("The rails and the probe follow the image drawn, not the one on its way (#1114)", function () {
  this.timeout(120_000);

  test("a restored section, before its turned-over render arrives", async () => {
    // A restored page paints in storage order (1000 mb at the top), then asks
    // for the Y axis and draws again turned over. Until that render lands,
    // the image on screen is still the first one.
    const { panel, close } = await levelsSection();
    try {
      const gate = panel.hold((m) => m.type === "gridReady" && m.options.flipY === true);
      const received = panel.received.length;
      const sent = panel.sent.length;
      panel.rebuild();
      const ready = await waitUntil(() => panel.received.slice(received).find(isType("ready")), "the restored page");
      assert.strictEqual(ready.flipY, false, "the ready paints in storage order");
      await panel.waitSent(isType("gridReady"), sent, "the restored page's render");
      const again = await waitUntil(
        () => panel.received.slice(received).find(isType("rerenderRequest")),
        "the render once the Y axis answers",
      );
      assert.strictEqual(again.flipY, true, "the Y axis turns the rows over");
      await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
      assert.strictEqual(lastDrawn(panel).options.flipY, false, "the image on screen is in storage order");
      await readsAsLabelled(panel, false, "while the turned-over render is on its way");

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).options.flipY === true ? true : undefined), "the turned-over render");
      await readsAsLabelled(panel, true, "once it arrives");
    } finally {
      close();
    }
  });

  test("after Flip Y axis, with an axis answer landing before the render", async () => {
    // A restored page asks for its X axis and stepper axis as well. Those
    // answers are held, Flip Y axis is ticked, and one lands while the
    // render the box asked for is still on its way: it redraws the rails.
    const { panel, close } = await levelsSection();
    try {
      let toggled = false;
      const gate = panel.hold((m) => (m.type === "axisResult" && m.dim !== 0) || (toggled && m.type === "gridReady"));
      const received = panel.received.length;
      const sent = panel.sent.length;
      panel.rebuild();
      await waitUntil(() => panel.received.slice(received).find(isType("ready")), "the restored page");
      await panel.waitSent((m) => m.type === "gridReady" && m.options.flipY === true, sent, "the turned-over render");
      await waitUntil(() => (gate.held.length >= 2 ? true : undefined), "the X and stepper axes' answers, held");
      await readsAsLabelled(panel, true, "before the box is ticked");

      toggled = true;
      const asked = panel.received.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const flip = await waitUntil(
        () => panel.received.slice(asked).find(isType("rerenderRequest")),
        "the render Flip Y axis asks for",
      );
      assert.strictEqual(flip.flipY, false, "the box asks for storage order");
      await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
      // An axis answer lands first.
      const axisAt = panel.sent.length;
      await gate.deliver(isType("axisResult"));
      await panel.waitSent(isType("axisResult"), axisAt, "the axis answers");
      assert.strictEqual(lastDrawn(panel).options.flipY, true, "the image on screen is still turned over");
      await readsAsLabelled(panel, true, "after the axis answer, before the render");

      await gate.release();
      await waitUntil(() => (lastDrawn(panel).options.flipY === false ? true : undefined), "the render Flip Y asked for");
      await readsAsLabelled(panel, false, "once it arrives");
    } finally {
      close();
    }
  });

  test("a GRIB map, after picking a projection, before its render arrives", async () => {
    // A Lambert conformal grid, drawn as stored, then on Equirectangular:
    // the same point of the image is another cell of the grid. Until the
    // equirectangular render lands, a click reads the source view's cell.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1114-"));
    const file = path.join(dir, "lambert.grib2");
    fs.copyFileSync(
      path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "eta_lambert_msg0.grib2"),
      file,
    );
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(first.options.projection, "source", "drawn as stored");
      // A point near the top middle of the image, as a click there reads.
      const click = async (what: string): Promise<{ asked: Msg; read: Msg }> => {
        const received = panel.received.length;
        const sent = panel.sent.length;
        await panel.probe(0.15);
        const asked = await waitUntil(() => panel.received.slice(received).find(isType("probeRequest")), `the click (${what})`);
        const read = (await panel.waitSent(isType("probeResult"), sent, `the probe (${what})`)).result;
        return { asked, read };
      };
      const source = await click("the source view");
      assert.ok(source.read, "the click lands on the grid");

      const gate = panel.hold(isType("gridReady"));
      const received = panel.received.length;
      await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      const render = await waitUntil(
        () => panel.received.slice(received).find(isType("rerenderRequest")),
        "the render the picker asks for",
      );
      assert.strictEqual(render.projection, "equirectangular", "the render asks for the new target");
      await waitUntil(() => gate.held.find(isType("gridReady")), "that render, held back");
      const waiting = await click("while the map is on its way");
      assert.strictEqual(waiting.asked.options.projection, "source", "the click asks in the projection on screen");
      assert.deepStrictEqual(
        [waiting.read?.gridI, waiting.read?.gridJ, waiting.read?.value],
        [source.read.gridI, source.read.gridJ, source.read.value],
        "and reads the cell on screen",
      );

      await gate.release();
      await waitUntil(
        () => (lastDrawn(panel).options.projection === "equirectangular" ? true : undefined),
        "the map",
      );
      const mapped = await click("on the map");
      assert.strictEqual(mapped.asked.options.projection, "equirectangular", "the click follows the map once drawn");
      // Otherwise the check above could not tell the two projections apart.
      assert.notDeepStrictEqual(
        [mapped.read?.gridI, mapped.read?.gridJ],
        [source.read.gridI, source.read.gridJ],
        "the same point of the map is another cell",
      );
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
