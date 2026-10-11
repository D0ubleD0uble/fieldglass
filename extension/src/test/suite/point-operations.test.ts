// The render panel offers contours, arrows, the long CSV and the zonal
// average only for a grid that takes them (#945).
//
// Which grids those are is Rust's answer (`GridSupport`, on `MessageInfo` and
// on a slice's `SliceGrid`), not a list kept here. A full-disc satellite view is
// placed on the Earth and takes coastlines, but its corner pixels look past
// the limb and have no position, so its contours were offered and then refused
// with a `contourError`.
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`, the way #840's tests do.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type GridSupport, type SlicePanelHandle } from "../../native";
import { NO_CONTOURS_NOTE, NO_LONG_CSV_NOTE, NO_ZONAL_NOTE } from "../../render-panel";
import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const NONE: GridSupport = { contours: false, vectors: false, longCsv: false, zonalMean: false };

function crateFixture(crate: string, name: string): string {
  return path.join(extensionPath(), "..", "crates", crate, "tests", "fixtures", name);
}

/** A NetCDF variable's index in `file`, by name. */
function variableIndex(file: string, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
    .variables()
    .find((x) => x.name === name);
  assert.ok(v, `${path.basename(file)} has ${name}`);
  return v.variableIndex;
}

suite("Point operations follow the grid (#945)", function () {
  this.timeout(120_000);

  test("a space view slice disables Contours and the zonal average, with the reason, and posts no contourError", async () => {
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
      assert.deepStrictEqual(first.sliceGrid.supports, NONE, "and takes no point operation");

      let state = await panel.drive([]);
      assert.strictEqual(state.overlayDisabled, false, "the Overlay row stays offered");
      assert.strictEqual(state.contoursDisabled, true, "Contours is disabled");
      assert.strictEqual(state.contourNote, NO_CONTOURS_NOTE, "with the reason");
      assert.strictEqual(state.zonalDisabled, true, "Zonal average is disabled");
      assert.strictEqual(state.zonalNote, NO_ZONAL_NOTE, "with the reason");

      // On a map, with coastlines, and with the Contours box set the way the
      // panel used to let a user set it: the coastlines are drawn, and no
      // contour is asked for or refused.
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
      assert.strictEqual(state.contoursDisabled, true);
      assert.strictEqual(state.contourNote, NO_CONTOURS_NOTE);
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
          const handle = file.endsWith(".grib2") ? native.Grib2Handle.fromBytes(bytes) : native.Grib1Handle.fromBytes(bytes);
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
            const v = native.NetcdfHandle.fromBytes(fs.readFileSync(file))
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
        assert.strictEqual(state.contoursDisabled, false, "Contours is offered");
        assert.strictEqual(state.contourNote, null, "with no note");
        assert.strictEqual(state.zonalDisabled, !supports.zonalMean, "Zonal average follows Rust's answer");
        assert.strictEqual(state.zonalNote, supports.zonalMean ? null : NO_ZONAL_NOTE);

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

  test("the CSV layout prompt offers Long only where the grid places every point", async () => {
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
        const handle = native.NetcdfHandle.fromBytes(fs.readFileSync(file));
        const v = handle.variables().find((x) => x.name === name);
        assert.ok(v, `${path.basename(file)} has ${name}`);
        const rank = v.dims.length;
        await p.handleExportSliceCsv(
          {
            handle: (): SlicePanelHandle => handle,
            gone: "",
            exportDir: vscode.Uri.file(path.dirname(file)),
            container: "NetCDF",
            noun: "file",
          },
          {
            variableIndex: v.variableIndex,
            yDim: v.detectedYDim ?? rank - 2,
            xDim: v.detectedXDim ?? rank - 1,
            sliceIndices: new Array(rank).fill(0),
          },
        );
      };
      await slice(crateFixture("fieldglass-netcdf", "goes_geostationary.nc"), "Rad");
      await slice(crateFixture("fieldglass-netcdf", "netcdf4_dimscale.nc"), "temperature");

      // A GRIB message, through the editor's own export message.
      const editor = await openEditor(p, crateFixture("fieldglass-grib2", "regular_latlon_surface.grib2"));
      editor.send({ type: "exportCsv", messageIndex: 0 });
      await new Promise((r) => setTimeout(r, 200));
      editor.close();
    } finally {
      vscode.window.showQuickPick = orig;
    }
    assert.deepStrictEqual(offered, [["matrix"], ["long", "matrix"], ["long", "matrix"]]);
    assert.strictEqual(placeholders[0], `CSV layout. ${NO_LONG_CSV_NOTE}`, "the prompt says why");
    assert.strictEqual(placeholders[1], "CSV layout");
  });
});
