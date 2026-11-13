#!/usr/bin/env node
// Node half of the nightly sentinel (#1031). Run it through `tools/sentinel.py`;
// it is split out only because the native addon is a Node module.
//
//   node tools/sentinel_census.js <file> [<file> ...]
//
// For each file it runs the preflight (`preflightFile` from
// tools/preflight_samples.js: decode, reproject, colormap range) and, for a
// GRIB file, reads every message's `message()` metadata: the packing, the grid
// and product templates and the parameter table, which the sentinel compares
// with its committed baseline. It prints one JSON document on stdout.
'use strict';
const fs = require('fs');
const path = require('path');

const REPO = path.resolve(__dirname, '..');
const { preflightFile } = require(path.join(REPO, 'tools', 'preflight_samples.js'));
const m = require(path.join(REPO, 'extension/bin/fieldglass.linux-x64-gnu.node'));

// The markers preflight_samples.js prints for a file it could not open, decode
// or render. "no CF axes detected" is not one: that is a file the preflight
// leaves to a manual axis pick, and the probe-parity sweep pins those.
const FAILURE_MARKERS = ['OPEN FAILED', 'RENDER FAILED', 'UNKNOWN format'];

function messages(buf) {
  const h = m.open(buf);
  const out = [];
  for (let i = 0; i < h.count(); i++) {
    const mm = h.message(i);
    out.push({ index: i, packing: mm.packing, gridKind: mm.grid == null ? null : mm.grid.kind, fingerprint: mm.fingerprint });
  }
  return out;
}

function census(file) {
  const buf = fs.readFileSync(file);
  const fmt = m.detectBytes(buf);
  const lines = preflightFile(buf, path.basename(file));
  const doc = {
    file: path.basename(file),
    format: fmt,
    preflight: lines,
    failed: lines.some((l) => FAILURE_MARKERS.some((k) => l.includes(k))),
    messages: null,
    error: null,
  };
  if (fmt === 'grib1' || fmt === 'grib2') {
    try {
      doc.messages = messages(buf);
    } catch (e) {
      doc.error = `message() failed: ${e.message}`;
    }
  }
  return doc;
}

function main() {
  const files = process.argv.slice(2);
  const docs = files.map((f) => {
    try {
      return census(f);
    } catch (e) {
      return { file: path.basename(f), format: null, preflight: [], failed: true, messages: null, error: e.message };
    }
  });
  process.stdout.write(JSON.stringify(docs));
}

main();
