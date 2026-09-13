// Wind arrows from a u/v pair (#241).
//
// The geometry and the rotation are proved in Rust
// (`crates/fieldglass/tests/vector_arrows.rs`, against a solid-rotation field on
// two targets). What is pinned here is the binding and the panel: that the
// addon projects arrows onto the rendered raster, that it reports which
// convention a file's components use, and that the Vectors row is built and
// wired — including the part that is easy to get wrong, passing the file's own
// `uvRelativeToGrid` rather than asking the user.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative, type MessageMeta } from "../../native";
import { renderImagePanelHtml, type CompareFieldOption } from "../../render-panel";

/** One arrow is one run of five vertices: tail, tip, barb, tip, barb. */
const ARROW_VERTICES = 5;

function extensionPath(): string {
  const ext = vscode.extensions.getExtension("fieldglass.fieldglass");
  assert.ok(ext, "extension is installed");
  return ext.extensionPath;
}

/** A committed GRIB1 fixture of the reader's own: CMC's 300 hPa wind, on a
 *  polar-stereographic grid that states grid-relative components. */
function cmcWind() {
  const native = loadNative();
  assert.ok(native, "native module must load");
  const p = path.join(
    extensionPath(),
    "..",
    "crates",
    "fieldglass-grib1",
    "tests",
    "fixtures",
    "cmc_wind_300_2010052400_p012.grib",
  );
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return native.Grib1Handle.fromBytes(fs.readFileSync(p));
}

const SOURCE = { projection: "source" as const, resampling: "nearest" as const, flipY: false };

suite("Vector arrows", () => {
  test("the addon reports which convention a file's components use", () => {
    const message = cmcWind().messages()[0];
    assert.strictEqual(
      message.uvRelativeToGrid,
      true,
      "this polar-stereographic wind file states components along the grid",
    );
    const native = loadNative();
    assert.ok(native);
    const latlon = native.Grib2Handle.fromBytes(
      fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2")),
    );
    assert.strictEqual(latlon.messages()[0].uvRelativeToGrid, false, "a plain lat/lon grid does not");
  });

  test("arrows come back as whole runs, and spacing thins them", () => {
    const handle = cmcWind();
    // The corpus holds one message per file, so the pair is this field against
    // itself: a flow at 45° to the grid. What matters here is the plumbing.
    const dense = handle.projectVectors(0, 0, SOURCE, 4, true);
    const sparse = handle.projectVectors(0, 0, SOURCE, 16, true);
    for (const arrows of [dense, sparse]) {
      assert.ok(arrows.segLengths.length > 0, "some arrows");
      assert.ok(
        arrows.segLengths.every((n) => n === ARROW_VERTICES),
        "every run is one whole arrow on the source projection",
      );
      assert.strictEqual(arrows.xy.length, arrows.segLengths.length * ARROW_VERTICES * 2);
    }
    assert.ok(
      dense.segLengths.length > sparse.segLengths.length * 3,
      `${dense.segLengths.length} dense vs ${sparse.segLengths.length} sparse`,
    );
    // The scale a legend puts beside its reference arrow, in the field's units:
    // the fastest cell *drawn*, so the plot fills itself at any spacing. The
    // sparse sample is a subset of the dense one here, so it cannot be faster.
    assert.ok(sparse.referenceSpeed > 0, `got ${sparse.referenceSpeed}`);
    assert.ok(
      dense.referenceSpeed >= sparse.referenceSpeed,
      `${dense.referenceSpeed} dense vs ${sparse.referenceSpeed} sparse`,
    );
  });

  test("the two conventions point differently on a projected grid", () => {
    const handle = cmcWind();
    const grid = handle.projectVectors(0, 0, SOURCE, 8, true);
    const earth = handle.projectVectors(0, 0, SOURCE, 8, false);
    assert.strictEqual(grid.segLengths.length, earth.segLengths.length);
    // Same tails, different tips: the rotation is the whole difference.
    let moved = 0;
    for (let a = 0; a < grid.segLengths.length; a++) {
      const at = a * ARROW_VERTICES * 2;
      assert.strictEqual(grid.xy[at], earth.xy[at], "same tail x");
      assert.strictEqual(grid.xy[at + 1], earth.xy[at + 1], "same tail y");
      if (Math.abs(grid.xy[at + 2] - earth.xy[at + 2]) > 0.01) moved += 1;
    }
    assert.ok(moved > grid.segLengths.length / 2, `${moved} of ${grid.segLengths.length} tips moved`);
  });

  test("a grid that cannot be placed is refused, not drawn", () => {
    const native = loadNative();
    assert.ok(native);
    // §3.20 declaring Dx = Dy = 0: it decodes, and has no extent to put an
    // arrow on.
    const p = path.join(
      extensionPath(),
      "..",
      "crates",
      "fieldglass-grib2",
      "tests",
      "fixtures",
      "polar_stereographic_surface.grib2",
    );
    const handle = native.Grib2Handle.fromBytes(fs.readFileSync(p));
    assert.throws(() => handle.projectVectors(0, 0, SOURCE, 8, false), /grid|extent|reproject|support/i);
  });

  test("the Vectors row appears with a pair to choose from, and carries the file's convention", () => {
    const native = loadNative();
    assert.ok(native);
    const meta = (uvRelativeToGrid: boolean | undefined) =>
      ({ gridType: "polar_stereographic", reprojectable: true, uvRelativeToGrid }) as unknown as MessageMeta;
    const fields: CompareFieldOption[] = [
      { index: 0, label: "#0 · UGRD" },
      { index: 1, label: "#1 · VGRD" },
    ];
    const html = (m: MessageMeta, f: CompareFieldOption[]) =>
      renderImagePanelHtml(
        { cspSource: "" } as unknown as vscode.Webview,
        m,
        "summary",
        native.colormaps(),
        native.combineOps(),
        undefined,
        f,
      );

    const paired = html(meta(true), fields);
    for (const id of [
      "vector-fieldset",
      "overlay-vectors",
      "vector-field-v",
      "vector-spacing",
      "color-vectors",
      "vector-scale",
    ]) {
      assert.ok(paired.includes(`id="${id}"`), `the Vectors row has #${id}`);
    }
    assert.ok(/const UV_GRID_RELATIVE = true;/.test(paired), "the file's convention reaches the script");
    assert.match(paired, /rotated to true north/, "and is explained in the row");
    for (const wiring of [
      /type: 'vectorRequest'/,
      /gridRelative: UV_GRID_RELATIVE/,
      /else if \(msg\.type === 'vectorResult'\) handleVectorResult\(msg\);/,
      /strokeRuns\(lastVectors\.xy \|\| \[\], lastVectors\.segLengths \|\| \[\]\);/,
      /lastVectors && lastVectors\.referenceSpeed/,
    ]) {
      assert.ok(wiring.test(paired), `the panel script wires ${wiring}`);
    }

    // Earth-relative: the row is there, the note is not.
    const plain = html(meta(false), fields);
    assert.ok(plain.includes('id="vector-fieldset"'));
    assert.ok(/const UV_GRID_RELATIVE = false;/.test(plain));
    assert.doesNotMatch(plain, /rotated to true north/);

    // One message is no pair, so there is nothing to offer.
    assert.ok(!html(meta(false), [fields[0]]).includes('id="vector-fieldset"'));
    // And a NetCDF slice panel has no message pair either.
    const slicePanel = renderImagePanelHtml(
      { cspSource: "" } as unknown as vscode.Webview,
      meta(false),
      "summary",
      native.colormaps(),
      native.combineOps(),
      { variables: [], initial: { variableIndex: 0, yDim: 0, xDim: 1, sliceIndices: [0, 0] } },
    );
    assert.ok(!slicePanel.includes('id="vector-fieldset"'), "vectors are a GRIB pair for now");
  });
});
