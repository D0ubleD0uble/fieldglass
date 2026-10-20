// The render panel offers contours, arrows, the long CSV and the zonal
// average only for a grid that takes them (#945).
//
// Which grids those are is Rust's answer (`GridSupport`, on `MessageInfo` and
// on a slice's `SliceGrid`), not a list kept here. A satellite view, a full disc
// or a sector wholly on the Earth, is placed and takes coastlines, but the
// library has no point map for the family yet, so its contours were offered
// and then refused with a `contourError`. The reasons say that, not that some
// of its points have no position (#979). A control a grid does not take is
// hidden, and the panel's "Not available for this grid" line says why (#1036).
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`, the way #840's tests do.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type GridSupport, type Handle } from "../../native";
import {
  NO_LONG_CSV_NOTE,
  NO_ZONAL_REASON,
  SATELLITE_NOT_YET_REASON,
  UNPLACED_LONG_CSV_NOTE,
  UNPLACED_REASON,
} from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  reasonFor,
  waitUntil,
} from "./real-panel";

const NONE: GridSupport = { contours: false, vectors: false, longCsv: false, zonalMean: false, bilinear: false };

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

/** The committed §3.20 polar stereographic fixture, edited to state an Earth
 *  of `radiusM` metres (shape of the Earth 1, radius given by the producer) and
 *  a 100 km Dx and Dy. The same edit `crates/fieldglass/tests/placement.rs`
 *  makes (#1002). */
function polarStereoOnAnEarthOf(radiusM: number): Buffer {
  const bytes = Buffer.from(fs.readFileSync(crateFixture("fieldglass-grib2", "polar_stereographic_surface.grib2")));
  let at = 16; // past §0
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    const len = bytes.readUInt32BE(at);
    if (bytes[at + 4] === 3) {
      assert.strictEqual(bytes.readUInt16BE(at + 12), 20, "a §3.20 grid");
      bytes[at + 14] = 1; // octet 15: shape of the Earth
      bytes[at + 15] = 0; // octet 16: scale factor of the radius
      bytes.writeUInt32BE(radiusM, at + 16); // octets 17-20
      bytes.writeUInt32BE(100_000_000, at + 55); // octets 56-59: Dx, 10^-3 m
      bytes.writeUInt32BE(100_000_000, at + 59); // octets 60-63: Dy
      return bytes;
    }
    at += len;
  }
  throw new Error("no §3 in the fixture");
}

suite("Point operations follow the grid (#945)", function () {
  this.timeout(120_000);

  test("a space view slice hides Contours and the zonal average, with the reason, and posts no contourError", async () => {
    const p = await provider();
    const file = crateFixture("fieldglass-netcdf", "goes_geostationary.nc");
    const rad = variableIndex(file, "Rad");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: rad }));
    try {
      const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
      // Placed, so the map and the coastlines are offered; no point operation.
      assert.strictEqual(first.sliceGrid.placed, true, "a full disc is placed");
      assert.strictEqual(first.sliceGrid.reprojectable, true, "and reprojects");
      assert.deepStrictEqual(first.sliceGrid.supports, { ...NONE, bilinear: true }, "and takes no point operation, though its warp blends");

      let state = await panel.drive([]);
      assert.strictEqual(state.controlShown.overlay, true, "the Overlay row stays offered");
      assert.strictEqual(state.controlDisplay.contours, "none", "Contours is hidden");
      assert.strictEqual(reasonFor(state, "Contours"), SATELLITE_NOT_YET_REASON, "with the reason");
      // The real reason (#979): this 6 x 6 sector is wholly on the Earth, so
      // no point of it lacks a position.
      assert.match(String(reasonFor(state, "Contours")), /not yet supported for satellite-view grids/);
      assert.doesNotMatch(String(reasonFor(state, "Contours")), /position|none/, "and no point is said to lack a position");
      assert.strictEqual(state.controlDisplay.zonal, "none", "Zonal average is hidden");
      assert.strictEqual(reasonFor(state, "Zonal average"), NO_ZONAL_REASON, "with the reason");

      // On a map, with coastlines, and with the hidden Contours box set the way
      // the panel used to let a user set it (or a restored snapshot can): the
      // coastlines are drawn, and no contour is asked for or refused.
      let from = panel.sent.length;
      state = await panel.drive([{ id: "picker-projection", value: "equirectangular" }]);
      await panel.waitSent((m) => m.type === "gridReady" && m.options.projection === "equirectangular", from, "the map");
      const before = { sent: panel.sent.length, received: panel.received.length };
      state = await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-contours", checked: true },
        { id: "zonal-toggle", checked: true },
      ]);
      await panel.waitSent(isType("overlayReady"), before.sent, "the coastlines");
      // A re-render goes through the gridReady path, which asks for contours
      // and the zonal mean again where they apply.
      from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      await panel.waitSent(isType("gridReady"), from, "a re-render");
      state = await panel.drive([]);
      assert.strictEqual(state.controlDisplay.contours, "none");
      assert.strictEqual(reasonFor(state, "Contours"), SATELLITE_NOT_YET_REASON);
      assert.deepStrictEqual(
        panel.received
          .slice(before.received)
          .filter((m: Msg) => m && ["contourRequest", "zonalRequest"].includes(m.type))
          .map((m: Msg) => m.type),
        [],
        "no contour or zonal request",
      );
      assert.deepStrictEqual(
        panel.sent
          .filter((m: Msg) => m && ["contourError", "contourReady", "zonalResult"].includes(m.type))
          .map((m: Msg) => `${m.type}: ${m.error}`),
        [],
        "and no contourError",
      );
    } finally {
      panel.dispose();
    }
  });

  // #1002. A polar stereographic grid whose declared Earth (80 km) is smaller
  // than one of its cells (100 km) but whose own plane, 2·R·k₀ ≈ 149 km, is
  // not. It used to be placed, then refused every point operation, and the
  // panel said contours aren't available "for satellite-view grids yet". It is
  // not placed, and the panel says that instead.
  test("a polar grid on an Earth smaller than one cell is not placed, and the panel says so", async () => {
    const p = await provider();
    const message = polarStereoOnAnEarthOf(80_000);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1002-"));
    // Twice, so the Vectors row is written: the message is its own u/v pair.
    const file = path.join(dir, "tiny_earth_pair.grib2");
    fs.writeFileSync(file, Buffer.concat([message, message]));
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.open(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.placement, "unplaceable", "the grid is not placed");
    assert.deepStrictEqual(meta.supports, NONE, "and takes no point operation");

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([
        { id: "overlay-coastlines", checked: true },
        { id: "overlay-contours", checked: true },
        { id: "overlay-vectors", checked: true },
      ]);
      assert.strictEqual(state.controlDisplay.overlay, "none", "the Overlay row is hidden");
      assert.strictEqual(reasonFor(state, "Overlay"), UNPLACED_REASON, "because the grid is not placed");
      assert.strictEqual(reasonFor(state, "Contours"), null, "the Overlay row's reason covers contours");
      assert.strictEqual(state.controlDisplay.arrows, "none");
      assert.strictEqual(reasonFor(state, "Vectors"), UNPLACED_REASON);
      assert.strictEqual(reasonFor(state, "Zonal average"), UNPLACED_REASON);
      for (const item of state.unavailable?.items ?? []) {
        assert.doesNotMatch(item.text, /satellite-view/, "nothing blames a satellite view");
      }
      assert.deepStrictEqual(
        panel.sent.filter((m: Msg) => m && /Error$|vectorResult|contourReady/.test(m.type)).map((m: Msg) => m.type),
        [],
        "and nothing is asked for and refused",
      );
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // Every placed family that contours today, through its own panel. The
  // answers the page shows are compared with the provider's own (the
  // `MessageInfo` or the render's `sliceGrid`), and contours are asked for and
  // drawn on each.
  const placedFamilies: Array<{ family: string; file: string; variable?: string }> = [
    { family: "latlon (GRIB2)", file: crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2") },
    { family: "rotated latlon", file: crateFixture("fieldglass-grib2", "rotated_latlon_surface.grib2") },
    { family: "regular Gaussian", file: crateFixture("fieldglass-grib2", "regular_gaussian_f32.grib2") },
    { family: "reduced Gaussian", file: crateFixture("fieldglass-grib2", "reduced_gaussian_pressure_level.grib2") },
    { family: "Lambert conformal", file: crateFixture("fieldglass-grib2", "eta_lambert_msg0.grib2") },
    { family: "transverse Mercator", file: crateFixture("fieldglass-grib2", "transverse_mercator_ukv.grib2") },
    { family: "Lambert azimuthal", file: crateFixture("fieldglass-grib2", "lambert_azimuthal_efas.grib2") },
    { family: "spectral, on lat/lon", file: crateFixture("fieldglass-grib2", "spectral_simple_t63.grib2") },
    { family: "HEALPix, on lat/lon", file: crateFixture("fieldglass-grib2", "healpix_n4_ring.grib2") },
    { family: "polar stereographic (GRIB1)", file: crateFixture("fieldglass-grib1", "cmc_wind_300_2010052400_p012.grib") },
    { family: "latlon (NetCDF)", file: crateFixture("fieldglass-netcdf", "netcdf4_dimscale.nc"), variable: "temperature" },
    { family: "Mercator (WRF)", file: crateFixture("fieldglass-netcdf", "wrf_mercator.nc"), variable: "T2" },
    { family: "Lambert (WRF)", file: crateFixture("fieldglass-netcdf", "wrf_lambert.nc"), variable: "T2" },
    { family: "polar stereographic (WRF)", file: crateFixture("fieldglass-netcdf", "wrf_polar.nc"), variable: "T2" },
    { family: "curvilinear", file: crateFixture("fieldglass-netcdf", "mirs_swath_n21.nc"), variable: "TPW" },
  ];

  for (const { family, file, variable } of placedFamilies) {
    test(`a ${family} panel keeps Contours offered and draws them`, async () => {
      const p = await provider();
      const editor = await openEditor(p, file);
      const open =
        variable === undefined
          ? { type: "decodeGrid", messageIndex: 0 }
          : { type: "renderVariable", variableIndex: variableIndex(file, variable) };
      const panel = await openReal(p, () => editor.send(open));
      try {
        const first = await panel.waitSent(isType("gridReady"), 0, "the first render");
        let supports: GridSupport;
        if (variable === undefined) {
          const native = loadNative();
          assert.ok(native, "native binding required");
          const bytes = fs.readFileSync(file);
          const handle = native.open(bytes);
          const meta = handle.message(0);
          assert.strictEqual(meta.placement, "placed", `${family} is placed`);
          supports = meta.supports;
        } else {
          // A WRF variable states no CF axes, so the panel opens on its first
          // two dimensions; the map is the last two, as a user picks them.
          let grid = first.sliceGrid;
          if (!grid.placed) {
            const native = loadNative();
            assert.ok(native, "native binding required");
            const v = native.open(fs.readFileSync(file))
              .variables()
              .find((x) => x.name === variable);
            assert.ok(v);
            const rank = v.dims.length;
            const from = panel.sent.length;
            await panel.drive([{ id: "slice-x", value: rank - 1 }]);
            await panel.drive([{ id: "slice-y", value: rank - 2 }]);
            const map = await panel.waitSent(
              (m: Msg) => m.type === "gridReady" && m.sliceGrid && m.sliceGrid.placed,
              from,
              `${family} on its map axes`,
            );
            grid = map.sliceGrid;
          }
          assert.strictEqual(grid.placed, true, `${family} is placed`);
          supports = grid.supports;
        }
        assert.strictEqual(supports.contours, true, `${family} contours today`);

        let state = await panel.drive([]);
        assert.strictEqual(state.controlShown.contours, true, "Contours is offered");
        assert.strictEqual(reasonFor(state, "Contours"), null, "with no reason against");
        assert.strictEqual(state.controlShown.zonal, supports.zonalMean, "Zonal average follows Rust's answer");
        assert.strictEqual(reasonFor(state, "Zonal average"), supports.zonalMean ? null : NO_ZONAL_REASON);

        const from = panel.sent.length;
        state = await panel.drive([{ id: "overlay-contours", checked: true }]);
        await panel.waitSent(isType("contourReady"), from, `${family}'s contours`);
        assert.deepStrictEqual(
          panel.sent.filter(isType("contourError")).map((m: Msg) => m.error),
          [],
          "and no contourError",
        );
      } finally {
        panel.dispose();
      }
    });
  }

  test("an unplaced slice says it is not placed, not that its rows or points are wrong", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const temperature = variableIndex(file, "temperature");
    const latBnds = variableIndex(file, "lat_bnds");
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.strictEqual(state.controlShown.zonal, true, "a lat/lon slice takes a zonal average");
      assert.strictEqual(reasonFor(state, "Zonal average"), null);

      // Onto lat_bnds, which nothing places: the toggle is hidden and the
      // reason is the placement, as the Overlay row's is.
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      await panel.waitSent((m: Msg) => m.type === "gridReady" && m.messageIndex === latBnds, from, "lat_bnds drawn");
      state = await panel.drive([]);
      assert.strictEqual(reasonFor(state, "Overlay"), UNPLACED_REASON);
      assert.strictEqual(state.controlDisplay.zonal, "none");
      assert.strictEqual(reasonFor(state, "Zonal average"), UNPLACED_REASON);
      assert.strictEqual(reasonFor(state, "Contours"), null, "the Overlay row's reason covers contours");
    } finally {
      panel.dispose();
    }

    // The same panel written directly for lat_bnds says the same.
    const direct = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: latBnds }));
    try {
      await direct.waitSent(isType("gridReady"), 0, "lat_bnds drawn");
      const state = await direct.drive([]);
      assert.strictEqual(reasonFor(state, "Zonal average"), UNPLACED_REASON);
    } finally {
      direct.dispose();
    }
  });

  test("the CSV layout prompt offers Long only where the grid's family places its points", async () => {
    const p = await provider();
    const native = loadNative();
    assert.ok(native, "native binding required");
    const offered: string[][] = [];
    const placeholders: string[] = [];
    const orig = vscode.window.showQuickPick;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (vscode.window as any).showQuickPick = async (items: Array<{ format: string }>, options: { placeHolder: string }) => {
      offered.push(items.map((i) => i.format));
      placeholders.push(options.placeHolder);
      return undefined; // cancelled: nothing is written
    };
    try {
      const slice = async (file: string, name: string) => {
        const handle = native.open(fs.readFileSync(file));
        const v = handle.variables().find((x) => x.name === name);
        assert.ok(v, `${path.basename(file)} has ${name}`);
        const rank = v.dims.length;
        await p.handleExportSliceCsv(
          {
            handle: (): Handle => handle,
            gone: "",
            exportDir: vscode.Uri.file(path.dirname(file)),
            container: "NetCDF",
            noun: "file",
            source: path.basename(file),
          },
          {
            variableIndex: v.index,
            yDim: v.detectedYDim ?? rank - 2,
            xDim: v.detectedXDim ?? rank - 1,
            sliceIndices: new Array(rank).fill(0),
          },
        );
      };
      await slice(crateFixture("fieldglass-netcdf", "goes_geostationary.nc"), "Rad");
      await slice(crateFixture("fieldglass-netcdf", "netcdf4_dimscale.nc"), "temperature");
      await slice(crateFixture("fieldglass-netcdf", "netcdf4_dimscale.nc"), "lat_bnds");

      // A GRIB message, through the editor's own export message, which does
      // not wait for the prompt: poll for it.
      const editor = await openEditor(p, crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
      editor.send({ type: "exportCsv", messageIndex: 0 });
      await waitUntil(() => (offered.length === 4 ? true : undefined), "the GRIB export's layout prompt");
      editor.close();
    } finally {
      vscode.window.showQuickPick = orig;
    }
    assert.deepStrictEqual(offered, [["matrix"], ["long", "matrix"], ["matrix"], ["long", "matrix"]]);
    assert.strictEqual(placeholders[0], `CSV layout. ${NO_LONG_CSV_NOTE}`, "a placed space view: not supported yet");
    assert.match(NO_LONG_CSV_NOTE, /satellite-view grids yet/);
    assert.doesNotMatch(NO_LONG_CSV_NOTE, /position|none/);
    assert.strictEqual(placeholders[1], "CSV layout");
    assert.strictEqual(placeholders[2], `CSV layout. ${UNPLACED_LONG_CSV_NOTE}`, "an unplaced slice: not placed");
    assert.strictEqual(placeholders[3], "CSV layout");
  });
});
