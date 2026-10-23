// A GRIB render panel whose file changes on disk under it (#959), run in a real
// webview through the driver in `real-panel.ts`.
//
// The panel outlives the editor it was opened from. It used to keep the
// message's metadata from when it opened and ask for its message by index, so
// after the file changed it drew whatever the new file held at that index under
// the old heading. It now finds its message by identity, Rust's
// `MessageInfo.identity`: exactly one match is drawn under its own heading, and
// none or several is refused with the new file's messages to pick from.
//
// The files are one committed GRIB2 message (`regular_latlon_surface.grib2`,
// 2 m temperature) with its parameter number rewritten, so every message is a
// different field on the same grid, and any two can be compared. The GRIB1
// case is one committed message (`spectral_simple_t63.grib1`) with its ensemble
// member rewritten.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { gribFieldLabel, MESSAGE_GONE } from "../../provider";
import { NO_MESSAGE_HEADING } from "../../render-panel";
import {
  assertNothingActs,
  type Editor,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
  waitUntil,
} from "./real-panel";

/** The fixture's one message, as parameter `number` of its category: 0 is
 *  temperature as committed, any other number another field. §4 octet 11 is
 *  the parameter number in every product template that carries one. */
function field(number: number): Buffer {
  const bytes = fs.readFileSync(
    path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2"),
  );
  // Sections follow the 16-octet §0, each with its length in octets 1-4 and
  // its number in octet 5, up to the closing "7777".
  let at = 16;
  while (at + 5 <= bytes.length && bytes.toString("latin1", at, at + 4) !== "7777") {
    if (bytes[at + 4] === 4) {
      bytes[at + 10] = number;
      return bytes;
    }
    at += bytes.readUInt32BE(at);
  }
  throw new Error("the fixture has no §4");
}

/** The fixture's one GRIB1 message, ECMWF local definition 1, as ensemble
 *  member `number`. It is committed as member 0 with a 52-octet PDS, which
 *  follows the 8-octet indicator section; PDS octet 50 is the perturbation
 *  number (`grib_dump -O`, eccodes 2.34.1). */
function member(number: number): Buffer {
  const bytes = fs.readFileSync(
    path.join(extensionPath(), "src", "test", "fixtures", "spectral_simple_t63.grib1"),
  );
  assert.strictEqual(bytes.readUIntBE(8, 3), 52, "a 52-octet PDS");
  bytes[8 + 49] = number;
  return bytes;
}

/** The labels the panel lists `bytes`' messages under. */
function labels(bytes: Buffer): string[] {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const h = native.open(bytes);
  return Array.from({ length: h.count() }, (_, i) => gribFieldLabel(h.message(i)));
}

/** The heading the panel draws message `index` of `bytes` under. */
function heading(bytes: Buffer, index: number): string {
  const native = loadNative();
  assert.ok(native, "native binding required");
  const m = native.open(bytes).message(index);
  return `Message ${index} — ${m.parameter}` + (m.units ? ` (${m.units})` : "");
}

/** A GRIB file in a temporary folder, opened in the editor with a render
 *  panel on message `index`, which can then change on disk under the panel. */
async function changingFile(first: Buffer, index: number, name = "changing.grib2"): Promise<{
  panel: RealPanel;
  change(bytes: Buffer): Promise<void>;
  done(): void;
}> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-959-"));
  const file = path.join(dir, name);
  fs.writeFileSync(file, first);
  let editor: Editor = await openEditor(p, file);
  const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: index }));
  return {
    panel,
    // The editor closes, which drops its handle, the file changes, and the
    // editor opens it again.
    change: async (bytes) => {
      editor.close();
      fs.writeFileSync(file, bytes);
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

suite("A GRIB render panel whose file changed", function () {
  this.timeout(60_000);

  test("draws its message where the changed file now holds it", async () => {
    const before = Buffer.concat([field(0), field(2)]);
    const after = Buffer.concat([field(2), field(0)]);
    const { panel, change, done } = await changingFile(before, 0);
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      assert.strictEqual(opened.titleLine, heading(before, 0));

      await change(after);
      let from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, 1, "temperature, now message 1");
      assert.strictEqual(reply.titleLine, heading(after, 1), "under its own heading");
      const state = await panel.drive([]);
      assert.strictEqual(state.title, heading(after, 1));
      assert.strictEqual(state.messageOffers, null, "nothing to pick");

      // A click reads the same message.
      from = panel.sent.length;
      await panel.probe();
      const probed = await panel.waitSent(isType("probeResult"), from, "the probe's answer");
      assert.strictEqual(probed.messageIndex, 1);
      assert.ok(probed.result, "the click lands on the grid");
    } finally {
      done();
    }
  });

  test("refuses when its message is gone, and offers the new file's messages", async () => {
    const before = Buffer.concat([field(0), field(2)]);
    // Message 0 is now another field, and temperature is in no message.
    const after = Buffer.concat([field(2), field(3)]);
    const { panel, change, done } = await changingFile(before, 0);
    try {
      const opened = await panel.waitSent(isType("gridReady"), 0, "the first render");
      const shownDrawn = (await panel.drive([])).drawnControlsShown;
      for (const key of ["frame", "zonal", "flipY", "colormap", "range", "overlay"] as const) {
        assert.strictEqual(shownDrawn[key], true, `${key} is on screen while the message is drawn`);
      }

      await change(after);
      let from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.strictEqual(reply.error, MESSAGE_GONE);
      assert.deepStrictEqual(reply.pickMessages.map((m: Msg) => m.label), labels(after));
      let state = await panel.drive([]);
      assert.strictEqual(state.status, `Error: ${MESSAGE_GONE}`);
      assert.deepStrictEqual(state.messageOffers, labels(after));
      assert.strictEqual(state.messagePicked, "", "nothing is chosen");
      assert.notStrictEqual(state.title, opened.titleLine, "the old heading is gone");
      // Compare and the arrows go while nothing is drawn: their pickers list
      // the old file's messages, and nothing they would combine is on screen
      // (#1237).
      assert.strictEqual(state.compareShown, false, "Compare is put away");
      assert.strictEqual(state.controlShown.arrows, false, "and so are the arrows");
      assert.strictEqual(state.canvasPainted, false, "the old picture is gone");
      // With its frame, the Zonal average box, Flip Y and the Colors, Color
      // Range and Overlay rows: nothing they act on is drawn (#1426).
      assertNothingActs(state, "No message drawn");

      // Until a message is picked, no control asks for anything, and nothing
      // asked for is served.
      const asked = panel.received.length;
      from = panel.sent.length;
      await panel.drive([
        { id: "flip-y", checked: false },
        { id: "overlay-coastlines", checked: true },
        { id: "zonal-toggle", checked: true },
      ]);
      assert.deepStrictEqual(
        panel.received.slice(asked).filter((m) => /Request$/.test(m.type)).map((m) => m.type),
        [],
        "no request before a message is picked",
      );
      await panel.drive([]);
      assert.deepStrictEqual(
        panel.sent.slice(from).filter((m) => m.type !== "test:drive").map((m) => m.type),
        [],
      );

      // Written again meanwhile, as an imported colormap writes every panel,
      // the page opens on the picker, not on the old heading, and still draws
      // nothing.
      from = panel.sent.length;
      panel.rebuild();
      assert.ok(
        panel.html().includes(`<h1 id="title-line">${NO_MESSAGE_HEADING}</h1>`),
        "the page is written with no message's heading",
      );
      assert.ok(!panel.html().includes(opened.titleLine), "and not the old one");
      await panel.waitSent(isType("gridError"), from, "the refusal, again");
      state = await panel.drive([]);
      assert.deepStrictEqual(state.messageOffers, labels(after));
      assert.strictEqual(state.title, NO_MESSAGE_HEADING);
      assert.strictEqual(state.compareShown, false, "Compare stays put away");
      assert.strictEqual(state.controlShown.arrows, false);
      assertNothingActs(state, "No message drawn, written again");
      assert.ok(!panel.sent.slice(from).some(isType("gridReady")), "nothing drawn");
      // Shown again after its tab was hidden, likewise (#1237).
      from = panel.sent.length;
      await panel.hideShow();
      await panel.waitSent(isType("gridError"), from, "the refusal after a hide and show");
      state = await panel.drive([]);
      assert.strictEqual(state.title, NO_MESSAGE_HEADING);
      assert.deepStrictEqual(state.messageOffers, labels(after));
      assert.strictEqual(state.compareShown, false);
      assert.ok(!panel.sent.slice(from).some(isType("gridReady")), "nothing drawn after a hide and show");

      // Picking one of the new file's messages draws it under its heading.
      from = panel.sent.length;
      await panel.drive([{ id: "message-pick", value: 1 }]);
      const drawn = await panel.waitSent(drawnOrRefused, from, "the picked message");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.strictEqual(drawn.messageIndex, 1);
      assert.strictEqual(drawn.titleLine, heading(after, 1));
      state = await panel.drive([]);
      assert.strictEqual(state.messageOffers, null, "the picker goes");
      assert.strictEqual(state.title, heading(after, 1));
      assert.strictEqual(state.compareShown, true, "Compare is back");
      assert.strictEqual(state.controlShown.arrows, true, "and so are the arrows");
      // And everything that acts on a picture, as it was (#1426).
      assert.deepStrictEqual(state.drawnControlsShown, shownDrawn, "every control is back");
    } finally {
      done();
    }
  });

  test("refuses when the changed file holds its message twice", async () => {
    const before = Buffer.concat([field(0), field(2)]);
    const after = Buffer.concat([field(0), field(0)]);
    const { panel, change, done } = await changingFile(before, 0);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(after);
      const from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.strictEqual(reply.error, MESSAGE_GONE);
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.messageOffers, labels(after));
    } finally {
      done();
    }
  });

  // GRIB1 has no ensemble octets of its own: a centre writes the member in its
  // PDS extension. A file that held member 3 and now holds member 5 has one
  // candidate, which used to match and be drawn under member 3's heading
  // (#994).
  test("refuses when a GRIB1 file now holds another ensemble member", async () => {
    const after = member(5);
    const { panel, change, done } = await changingFile(member(3), 0, "changing.grib1");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(after);
      const from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(
        reply.type,
        "gridError",
        `drew message ${reply.messageIndex} titled "${reply.titleLine}" instead of refusing`,
      );
      assert.strictEqual(reply.error, MESSAGE_GONE);
      const state = await panel.drive([]);
      assert.deepStrictEqual(state.messageOffers, labels(after));
    } finally {
      done();
    }
  });

  // The control for the refusal above: a GRIB1 member that moved is found by
  // the same identity, and drawn where it now sits.
  test("draws a GRIB1 ensemble member where the changed file now holds it", async () => {
    const after = Buffer.concat([member(5), member(3)]);
    const { panel, change, done } = await changingFile(member(3), 0, "changing.grib1");
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(after);
      const from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after the change");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, 1, "member 3, now message 1");
      const state = await panel.drive([]);
      assert.strictEqual(state.messageOffers, null, "nothing to pick");
    } finally {
      done();
    }
  });

  // Closing the editor and opening the same file again brings a new handle on
  // the same bytes. The messages are where they were, so the panel draws on,
  // even where the file holds a message twice.
  test("draws on when the file opened again has not changed", async () => {
    const same = Buffer.concat([field(0), field(0)]);
    const { panel, change, done } = await changingFile(same, 1);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      await change(same);
      const from = panel.sent.length;
      await panel.drive([{ id: "flip-y", checked: true }]);
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after reopening");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, 1);
    } finally {
      done();
    }
  });

  // Field B of a difference map is named by index too. A page written again
  // for the changed file finds it by identity; one the file no longer holds is
  // left unchosen, so nothing is compared against a field nobody picked.
  test("finds Field B by identity, and drops one that is gone", async () => {
    const first = Buffer.concat([field(0), field(2), field(3)]);
    const moved = Buffer.concat([field(3), field(0), field(2)]);
    const gone = Buffer.concat([field(0), field(2)]);
    const { panel, change, done } = await changingFile(first, 0);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 2 },
      ]);
      await panel.waitSent(isType("gridReady"), from, "the difference map");

      await change(moved);
      let asked = panel.received.length;
      from = panel.sent.length;
      panel.rebuild();
      let reply = await panel.waitSent(drawnOrRefused, from, "the render after the move");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, 1, "Field A, now message 1");
      let ready = panel.received.slice(asked).filter(isType("ready")).pop();
      assert.deepStrictEqual(ready?.compare, { op: "a_minus_b", messageIndexB: 0 }, "Field B, now message 0");
      let state = await panel.drive([]);
      assert.strictEqual(state.compareFieldB, "0");

      await change(gone);
      asked = panel.received.length;
      from = panel.sent.length;
      panel.rebuild();
      reply = await panel.waitSent(drawnOrRefused, from, "the render after Field B went");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.messageIndex, 0, "Field A, message 0 again");
      ready = panel.received.slice(asked).filter(isType("ready")).pop();
      assert.strictEqual(ready?.compare, undefined, "no Field B to compare with");
      state = await panel.drive([]);
      assert.strictEqual(state.compareFieldB, "", "Field B is unchosen");
      // Compare is off with Field B to pick, and the single field drawn is not
      // labelled a difference (#1230).
      assert.strictEqual((await panel.controls())["compare-op"], "", "Operation is off");
      assert.ok(!state.status.startsWith("A − B"), `the summary "${state.status}" names no operation`);
    } finally {
      done();
    }
  });

  // The §23 case (#1230): the panel's own message is gone, so it offers the new
  // file's to pick from, and the page is written again for the one picked.
  // Compare comes back with it only where Field B is found again by identity.
  test("after a re-pick, Compare finds Field B by identity or is off", async () => {
    const first = Buffer.concat([field(0), field(2), field(3)]);
    const { panel, change, done } = await changingFile(first, 0);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
        { id: "overlay-vectors", checked: true },
        { id: "vector-field-v", value: 2 },
      ]);
      const compared = await panel.waitSent(
        (m) => isType("gridReady")(m) && m.compare?.messageIndexB === 1,
        from,
        "the difference map",
      );
      assert.match((await panel.drive([])).status, /^A − B · /, `${compared.messageIndex}`);

      // Field A (temperature) is gone; Field B (parameter 2) is message 0 now,
      // and the V component (parameter 3) is gone too.
      const kept = Buffer.concat([field(2), field(4)]);
      await change(kept);
      from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridError"), from, "the refusal");
      let asked = panel.received.length;
      from = panel.sent.length;
      await panel.drive([{ id: "message-pick", value: 1 }]);
      let drawn = await panel.waitSent(drawnOrRefused, from, "the picked message");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.strictEqual(drawn.messageIndex, 1);
      assert.deepStrictEqual(drawn.compare, { op: "a_minus_b", messageIndexB: 0 }, "Field B, found by identity");
      let state = await panel.drive([]);
      assert.strictEqual(state.compareFieldB, "0");
      assert.match(state.status, /^A − B · /, "and the difference is labelled as one");
      let controls = await panel.controls();
      assert.strictEqual(controls["compare-op"], "a_minus_b");
      assert.strictEqual(controls["overlay-vectors"], false, "the arrows go with their V component");
      assert.strictEqual(controls["vector-field-v"], "", "which is left to pick");
      assert.ok(
        !panel.received.slice(asked).some(isType("vectorRequest")),
        "no arrows asked for against a V component nobody picked",
      );

      // Field A and Field B are both gone this time.
      const gone = Buffer.concat([field(5), field(6)]);
      await change(gone);
      from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(isType("gridError"), from, "the refusal");
      // Nothing is drawn, so nothing is exported as if it were.
      asked = panel.received.length;
      state = await panel.drive([{ id: "export-csv", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportCsv")), "no CSV of a message nobody picked");
      assert.match(state.exportStatus, /^Pick a message to draw before exporting CSV/);
      asked = panel.received.length;
      state = await panel.drive([{ id: "export-png", click: true }]);
      assert.ok(!panel.received.slice(asked).some(isType("exportPng")), "nor a PNG");
      assert.match(state.exportStatus, /^Pick a message to draw before exporting a PNG/);

      asked = panel.received.length;
      from = panel.sent.length;
      await panel.drive([{ id: "message-pick", value: 0 }]);
      drawn = await panel.waitSent(drawnOrRefused, from, "the picked message");
      assert.strictEqual(drawn.type, "gridReady", `${drawn.error ?? ""}`);
      assert.strictEqual(drawn.messageIndex, 0);
      assert.strictEqual(drawn.compare, null, "a single field");
      const ready = panel.received.slice(asked).filter(isType("ready")).pop();
      assert.strictEqual(ready?.compare, undefined, "no Field B asked for");
      state = await panel.drive([]);
      controls = await panel.controls();
      assert.strictEqual(controls["compare-op"], "", "Compare is off");
      assert.strictEqual(state.compareFieldB, "", "with Field B to pick");
      assert.ok(!state.status.startsWith("A − B"), `the summary "${state.status}" names no operation`);
    } finally {
      done();
    }
  });

  // Compare turned off for want of Field B is saved off (#1230): a later file
  // that holds Field B again does not turn it back on behind the user's back.
  test("Compare turned off for a missing Field B stays off when Field B comes back", async () => {
    const first = Buffer.concat([field(0), field(2), field(3)]);
    const gone = Buffer.concat([field(0), field(2)]);
    const { panel, change, done } = await changingFile(first, 0);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 2 },
      ]);
      await panel.waitSent((m) => isType("gridReady")(m) && m.compare?.messageIndexB === 2, from, "the difference map");

      await change(gone);
      from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(drawnOrRefused, from, "the render after Field B went");
      assert.strictEqual((await panel.controls())["compare-op"], "", "Compare is off");

      // Written again as it is (an imported colormap), then for the first file,
      // which holds Field B once more.
      from = panel.sent.length;
      panel.rebuild();
      await panel.waitSent(drawnOrRefused, from, "the render after a rebuild");
      await change(first);
      const asked = panel.received.length;
      from = panel.sent.length;
      panel.rebuild();
      const reply = await panel.waitSent(drawnOrRefused, from, "the render after Field B came back");
      assert.strictEqual(reply.type, "gridReady", `${reply.error ?? ""}`);
      assert.strictEqual(reply.compare, null, "a single field");
      const ready = panel.received.slice(asked).filter(isType("ready")).pop();
      assert.strictEqual(ready?.compare, undefined, "no Field B asked for");
      assert.strictEqual((await panel.controls())["compare-op"], "", "Compare stays off");
    } finally {
      done();
    }
  });

  // The summary names the operation the render drew with, which the reply
  // carries, not whatever the Operation picker reads when it lands (#1230).
  test("the summary names the operation drawn, not the picker's", async () => {
    const { panel, done } = await changingFile(Buffer.concat([field(0), field(2)]), 0);
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      const gate = panel.hold(isType("gridReady"));
      const asked = panel.received.length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      await waitUntil(() => gate.held.find((m: Msg) => m.compare?.op === "a_minus_b"), "the difference, held back");
      // The picker goes off before the difference lands.
      await panel.drive([{ id: "compare-op", value: "" }]);
      await waitUntil(
        () => panel.received.slice(asked).find((m: Msg) => m.type === "rerenderRequest" && m.compare === undefined),
        "the single render asked for",
      );
      await gate.deliver((m) => m.compare?.op === "a_minus_b");
      let state = await panel.drive([]);
      assert.strictEqual((await panel.controls())["compare-op"], "", "the picker reads off");
      assert.match(state.status, /^A − B · /, "the difference on screen is labelled as one");

      await gate.release();
      await waitUntil(() => (panel.sent.filter(isType("gridReady")).pop()?.compare === null ? true : undefined), "the single field");
      state = await panel.drive([]);
      assert.ok(!state.status.startsWith("A − B"), `the single field's summary "${state.status}" names no operation`);
    } finally {
      done();
    }
  });
});
