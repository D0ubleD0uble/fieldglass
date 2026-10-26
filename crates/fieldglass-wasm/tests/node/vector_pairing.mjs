// `vectorPairing()` through the browser module (#1049, #1142).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/vector_pairing.mjs
//
// Whether two messages are a u/v pair is the umbrella's answer, tested there
// against every table it reads. This checks the binding's part: the method is
// reachable, takes two indices, and returns the wire string the generated
// `VectorPairing` type declares, for each answer.
//
// No committed GRIB file holds a wind component, so the pairs are the GFS
// fixture with its §4 parameter octets rewritten, as the release plan builds
// its samples and the umbrella's `tests/vector_pairing.rs` does: discipline 0,
// category 2, numbers 2 (UGRD), 3 (VGRD), 1 (wind speed), 0 (wind direction),
// and 250 and 251, NCEP local codes no table names.
//
// A CI gate, like `netcdf.mjs`: it needs only a committed fixture and a built
// `pkg/nodejs`, and it fails loudly when either is missing.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const GFS = join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [GFS, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);

let failures = 0;
function equal(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}: got ${a}, expected ${e}`);
    failures += 1;
  }
}

/** The offset of a GRIB2 message's section `number`. */
function section(msg, number) {
  let at = 16;
  while (Buffer.from(msg.subarray(at, at + 4)).toString('ascii') !== '7777') {
    if (msg[at + 4] === number) return at;
    at += new DataView(msg.buffer, msg.byteOffset + at, 4).getUint32(0);
  }
  throw new Error(`no section ${number}`);
}

const gfs = new Uint8Array(readFileSync(GFS));

/** The GFS message as discipline-0 parameter 2-`number`. */
function gfsAs(number) {
  const msg = gfs.slice();
  const s4 = section(msg, 4);
  msg[s4 + 9] = 2;
  msg[s4 + 10] = number;
  return msg;
}

console.log(`vectorPairing through ${pkgDir}`);

const messages = [gfsAs(2), gfsAs(3), gfsAs(1), gfsAs(0), gfsAs(250), gfsAs(251)];
const bytes = new Uint8Array(messages.reduce((n, m) => n + m.length, 0));
messages.reduce((at, m) => (bytes.set(m, at), at + m.length), 0);
const handle = wasm.open(bytes);

equal('six messages', handle.count(), 6);
equal('UGRD with VGRD', handle.vectorPairing(0, 1), 'recognized');
equal('VGRD with UGRD', handle.vectorPairing(1, 0), 'swapped');
equal('wind speed with wind direction', handle.vectorPairing(2, 3), 'not_a_pair');
equal('UGRD with itself', handle.vectorPairing(0, 0), 'not_a_pair');
equal('two unnamed local parameters', handle.vectorPairing(4, 5), 'unknown');
equal('wind speed with an unnamed one', handle.vectorPairing(2, 5), 'not_a_pair');

let code;
try {
  handle.vectorPairing(0, 6);
} catch (e) {
  code = e.code;
}
equal('an index past the end is refused', code, 'no_such_message');

handle.free();

if (failures > 0) {
  console.log(`${failures} failed`);
  process.exit(1);
}
console.log('all passed');
