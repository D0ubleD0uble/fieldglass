#!/usr/bin/env node
// Probe half of the probe-parity sweep (#1029). Run it through
// `tools/probe_parity_sweep.py`; it is split out only because the native
// addon is a Node module and the oracles (eccodes, xarray) are Python.
//
//   node tools/probe_parity_probe.js <file-or-zarr-dir> [--n 200] [--seed 1]
//        [--slices 3] [--addon path/to/fieldglass.linux-x64-gnu.node]
//        [--variable NAME | --list-variables]
//
// For every GRIB message and every 2-D NetCDF/Zarr slice it picks `--n`
// seeded pixels of the Equirectangular, nearest raster and reads each through
// the addon's `probePixel`: the same call the hover readout makes. It prints
// one JSON document on stdout: what each probe saw (lat, lon, value, gridI,
// gridJ) and what the file says about itself, for the Python side to check.
//
// The sample is seeded per item (seed + the item's label), so adding a file or
// a message to a directory does not move the points of another one.
'use strict';
const fs = require('fs');
const path = require('path');

const REPO = path.resolve(__dirname, '..');

function parseArgs(argv) {
  const a = { n: 200, seed: 1, slices: 3, addon: null, target: null, variable: null, listVariables: false };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    if (k === '--n') a.n = Number(argv[++i]);
    else if (k === '--seed') a.seed = Number(argv[++i]);
    else if (k === '--slices') a.slices = Number(argv[++i]);
    else if (k === '--addon') a.addon = argv[++i];
    else if (k === '--variable') a.variable = argv[++i];
    else if (k === '--list-variables') a.listVariables = true;
    else if (!k.startsWith('--') && a.target === null) a.target = k;
    else throw new Error(`unknown argument ${k}`);
  }
  if (a.target === null) throw new Error('usage: probe_parity_probe.js <file-or-dir> [--n N] [--seed S] [--slices K] [--addon PATH] [--variable NAME | --list-variables]');
  for (const k of ['n', 'seed', 'slices']) {
    if (!Number.isInteger(a[k]) || a[k] < 0) throw new Error(`--${k} must be a non-negative integer`);
  }
  return a;
}

// FNV-1a over the label, mixed with the seed: a stable per-item stream.
function streamSeed(seed, label) {
  let h = 2166136261 ^ seed;
  for (let i = 0; i < label.length; i++) {
    h ^= label.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

// mulberry32: tiny, seedable, identical on every machine.
function rng(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const OPTS = { projection: 'equirectangular', resampling: 'nearest', flipY: false };

/** Probe `n` seeded on-grid pixels of the raster `target` paints. */
function sample(h, target, label, args) {
  const r = h.renderGrid(target, OPTS);
  const { width, height } = r;
  const rand = rng(streamSeed(args.seed, label));
  const probes = [];
  let attempts = 0;
  // A swath or a regional grid covers a sliver of the world raster. Draw from
  // the whole raster until a few pixels land on the grid, then from the box
  // around those, so a thin footprint does not cost thousands of misses.
  let box = [0, 0, width, height];
  let widened = false;
  const maxAttempts = Math.max(50 * args.n, 1000);
  while (probes.length < args.n && attempts < maxAttempts) {
    attempts++;
    if (!widened && probes.length >= 20) {
      const xs = probes.map((q) => q.px);
      const ys = probes.map((q) => q.py);
      const padX = Math.ceil(0.25 * (Math.max(...xs) - Math.min(...xs) + 1));
      const padY = Math.ceil(0.25 * (Math.max(...ys) - Math.min(...ys) + 1));
      box = [
        Math.max(0, Math.min(...xs) - padX),
        Math.max(0, Math.min(...ys) - padY),
        Math.min(width, Math.max(...xs) + 1 + padX),
        Math.min(height, Math.max(...ys) + 1 + padY),
      ];
      widened = true;
    }
    const px = box[0] + Math.floor(rand() * (box[2] - box[0]));
    const py = box[1] + Math.floor(rand() * (box[3] - box[1]));
    const p = h.probePixel(target, OPTS, px, py);
    if (p == null || p.gridI == null || p.gridJ == null) continue; // off the raster or the grid
    probes.push({ px, py, lat: p.lat, lon: p.lon, value: p.value, gridI: p.gridI, gridJ: p.gridJ });
  }
  // The extent the raster was drawn over, for the Python side to turn a pixel
  // back into a position and hold probePixel's lat/lon to it.
  const bounds = [r.usedLatMin, r.usedLatMax, r.usedLonMin, r.usedLonMax];
  return { raster: [width, height], bounds, attempts, probes };
}

function skipped(label, extra, reason) {
  return { label, ...extra, status: 'skipped', reason, probes: [] };
}

/** A render or probe that threw. Not a skip: the sweep did not check the item. */
function failed(label, extra, e) {
  return { label, ...extra, status: 'failed', reason: `probe failed: ${e.message}`, probes: [] };
}

function gribItems(h, args) {
  const items = [];
  const count = h.count();
  for (let i = 0; i < count; i++) {
    const mm = h.message(i);
    const label = `msg#${i}`;
    const g = mm.grid;
    const extra = {
      kind: 'message',
      index: i,
      packing: mm.packing,
      grid: g == null ? null : {
        kind: g.kind, ni: g.ni, nj: g.nj, scan: g.scan, pointsPerRow: g.pointsPerRow != null,
      },
    };
    if (g == null) {
      items.push(skipped(label, extra, mm.truncation ? 'spectral field, no grid to probe' : 'no grid'));
      continue;
    }
    if (!g.reprojectable) {
      items.push(skipped(label, extra, `grid is not reprojectable (${g.placement})`));
      continue;
    }
    try {
      items.push({ label, ...extra, status: 'ok', periodic: g.periodicX === true, ...sample(h, i, label, args) });
    } catch (e) {
      items.push(failed(label, extra, e));
    }
  }
  return items;
}

/** Up to `k` slice-index tuples over the non-image dims: all first, all last,
 *  then seeded draws. */
function slicePicks(v, y, x, k, seed, name) {
  const dims = v.dims.map((d) => d.length);
  const other = dims.map((_, i) => i).filter((i) => i !== y && i !== x);
  const mk = (f) => dims.map((len, i) => (other.includes(i) ? f(len, i) : 0));
  const picks = [mk(() => 0)];
  if (other.length) picks.push(mk((len) => len - 1));
  const rand = rng(streamSeed(seed, `${name}:slices`));
  let guard = 0;
  while (picks.length < k && other.length && guard++ < 50) {
    picks.push(mk((len) => Math.floor(rand() * len)));
  }
  const seen = new Set();
  return picks.filter((p) => !seen.has(p.join(',')) && seen.add(p.join(','))).slice(0, Math.max(k, 1));
}

function netcdfItems(h, args) {
  const items = [];
  for (const v of h.variables()) {
    if (args.variable !== null && v.name !== args.variable) continue;
    const label = v.name;
    const extra = { kind: 'variable', name: v.name, variable: v.index, dims: v.dims.map((d) => ({ name: d.name, length: d.length })) };
    if (v.detectedYDim == null || v.detectedXDim == null) {
      items.push(skipped(label, extra, 'no CF image axes detected'));
      continue;
    }
    const y = v.detectedYDim;
    const x = v.detectedXDim;
    for (const idx of slicePicks(v, y, x, args.slices, args.seed, v.name)) {
      const sliceLabel = `${v.name}[${idx.map((n, d) => (d === y || d === x ? ':' : n)).join(',')}]`;
      const target = { variable: v.index, yDim: y, xDim: x, sliceIndices: idx };
      const it = { ...extra, yDim: y, xDim: x, sliceIndices: idx };
      try {
        items.push({ label: sliceLabel, ...it, status: 'ok', ...sample(h, target, sliceLabel, args) });
      } catch (e) {
        items.push(failed(sliceLabel, it, e));
      }
    }
  }
  return items;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const addon = path.resolve(args.addon || path.join(REPO, 'extension/bin/fieldglass.linux-x64-gnu.node'));
  if (!fs.existsSync(addon)) throw new Error(`native addon not found at ${addon}; build it (see RELEASE-TEST-PLAN.md section 0) or pass --addon`);
  const m = require(addon);
  const target = path.resolve(args.target);
  const isDir = fs.statSync(target).isDirectory();
  const h = isDir ? m.openStore(target) : m.open(fs.readFileSync(target));
  let count = 0;
  try { count = h.count(); } catch (_) { /* addressed by variables */ }
  const format = isDir ? 'zarr' : m.detectBytes(fs.readFileSync(target));
  if (args.listVariables) {
    // Names only, so the caller can hand each variable to its own process.
    const names = count > 0 ? [] : h.variables().map((v) => v.name);
    process.stdout.write(JSON.stringify({ file: target, format, variables: names }));
    return;
  }
  const items = count > 0 || format === 'grib1' || format === 'grib2' ? gribItems(h, args) : netcdfItems(h, args);
  process.stdout.write(JSON.stringify({ file: target, format, n: args.n, seed: args.seed, items }));
}

try {
  main();
} catch (e) {
  process.stderr.write(`probe_parity_probe: ${e.message}\n`);
  process.exit(2);
}
