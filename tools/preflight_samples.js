#!/usr/bin/env node
// Headless pre-flight for the manual sample corpus (see samples/README.md).
// Loads the same native module the extension uses and runs the full render
// path (decode -> reproject -> colormap range) on every file in samples/, so a
// decode/render failure surfaces before you open anything in the UI.
//
//   node tools/preflight_samples.js
//
// Requires the native module built into extension/bin (see samples/README.md).
const fs = require('fs');
const path = require('path');
const REPO = path.resolve(__dirname, '..');
const m = require(path.join(REPO, 'extension/bin/fieldglass.linux-x64-gnu.node'));
const opts = { projection: 'equirectangular', resampling: 'nearest', flipY: false };

function grib(HandleName, buf, label) {
  const h = m[HandleName].fromBytes(buf);
  const msgs = h.messages();
  const i = msgs.findIndex((x) => x.reprojectable);
  const idx = i >= 0 ? i : 0;
  const mm = msgs[idx];
  let r;
  try {
    r = h.renderGrid(idx, opts);
  } catch (e) {
    return console.log(`  ${label}: ${msgs.length} msgs; grid=${mm.gridType} packing=${mm.packing}  RENDER FAILED: ${e.message}`);
  }
  console.log(`  ${label}: ${msgs.length} msgs | msg#${idx} ${mm.parameterName} | grid=${mm.gridType} ${mm.gridNi}x${mm.gridNj} | packing=${mm.packing} | reproj=${mm.reprojectable}`);
  console.log(`         render: ${r.width}x${r.height} | ${r.projectionSummary} | range ${r.usedMin.toFixed(2)}..${r.usedMax.toFixed(2)}`);
}

function netcdf(buf, label) {
  const h = m.NetcdfHandle.fromBytes(buf);
  const vars = h.variables();
  if (!vars.length) {
    const d = m.openNetcdf(buf);
    return console.log(`  ${label}: ${d.backingLabel} | 0 renderable vars${d.note ? ` | note: ${d.note}` : ''}`);
  }
  const v = vars.find((x) => x.detectedYDim != null && x.detectedXDim != null) || vars[0];
  console.log(`  ${label}: ${vars.length} renderable vars | var '${v.name}' dims=[${v.dims.map((d) => `${d.name}:${d.length}`).join(',')}] Y=${v.detectedYDim} X=${v.detectedXDim}`);
  if (v.detectedYDim == null || v.detectedXDim == null) return console.log('         (no CF axes detected — needs manual axis pick)');
  const slice = v.dims.map(() => 0);
  try {
    const r = h.renderSlice(v.variableIndex, v.detectedYDim, v.detectedXDim, slice, opts);
    console.log(`         render: ${r.width}x${r.height} | ${r.projectionSummary} | range ${r.usedMin.toFixed(2)}..${r.usedMax.toFixed(2)}`);
  } catch (e) {
    console.log(`         RENDER FAILED: ${e.message}`);
  }
}

const dir = path.join(REPO, 'samples');
const files = fs.readdirSync(dir).filter((f) => /\.(grib2|grib|grb2|nc|nc4)$/.test(f)).sort();
if (!files.length) {
  console.log('No sample files in samples/ — run tools/fetch_samples.sh first.');
  process.exit(0);
}
for (const f of files) {
  const buf = fs.readFileSync(path.join(dir, f));
  const fmt = m.detectBytes(buf);
  try {
    if (fmt === 'grib2') grib('Grib2Handle', buf, `${f} [${fmt}]`);
    else if (fmt === 'grib1') grib('Grib1Handle', buf, `${f} [${fmt}]`);
    else if (fmt === 'netcdf') netcdf(buf, `${f} [${fmt}]`);
    else console.log(`  ${f}: UNKNOWN format (${fmt})`);
  } catch (e) {
    console.log(`  ${f} [${fmt}]: OPEN FAILED: ${e.message}`);
  }
}
