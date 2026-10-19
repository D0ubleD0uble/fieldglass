#!/usr/bin/env node
// Headless pre-flight for the manual sample corpus (see samples/README.md).
// Loads the same native module the extension uses and runs the full render
// path (decode -> reproject -> colormap range) on every file in samples/ (or
// the directory given), so a decode/render failure surfaces before you open
// anything in the UI.
//
//   node tools/preflight_samples.js [dir]
//
// Requires the native module built into extension/bin (see samples/README.md).
//
// `preflightFile(bytes, label)` is exported so the extension's test suite runs
// the NetCDF path over a committed fixture (preflight-samples.test.ts): this
// script is plain JS that tsc and CI never see, so a renamed addon field would
// otherwise only show up as a false RENDER FAILED in a manual pass (#1209).
const fs = require('fs');
const path = require('path');
const REPO = path.resolve(__dirname, '..');
const m = require(path.join(REPO, 'extension/bin/fieldglass.linux-x64-gnu.node'));
let out = console.log;
const opts = { projection: 'equirectangular', resampling: 'nearest', flipY: false };

function grib(buf, label) {
  const h = m.open(buf);
  const msgs = Array.from({ length: h.count() }, (_, i) => h.message(i));
  const i = msgs.findIndex((x) => x.reprojectable);
  const idx = i >= 0 ? i : 0;
  const mm = msgs[idx];
  let r;
  try {
    r = h.renderGrid(idx, opts);
  } catch (e) {
    return out(`  ${label}: ${msgs.length} msgs; grid=${gridLabel(mm)} packing=${mm.packing}  RENDER FAILED: ${e.message}`);
  }
  out(`  ${label}: ${msgs.length} msgs | msg#${idx} ${mm.parameter ?? mm.abbreviation} | grid=${gridLabel(mm)} | packing=${mm.packing} | reproj=${mm.reprojectable}`);
  out(`         render: ${r.width}x${r.height} | ${r.projectionSummary} | ${range(r)}`);
}

function gridLabel(mm) {
  const g = mm.grid;
  if (g == null) return mm.truncation ? 'spectral' : 'none';
  return g.ni == null ? g.kind : `${g.kind} ${g.ni}x${g.nj}`;
}

// `usedMin`/`usedMax` are `null` when every cell is masked (#871).
function range(r) {
  return r.usedMin == null ? 'no values' : `range ${r.usedMin.toFixed(2)}..${r.usedMax.toFixed(2)}`;
}

function netcdf(buf, label) {
  const h = m.open(buf);
  const vars = h.variables();
  if (!vars.length) {
    const d = h.metadata();
    return out(`  ${label}: ${d.backingLabel} | 0 renderable vars${d.note ? ` | note: ${d.note}` : ''}`);
  }
  const v = vars.find((x) => x.detectedYDim != null && x.detectedXDim != null) || vars[0];
  out(`  ${label}: ${vars.length} renderable vars | var '${v.name}' dims=[${v.dims.map((d) => `${d.name}:${d.length}`).join(',')}] Y=${v.detectedYDim} X=${v.detectedXDim}`);
  if (v.detectedYDim == null || v.detectedXDim == null) return out('         (no CF axes detected — needs manual axis pick)');
  const slice = v.dims.map(() => 0);
  try {
    const r = h.renderGrid(
      { variable: v.index, yDim: v.detectedYDim, xDim: v.detectedXDim, sliceIndices: slice },
      opts,
    );
    out(`         render: ${r.width}x${r.height} | ${r.projectionSummary} | ${range(r)}`);
  } catch (e) {
    out(`         RENDER FAILED: ${e.message}`);
  }
}

/** Preflight one file; returns the lines it would print. */
function preflightFile(buf, label) {
  const lines = [];
  const prev = out;
  out = (l) => lines.push(l);
  try {
    const fmt = m.detectBytes(buf);
    try {
      if (fmt === 'grib2' || fmt === 'grib1') grib(buf, `${label} [${fmt}]`);
      else if (fmt === 'netcdf') netcdf(buf, `${label} [${fmt}]`);
      else out(`  ${label}: UNKNOWN format (${fmt})`);
    } catch (e) {
      out(`  ${label} [${fmt}]: OPEN FAILED: ${e.message}`);
    }
  } finally {
    out = prev;
  }
  return lines;
}

function main() {
  const dir = path.resolve(REPO, process.argv[2] || 'samples');
  const files = fs.readdirSync(dir).filter((f) => /\.(grib2|grib1|grib|grb2|nc|nc4)$/.test(f)).sort();
  if (!files.length) {
    console.log(`No sample files in ${path.relative(REPO, dir)}/ - run tools/fetch_samples.sh first.`);
    return;
  }
  for (const f of files) {
    for (const l of preflightFile(fs.readFileSync(path.join(dir, f)), f)) console.log(l);
  }
}

if (require.main === module) main();
module.exports = { preflightFile };
