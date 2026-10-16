// Issue #1087 browser probe: load the napi-rs WASI build, decode one GRIB2 message,
// and report whether the page was cross-origin isolated, in the main thread or a Worker.
const report = (r) => {
  const out = { where: typeof window === 'undefined' ? 'worker' : 'main', crossOriginIsolated: globalThis.crossOriginIsolated, ...r };
  if (typeof window === 'undefined') postMessage(out); else { window.__result = out; document.title = 'done'; }
};
async function run() {
  if (new URLSearchParams(globalThis.location.search).has('polyfill') || globalThis.__POLYFILL) {
    const { Buffer } = await import('buffer');
    globalThis.Buffer = Buffer;
  }
  let t = performance.now();
  let mod;
  try {
    mod = await import('./fieldglass.wasi-browser.js');
  } catch (e) {
    return report({ ok: false, stage: 'load', error: String(e && e.stack || e) });
  }
  const loadMs = performance.now() - t;
  try {
    const bytes = (globalThis.Buffer ? globalThis.Buffer.from : (x) => x)(new Uint8Array(await (await fetch('./hrrr_complex_spd_lambert.grib2')).arrayBuffer()));
    const ms = [];
    let g;
    for (let i = 0; i < 6; i++) { t = performance.now(); g = mod.Grib2Handle.fromBytes(bytes).decodeGrid(0); ms.push(performance.now() - t); }
    let sum = 0, n = 0;
    for (let i = 0; i < g.values.length; i++) if (g.mask[i]) { sum += g.values[i]; n++; }
    // What a texture upload would be handed: is the values array a view on shared wasm memory?
    const shared = typeof SharedArrayBuffer !== 'undefined' && g.values.buffer instanceof SharedArrayBuffer;
    report({ ok: true, loadMs, decodeMs: ms.slice(1).sort((a, b) => a - b)[2], n, mean: sum / n, valuesBufferIsShared: shared, valuesCtor: g.values.constructor.name });
  } catch (e) {
    report({ ok: false, stage: 'decode', error: String(e && e.stack || e) });
  }
}
run();
