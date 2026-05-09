import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";

const EXT_ID = "fieldglass.fieldglass";
const FIXTURE_NAME = "cmc_wind_300_2010052400_p012.grib";

// PDS `p1` (forecast period) sits at message_offset + 8 (IS) + 18 (PDS p1).
// The fixture has a single message at byte_offset 0.
const P1_BYTE_OFFSET = 26;

async function activateExtension(): Promise<FieldglassApi> {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  if (!ext) {
    throw new Error(`extension ${EXT_ID} not found`);
  }
  return ext.activate();
}

function fixturePath(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  if (!ext) {
    throw new Error(`extension ${EXT_ID} not found`);
  }
  return path.join(ext.extensionPath, "src", "test", "fixtures", FIXTURE_NAME);
}

/** Copy the fixture into a fresh tmp file so save() doesn't mutate the source. */
function copyFixtureToTmp(): vscode.Uri {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-test-"));
  const dest = path.join(dir, FIXTURE_NAME);
  fs.copyFileSync(fixturePath(), dest);
  return vscode.Uri.file(dest);
}

async function openDoc(
  provider: FieldglassEditorProvider,
  uri: vscode.Uri
): Promise<FieldglassDocument> {
  return (await provider.openCustomDocument(
    uri,
    {} as vscode.CustomDocumentOpenContext,
    new vscode.CancellationTokenSource().token
  )) as FieldglassDocument;
}

suite("FieldglassEditorProvider edit pipeline", () => {
  let api: FieldglassApi;
  let provider: FieldglassEditorProvider;

  suiteSetup(async () => {
    api = await activateExtension();
    provider = api.provider;
  });

  test("open populates bytes that match the fixture on disk", async () => {
    const uri = copyFixtureToTmp();
    const doc = await openDoc(provider, uri);
    const onDisk = fs.readFileSync(uri.fsPath);
    assert.strictEqual(doc.bytes.length, onDisk.length, "byte length matches");
    assert.deepStrictEqual(
      Buffer.from(doc.bytes),
      onDisk,
      "byte contents match disk"
    );
  });

  test("applyP1Edit patches the byte and fires onDidChangeCustomDocument", async () => {
    const uri = copyFixtureToTmp();
    const doc = await openDoc(provider, uri);
    const originalP1 = doc.bytes[P1_BYTE_OFFSET];
    const newP1 = (originalP1 + 5) & 0xff;

    let captured: vscode.CustomDocumentEditEvent<FieldglassDocument> | undefined;
    const sub = provider.onDidChangeCustomDocument((evt) => {
      if (evt.document === doc) captured = evt;
    });

    try {
      provider.applyP1Edit(doc, 0, newP1);
    } finally {
      sub.dispose();
    }

    assert.strictEqual(doc.bytes[P1_BYTE_OFFSET], newP1, "byte at offset patched");
    assert.ok(captured, "edit event fired");
    const label = captured!.label;
    assert.ok(typeof label === "string", "edit event has a label");
    assert.match(label!, /forecast period/i, "label mentions forecast period");
    assert.match(label!, /message 0/, "label mentions the message index");
  });

  test("undo restores prior bytes; redo reapplies the edit", async () => {
    const uri = copyFixtureToTmp();
    const doc = await openDoc(provider, uri);
    const originalP1 = doc.bytes[P1_BYTE_OFFSET];
    const newP1 = (originalP1 + 17) & 0xff;

    let captured: vscode.CustomDocumentEditEvent<FieldglassDocument> | undefined;
    const sub = provider.onDidChangeCustomDocument((evt) => {
      if (evt.document === doc) captured = evt;
    });

    try {
      provider.applyP1Edit(doc, 0, newP1);
    } finally {
      sub.dispose();
    }

    assert.ok(captured, "edit event fired");
    assert.strictEqual(doc.bytes[P1_BYTE_OFFSET], newP1, "byte starts at new value");

    await Promise.resolve(captured!.undo());
    assert.strictEqual(doc.bytes[P1_BYTE_OFFSET], originalP1, "undo restores original");

    await Promise.resolve(captured!.redo());
    assert.strictEqual(doc.bytes[P1_BYTE_OFFSET], newP1, "redo reapplies the edit");
  });

  test("saveCustomDocument writes the patched bytes to disk", async () => {
    const uri = copyFixtureToTmp();
    const doc = await openDoc(provider, uri);
    const originalP1 = doc.bytes[P1_BYTE_OFFSET];
    const newP1 = (originalP1 + 1) & 0xff;

    provider.applyP1Edit(doc, 0, newP1);
    await provider.saveCustomDocument(doc, new vscode.CancellationTokenSource().token);

    const onDisk = fs.readFileSync(uri.fsPath);
    assert.strictEqual(onDisk[P1_BYTE_OFFSET], newP1, "disk byte matches edited value");
  });

  test("revertCustomDocument restores bytes from disk", async () => {
    const uri = copyFixtureToTmp();
    const doc = await openDoc(provider, uri);
    const original = Buffer.from(doc.bytes);

    provider.applyP1Edit(doc, 0, (original[P1_BYTE_OFFSET] + 3) & 0xff);
    provider.applyP1Edit(doc, 0, (original[P1_BYTE_OFFSET] + 9) & 0xff);
    assert.notStrictEqual(
      doc.bytes[P1_BYTE_OFFSET],
      original[P1_BYTE_OFFSET],
      "byte was changed before revert"
    );

    await provider.revertCustomDocument(doc, new vscode.CancellationTokenSource().token);

    assert.deepStrictEqual(
      Buffer.from(doc.bytes),
      original,
      "revert restores full original byte buffer"
    );
  });
});
