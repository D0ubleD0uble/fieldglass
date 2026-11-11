// A click on a space view in Source projection names the cell's position
// (#1676).
//
// On a geostationary grid drawn in its own projection, a click read
// `279.08 K · grid 12,12`, with no latitude and longitude; after switching to
// Equirectangular the same cell read `38.48°N, 74.99°W`. The probe's source
// view now reads the space view's forward map, and a pixel whose line of sight
// misses the Earth says it is off the Earth.
//
// The position oracle is PROJ 9.4.0's `+proj=geos` inverse of cell (12, 12)
// of the committed GOES-16 window: 38.479027669 N, -74.988007277
// (`crates/fieldglass/tests/space_view_probe.rs` has the command).
//
// Driven through the real panel and the real addon.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { classicNetcdf, extensionPath, isType, openEditor, openReal, provider, type RealPanel } from "./real-panel";

function crateFixture(crate: string, name: string): string {
  return path.join(extensionPath(), "..", "crates", crate, "tests", "fixtures", name);
}

/** A NetCDF variable's index in `file`, by name. */
function variableIndex(file: string, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(fs.readFileSync(file))
    .variables()
    .find((x) => x.name === name);
  assert.ok(v, `${path.basename(file)} has ${name}`);
  return v.index;
}

/** A full-disc GOES-East scan, `n × n` over ±0.16 rad: wider than the Earth
 *  (about ±0.151 rad), so the corners look past the limb. `Rad` is filled
 *  everywhere, so the readout's value is not what tells the corner apart. */
function fullDisc(n: number): Buffer {
  const scan = Array.from({ length: n }, (_, k) => -0.16 + (0.32 * k) / (n - 1));
  return classicNetcdf(
    [["y", n], ["x", n]],
    [
      {
        name: "goes_imager_projection",
        dims: [],
        attrs: {
          grid_mapping_name: "geostationary",
          perspective_point_height: 35786023.0,
          semi_major_axis: 6378137.0,
          semi_minor_axis: 6356752.31414,
          longitude_of_projection_origin: -75.0,
          sweep_angle_axis: "x",
        },
        values: [0],
      },
      {
        name: "x",
        dims: ["x"],
        attrs: { units: "rad", axis: "X", standard_name: "projection_x_coordinate" },
        values: scan,
      },
      {
        name: "y",
        dims: ["y"],
        attrs: { units: "rad", axis: "Y", standard_name: "projection_y_coordinate" },
        values: [...scan].reverse(),
      },
      {
        name: "Rad",
        dims: ["y", "x"],
        attrs: { units: "W m-2 sr-1 um-1", grid_mapping: "goes_imager_projection" },
        values: Array.from({ length: n * n }, (_, k) => 100 + k),
      },
    ],
  );
}

/** The readout once the page has written one that `done` accepts. */
async function readout(panel: RealPanel, done: (text: string) => boolean): Promise<string> {
  let last = "";
  for (let tries = 0; tries < 50; tries++) {
    last = (await panel.drive([])).probe;
    if (done(last)) return last;
    await panel.pageTimer(100);
  }
  throw new Error(`the readout never settled: ${JSON.stringify(last)}`);
}

/** Click the middle of cell `(i, j)` of an `ni × nj` source view, whichever
 *  way up the view draws its rows; the probe result and the readout. */
async function clickCell(
  panel: RealPanel,
  [ni, nj]: [number, number],
  [i, j]: [number, number],
  done: (text: string) => boolean,
): Promise<{ result: { gridI: number; gridJ: number; lat: number | null; lon: number | null; offEarth: boolean }; text: string }> {
  for (const row of [j, nj - 1 - j]) {
    const from = panel.sent.length;
    await panel.probe((row + 0.5) / nj, (i + 0.5) / ni);
    const result = (await panel.waitSent(isType("probeResult"), from, "the probe")).result;
    assert.ok(result, "the click lands on the raster");
    if (result.gridI === i && result.gridJ === j) return { result, text: await readout(panel, done) };
  }
  throw new Error(`no click reached cell (${i}, ${j})`);
}

suite("A space view's Source readout names the cell's position (#1676)", function () {
  this.timeout(120_000);

  async function onSlice(file: string, variable: string, body: (panel: RealPanel) => Promise<void>): Promise<void> {
    const p = await provider();
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: variableIndex(file, variable) }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(first.options.projection, "source", "the panel opens in Source projection");
      await body(panel);
    } finally {
      panel.dispose();
      editor.close();
    }
  }

  test("GOES-16 cell 12,12 reads where PROJ places it", async () => {
    await onSlice(crateFixture("fieldglass-netcdf", "goes16_abi_cmip.nc"), "CMI", async (panel) => {
      const { result, text } = await clickCell(panel, [24, 24], [12, 12], (t) => t.includes("grid 12,12"));
      assert.ok(result.lat != null && result.lon != null, `a position: ${JSON.stringify(result)}`);
      assert.ok(Math.abs(result.lat - 38.479027669) < 1e-6, `lat ${result.lat}`);
      assert.ok(Math.abs(result.lon - -74.988007277) < 1e-6, `lon ${result.lon}`);
      assert.strictEqual(result.offEarth, false);
      assert.match(text, /^38\.48°N, 74\.99°W · 279\.\d+ K · grid 12,12$/, text);

      // Off the diagonal, so a map that swapped i and j would read wrong: the
      // fixture's own oracle (an independent NumPy transcription of the GOES-R
      // PUG fixed grid) for cells (12, 0) and (0, 12).
      for (const [i, j, lat, lon, shown] of [
        [12, 0, 38.79754080998238, -74.98794606430779, "38.80°N, 74.99°W"],
        [0, 12, 38.47910486143625, -75.27583478074777, "38.48°N, 75.28°W"],
      ] as const) {
        const cell = await clickCell(panel, [24, 24], [i, j], (t) => t.includes(`grid ${i},${j}`));
        assert.ok(cell.result.lat != null && Math.abs(cell.result.lat - lat) < 1e-6, `(${i}, ${j}) lat ${cell.result.lat}`);
        assert.ok(cell.result.lon != null && Math.abs(cell.result.lon - lon) < 1e-6, `(${i}, ${j}) lon ${cell.result.lon}`);
        assert.ok(cell.text.startsWith(shown + " · "), cell.text);
      }
    });
  });

  test("a full-disc corner says it is off the Earth, and the middle has a position", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1676-"));
    const file = path.join(dir, "full_disc.nc");
    fs.writeFileSync(file, fullDisc(9));
    try {
      await onSlice(file, "Rad", async (panel) => {
        const corner = await clickCell(panel, [9, 9], [0, 0], (t) => t.includes("grid 0,0"));
        assert.strictEqual(corner.result.offEarth, true, JSON.stringify(corner.result));
        assert.strictEqual(corner.result.lat, null);
        assert.match(corner.text, /^off the Earth · .+ · grid 0,0$/, corner.text);

        const middle = await clickCell(panel, [9, 9], [4, 4], (t) => t.includes("grid 4,4"));
        assert.strictEqual(middle.result.offEarth, false);
        // The sub-satellite point: the equator, at the satellite's longitude.
        assert.ok(middle.result.lat != null && Math.abs(middle.result.lat) < 1e-9, JSON.stringify(middle.result));
        assert.ok(middle.result.lon != null && Math.abs(middle.result.lon - -75) < 1e-9, JSON.stringify(middle.result));
        assert.match(middle.text, /^0\.00°[NS], 75\.00°W · /, middle.text);
      });
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
