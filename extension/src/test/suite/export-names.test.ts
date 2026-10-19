// Export PNG and Export CSV sit side by side and follow one naming rule
// (#1039, #1112).
//
// From the 0.6.0 manual pass (§2): Export CSV sat in another toolbar row from
// Export PNG, and a GRIB render panel had no CSV button at all. The CSV save
// dialog suggested no name or folder, and a name typed without `.csv` was
// saved as typed, so the export landed at `~/Downloads/test-fieldglass`. The
// PNG's default name left out the slice, so level 5 and level 6 of one
// variable were offered the same file, and its 96-character cap cut a long
// parameter's `-message-N` off the end, so two messages could share a name.
//
// Driven through the real panel: the buttons are the page's own, a click goes
// through the provider's real handler, and only VS Code's dialogs are stubbed,
// as `png-export.test.ts` stubs them.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { loadNative } from "../../native";
import {
  composeDefaultPngName,
  composeExportName,
  CSV_FIELD_A_NOTE,
  EXPORT_CSV_TITLE,
  sanitizeExportName,
  sanitizePngName,
} from "../../render-panel";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  type PageState,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const FIXTURES = () => path.join(extensionPath(), "src", "test", "fixtures");
const ZARR_STORES = () => path.join(extensionPath(), "..", "crates", "fieldglass-zarr", "tests", "fixtures", "stores");

/** A committed fixture, asserted to exist so a moved one fails loudly. */
function fixture(p: string): string {
  assert.ok(fs.existsSync(p), `fixture missing: ${p}`);
  return p;
}

/** Characters no file name may hold on Windows, macOS or Linux. */
const UNSAFE = /[<>:"/\\|?*\u0000-\u001f]/;

/** What the stubbed save dialog was asked, and what it answers. */
interface SaveCall {
  defaultUri?: vscode.Uri;
  filters?: Record<string, string[]>;
}

/** Run `act` with VS Code's dialogs stubbed: the CSV layout prompt picks the
 *  matrix, the save dialog records what it was asked and answers `answer`,
 *  and a replace prompt answers `replace`. Resolves with the save dialog's
 *  call once `done` holds. */
async function withDialogs(
  answer: vscode.Uri | undefined,
  act: () => Promise<void>,
  done: (seen: { calls: SaveCall[]; warnings: string[] }) => boolean,
  replace?: string,
): Promise<{ calls: SaveCall[]; warnings: string[]; prompts: string[] }> {
  const calls: SaveCall[] = [];
  const warnings: string[] = [];
  const prompts: string[] = [];
  const w = vscode.window as unknown as Record<string, unknown>;
  const saved = {
    showSaveDialog: w.showSaveDialog,
    showQuickPick: w.showQuickPick,
    showInformationMessage: w.showInformationMessage,
    showWarningMessage: w.showWarningMessage,
  };
  w.showSaveDialog = (opts: SaveCall) => {
    calls.push(opts);
    return Promise.resolve(answer);
  };
  w.showQuickPick = (items: Array<{ format?: string }>, opts?: { placeHolder?: string }) => {
    prompts.push(opts?.placeHolder ?? "");
    return Promise.resolve(Array.isArray(items) ? items.find((i) => i.format === "matrix") : undefined);
  };
  w.showInformationMessage = () => Promise.resolve(undefined);
  w.showWarningMessage = (message: string) => {
    warnings.push(message);
    return Promise.resolve(replace);
  };
  try {
    await act();
    await waitUntil(() => (done({ calls, warnings }) ? true : undefined), "the export to finish");
    // And the CSV export the click started has settled, so nothing it does
    // later can land after the test has read the result.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await ((await provider()) as any)._csvExport;
  } finally {
    Object.assign(w, saved);
  }
  return { calls, warnings, prompts };
}

/** The two export buttons, asserted shown and in one toolbar row. */
function exportButtons(state: PageState, what: string): { png: string; csv: string } {
  const png = state.exports.find((b) => b.text === "Export PNG…");
  const csv = state.exports.find((b) => b.text === "Export CSV…");
  assert.ok(png && csv, `${what} has both export buttons, got ${JSON.stringify(state.exports)}`);
  assert.ok(png.shown && csv.shown, `${what} shows both: ${JSON.stringify(state.exports)}`);
  assert.strictEqual(csv.row, png.row, `${what}: Export CSV sits in Export PNG's row: ${JSON.stringify(state.exports)}`);
  return { png: png.id, csv: csv.id };
}

/** Click the button with `id`, as a user does. */
async function click(panel: RealPanel, id: string): Promise<void> {
  await panel.drive([{ id, click: true }]);
}

async function openGrib(): Promise<{ panel: RealPanel; close: () => void; file: string }> {
  const p = await provider();
  const file = fixture(path.join(FIXTURES(), "regular_latlon_surface.grib2"));
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    file,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

/** `netcdf4_dimscale.nc`'s `temperature(time, lat, lon)`, whose two times are
 *  2020-01-01 00:00 and 06:00. */
async function openDimscale(): Promise<{ panel: RealPanel; close: () => void; file: string }> {
  const p = await provider();
  const file = fixture(path.join(FIXTURES(), "netcdf4_dimscale.nc"));
  const native = loadNative();
  assert.ok(native, "native binding required");
  const temperature = native.open(fs.readFileSync(file))
    .variables()
    .find((v) => v.name === "temperature");
  assert.ok(temperature, "the fixture has temperature");
  assert.deepStrictEqual(temperature.dims.map((d) => d.name), ["time", "lat", "lon"]);
  const editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "renderVariable", variableIndex: temperature.index }));
  await panel.waitSent(isType("gridReady"), 0, "the first render");
  return {
    panel,
    file,
    close: () => {
      panel.dispose();
      editor.close();
    },
  };
}

const drawn = (m: Msg) => m && m.type === "gridReady";

/** Step the panel's time stepper to `index` and wait for that render. */
async function stepTime(panel: RealPanel, index: number): Promise<Msg> {
  const from = panel.sent.length;
  await panel.drive([{ selector: '#slice-dims .slice-number[data-dim="0"]', value: index }]);
  return panel.waitSent(drawn, from, `the render at time ${index}`);
}

suite("Export names, folders and buttons (#1039, #1112)", function () {
  this.timeout(120_000);

  test("one rule names a PNG and a CSV: source, parameter, then the message or the slice", () => {
    const gfs = { kind: "message" as const, name: "gfs.grib2" };
    const temp = { index: 4, parameter: "Temperature" };
    assert.strictEqual(composeExportName(temp, gfs, "png"), "gfs_temperature_message-4.png");
    assert.strictEqual(composeExportName(temp, gfs, "csv"), "gfs_temperature_message-4.csv");

    const daily = { kind: "variable" as const, name: "ncep_air_daily_2024.nc" };
    const air = { index: 0, parameter: "air" };
    const day = { name: "time", index: 14, coordinate: "2024-01-15" };
    assert.strictEqual(composeExportName(air, daily, "csv", [day]), "ncep_air_daily_2024_air_time-2024-01-15.csv");
    assert.strictEqual(composeExportName(air, daily, "png", [day]), "ncep_air_daily_2024_air_time-2024-01-15.png");
    // A 2-D variable holds nothing, and keeps its name.
    assert.strictEqual(composeExportName(air, daily, "png"), "ncep_air_daily_2024_air.png");

    const levels = { kind: "variable" as const, name: "ncep_air_levels_ltm.nc" };
    const level = { name: "level", index: 5, coordinate: "500 millibar" };
    assert.strictEqual(
      composeExportName(air, levels, "png", [{ name: "time", index: 0, coordinate: "0000-12-30" }, level]),
      "ncep_air_levels_ltm_air_time-0000-12-30_level-500-millibar.png",
    );
    // An axis with no coordinates is named by its index.
    assert.strictEqual(
      composeExportName({ index: 0, parameter: "k" }, { kind: "variable", name: "k.zarr" }, "png", [
        { name: "phony_dim_0", index: 3, coordinate: null },
      ]),
      "k_k_phony_dim_0-index-3.png",
    );
  });

  test("a slice's name is a valid file name everywhere, and keeps a negative value's sign", () => {
    const src = { kind: "variable" as const, name: "f.nc" };
    const v = { index: 0, parameter: "v" };
    const six = composeExportName(v, src, "png", [{ name: "time", index: 1, coordinate: "2020-01-01 06:00" }]);
    assert.strictEqual(six, "f_v_time-2020-01-01-0600.png");
    const deep = composeExportName(v, src, "csv", [{ name: "depth", index: 2, coordinate: "−5.5 m" }]);
    const high = composeExportName(v, src, "csv", [{ name: "depth", index: 3, coordinate: "5.5 m" }]);
    assert.strictEqual(deep, "f_v_depth-minus5.5-m.csv");
    assert.notStrictEqual(deep, high, "−5.5 and 5.5 are two names");
    // A time whose reference cannot be read keeps its count first.
    const count = composeExportName(v, src, "png", [
      { name: "time", index: 0, coordinate: "1963872 hours since the flood of 1800/01/01 12:00" },
    ]);
    for (const name of [six, deep, high, count]) {
      assert.ok(!UNSAFE.test(name), `no reserved character in ${name}`);
    }
    assert.ok(count.startsWith("f_v_time-1963872-hours"), count);
  });

  test("a long parameter name keeps the message index, so two messages get two names", () => {
    // A 24-character GFS stem and a 70-character parameter: the cut used to
    // land on `-message-N`.
    const gfs = { kind: "message" as const, name: "gfs.t00z.pgrb2.0p25.f000" };
    const parameter = "Shape factor with respect to temperature profile in the thermocline x";
    assert.ok(parameter.length >= 60);
    const three = composeDefaultPngName({ index: 3, parameter }, gfs);
    const thirteen = composeDefaultPngName({ index: 13, parameter }, gfs);
    assert.match(three, /message-3\.png$/, three);
    assert.match(thirteen, /message-13\.png$/, thirteen);
    assert.notStrictEqual(three, thirteen);
    assert.ok(three.length <= 100, `capped: ${three.length} characters`);
    assert.ok(three.startsWith("gfs.t00z.pgrb2.0p25.f000"), `the stem stays whole: ${three}`);
    // The slice is kept the same way.
    const slice = composeExportName({ index: 1, parameter: parameter.repeat(2) }, { kind: "variable", name: "a.nc" }, "csv", [
      { name: "level", index: 5, coordinate: "500 millibar" },
    ]);
    assert.match(slice, /_level-500-millibar\.csv$/, slice);
    assert.ok(slice.length <= 100, `capped: ${slice}`);
  });

  test("a GRIB panel has Export CSV beside Export PNG, and its dialog starts in the file's folder on a .csv name", async () => {
    const { panel, file, close } = await openGrib();
    try {
      const state = await panel.drive([]);
      const ids = exportButtons(state, "the GRIB panel");
      const ready = panel.sent.find(isType("gridReady"));
      const { calls } = await withDialogs(undefined, () => click(panel, ids.csv), (s) => s.calls.length > 0);
      const asked = calls[0];
      assert.ok(asked.defaultUri, "the CSV dialog suggests a file");
      assert.strictEqual(path.dirname(asked.defaultUri.fsPath), path.dirname(file), "in the GRIB file's folder");
      // The same name as the PNG, as a .csv.
      assert.strictEqual(path.basename(asked.defaultUri.fsPath), ready.defaultPngName.replace(/\.png$/, ".csv"));
      assert.match(path.basename(asked.defaultUri.fsPath), /^regular_latlon_surface_.+_message-0\.csv$/);
      assert.deepStrictEqual(asked.filters, { "CSV files": ["csv"] });
    } finally {
      close();
    }
  });

  test("typing `out` saves out.csv and out.png", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1039-"));
    const typed = vscode.Uri.file(path.join(dir, "out"));
    const { panel, close } = await openGrib();
    try {
      const ids = exportButtons(await panel.drive([]), "the GRIB panel");
      await withDialogs(typed, () => click(panel, ids.csv), () => fs.existsSync(path.join(dir, "out.csv")));
      assert.ok(!fs.existsSync(typed.fsPath), "nothing is written without the extension");
      const csv = fs.readFileSync(path.join(dir, "out.csv"), "utf8");
      assert.ok(csv.length > 0, "the CSV has the field in it");

      // The PNG the page composites, saved through the same dialog.
      const from = panel.sent.length;
      await withDialogs(typed, () => click(panel, ids.png), () => fs.existsSync(path.join(dir, "out.png")));
      const done = await panel.waitSent(isType("exportPngDone"), from, "the PNG export's outcome");
      assert.strictEqual(done.outcome.status, "saved");
      assert.strictEqual(done.outcome.path, path.join(dir, "out.png"));
      assert.ok(!fs.existsSync(typed.fsPath), "nothing is written without the extension");
      assert.deepStrictEqual(
        [...fs.readFileSync(path.join(dir, "out.png")).subarray(1, 4)],
        [...Buffer.from("PNG")],
        "a PNG",
      );
    } finally {
      close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a typed name whose .csv exists is asked about before it is replaced", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1039-"));
    const typed = vscode.Uri.file(path.join(dir, "out"));
    fs.writeFileSync(path.join(dir, "out.csv"), "keep me");
    const { panel, close } = await openGrib();
    try {
      const ids = exportButtons(await panel.drive([]), "the GRIB panel");
      const kept = await withDialogs(typed, () => click(panel, ids.csv), (s) => s.warnings.length > 0);
      assert.match(kept.warnings[0], /out\.csv already exists/);
      assert.strictEqual(fs.readFileSync(path.join(dir, "out.csv"), "utf8"), "keep me", "kept when not confirmed");
      await withDialogs(
        typed,
        () => click(panel, ids.csv),
        () => fs.readFileSync(path.join(dir, "out.csv"), "utf8") !== "keep me",
        "Replace",
      );
    } finally {
      close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a NetCDF panel has both buttons in one row, and two slices get two names, PNG and CSV", async () => {
    const { panel, file, close } = await openDimscale();
    try {
      const ids = exportButtons(await panel.drive([]), "the NetCDF panel");
      const names: Array<{ png: string; csv: string }> = [];
      // The panel opens on time 0; then time 1.
      for (const index of [0, 1]) {
        const ready = index === 0 ? panel.sent.find(drawn) : await stepTime(panel, index);
        const { calls } = await withDialogs(undefined, () => click(panel, ids.csv), (s) => s.calls.length > 0);
        const asked = calls[0].defaultUri;
        assert.ok(asked, "the CSV dialog suggests a file");
        assert.strictEqual(path.dirname(asked.fsPath), path.dirname(file), "in the NetCDF file's folder");
        names.push({ png: ready.defaultPngName, csv: path.basename(asked.fsPath) });
      }
      assert.deepStrictEqual(names, [
        { png: "netcdf4_dimscale_temperature_time-2020-01-01.png", csv: "netcdf4_dimscale_temperature_time-2020-01-01.csv" },
        {
          png: "netcdf4_dimscale_temperature_time-2020-01-01-0600.png",
          csv: "netcdf4_dimscale_temperature_time-2020-01-01-0600.csv",
        },
      ]);
      for (const n of names) assert.ok(!UNSAFE.test(n.png) && !UNSAFE.test(n.csv), `valid everywhere: ${JSON.stringify(n)}`);
    } finally {
      close();
    }
  });

  test("a Zarr panel has both buttons in one row, and its CSV dialog starts in the store", async () => {
    const p = await provider();
    const store = fixture(path.join(ZARR_STORES(), "cf_v2"));
    const panel = await openReal(p, () => p.openZarrStore(vscode.Uri.file(store)));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const ids = exportButtons(await panel.drive([]), "the Zarr panel");
      const { calls } = await withDialogs(undefined, () => click(panel, ids.csv), (s) => s.calls.length > 0);
      const asked = calls[0].defaultUri;
      assert.ok(asked, "the CSV dialog suggests a file");
      // A store is a folder, and an export lands inside it, as the PNG's does.
      assert.strictEqual(path.dirname(asked.fsPath), store);
      assert.strictEqual(path.basename(asked.fsPath), "cf_v2_a.csv");
    } finally {
      panel.dispose();
    }
  });

  test("a 5-D tail keeps every value: units and names are shortened first", () => {
    // A long stem and parameter at their floor, a time whose reference cannot
    // be read, a level, and an ensemble member with no coordinates: the cut
    // used to land on the member's index, so members 12 and 13 shared a name.
    const src = { kind: "variable" as const, name: "a_rather_long_ensemble_forecast_archive_name.nc" };
    const v = { index: 0, parameter: "air_temperature_at_isobaric_levels" };
    const tail = (member: number) => [
      { name: "time", index: 4, coordinate: "1963872 hours since the flood of 1800/01/01 12:00" },
      { name: "level", index: 5, coordinate: "500 millibar" },
      { name: "ensemble_member", index: member, coordinate: null },
    ];
    const twelve = composeExportName(v, src, "png", tail(12));
    const thirteen = composeExportName(v, src, "png", tail(13));
    assert.notStrictEqual(twelve, thirteen);
    assert.ok(twelve.length <= 100, `capped: ${twelve.length} characters: ${twelve}`);
    assert.match(twelve, /-index-12\.png$/, twelve);
    assert.match(twelve, /_time-1963872/, `the time's value is kept: ${twelve}`);
    assert.match(twelve, /_lev[a-z]*-500[-_]/, `the level's value is kept: ${twelve}`);
  });

  test("a long dimension name is shortened, never its index or value", () => {
    const src = { kind: "variable" as const, name: "f.nc" };
    const v = { index: 0, parameter: "v" };
    const long = "number_of_layers_in_vertical_profile_of_the_retrieval";
    const indexed = composeExportName(v, src, "csv", [{ name: long, index: 3, coordinate: null }]);
    assert.match(indexed, /-index-3\.csv$/, indexed);
    const part = indexed.slice("f_v_".length, -".csv".length);
    assert.ok(part.length <= 40, `the slice part is capped: ${part}`);
    const valued = composeExportName(v, src, "csv", [{ name: long, index: 3, coordinate: "1013.25 hectopascal" }]);
    assert.match(valued, /-1013\.25(-|\.csv$)/, valued);
  });

  test("a Windows device name gets a suffix, with or without an extension", () => {
    for (const reserved of ["con", "NUL", "aux", "prn", "com1", "COM9", "lpt1", "lpt9"]) {
      const lower = reserved.toLowerCase();
      assert.strictEqual(sanitizeExportName(reserved, "csv"), `${lower}-.csv`);
      assert.strictEqual(sanitizePngName(`${reserved}.png`), `${lower}-.png`);
      assert.strictEqual(sanitizePngName(`${reserved}.tar`), `${lower}-.tar.png`);
    }
    // Only the device name itself: these are ordinary names.
    assert.strictEqual(sanitizeExportName("console", "csv"), "console.csv");
    assert.strictEqual(sanitizeExportName("com10", "csv"), "com10.csv");
    assert.strictEqual(sanitizeExportName("aux_air", "csv"), "aux_air.csv");
    assert.strictEqual(
      composeExportName({ index: 0, parameter: null }, { kind: "variable", name: "nul.nc" }, "csv"),
      "nul_0.csv",
    );
  });

  test("with Compare on, a GRIB panel's Export CSV says it holds Field A, in the tooltip and the layout prompt", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1039-"));
    const file = path.join(dir, "pair.grib2");
    const bytes = fs.readFileSync(fixture(path.join(FIXTURES(), "regular_latlon_surface.grib2")));
    fs.writeFileSync(file, Buffer.concat([bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const state = await panel.drive([]);
      const ids = exportButtons(state, "the GRIB panel");
      assert.strictEqual(state.exports.find((b) => b.id === ids.csv)?.title, EXPORT_CSV_TITLE);
      assert.match(EXPORT_CSV_TITLE, /Field A/);

      const plain = await withDialogs(undefined, () => click(panel, ids.csv), (d) => d.calls.length > 0);
      assert.ok(!plain.prompts[0].includes(CSV_FIELD_A_NOTE), `no Compare, no note: ${plain.prompts[0]}`);

      const from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      await panel.waitSent(drawn, from, "the difference map");
      const compared = await withDialogs(undefined, () => click(panel, ids.csv), (d) => d.calls.length > 0);
      assert.ok(compared.prompts[0].startsWith(CSV_FIELD_A_NOTE), `the prompt says Field A: ${compared.prompts[0]}`);
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("with Compare on, a NetCDF panel's Export CSV says it holds Field A", async () => {
    const { panel, close } = await openDimscale();
    try {
      const state = await panel.drive([]);
      const ids = exportButtons(state, "the NetCDF panel");
      assert.strictEqual(state.exports.find((b) => b.id === ids.csv)?.title, EXPORT_CSV_TITLE);
      const from = panel.sent.length;
      await panel.drive([{ id: "compare-op", value: "a_minus_b" }]);
      await panel.waitSent(drawn, from, "the difference map");
      const { prompts } = await withDialogs(undefined, () => click(panel, ids.csv), (d) => d.calls.length > 0);
      assert.ok(prompts[0].startsWith(CSV_FIELD_A_NOTE), `the prompt says Field A: ${prompts[0]}`);
    } finally {
      close();
    }
  });
});
