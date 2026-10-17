// The fetch planner in the browser module (#1081).
//
//   crates/fieldglass-wasm/build.sh nodejs
//   node crates/fieldglass-wasm/tests/node/fetchplan.mjs
//
// One GFS field, planned the way a Worker plans it: parse the committed `.idx`,
// select a record, build the `Range` header, check the envelope of the bytes
// that range fetched (committed beside the sidecar), and open them. Then every
// other answer the planner gives, compared against what the Rust planner gave
// for the same input.
//
// # Two kinds of expectation
//
// The **headline** is checked against values from outside this project: the
// offset and length come from the sidecar's own lines, the header is the one
// the fixture was fetched from the bucket with, and the decoded field is what
// eccodes reads (NOTICE.md beside the fixture).
//
// The **parity** half reads `crates/fieldglass/tests/fetch_plan_wire.json`,
// which `fieldglass`'s own test records from the Rust planner: the queries, the
// plan items, the headers, the error codes and the run discovery. It replays
// each question through this bundle and compares. That catches the binding
// disagreeing with the API, which is the one thing a Rust test cannot see:
// serde-wasm-bindgen's conversion, `null` for every absent key, the error
// mapping.
//
// A CI gate, like `conformance.mjs`: it needs only committed files and a built
// `pkg/nodejs`, and it fails loudly when either is missing.

import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { isDeepStrictEqual } from 'node:util';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');

const pkgArg = process.argv.indexOf('--pkg');
const pkgDir = pkgArg === -1 ? 'crates/fieldglass-wasm/pkg/nodejs' : process.argv[pkgArg + 1];
const WASM = join(repoRoot, pkgDir, 'fieldglass_wasm.js');
const RECORDING = join(repoRoot, 'crates/fieldglass/tests/fetch_plan_wire.json');

for (const [path, how] of [
  [WASM, 'crates/fieldglass-wasm/build.sh nodejs'],
  [RECORDING, 'it is committed; check out the repository'],
]) {
  if (!existsSync(path)) {
    console.error(`missing: ${path}\n  ${how}`);
    process.exit(1);
  }
}

const wasm = require(WASM);
const recording = JSON.parse(readFileSync(RECORDING, 'utf8'));
const committed = (path) => readFileSync(join(repoRoot, path));

let failures = 0;
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail === undefined ? '' : `: ${detail}`}`);
    failures += 1;
  }
}

/** Structural equality, key order ignored: the bundle and serde_json need not
 *  write an object's keys in the same order, and nothing reads that order. */
function same(name, actual, expected) {
  check(
    name,
    isDeepStrictEqual(actual, expected),
    `got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`,
  );
}

/** The `code` a call throws, or a note that it did not throw. */
function codeOf(f) {
  try {
    f();
    return 'did not throw';
  } catch (e) {
    return e.code ?? `threw without a code: ${e}`;
  }
}

console.log(`fetchplan through ${pkgDir}`);

// ---------------------------------------------------------------------------
// The headline: one GFS field, sidecar to decoded values
// ---------------------------------------------------------------------------

const gfs = recording.wgrib2;
const idx = wasm.parseWgrib2Idx(gfs.key, committed(gfs.path).toString('utf8'));

// The sidecar's own words, and then the meaning: WMO codes through the
// table-backed resolver and a level through the level grammar.
const byWords = idx.select({ abbreviation: 'RH', levelText: '2 mb' });
const byMeaning = idx.select({
  parameter: { discipline: 0, category: 1, number: 1 },
  level: { surface: 'isobaric', value: 2, value2: null },
});
check('one record is RH at 2 mb', byWords.length === 1, `got ${byWords.length}`);
same('the codes select the same record as the words', byMeaning, byWords);

const item = byWords[0];
same('its range is the sidecar line\'s, to the next line\'s offset', item.range, {
  kind: 'exact',
  offset: 76032983,
  length: 210,
});
same('it is message 107 of the sidecar', item.address, {
  kind: 'message',
  index: 106,
  subIndex: null,
});
check('it names the object it was planned for', item.key === gfs.key, item.key);
check(
  'the Range header is the one the fixture was fetched with',
  wasm.httpRangeHeader(item.range) === 'bytes=76032983-76033192',
  wasm.httpRangeHeader(item.range),
);

const bytes = new Uint8Array(committed(recording.envelope.path));
check('the envelope holds, and states the length', wasm.verifyEnvelope(item, bytes) === 210);

// The bytes the plan promised, opened: RH, zero everywhere (eccodes 2.34.1).
const handle = wasm.open(bytes);
const info = handle.message(0);
check('the message is the promised parameter', info.abbreviation === item.expect.abbreviation, info.abbreviation);
const field = handle.decode(0, {});
const stats = field.stats();
check('it is the 0.25° global grid', field.ni() === 1440 && field.nj() === 721, `${field.ni()} x ${field.nj()}`);
check('its values are eccodes\' (min 0, max 0)', stats.min === 0 && stats.max === 0, JSON.stringify(stats));
field.free();
handle.free();

// What a Worker does when the plan and the bytes disagree.
check(
  'a short fetch is short_read, worth a retry',
  codeOf(() => wasm.verifyEnvelope(item, bytes.subarray(0, 100))) === 'short_read',
);
const stale = bytes.slice();
stale[0] = 0x58; // 'X': a mid-message offset lands in packed data.
check(
  'bytes that are not the message are decode, a stale sidecar',
  codeOf(() => wasm.verifyEnvelope(item, stale)) === 'decode',
);

// The whole object is one fetch with no header.
check('a whole range sends no header', wasm.httpRangeHeader({ kind: 'whole' }) === null);
check(
  'an open range runs to the end of the object',
  wasm.httpRangeHeader({ kind: 'openEnded', offset: 490148925 }) === 'bytes=490148925-',
);

// Misuse is refused, not guessed at.
check(
  'a misspelt query key throws rather than widening the query',
  /levelTxt/.test(String((() => { try { idx.select({ levelTxt: '2 mb' }); } catch (e) { return e.message ?? e; } return 'did not throw'; })())),
);
check(
  'text that is not a .idx is decode, naming the line',
  codeOf(() => wasm.parseWgrib2Idx('k', '1:notanumber:d=2026090400:TMP:surface:anl:')) === 'decode',
);
same('an empty query selects every record', idx.select({}).length, gfs.plan.itemCount);
same('so does none at all', idx.select().length, gfs.plan.itemCount);

idx.free();

// ---------------------------------------------------------------------------
// Parity: the recording, replayed
// ---------------------------------------------------------------------------

function replay(label, parse, side) {
  const manifest = parse(side.key, committed(side.path).toString('utf8'));
  const items = manifest.items();
  same(`${label}: item count`, items.length, side.plan.itemCount);
  same(`${label}: message count`, manifest.messages().length, side.plan.messageCount);
  same(`${label}: first item`, items[0], side.plan.first);
  same(`${label}: last item`, items[items.length - 1], side.plan.last);
  for (const selection of side.plan.selections) {
    const got = manifest.select(selection.query);
    same(`${label}: select ${JSON.stringify(selection.query)}`, got, selection.items);
    same(
      `${label}: its Range headers`,
      got.map((i) => wasm.httpRangeHeader(i.range)),
      selection.rangeHeaders,
    );
  }
  manifest.free();
}

replay('wgrib2', wasm.parseWgrib2Idx, recording.wgrib2);
replay('ecmwf', wasm.parseEcmwfIndex, recording.ecmwf);

const env = recording.envelope;
same('envelope: the recorded item is the one selected', item, env.item);
same('envelope: declared length', wasm.verifyEnvelope(env.item, bytes), env.declaredLength);
same(
  'envelope: truncated code',
  codeOf(() => wasm.verifyEnvelope(env.item, bytes.subarray(0, env.truncatedTo))),
  env.truncatedCode,
);
same('envelope: stale code', codeOf(() => wasm.verifyEnvelope(env.item, stale)), env.staleCode);

const src = recording.sources;
same('sources: candidates', wasm.candidates(src.source, src.options), src.candidates);
check('sources: a valid source validates', codeOf(() => wasm.validateSource(src.source)) === 'did not throw');
for (const name of ['noCycleHours', 'unknownPlaceholder']) {
  same(`sources: ${name} from validateSource`, codeOf(() => wasm.validateSource(src[name].source)), src[name].code);
  same(
    `sources: ${name} from candidates`,
    codeOf(() => wasm.candidates(src[name].source, src.options)),
    src[name].code,
  );
}

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
