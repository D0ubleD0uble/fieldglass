// Issue #1087: native napi vs napi-on-WASI vs wasm-bindgen, decode time under Node.
// Each timed iteration opens a fresh handle (napi memoises decoded values per handle)
// and decodes message 0. Median of N after one warm-up.
//   node bench-node.mjs <native .node> <wasi .cjs> <wasm-bindgen web dir>
import { createRequire } from 'node:module';
import { readFileSync, copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';

const require = createRequire(import.meta.url);
const [nativePath, wasiPath, wbDir] = process.argv.slice(2);
const FIX = process.env.FIXTURES;
if (!FIX) throw new Error('set FIXTURES to crates/fieldglass-grib2/tests/fixtures');
const FILES = ['hrrr_complex_spd_lambert.grib2', 'ecmwf_ccsds_latlon.grib2', 'rap_jpeg2000_lambert.grib2'];
const N = 7;

function median(xs) { const s = [...xs].sort((a, b) => a - b); return s[s.length >> 1]; }
function time(fn) {
  fn();
  const ts = [];
  let out;
  for (let i = 0; i < N; i++) { const t = performance.now(); out = fn(); ts.push(performance.now() - t); }
  return { ms: median(ts), out };
}
function summary(r) {
  const values = r.values, mask = r.mask;
  let n = 0, sum = 0;
  for (let i = 0; i < values.length; i++) if (mask[i]) { n++; sum += values[i]; }
  return `${values.length} pts, ${n} valid, mean ${(sum / n).toPrecision(8)}`;
}

let t0 = performance.now();
const native = require(nativePath);
const nativeLoad = performance.now() - t0;
t0 = performance.now();
const wasi = require(wasiPath);
const wasiLoad = performance.now() - t0;
t0 = performance.now();
const wb = await import(join(wbDir, 'fieldglass_wasm.js'));
wb.initSync({ module: readFileSync(join(wbDir, 'fieldglass_wasm_bg.wasm')) });
const wbLoad = performance.now() - t0;
console.log(`load ms: native ${nativeLoad.toFixed(1)}, napi-wasi ${wasiLoad.toFixed(1)}, wasm-bindgen ${wbLoad.toFixed(1)}`);

for (const f of FILES) {
  const bytes = readFileSync(join(FIX, f));
  const n = time(() => native.Grib2Handle.fromBytes(bytes).decodeGrid(0));
  const w = time(() => wasi.Grib2Handle.fromBytes(bytes).decodeGrid(0));
  const b = time(() => { const h = wb.open(new Uint8Array(bytes)); const f = h.decode(0, { dtype: 'f64' }); const r = { values: f.values(), mask: f.mask() }; f.free?.(); h.free?.(); return r; });
  console.log(`${f}`);
  console.log(`  native      ${n.ms.toFixed(1)} ms  ${summary(n.out)}`);
  console.log(`  napi-wasi   ${w.ms.toFixed(1)} ms  ${summary(w.out)}  (${(w.ms / n.ms).toFixed(2)}x native)`);
  console.log(`  wasm-bindgen ${b.ms.toFixed(1)} ms  ${summary(b.out)}  (${(b.ms / n.ms).toFixed(2)}x native)`);
}
process.exit(0);
