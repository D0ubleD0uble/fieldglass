// Contours follow a Compare change under a manual range (#1149).
//
// `contourKey()` left out the compare rider that `requestContours` sends, and
// a manual range holds the used min and max still, so turning Compare on, or
// picking another Field B, drew the difference map under Field A's isolines.
// Driven through the real panel and the real provider.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { extensionPath, isType, type Msg, openEditor, openReal, provider } from "./real-panel";

const FIXTURE = () => path.join(extensionPath(), "src", "test", "fixtures", "regular_latlon_surface.grib2");

suite("Contours follow Compare (#1149)", () => {
  test("under a manual range, Compare on and a new Field B each ask for contours with the rider", async () => {
    const p = await provider();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fieldglass-1149-"));
    const file = path.join(dir, "triple.grib2");
    assert.ok(fs.existsSync(FIXTURE()), "fixture missing");
    const bytes = fs.readFileSync(FIXTURE());
    fs.writeFileSync(file, Buffer.concat([bytes, bytes, bytes]));
    const editor = await openEditor(p, file);
    const panel = await openReal(p, () => editor.send({ type: "decodeGrid", messageIndex: 0 }));
    try {
      await panel.waitSent(isType("gridReady"), 0, "the first render");
      let from = panel.sent.length;
      await panel.drive([
        { selector: 'input[name="range-mode"][value="manual"]', checked: true },
        { id: "range-min", value: 200 },
        { id: "range-max", value: 320 },
      ]);
      await panel.waitSent(isType("gridReady"), from, "the manual range");
      from = panel.sent.length;
      await panel.drive([{ id: "overlay-contours", checked: true }]);
      await panel.waitSent(isType("contourReady"), from, "the contours");

      const contourRequests = (): Msg[] => panel.received.filter((m: Msg) => m && m.type === "contourRequest");
      const waitRequest = async (n: number, why: string): Promise<Msg> => {
        const deadline = Date.now() + 10_000;
        while (contourRequests().length <= n) {
          assert.ok(Date.now() < deadline, `no contourRequest for ${why}`);
          await new Promise((r) => setTimeout(r, 50));
        }
        return contourRequests()[n];
      };

      let n = contourRequests().length;
      await panel.drive([
        { id: "compare-op", value: "a_minus_b" },
        { id: "compare-field-b", value: 1 },
      ]);
      const on = await waitRequest(n, "Compare switched on");
      assert.deepStrictEqual(on.compare, { op: "a_minus_b", messageIndexB: 1 });

      n = contourRequests().length;
      await panel.drive([{ id: "compare-field-b", value: 2 }]);
      const other = await waitRequest(n, "a changed Field B");
      assert.deepStrictEqual(other.compare, { op: "a_minus_b", messageIndexB: 2 });
    } finally {
      panel.dispose();
      editor.close();
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
