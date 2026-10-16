// The reason the panel gives for a field with only the source view comes from
// the placement Rust reports (#960).
//
// A grid of a family this build reprojects can still be declined, because its
// own numbers place nothing: a GRIB2 Lambert grid on an Earth of radius zero is
// `unplaceable` under the label `lambert` (#844). The note used to key on the
// label alone and say "Reprojection isn't available for lambert grids yet.",
// beside an Overlay note saying the grid is not placed. A NetCDF slice with no
// coordinates read "... for source grids yet." Only a template this build does
// not model (`unsupported`) is a family that is not supported yet.
//
// These run the panel's own script in a real webview, through the driver in
// `real-panel.ts`, so they read the reason the page shows in its "Not available"
// line (#1036): the one written into the panel's HTML and the one a slice render sends.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { type UnavailableGroup, UNPLACED_REASON } from "../../render-panel";
import { extensionPath, isType, openEditor, openReal, provider, reasonFor } from "./real-panel";

/** Says the grid is not placed, and names no family as unsupported. */
function assertSaysNotPlaced(reason: string | null, what: string): void {
  assert.ok(reason, `${what}: the reason is given`);
  assert.match(reason, /placed on the Earth/, `${what}: the reason says the grid is not placed, got "${reason}"`);
  assert.doesNotMatch(reason, /not yet|grids/, `${what}: the reason calls no family unsupported, got "${reason}"`);
  assert.strictEqual(reason, UNPLACED_REASON, what);
}

/** The reason a render's answer gives for hiding the projection picker. */
function projectionReasonIn(groups: UnavailableGroup[]): string | null {
  const group = groups.find((g) => g.controls.some((c) => c.key === "projection"));
  return group ? group.reason : null;
}

/** The committed Lambert fixture on a Code Table 3.2 shape-1 Earth of radius
 *  zero: the `shapeless_earths()` edit in
 *  `crates/fieldglass/tests/non_finite_geometry.rs`. */
function zeroRadiusLambert(): Buffer {
  const bytes = fs.readFileSync(
    path.join(extensionPath(), "..", "crates", "fieldglass-grib2", "tests", "fixtures", "eta_lambert_msg0.grib2"),
  );
  // Walk the sections after §0 (16 octets) to §3; its template payload starts
  // 14 octets in.
  let at = 16;
  while (bytes.readUInt8(at + 4) !== 3) at += bytes.readUInt32BE(at);
  const payload = at + 14;
  const earth = Buffer.alloc(16, 0xff);
  earth[0] = 1; // shape 1: a sphere of the radius stated next
  earth[1] = 0; // scale factor
  earth.writeUInt32BE(0, 2); // radius 0
  earth.copy(bytes, payload);
  return bytes;
}

suite("The projection note follows placement (#960)", function () {
  this.timeout(60_000);

  test("a GRIB Lambert grid on a zero-radius Earth is called not placed, not unsupported", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-960-"));
    const file = path.join(dir, "lambert_radius_zero.grib2");
    fs.writeFileSync(file, zeroRadiusLambert());
    const native = loadNative();
    assert.ok(native, "native binding required");
    const meta = native.Grib2Handle.fromBytes(fs.readFileSync(file)).message(0);
    assert.strictEqual(meta.grid?.label, "lambert", "declined under its family's name");
    assert.strictEqual(meta.placement, "unplaceable");
    assert.strictEqual(meta.reprojectable, false);

    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.projectionOffers, ["source"]);
      assertSaysNotPlaced(reasonFor(state, "Projection"), "zero-radius Lambert");
      // The two notes now agree.
      assert.strictEqual(reasonFor(state, "Overlay"), UNPLACED_REASON);
    } finally {
      panel.dispose();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF slice with no coordinates is called not placed, opened on it or moved onto it", async () => {
    const p = await provider();
    const file = path.join(extensionPath(), "src", "test", "fixtures", "netcdf4_dimscale.nc");
    const native = loadNative();
    assert.ok(native, "native binding required");
    const handle = native.NetcdfHandle.fromBytes(fs.readFileSync(file));
    const index = (name: string) => {
      const v = handle.variables().find((x) => x.name === name);
      assert.ok(v, `the fixture has ${name}`);
      return v.variableIndex;
    };
    const temperature = index("temperature");
    const latBnds = index("lat_bnds");

    // Opened on lat_bnds: the note written into the panel's HTML.
    const editor = await openEditor(p, file);
    let panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: latBnds }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.projectionOffers, ["source"]);
      assertSaysNotPlaced(reasonFor(state, "Projection"), "lat_bnds, opened on");
    } finally {
      panel.dispose();
    }

    // Opened on temperature and moved onto lat_bnds: the note the render sends.
    panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.strictEqual(reasonFor(state, "Projection"), null, "a placed latlon slice has no reason");
      const from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: latBnds }]);
      const drawn = await panel.waitSent(
        (m) => m.type === "gridReady" && m.messageIndex === latBnds,
        from,
        "lat_bnds drawn",
      );
      assertSaysNotPlaced(projectionReasonIn(drawn.sliceGrid.unavailable), "lat_bnds, the render's answer");
      state = await panel.drive([]);
      assertSaysNotPlaced(reasonFor(state, "Projection"), "lat_bnds, moved onto");
      assert.strictEqual(reasonFor(state, "Overlay"), UNPLACED_REASON);
    } finally {
      panel.dispose();
    }
  });
});
