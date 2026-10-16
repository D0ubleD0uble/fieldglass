// Browser/Node probe for the non-threaded napi WASI build.
import { Buffer } from 'buffer';
import { load } from './nothreads.mjs';

const isNode = typeof process !== 'undefined' && process.versions?.node;
if (!globalThis.Buffer) globalThis.Buffer = Buffer;
const report = (r) => {
  const out = { where: isNode ? 'node' : (typeof window === 'undefined' ? 'worker' : 'main'), crossOriginIsolated: globalThis.crossOriginIsolated, ...r };
  if (isNode) console.log(JSON.stringify(out));
  else if (typeof window === 'undefined') postMessage(out);
  else { window.__result = out; document.title = 'done'; }
};
async function get(path) {
  if (isNode) { const fs = await import('node:fs'); return fs.readFileSync(new URL(path, 'file://' + process.argv[2] + '/')); }
  return new Uint8Array(await (await fetch(path)).arrayBuffer());
}
try {
  let t = performance.now();
  const mod = await load(await get('./fieldglass_napi_nothreads.wasm'));
  const loadMs = performance.now() - t;
  const bytes = Buffer.from(await get('./hrrr_complex_spd_lambert.grib2'));
  const ms = [];
  let g;
  for (let i = 0; i < 8; i++) { t = performance.now(); g = mod.Grib2Handle.fromBytes(bytes).decodeGrid(0); ms.push(performance.now() - t); }
  let sum = 0, n = 0;
  for (let i = 0; i < g.values.length; i++) if (g.mask[i]) { sum += g.values[i]; n++; }
  report({ ok: true, build: 'wasm32-wasip1 (no threads)', loadMs, decodeMs: ms.slice(1).sort((a, b) => a - b)[3], n, mean: sum / n });
} catch (e) {
  report({ ok: false, error: String(e && e.stack || e).split('\n')[0] });
}
