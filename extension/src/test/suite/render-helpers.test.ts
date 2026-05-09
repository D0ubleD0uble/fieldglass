import * as assert from "assert";

import {
  VIRIDIS_LUT,
  viridis,
  minMaxIgnoringMask,
  paintGridRgba,
} from "../../render-helpers";

suite("render-helpers", () => {
  test("VIRIDIS_LUT has 256 RGB triples (768 bytes)", () => {
    assert.strictEqual(VIRIDIS_LUT.length, 768);
    // sanity: first entry is the canonical viridis dark-purple,
    // last entry is the canonical viridis bright-yellow.
    const r0 = VIRIDIS_LUT[0], g0 = VIRIDIS_LUT[1], b0 = VIRIDIS_LUT[2];
    assert.ok(r0 < 100 && g0 < 50 && b0 > 50, `first viridis stop unexpected: ${r0},${g0},${b0}`);
    const rL = VIRIDIS_LUT[765], gL = VIRIDIS_LUT[766], bL = VIRIDIS_LUT[767];
    assert.ok(rL > 200 && gL > 200 && bL < 80, `last viridis stop unexpected: ${rL},${gL},${bL}`);
  });

  test("viridis() clamps out-of-range t to [0, 1]", () => {
    const lo = viridis(-5);
    const hi = viridis(5);
    assert.deepStrictEqual(lo, [VIRIDIS_LUT[0], VIRIDIS_LUT[1], VIRIDIS_LUT[2]]);
    assert.deepStrictEqual(hi, [VIRIDIS_LUT[765], VIRIDIS_LUT[766], VIRIDIS_LUT[767]]);
  });

  test("minMaxIgnoringMask skips nulls and non-finite numbers", () => {
    const out = minMaxIgnoringMask([null, 1, 2, null, 3, Number.NaN, -1, Number.POSITIVE_INFINITY]);
    assert.ok(out);
    assert.strictEqual(out!.min, -1);
    assert.strictEqual(out!.max, 3);
  });

  test("minMaxIgnoringMask returns null when every entry is masked", () => {
    assert.strictEqual(minMaxIgnoringMask([null, null]), null);
    assert.strictEqual(minMaxIgnoringMask([]), null);
  });

  test("paintGridRgba writes alpha=0 for masked cells and 255 elsewhere", () => {
    const buf = paintGridRgba([0, null, 1, 0.5], 2, 2, 0, 1);
    assert.strictEqual(buf.length, 16);
    // Cell 0 (value 0, t=0) → opaque
    assert.strictEqual(buf[3], 255);
    // Cell 1 (masked) → transparent
    assert.strictEqual(buf[4], 0);
    assert.strictEqual(buf[5], 0);
    assert.strictEqual(buf[6], 0);
    assert.strictEqual(buf[7], 0);
    // Cell 2 (value 1, t=1) → opaque, matches LUT tail
    assert.strictEqual(buf[11], 255);
    assert.strictEqual(buf[8], VIRIDIS_LUT[765]);
    assert.strictEqual(buf[9], VIRIDIS_LUT[766]);
    assert.strictEqual(buf[10], VIRIDIS_LUT[767]);
  });

  test("paintGridRgba with constant field paints LUT index 0 everywhere present", () => {
    const buf = paintGridRgba([7, 7, 7, 7], 2, 2, 7, 7);
    for (let i = 0; i < 4; i++) {
      assert.strictEqual(buf[i * 4 + 0], VIRIDIS_LUT[0]);
      assert.strictEqual(buf[i * 4 + 1], VIRIDIS_LUT[1]);
      assert.strictEqual(buf[i * 4 + 2], VIRIDIS_LUT[2]);
      assert.strictEqual(buf[i * 4 + 3], 255);
    }
  });

  test("paintGridRgba returns empty buffer for non-positive dims", () => {
    assert.strictEqual(paintGridRgba([], 0, 0, 0, 1).length, 0);
    assert.strictEqual(paintGridRgba([1, 2], -1, 1, 0, 1).length, 0);
  });
});
