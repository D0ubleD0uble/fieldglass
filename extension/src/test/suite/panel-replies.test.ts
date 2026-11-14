// Every request a render panel's page makes gets exactly one reply (#1808).
//
// The page counts what it has asked for and not yet heard back: an Export PNG
// waits on the overlay, contour and arrow answers in flight (#1379), and a
// request left unanswered holds it until its 15 s timeout. A GRIB panel and a
// slice panel each used to drop most requests in silence once their file
// could no longer be read, and a GRIB panel did the same while it waited for a
// message to be picked. Each request kind is now answered once, with an error
// or an empty result, through the dispatch both panels share.
//
// Driven through the real panel. The requests are posted by the page exactly
// as given, each with a `seq` of its own, so every reply can be matched to the
// request it answers.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { FILE_UNREADABLE, MESSAGE_GONE } from "../../provider";
import {
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  pageNumberOf,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

const GRIB = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");
const NETCDF = () =>
  path.join(extensionPath(), "..", "crates", "fieldglass-netcdf", "tests", "fixtures", "ersst_v5_187001_cdf1.nc");

const isReply = (m: Msg) => m && (m.type === "gridReady" || m.type === "gridError");

/** One of each request kind a render panel's page sends that asks for an
 *  answer, numbered from `seq`, with the reply type each is answered by. The
 *  axis request carries no `seq`; its answer echoes `dim`, which no other
 *  request here sends. */
function everyRequest(seq: number): Array<{ request: Msg; replies: string[] }> {
  return [
    { request: { type: "ready", seq }, replies: ["gridError"] },
    { request: { type: "rerenderRequest", seq: seq + 1 }, replies: ["gridError"] },
    { request: { type: "overlayRequest", seq: seq + 2, coastlines: true }, replies: ["overlayError"] },
    { request: { type: "contourRequest", seq: seq + 3 }, replies: ["contourError"] },
    { request: { type: "vectorRequest", seq: seq + 4, messageIndexV: 0 }, replies: ["vectorResult"] },
    { request: { type: "probeRequest", seq: seq + 5, px: 1, py: 1 }, replies: ["probeResult"] },
    { request: { type: "zonalRequest", seq: seq + 6 }, replies: ["zonalResult"] },
    { request: { type: "lineRequest", seq: seq + 7, alongDim: 0, gridI: 0, gridJ: 0 }, replies: ["lineResult"] },
    { request: { type: "axisRequest", dim: 0 }, replies: ["axisResult"] },
    // No image: the export fails at once, with no dialog.
    { request: { type: "exportPng" }, replies: ["exportPngDone"] },
  ];
}

/** Whether `reply` answers `request`: by `seq`, or for the two kinds that
 *  carry none, by kind. */
function answers(request: Msg, reply: Msg): boolean {
  if (request.type === "axisRequest") return reply.type === "axisResult" && reply.dim === request.dim;
  if (request.type === "exportPng") return reply.type === "exportPngDone";
  return reply.seq === request.seq;
}

/** Send every request kind to `panel` as its current page, and check each is
 *  answered exactly once, by the reply its kind expects, saying `why`. */
async function eachAnsweredOnce(panel: RealPanel, seq: number, why: string, what: string): Promise<void> {
  const page = pageNumberOf(panel.html());
  const asked = everyRequest(seq);
  const from = panel.sent.length;
  const shown = vscode.window.showErrorMessage;
  // The PNG export says it failed as a notification as well; kept off screen.
  (vscode.window as unknown as Record<string, unknown>).showErrorMessage = () => Promise.resolve(undefined);
  try {
    for (const { request } of asked) await panel.sendAsPage({ ...request, page });
    // The PNG export answers after a promise settles; the rest at once.
    await waitUntil(
      () => (panel.sent.slice(from).some(isType("exportPngDone")) ? true : undefined),
      `the PNG export's answer ${what}`,
      5_000,
    ).catch(() => undefined);
    // Room for a second answer to anything, which would be a defect too.
    await new Promise((r) => setTimeout(r, 300));
  } finally {
    (vscode.window as unknown as Record<string, unknown>).showErrorMessage = shown;
  }
  const sent = panel.sent.slice(from);
  const problems: string[] = [];
  for (const { request, replies } of asked) {
    const got = sent.filter((m: Msg) => answers(request, m));
    if (got.length !== 1) {
      problems.push(`${request.type}: ${got.length} replies [${got.map((m: Msg) => m.type).join(", ")}]`);
      continue;
    }
    const [reply] = got;
    if (!replies.includes(reply.type)) problems.push(`${request.type}: answered by ${reply.type}`);
    // What the answer says: the reason, or no result at all.
    if (reply.type === "exportPngDone") continue;
    if ("result" in reply && reply.result !== null) problems.push(`${request.type}: carries a result`);
    if (typeof reply.error === "string" && reply.error !== why) {
      problems.push(`${request.type}: says "${reply.error}", not "${why}"`);
    }
  }
  assert.deepStrictEqual(problems, [], `every request ${what} is answered once`);
}

/** A copy of `source` in a directory of its own, removed by `done`. */
function copyOf(source: string): { file: string; done: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1808-"));
  const file = path.join(dir, path.basename(source));
  fs.copyFileSync(source, file);
  return { file, done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

suite("Every render panel request is answered once (#1808)", function () {
  this.timeout(120_000);

  for (const [kind, source, open] of [
    ["GRIB", GRIB, { type: "decodeGrid", messageIndex: 0 }],
    ["slice", NETCDF, { type: "renderVariable", variableIndex: 0 }],
  ] as const) {
    test(`a ${kind} panel whose file can no longer be read answers each request once`, async () => {
      const p = await provider();
      const copy = copyOf(source());
      const editor = await openEditor(p, copy.file);
      const panel = await openReal(p, () => editor.send(open));
      try {
        await panel.waitSent(isType("gridReady"), 0, "the first render");
        // Read again into bytes that do not open: the panel has no file.
        fs.writeFileSync(copy.file, Buffer.from("not a GRIB or NetCDF file"));
        const from = panel.sent.length;
        await p.revertCustomDocument(editor.document, new vscode.CancellationTokenSource().token);
        const gone = await panel.waitSent(isReply, from, "the answer after the revert");
        assert.strictEqual(gone.handleGone, true, "the panel says its file is gone");
        await eachAnsweredOnce(panel, 9000, FILE_UNREADABLE, "with the file gone");
      } finally {
        panel.dispose();
        editor.close();
        copy.done();
      }
    });
  }

  test("a GRIB panel waiting for a message to be picked answers each request once", async () => {
    // The panel's message is gone from the file opened again, so it offers
    // the new file's messages and draws nothing until one is picked.
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1808-"));
    const file = path.join(dir, "changing.grib2");
    fs.copyFileSync(GRIB(), file);
    let editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      editor.close();
      fs.writeFileSync(file, fs.readFileSync(path.join(extensionPath(), "src", "test", "fixtures", "eta_lambert_msg0.grib2")));
      editor = await openEditor(p, file);
      const from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const refused = await panel.waitSent(isReply, from, "the render after the change");
      assert.strictEqual(refused.error, MESSAGE_GONE, "the panel waits for a pick");
      await eachAnsweredOnce(panel, 9100, MESSAGE_GONE, "while a pick is awaited");
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
