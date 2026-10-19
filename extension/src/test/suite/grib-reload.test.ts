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
// different field on the same grid, and any two can be compared.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { loadNative } from "../../native";
import { gribFieldLabel, MESSAGE_GONE } from "../../provider";
import { NO_MESSAGE_HEADING } from "../../render-panel";
import {
  type Editor,
  extensionPath,
  isType,
  type Msg,
  openEditor,
  openReal,
  provider,
  type RealPanel,
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

/** A GRIB2 file in a temporary folder, opened in the editor with a render
 *  panel on message `index`, which can then change on disk under the panel. */
async function changingFile(first: Buffer, index: number): Promise<{
  panel: RealPanel;
  change(bytes: Buffer): Promise<void>;
  done(): void;
}> {
  const p = await provider();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-959-"));
  const file = path.join(dir, "changing.grib2");
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
      assert.ok(!panel.sent.slice(from).some(isType("gridReady")), "nothing drawn");

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
    } finally {
      done();
    }
  });
});
