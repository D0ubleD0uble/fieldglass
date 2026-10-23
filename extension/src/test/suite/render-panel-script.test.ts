// The render panel's own script, run in a real webview through the driver in
// `real-panel.ts`.
//
// - A slice render that fails still moves the projection picker and the Overlay
//   row onto the slice the user picked (#839). They used to follow `gridReady`
//   alone, so an error left them on the previous slice's answer.
// - A slice panel whose file changed under it takes the new file's variables,
//   and finds the one it was showing by name (#839). It used to keep the list
//   it opened with, so a variable number the new file reuses drew another
//   variable under the old name.
// - What a page the panel has written over sends is not served, its `ready`
//   included (#1202). The provider took the first `ready` after a rewrite as
//   the new page's, so a ready the old page posted late drew the variable at
//   its old number in the new file.
// - A panel never names a field it is not showing (#1388). A variable that
//   fails to draw is the one the heading and tab name, over no picture; a
//   panel whose file cannot be read says nothing is drawn in its tab too, and
//   keeps Compare away through a pick; a file left with no variable shows
//   none of the old axes, steppers or Animate row.
// - Every helper the script injects with `.toString()` reaches the page, shown
//   by using it (#841). The script parse check cannot see an unbound name, so a
//   dropped injection parses fine and throws only when it is called.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import { FILE_UNREADABLE, PanelHandleGate } from "../../provider";
import { NO_FIELD_HEADING, NO_VARIABLE_HEADING, UNPLACED_REASON } from "../../render-panel";
import {
  classicNetcdf,
  type Editor,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  overlayTraffic,
  provider,
  type RealPanel,
  reasonFor,
  refusals,
} from "./real-panel";

function fixture(name: string): string {
  return path.join(extensionPath(), "src", "test", "fixtures", name);
}

function netcdfFixture(name: string): string {
  return path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", name);
}

/** The variable index `name` has in the NetCDF file `bytes`. */
function indexOf(bytes: Buffer, name: string): number {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const v = native.open(bytes).variables().find((x) => x.name === name);
  assert.ok(v, `the file has ${name}`);
  return v.index;
}

/** A copy of a NetCDF file in a temporary folder, opened in the editor with a
 *  slice panel on `name`, which can then change on disk under the panel. */
async function changingFile(
  first: Buffer,
  name: string,
): Promise<{
  panel: RealPanel;
  change(bytes: Buffer): Promise<void>;
  changeNow(bytes: Buffer): void;
  close(): void;
  reopen(): Promise<void>;
  done(): void;
}> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
  const file = path.join(dir, "changing.nc");
  fs.writeFileSync(file, first);
  let editor: Editor = await openEditor(p, file);
  const variableIndex = indexOf(first, name);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex }));
  return {
    panel,
    // The editor closes, which drops its handle, the file changes, and the
    // editor opens it again.
    change: async (bytes) => {
      editor.close();
      fs.writeFileSync(file, bytes);
      editor = await openEditor(p, file);
    },
    // The same change, done at once: the file changes and the document's
    // handle is replaced, as opening it again replaces it, without waiting on
    // the editor. For a change that has to land at a given moment.
    changeNow: (bytes) => {
      fs.writeFileSync(file, bytes);
      const native = loadNative();
      assert.ok(native, "native binding required");
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const handles = (p as any)._netcdfHandlesByDoc as Map<string, unknown>;
      assert.ok(handles.has(vscode.Uri.file(file).toString()), "the document has a handle to replace");
      handles.set(vscode.Uri.file(file).toString(), native.open(bytes));
    },
    // The editor closes, which drops the document's handle (the panel keeps
    // the one it holds, #1338), and opens the file again.
    close: () => editor.close(),
    reopen: async () => {
      editor = await openEditor(p, file);
    },
    done: () => {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

const drawnOrRefused = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

suite("The render panel's own script", function () {
  this.timeout(60_000);

  // A slice panel outlives the editor it was opened from (#839). It holds the
  // file open (#1204) and goes on drawing from it (#1338), so a restore with a
  // map target saved, the case #839 asked about, draws on that target and
  // keeps the map targets and overlays on offer.
  test("a slice panel restored after its editor closed draws on its saved map target (#839, #1338)", async () => {
    const original = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const temperature = indexOf(original, "temperature");
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
    const file = path.join(dir, "closing.nc");
    fs.writeFileSync(file, original);
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      let state = await panel.drive([
        { id: "picker-projection", value: "equirectangular" },
        { id: "overlay-coastlines", checked: true },
      ]);
      await panel.waitSent(
        (m) => m.type === "gridReady" && m.options.projection === "equirectangular",
        from,
        "temperature on a map",
      );
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"));
      assert.strictEqual(state.controlShown.overlay, true);

      // The editor closes under the panel, and the panel is restored with
      // equirectangular saved. It draws from the file it holds.
      editor.close();
      from = panel.sent.length;
      panel.rebuild();
      const restored = await panel.waitSent(drawnOrRefused, from, "the restored render");
      assert.strictEqual(restored.type, "gridReady", `${restored.error ?? ""}`);
      assert.strictEqual(restored.options.projection, "equirectangular", "the saved map target is drawn");
      assert.strictEqual(restored.sliceGrid.placed, true);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines after the restore");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"), `got ${state.projectionOffers.join(", ")}`);
      assert.strictEqual(state.projection, "equirectangular");
      assert.strictEqual(state.controlShown.projection, true);
      assert.strictEqual(reasonFor(state, "Projection"), null);
      assert.strictEqual(state.controlShown.overlay, true);
      assert.strictEqual(reasonFor(state, "Overlay"), null);

      // The file opened again: the panel moves to its new handle and still
      // draws temperature on the map.
      editor = await openEditor(p, file);
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: temperature }]);
      const drawn = await panel.waitSent(isType("gridReady"), from, "temperature again");
      assert.strictEqual(drawn.options.projection, "equirectangular", "the saved map target survives");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // The file is read again (File: Revert File) into bytes that do not open
  // (#1338). The panel's handle is the old file's, so it is not drawn from:
  // the panel has no file, and a restore with a map target saved fails, the
  // case #839 asked about. The page then offers what a slice with no file
  // takes, and draws on its map target again once the file opens.
  test("a slice panel whose file is reverted to bytes that do not open withdraws the map targets and overlays (#839, #1338)", async () => {
    const original = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const temperature = indexOf(original, "temperature");
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-839-"));
    const file = path.join(dir, "closing.nc");
    fs.writeFileSync(file, original);
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature }));
    const revert = () => p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      let state = await panel.drive([
        { id: "picker-projection", value: "equirectangular" },
        { id: "overlay-coastlines", checked: true },
      ]);
      await panel.waitSent(
        (m) => m.type === "gridReady" && m.options.projection === "equirectangular",
        from,
        "temperature on a map",
      );
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"));
      assert.strictEqual(state.controlShown.overlay, true);

      // Reverted to bytes that are not NetCDF: the panel is written again
      // with equirectangular saved, and its first render fails.
      fs.writeFileSync(file, Buffer.from("not a NetCDF file"));
      const before = { sent: panel.sent.length, received: panel.received.length };
      await revert();
      const failed = await panel.waitSent(drawnOrRefused, before.sent, "the restored render's answer");
      assert.strictEqual(failed.type, "gridError", `drew "${failed.titleLine}" from the file before the revert`);
      assert.strictEqual(failed.error, FILE_UNREADABLE);
      // The page offers what the slice takes: source only, and no overlays.
      // Turning on another layer asks for nothing.
      state = await panel.drive([{ id: "overlay-graticule", checked: true }]);
      assert.deepStrictEqual(state.projectionOffers, ["source"], "no map targets for a slice nothing places");
      assert.strictEqual(state.projection, "source");
      assert.strictEqual(failed.handleGone, true, "the answer is about the missing file");
      assert.strictEqual(state.controlDisplay.projection, "none", "the picker is hidden");
      assert.strictEqual(state.controlDisplay.overlay, "none", "the Overlay row is hidden");
      // A missing file says nothing about the grid: the line does not claim
      // it has no position on the Earth (#1036), and the status says what
      // happened.
      assert.strictEqual(state.unavailable?.display, "none", "no 'Not available for this grid' line");
      assert.strictEqual(reasonFor(state, "Projection"), null);
      assert.strictEqual(state.status, `Error: ${FILE_UNREADABLE}`);
      assert.strictEqual(state.title, NO_FIELD_HEADING, "the heading names no variable of the old file");
      assert.deepStrictEqual(
        panel.received.slice(before.received).filter(overlayTraffic).map((m) => m.type),
        [],
        "no overlay request for the slice that failed",
      );
      assert.deepStrictEqual(
        panel.sent.slice(before.sent).filter(refusals).map((m) => m.type),
        ["gridError"],
        "and no refusal beyond the render's own error",
      );
      // Because the error carries the slice's answers.
      assert.deepStrictEqual(failed.sliceGrid, {
        label: null,
        reprojectable: false,
        placed: false,
        supports: { contours: false, vectors: false, longCsv: false, zonalMean: false, bilinear: false },
        unavailable: [
          {
            kind: "structural",
            reason: UNPLACED_REASON,
            controls: [
              { key: "projection", label: "Projection" },
              { key: "resampling", label: "Resampling" },
              { key: "overlay", label: "Overlay" },
              { key: "zonal", label: "Zonal average" },
              { key: "longCsv", label: "Long layout (Export CSV)" },
            ],
          },
        ],
      });

      // The file is written back and reverted again. The panel moves to the
      // new handle and draws temperature, and the map targets and overlays
      // come back with it, on the map target saved before: a missing file
      // says nothing about the slice, so it did not overwrite that choice.
      fs.writeFileSync(file, original);
      from = panel.sent.length;
      await revert();
      const drawn = await panel.waitSent(isType("gridReady"), from, "temperature again");
      assert.strictEqual(drawn.sliceGrid.placed, true);
      assert.strictEqual(drawn.options.projection, "equirectangular", "the saved map target survives");
      await panel.waitSent(isType("overlayReady"), from, "the coastlines again");
      state = await panel.drive([]);
      assert.ok(state.projectionOffers.includes("equirectangular"), `got ${state.projectionOffers.join(", ")}`);
      assert.strictEqual(state.projection, "equirectangular");
      assert.strictEqual(state.controlShown.projection, true);
      assert.strictEqual(reasonFor(state, "Projection"), null);
      assert.strictEqual(state.controlShown.overlay, true);
      assert.strictEqual(reasonFor(state, "Overlay"), null);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // The new file has the variable, under another number, and something else
  // under the old one. The panel is restored after the change.
  test("a slice panel finds its variable by name in a file that changed (#839)", async () => {
    // `plain_deflate` is last of one file's five variables and first of the
    // other's: a number is a position in the list (#1073), so it moves with the
    // list. Two-dimensional in both, so the page asks for no axis labels before
    // its `ready`.
    const first = fs.readFileSync(netcdfFixture("hdf5_fletcher32.h5"));
    const second = fs.readFileSync(netcdfFixture("hdf5_zstd.h5"));
    const before = indexOf(first, "plain_deflate");
    const after = indexOf(second, "plain_deflate");
    assert.notStrictEqual(before, after, "plain_deflate moves");
    const { panel, change, done } = await changingFile(first, "plain_deflate");
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(opened.messageIndex, before);

      await change(second);
      const from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the restored render");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, after, "plain_deflate at its number in the new file");
      assert.match(reply.titleLine, /^plain_deflate\b/);
      const state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "plain_deflate");
      assert.deepStrictEqual(state.sliceVariables, ["plain_deflate", "shuffle_zstd", "zstd", "zstd_fletcher32", "zstd_high"]);
      assert.match(state.title, /^plain_deflate\b/);
    } finally {
      done();
    }
  });

  // The same, on a four-dimensional variable. Restored, the page asks for its
  // axis labels before its `ready`, so the first request the provider reads
  // after the change is an axis request, and it writes the page again while
  // that `ready` is still on its way. The ready was posted by a page that is
  // gone, naming `sst` by its number in the old file, and is not the new
  // page's (#1202).
  test("a slice panel finds its variable by name when its page asks for axes first (#1202)", async () => {
    // `sst` is first of ERSST's two variables and second of OISST's.
    const first = fs.readFileSync(netcdfFixture("ersst_v5_187001_cdf1.nc"));
    const second = fs.readFileSync(netcdfFixture("oisst_avhrr_v2.nc"));
    const before = indexOf(first, "sst");
    const after = indexOf(second, "sst");
    assert.notStrictEqual(before, after, "sst moves");
    const { panel, change, done } = await changingFile(first, "sst");
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(opened.messageIndex, before);

      await change(second);
      const from = panel.sent.length;
      const received = panel.received.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the restored render");
      // The page did ask for an axis ahead of its ready, or this test does not
      // reach the case it is about.
      const asked = panel.received.slice(received);
      const firstAxis = asked.findIndex(isType("axisRequest"));
      const firstReady = asked.findIndex(isType("ready"));
      assert.ok(firstAxis >= 0 && firstAxis < firstReady, `asked ${asked.map((m) => m.type).join(", ")}`);
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, after, "sst at its number in the new file");
      assert.match(reply.titleLine, /^sst\b/);
      // Nothing is drawn under another variable's name afterwards either.
      const state = await panel.drive([]);
      const others = panel.sent.slice(from).filter((m) => m.type === "gridReady" && !/^sst\b/.test(m.titleLine));
      assert.deepStrictEqual(others.map((m) => m.titleLine), [], "every render after the change draws sst");
      assert.strictEqual(state.sliceVariable, "sst");
      assert.deepStrictEqual(state.sliceVariables, ["ice", "sst"]);
      assert.match(state.title, /^sst\b/);
    } finally {
      done();
    }
  });

  // A `ready` from a page the panel has written over is not answered, however
  // late it arrives: here after the new page's own (#1202). It names `sst` by
  // its number in the old file, which is `ice` in the new one.
  test("a ready from a page the panel has written over is not served (#1202)", async () => {
    const first = fs.readFileSync(netcdfFixture("ersst_v5_187001_cdf1.nc"));
    const second = fs.readFileSync(netcdfFixture("oisst_avhrr_v2.nc"));
    const after = indexOf(second, "sst");
    const { panel, change, done } = await changingFile(first, "sst");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const old = panel.received.filter(isType("ready")).pop();
      assert.ok(old, "the first page's ready");

      // The first request after the change has the page written again.
      await change(second);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const restored = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(restored.type, "gridReady", `${restored.error ?? ""}`);
      assert.strictEqual(restored.messageIndex, after);

      // The old page's ready arrives now.
      from = panel.sent.length;
      const seq = 990_001;
      await panel.sendAsPage({ ...old, seq });
      // A request after it is answered, so the ready has been read by then.
      await panel.drive([{ id: "flip-y", checked: false }]);
      const next = await panel.waitSent(isType("gridReady"), from, "the render after it");
      assert.match(next.titleLine, /^sst\b/);
      assert.deepStrictEqual(
        panel.sent.slice(from).filter((m) => m.seq === seq).map((m) => `${m.type} ${m.titleLine ?? m.error}`),
        [],
        "the old page's ready is not answered",
      );
    } finally {
      done();
    }
  });

  // The new file reuses the variable's number for another variable and does
  // not have it at all. Nothing is drawn under the old name: the panel says
  // the variable is gone and offers the new file's variables.
  test("a slice panel whose variable is gone says so rather than drawing another (#839)", async () => {
    // Both two-dimensional, so the old number draws in the new file.
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    const cmi = indexOf(second, "CMI");
    assert.strictEqual(cmi, indexOf(first, "temp"), "the new file has CMI at temp's number");
    const { panel, change, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");

      // A control changes after the file did. Nothing is drawn under the old
      // name; the panel says why.
      await change(second);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.match(reply.error, /temp is no longer in the file/);
      let state = await panel.drive([]);
      assert.match(state.status, /temp is no longer in the file/);
      assert.deepStrictEqual(state.sliceVariables, ["CMI", "DQF"]);
      assert.ok(!/^temp\b/.test(state.title), `the heading "${state.title}" does not name temp`);
      // Nor does it name a variable nobody picked: like a GRIB panel's `No
      // message drawn`, it says nothing is drawn, and so do the line under it
      // and the tab (#1230). The page was written so, not only relabelled.
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
      assert.strictEqual(state.subtitle, "changing.nc", "the line under it names only the file");
      assert.strictEqual(panel.title(), "pick a variable · changing.nc");
      assert.ok(
        panel.html().includes(`<h1 id="title-line">${NO_VARIABLE_HEADING}</h1>`),
        "the page is written with no variable's heading",
      );
      // Nothing drawn is exported either: the slice the pickers hold is one
      // nobody chose.
      let asked = panel.received.length;
      state = await panel.drive([{ id: "export-csv", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportSliceCsv")), "no CSV of a variable nobody picked");
      assert.match(state.exportStatus, /^Pick a variable to draw before exporting CSV/);
      asked = panel.received.length;
      state = await panel.drive([{ id: "export-png", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportPng")), "nor a PNG");
      assert.match(state.exportStatus, /^Pick a variable to draw before exporting a PNG/);
      // Nothing is chosen, so any variable picked is a change, the first one
      // included; until then no other control asks for a render.
      assert.strictEqual(state.sliceVariable, "", "the picker has nothing chosen");
      // Play waits on each frame, and none would be asked for.
      assert.strictEqual(state.playDisabled, true, "Play is off until a variable is picked");
      asked = panel.received.length;
      await panel.drive([{ id: "flip-y", checked: false }]);
      assert.deepStrictEqual(
        panel.received.slice(asked).filter((m) => m.type === "rerenderRequest"),
        [],
        "no render before a variable is picked",
      );

      // Picking one of the new file's variables draws it, here the first.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: cmi }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "CMI");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.match(drawn.titleLine, /^CMI\b/);
      state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "CMI");
      assert.strictEqual(state.playDisabled, false, "and Play is back");
      assert.match(state.title, /^CMI\b/, "under its own heading");
      assert.ok(state.subtitle.startsWith("changing.nc"), state.subtitle);
      assert.strictEqual(panel.title(), "CMI · changing.nc");
    } finally {
      done();
    }
  });

  // The overlay on screen goes with the picture when the panel waits for a pick,
  // so a pick that lands on the same variable number and axes asks for the
  // coastlines again (#1342). A file with no variable is the one answer that
  // leaves the page as it is, so its `clearDrawn` runs in the page that drew
  // the overlay. The pick that follows has the provider write the page again
  // (`adopt`), which also drops the key, so this holds with or without the key
  // reset in `clearDrawn`: it pins what the user sees, and the reset keeps the
  // page right on its own.
  test("a pick after a file with no variable asks for the coastlines again at the same variable number (#1342)", async () => {
    const first = fs.readFileSync(netcdfFixture("goes_geostationary.nc"));
    const empty = classicNetcdf([["time", 3]], []);
    const rad = indexOf(first, "Rad");
    const { panel, change, done } = await changingFile(first, "Rad");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "overlay-coastlines", checked: true }]);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines");

      // The file has no variable: the picture and its overlay go.
      await change(empty);
      from = panel.sent.length;
      await panel.drive([{ id: "reverse-colormap", checked: true }]);
      const gone = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(gone.type, "gridError");
      assert.strictEqual(gone.noVariables, true);
      assert.strictEqual((await panel.drive([])).canvasPainted, false);

      // Rad is back at the same number, on the same axes, and is picked with
      // Coastlines still ticked: they are asked for, and they arrive.
      await change(first);
      from = panel.sent.length;
      const asked = panel.received.length;
      await panel.drive([{ id: "slice-variable", value: rad }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "Rad again");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      await panel.waitSent(isType("overlayReady"), from, "the coastlines after the pick");
      assert.ok(
        panel.received.slice(asked).some((m) => m.type === "overlayRequest"),
        "the coastlines are asked for again",
      );
      assert.strictEqual((await panel.drive([])).coastlines, true);
    } finally {
      done();
    }
  });

  // The panel waits for a pick until one is made (#1237), as a GRIB panel does
  // (#959). A page written again (an imported colour table writes every panel)
  // or shown again after its tab was hidden used to restore the new file's
  // first variable and draw it, although nobody picked it. Compare is put away
  // meanwhile: nothing it would combine is on screen.
  test("a slice panel waiting for a pick stays on it through a rebuild and a hide/show (#1237)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    const cmi = indexOf(second, "CMI");
    const { panel, change, close, reopen, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(drawnOrRefused, from, "the render with Compare on");
      assert.strictEqual((await panel.drive([])).compareShown, true, "Compare is on screen while temp is drawn");

      await change(second);
      from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(reply.type, "gridError", `drew "${reply.titleLine}" instead of refusing`);
      let state = await panel.drive([]);
      assert.strictEqual(state.compareShown, false, "Compare goes while nothing is drawn");

      const again: Array<[string, () => Promise<void>]> = [
        ["a rebuild", async () => panel.rebuild()],
        ["a hide and show", () => panel.hideShow()],
        ["a second rebuild", async () => panel.rebuild()],
      ];
      for (const [what, redo] of again) {
        from = panel.sent.length;
        await redo();
        const answer = await panel.waitSent(drawnOrRefused, from, `the answer after ${what}`);
        assert.strictEqual(
          answer.type,
          "gridError",
          `after ${what}, drew "${answer.titleLine}", which nobody picked`,
        );
        assert.match(answer.error, /temp is no longer in the file/, what);
        state = await panel.drive([]);
        assert.strictEqual(state.title, NO_VARIABLE_HEADING, what);
        assert.strictEqual(state.subtitle, "changing.nc", what);
        assert.strictEqual(state.sliceVariable, "", `after ${what}, the picker has nothing chosen`);
        assert.deepStrictEqual(state.sliceVariables, ["CMI", "DQF"], what);
        assert.strictEqual(state.playDisabled, true, what);
        assert.strictEqual(state.compareShown, false, `after ${what}, Compare is still put away`);
        assert.strictEqual(state.canvasPainted, false, `after ${what}, nothing is drawn`);
        assert.ok(
          state.unavailable === null || state.unavailable.display === "none",
          `after ${what}, no "Not available" line speaks for a grid nobody picked`,
        );
        assert.ok(
          panel.html().includes('<fieldset id="compare-fieldset" hidden>'),
          "the page is written with Compare put away",
        );
        assert.strictEqual(panel.title(), "pick a variable · changing.nc", what);
        assert.ok(!panel.sent.slice(from).some(isType("gridReady")), `nothing is drawn after ${what}`);
      }

      // Picking a variable draws it, and Compare comes back with it.
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: cmi }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "CMI");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.match(drawn.titleLine, /^CMI\b/);
      state = await panel.drive([]);
      assert.strictEqual(state.compareShown, true, "Compare is back with a picture to compare");
      assert.strictEqual(state.canvasPainted, true, "CMI is drawn");
      assert.strictEqual(state.colorbarShown, true, "with its colour bar");
      assert.strictEqual(panel.title(), "CMI · changing.nc");

      // The page on screen was written while the pick was awaited. Shown
      // again from that HTML with the editor closed, it draws the variable
      // picked from the file the panel holds (#1338), not the refusal: the
      // pick was made, so the page does not go on waiting.
      close();
      from = panel.sent.length;
      await panel.hideShow();
      const closed = await panel.waitSent(drawnOrRefused, from, "the answer with the editor closed");
      assert.strictEqual(closed.type, "gridReady", `${closed.error ?? ""}`);
      assert.match(closed.titleLine, /^CMI\b/);
      state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "CMI", "the picker shows the variable picked");
      assert.strictEqual(state.playDisabled, false, "and the page is not waiting");
      await reopen();
      from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: false }]);
      const back = await panel.waitSent(drawnOrRefused, from, "the render once the file is open again");
      assert.strictEqual(back.type, "gridReady", `${back.error ?? ""}`);
      assert.match(back.titleLine, /^CMI\b/);

      // The pick holds: a rebuild now draws CMI again.
      from = panel.sent.length;
      panel.rebuild();
      const redrawn = await panel.waitSent(drawnOrRefused, from, "the render after a rebuild");
      assert.strictEqual(redrawn.type, "gridReady", `${redrawn.error ?? ""}`);
      assert.match(redrawn.titleLine, /^CMI\b/);
    } finally {
      done();
    }
  });

  // The file changes again while the panel waits for a pick (#1237). The
  // page's slice is then the first variable of the file in between, which
  // nobody picked: the panel goes on looking for the variable it drew last,
  // and draws it once a file holds it again. With the editor closed
  // meanwhile, it still refuses from the file it holds, and still waits.
  test("a slice panel waiting for a pick looks for its own variable through further changes (#1237)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    const third = fs.readFileSync(netcdfFixture("ersst_v5_187001_cdf1.nc"));
    const { panel, change, close, reopen, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(second);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      assert.strictEqual((await panel.waitSent(drawnOrRefused, from, "the refusal")).type, "gridError");

      // The editor closed while waiting: a rebuild is refused from the file
      // the panel holds (#1338), not drawn.
      close();
      from = panel.sent.length;
      panel.rebuild();
      const closed = await panel.waitSent(drawnOrRefused, from, "the answer with the editor closed");
      assert.strictEqual(closed.type, "gridError", `drew "${closed.titleLine}"`);
      assert.match(closed.error, /temp is no longer in the file/);
      let state = await panel.drive([]);
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
      assert.strictEqual(state.sliceVariable, "", "still waiting for a pick");
      await reopen();

      // The same file written again: CMI, the variable the page holds, is
      // in it, but nobody picked CMI.
      await change(second);
      from = panel.sent.length;
      panel.rebuild();
      const again = await panel.waitSent(drawnOrRefused, from, "the answer after the file is written again");
      assert.strictEqual(again.type, "gridError", `drew "${again.titleLine}", which nobody picked`);
      assert.match(again.error, /temp is no longer in the file/);

      // A third file, without temp: still refused, by temp's name.
      await change(third);
      from = panel.sent.length;
      panel.rebuild();
      const refused = await panel.waitSent(drawnOrRefused, from, "the answer after another change");
      assert.strictEqual(refused.type, "gridError", `drew "${refused.titleLine}", which nobody picked`);
      assert.match(refused.error, /temp is no longer in the file/);
      state = await panel.drive([]);
      assert.ok(!(state.sliceVariables ?? []).includes("CMI"), "the list is the third file's");
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
      assert.strictEqual(state.sliceVariable, "");
      assert.ok(!panel.sent.slice(from).some(isType("gridReady")), "nothing is drawn");

      // The first file again: temp is drawn, by name, under its own heading.
      await change(first);
      from = panel.sent.length;
      panel.rebuild();
      const drawn = await panel.waitSent(drawnOrRefused, from, "temp, back in the file");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.match(drawn.titleLine, /^temp\b/);
      state = await panel.drive([]);
      assert.strictEqual(state.sliceVariable, "temp");
      assert.strictEqual(state.playDisabled, false);
      assert.strictEqual(panel.title(), "temp · changing.nc");
    } finally {
      done();
    }
  });

  // The changed file has no variable at all (#1237). The panel said so but
  // left the old file's picture and heading on screen.
  test("a slice panel whose file has no variable left clears the old picture (#1237)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const empty = classicNetcdf([["time", 3]], []);
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    const { panel, change, close, done } = await changingFile(first, "temp");
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.strictEqual(state.canvasPainted, true, "temp is drawn");
      assert.strictEqual(state.colorbarShown, true);

      await change(empty);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(reply.type, "gridError");
      assert.match(reply.error, /no longer has a variable to draw/);
      state = await panel.drive([]);
      assert.match(state.status, /no longer has a variable to draw/);
      assert.strictEqual(state.title, NO_VARIABLE_HEADING, `not "${opened.titleLine}"`);
      assert.strictEqual(state.subtitle, "changing.nc");
      assert.strictEqual(state.canvasPainted, false, "the old picture is gone");
      assert.strictEqual(state.colorbarShown, false, "and its colour bar");
      assert.strictEqual(state.probe, "");
      assert.strictEqual(state.sliceVariable, "", "the picker has nothing chosen");
      assert.strictEqual(state.compareShown, false);
      assert.strictEqual(panel.title(), "pick a variable · changing.nc");

      // Written again meanwhile, the page still names no variable.
      from = panel.sent.length;
      panel.rebuild();
      assert.ok(
        panel.html().includes(`<h1 id="title-line">${NO_VARIABLE_HEADING}</h1>`),
        "the page is written with no variable's heading",
      );
      await panel.waitSent(drawnOrRefused, from, "the answer after a rebuild");
      state = await panel.drive([]);
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
      assert.ok(!panel.sent.slice(from).some(isType("gridReady")), "nothing is drawn");

      // The editor closes while the file has nothing to draw. The panel still
      // holds the file it drew temp from, but a pick is not drawn from it
      // (#1338): the panel says it has no file.
      close();
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: indexOf(first, "temp") }]);
      const closed = await panel.waitSent(drawnOrRefused, from, "the answer with the editor closed");
      assert.strictEqual(closed.type, "gridError", `drew "${closed.titleLine}" from the file before the change`);
      assert.strictEqual(closed.handleGone, true, "the answer is that the panel has no file");

      // The file gets variables again. temp is not one of them, so the panel
      // waits for a pick in the new file's list, which picking any variable
      // the page still lists brings up.
      await change(second);
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: indexOf(first, "temp") }]);
      const refused = await panel.waitSent(drawnOrRefused, from, "the answer once the file has variables");
      assert.strictEqual(refused.type, "gridError", `drew "${refused.titleLine}"`);
      assert.match(refused.error, /temp is no longer in the file/);
      state = await panel.drive([]);
      assert.deepStrictEqual(state.sliceVariables, ["CMI", "DQF"]);
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
    } finally {
      done();
    }
  });

  // Two changes leave the page back to back, after the file changed. The first
  // has the panel rewritten; the second is already on its way, naming the
  // variable by its old number, and must not be read from the new file.
  test("requests already sent when the file changed are not read from the new file (#839)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("goes16_abi_cmip.nc"));
    assert.strictEqual(indexOf(second, "CMI"), indexOf(first, "temp"), "the new file has CMI at temp's number");
    const { panel, change, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(second);
      const from = { sent: panel.sent.length, received: panel.received.length };
      await panel.drive([
        { id: "flip-y", checked: true },
        { id: "reverse-colormap", checked: true },
      ]);
      assert.strictEqual(
        panel.received.slice(from.received).filter((m) => m.type === "rerenderRequest").length,
        2,
        "two renders asked for, back to back",
      );
      const refused = await panel.waitSent(isType("gridError"), from.sent, "the refusal");
      assert.match(refused.error, /temp is no longer in the file/);
      await panel.drive([]);
      assert.deepStrictEqual(
        panel.sent.slice(from.sent).filter(drawnOrRefused).map((m: Msg) => `${m.type} ${m.titleLine ?? m.error}`),
        [`gridError ${refused.error}`],
        "nothing drawn from the old numbers",
      );
    } finally {
      done();
    }
  });

  // The file changes, and changes again while the panel is being rewritten for
  // it. The rewritten page numbers its variables as the second file does, and
  // its first request must be checked against the third.
  test("a file that changes again while the panel is rewritten is read afresh (#839)", async () => {
    const first = fs.readFileSync(netcdfFixture("missing_value_classic.nc"));
    const second = fs.readFileSync(netcdfFixture("cf_packed_data.nc"));
    const third = fs.readFileSync(netcdfFixture("record_mixed_cdf1.nc"));
    assert.strictEqual(indexOf(third, "a"), indexOf(second, "temp"), "the third file has a at temp's number in the second");
    const { panel, change, changeNow, done } = await changingFile(first, "temp");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(second);
      // The next change lands as the panel is written for the second file,
      // before that page can say it is ready.
      panel.onNextWrite(() => changeNow(third));
      const from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after both changes");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.match(reply.error, /temp is no longer in the file/);
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.sliceVariables, ["a", "b"], "the third file's variables");
    } finally {
      done();
    }
  });

  // A variable that fails to draw (#1388, T5 F1). The heading, the tab and the
  // projection caption went on naming the variable drawn before it, over its
  // picture, while the picker and the error were about the new one; after a
  // run of failures, the first one's. They name the variable picked now, and
  // the old picture goes.
  test("a slice panel names the variable it failed to draw, not the one drawn before (#1388)", async () => {
    const first = fs.readFileSync(netcdfFixture("non_finite_coordinates.nc"));
    const { panel, done } = await changingFile(first, "placed");
    try {
      const drawn = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.match(drawn.titleLine, /^placed\b/);
      let state = await panel.drive([]);
      assert.strictEqual(state.title, drawn.titleLine);
      assert.notStrictEqual(state.caption, "", "placed has a caption");
      assert.ok(state.canvasPainted, "placed is drawn");
      assert.strictEqual(panel.title(), "placed · changing.nc");

      // hole after placed is drawn; then the ones T5 saw keep all_nan's
      // heading, each after a failure.
      let before = "placed";
      for (const after of ["hole", "all_nan", "infinite_end", "masked"]) {
        const from = panel.sent.length;
        await panel.drive([{ id: "slice-variable", value: indexOf(first, after) }]);
        const failed = await panel.waitSent(drawnOrRefused, from, `${after}'s answer`);
        assert.strictEqual(failed.type, "gridError", `${after} was expected to fail to draw`);
        state = await panel.drive([]);
        const heading = drawn.titleLine.replace("placed", after);
        const step = `${before} → ${after}`;
        assert.strictEqual(state.title, heading, `${step}: the heading`);
        assert.strictEqual(panel.title(), `${after} · changing.nc`, `${step}: the tab`);
        assert.strictEqual(state.subtitle, "changing.nc", `${step}: the line under the heading`);
        assert.strictEqual(state.caption, "", `${step}: no caption of another grid`);
        assert.strictEqual(state.canvasPainted, false, `${step}: no picture of placed`);
        assert.strictEqual(state.colorbarShown, false);
        assert.match(state.status, /^Error: render failed/);
        // The heading names a field, so Compare stays on offer.
        assert.strictEqual(state.compareShown, true);
        before = after;
      }

      // A page written again names the last one too.
      const from = panel.sent.length;
      panel.rebuild();
      assert.ok(
        panel.html().includes(`<h1 id="title-line">${drawn.titleLine.replace("placed", "masked")}</h1>`),
        "the page is written with masked's heading",
      );
      await panel.waitSent(drawnOrRefused, from, "the answer after a rebuild");
      state = await panel.drive([]);
      assert.match(state.title, /^masked\b/);
    } finally {
      done();
    }
  });

  // After the file can no longer be read, picking a variable (T4 F2) took the
  // pick's wait off and brought the Compare row back, under a heading that
  // said nothing was drawn. Compare follows the heading (#1388), and the tab
  // says nothing is drawn too.
  test("a pick while the file cannot be read keeps Compare away (#1388)", async () => {
    const original = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1388-"));
    const file = path.join(dir, "closing.nc");
    fs.writeFileSync(file, original);
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () =>
      editor.send({ type: "renderVariable", variableIndex: indexOf(original, "temperature") }),
    );
    const revert = () => p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.strictEqual(state.compareShown, true, "Compare is offered while temperature is drawn");

      fs.writeFileSync(file, Buffer.from("not a NetCDF file"));
      let from = panel.sent.length;
      await revert();
      let failed = await panel.waitSent(drawnOrRefused, from, "the restored render's answer");
      assert.strictEqual(failed.handleGone, true);
      state = await panel.drive([]);
      assert.strictEqual(state.title, NO_FIELD_HEADING);
      assert.strictEqual(state.compareShown, false, "no Compare row while nothing is drawn");
      assert.strictEqual(panel.title(), "nothing drawn · closing.nc");

      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: indexOf(original, "lat_bnds") }]);
      failed = await panel.waitSent(drawnOrRefused, from, "the pick's answer");
      assert.strictEqual(failed.type, "gridError");
      assert.strictEqual(failed.handleGone, true);
      state = await panel.drive([]);
      assert.strictEqual(state.title, NO_FIELD_HEADING);
      assert.strictEqual(state.compareShown, false, "the pick does not bring Compare back");
      assert.strictEqual(state.canvasPainted, false);
      assert.strictEqual(state.caption, "");
      assert.strictEqual(panel.title(), "nothing drawn · closing.nc");

      // The file is written back: what is drawn is named, and Compare is back.
      fs.writeFileSync(file, original);
      from = panel.sent.length;
      await revert();
      const drawn = await panel.waitSent(drawnOrRefused, from, "the render once the file is back");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      state = await panel.drive([]);
      assert.strictEqual(state.title, drawn.titleLine);
      assert.strictEqual(state.compareShown, true);
      assert.notStrictEqual(panel.title(), "nothing drawn · closing.nc");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // The file is overwritten with one that has no variable to draw (T4 O4).
  // The picture and the heading went, but the Y/X pickers, the steppers and
  // the Animate row of the old variable stayed (#1388).
  test("a slice panel whose file has no variable left shows none of the old axes, steppers or Animate row (#1388)", async () => {
    const first = fs.readFileSync(fixture("netcdf4_dimscale.nc"));
    const { panel, change, done } = await changingFile(first, "temperature");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let state = await panel.drive([]);
      assert.deepStrictEqual(state.sliceAxesShown, { y: true, x: true, steppers: true });
      assert.notStrictEqual(state.animateRowDisplay, "none", "temperature steps through time");

      await change(fs.readFileSync(netcdfFixture("netcdf4_hdf5_dummy.nc")));
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const gone = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(gone.noVariables, true);
      state = await panel.drive([]);
      assert.strictEqual(state.title, NO_VARIABLE_HEADING);
      assert.deepStrictEqual(state.sliceAxesShown, { y: false, x: false, steppers: false });
      assert.strictEqual(state.animateRowDisplay, "none");
      assert.strictEqual(state.caption, "");

      // temperature is back and picked: its axes and Animate row come back.
      await change(first);
      from = panel.sent.length;
      await panel.drive([{ id: "slice-variable", value: indexOf(first, "temperature") }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "temperature again");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      state = await panel.drive([]);
      assert.deepStrictEqual(state.sliceAxesShown, { y: true, x: true, steppers: true });
      assert.notStrictEqual(state.animateRowDisplay, "none");
    } finally {
      done();
    }
  });

  test("a GRIB panel whose file is reverted to bytes that do not open says so and names no message (#1350, #1388)", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1350-"));
    const file = path.join(dir, "closing.grib2");
    fs.writeFileSync(file, fs.readFileSync(fixture("regular_latlon_surface.grib2")));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const before = await panel.drive([]);
      const tab = panel.title();
      assert.notStrictEqual(before.title, NO_FIELD_HEADING);
      assert.ok(before.canvasPainted, "the message is drawn");
      fs.writeFileSync(file, Buffer.from("not a GRIB file"));
      let from = panel.sent.length;
      await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
      const failed = await panel.waitSent(drawnOrRefused, from, "the restored render's answer");
      assert.strictEqual(failed.type, "gridError", `drew "${failed.titleLine}" from the file before the revert`);
      assert.strictEqual(failed.error, FILE_UNREADABLE);
      assert.strictEqual(failed.handleGone, true);
      let state = await panel.drive([]);
      assert.strictEqual(state.status, `Error: ${FILE_UNREADABLE}`);
      assert.strictEqual(state.title, NO_FIELD_HEADING, "the heading names no message of the old file");
      // Nor does the tab, or a picture under the heading (#1388).
      assert.strictEqual(panel.title(), "nothing drawn · closing.grib2", `not "${tab}"`);
      assert.strictEqual(state.canvasPainted, false);
      assert.strictEqual(state.caption, "");

      // The file is written back: the message is drawn and named again.
      fs.writeFileSync(file, fs.readFileSync(fixture("regular_latlon_surface.grib2")));
      from = panel.sent.length;
      await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
      const drawn = await panel.waitSent(drawnOrRefused, from, "the render once the file is back");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      state = await panel.drive([]);
      assert.strictEqual(state.title, before.title);
      assert.strictEqual(panel.title(), tab);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  // composeProbeValue writes the value part of the readout. Without it on the
  // page, a click on the image throws and the readout stays empty.
  test("a click on the image reads the value there (#841)", async () => {
    const p = await provider();
    const editor = await openEditor(p, fixture("regular_latlon_surface.grib2"));
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const from = panel.sent.length;
      await panel.probe();
      const reply = await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      assert.ok(reply.result, "the click lands on the grid");
      assert.strictEqual(typeof reply.result.value, "number", "and finds a value there");
      const state = await panel.drive([]);
      // The readout is "<lat, lon> · <value> <units> · grid i,j"; the value
      // part is composeProbeValue's, five significant figures.
      const value = Number(reply.result.value).toPrecision(5);
      assert.ok(state.probe.includes(value), `readout "${state.probe}" carries ${value}`);
    } finally {
      panel.dispose();
    }
  });
});

// The gate both kinds of render panel read requests through, on its own.
suite("A render panel's handle gate (#1202)", () => {
  const serve = () => true;

  test("only the page written last is served, and a page written again waits for its ready", () => {
    const gate = new PanelHandleGate<string>("a");
    const old = gate.page;
    assert.strictEqual(gate.admit({ type: "ready", page: old }, "a", serve), true);
    assert.strictEqual(gate.admit({ type: "ready" }, "a", serve), false, "an unnumbered message");

    gate.rewrite("b");
    const now = gate.page;
    assert.notStrictEqual(now, old, "a page written again has a number of its own");
    assert.strictEqual(gate.admit({ type: "ready", page: old }, "b", serve), false, "the old page's ready");
    assert.strictEqual(gate.admit({ type: "axisRequest", page: now }, "b", serve), false, "before its ready");
    assert.strictEqual(gate.admit({ type: "ready", page: now }, "b", serve), true);
    assert.strictEqual(gate.admit({ type: "ready", page: old }, "b", serve), false, "after it too");
    assert.strictEqual(gate.admit({ type: "rerenderRequest", page: old }, "b", serve), false);
    assert.strictEqual(gate.admit({ type: "rerenderRequest", page: now }, "b", serve), true);
    // A PNG export reads the page's own pixels.
    assert.strictEqual(gate.admit({ type: "exportPng", page: old }, "b", serve), true);
  });

  test("a page kept for a new file keeps its number", () => {
    const gate = new PanelHandleGate<string>("a");
    const page = gate.page;
    let adopted: string | undefined;
    const served = gate.admit({ type: "rerenderRequest", page }, "b", (h) => {
      adopted = h;
      gate.keep(h);
      return true;
    });
    assert.strictEqual(served, true);
    assert.strictEqual(adopted, "b");
    assert.strictEqual(gate.page, page);
    assert.strictEqual(gate.admit({ type: "rerenderRequest", page }, "b", () => false), true, "served, not adopted again");
  });
});
