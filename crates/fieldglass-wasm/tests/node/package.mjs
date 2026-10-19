// The published npm package works when installed (#466).
//
//   crates/fieldglass-wasm/pack.sh
//   node crates/fieldglass-wasm/tests/node/package.mjs
//
// `pack.sh` produces the tarball `npm publish` would upload. This installs that
// tarball into a throwaway project and decodes through it, which is a different
// question from the one the other Node tests answer: they load
// `pkg/nodejs/fieldglass_wasm.js` by path, so they would pass with a
// `package.json` that shipped none of it.
//
// What only this can catch:
//
//   * `files` missing an artefact — the module resolves and then fails to load
//     its `.wasm`;
//   * `exports` not offering the `.wasm` subpath, which is what a bundler
//     resolves and what a consumer needs to hand to `init` in a Worker;
//   * `types` pointing somewhere that is not in the tarball;
//   * the version not matching the workspace, since `pack.sh` reads it from
//     `Cargo.toml` rather than being told;
//   * the `@fieldglass/wasm/full` subpath (#1158) not resolving, and either
//     entry's `.d.ts` declaring what its module does not contain: the default
//     one is held to having none of the planner's or Zarr's names.
//
// # What it is not
//
// Not a decode oracle. Whether the numbers are *right* is settled by the
// eccodes cross-checks and the conformance suite; the expectations below are
// this project's own output, and they are here so that a package which loads
// but computes nothing cannot pass. A field of the right shape carrying the
// wrong values would be caught upstream, long before it reached a tarball.

import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..', '..');
const packDir = join(repoRoot, 'crates/fieldglass-wasm/pkg/npm');

let failures = 0;
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail === undefined ? '' : `: ${detail}`}`);
    failures += 1;
  }
}

// ---------------------------------------------------------------------------
// Find what pack.sh built
// ---------------------------------------------------------------------------

let tarballs = [];
try {
  tarballs = readdirSync(packDir).filter((f) => f.endsWith('.tgz'));
} catch {
  console.error(`missing: ${packDir}\n  crates/fieldglass-wasm/pack.sh`);
  process.exit(1);
}
if (tarballs.length !== 1) {
  console.error(`expected exactly one .tgz in ${packDir}, found ${tarballs.length}`);
  process.exit(1);
}
const tarball = join(packDir, tarballs[0]);

// The version is the workspace's, not an argument, so the two cannot drift.
const workspaceVersion = readFileSync(join(repoRoot, 'Cargo.toml'), 'utf8')
  .split('\n')
  .find((line) => line.startsWith('version'))
  .match(/"([^"]+)"/)[1];
const packed = JSON.parse(readFileSync(join(packDir, 'package.json'), 'utf8'));
check(
  'the package version is the workspace version',
  packed.version === workspaceVersion,
  `package.json says ${packed.version}, Cargo.toml says ${workspaceVersion}`,
);

// ---------------------------------------------------------------------------
// Install it the way a consumer would
// ---------------------------------------------------------------------------

const project = mkdtempSync(join(tmpdir(), 'fieldglass-consumer-'));
try {
  writeFileSync(
    join(project, 'package.json'),
    JSON.stringify({ name: 'consumer', private: true, type: 'module' }, null, 2),
  );
  // `--no-audit --no-fund` keeps the output to the point; offline is not forced
  // because npm still has to unpack a local file, not reach the registry.
  execFileSync('npm', ['install', '--no-audit', '--no-fund', tarball], {
    cwd: project,
    stdio: 'pipe',
  });

  const modules = join(project, 'node_modules', '@fieldglass', 'wasm');
  const entry = pathToFileURL(join(modules, 'fieldglass_wasm.js')).href;
  const { default: init, open } = await import(entry);

  // The subpath a bundler resolves, and what a Worker hands to `init`. Read off
  // the installed tree rather than the source, so a `files` omission shows up.
  const wasmPath = join(modules, 'fieldglass_wasm_bg.wasm');
  check('the .wasm is in the tarball', packed.files.includes('fieldglass_wasm_bg.wasm'));
  check(
    'the .wasm has an exports subpath',
    packed.exports['./fieldglass_wasm_bg.wasm'] === './fieldglass_wasm_bg.wasm',
  );
  check('the declarations are in the tarball', packed.files.includes(packed.types.replace('./', '')));

  // The API's wire types, generated from the Rust schema and embedded in
  // wasm-bindgen's own `.d.ts` (#574), so a consumer's `message(i)` is a
  // `MessageInfo` and not `any`. Read off the installed file: a custom section
  // that stopped reaching the output would leave every method typed and every
  // type it names undefined.
  const dts = await readFile(join(modules, packed.types.replace('./', '')), 'utf8');
  for (const [what, text] of [
    ['MessageInfo is declared', 'export interface MessageInfo {'],
    ['message(i) returns a MessageInfo', 'message(index: number): MessageInfo;'],
    ['an absent field is declared as a present null', '  truncation: SpectralTruncation | null;'],
    ['the edition-tagged identification is declared (#773)', 'export type Identification ='],
    ['probe answers null off the grid', '): Probe | null;'],
    ['grid() returns a Georef', 'grid(): Georef;'],
  ]) {
    check(what, dts.includes(text));
  }

  // The names those declarations use are declared (#818). wasm-bindgen copies
  // each `unchecked_return_type` / `unchecked_param_type` into the `.d.ts` as
  // free text, so a typo there — `FullDetails` for `FullDetail` — ships a
  // method typed against a name nothing declares, and every string check above
  // still passes. `tsc --strict` over a consumer that imports the installed
  // package checks the whole `.d.ts` (library checking stays on), which is the
  // check a TypeScript consumer's own build would make. The compiler is the
  // extension's, at the exact version its lockfile resolves, so this gate and
  // the extension's own type-checking cannot drift apart.
  const lock = JSON.parse(readFileSync(join(repoRoot, 'extension', 'package-lock.json'), 'utf8'));
  const tscVersion = lock.packages['node_modules/typescript'].version;
  // `--ignore-scripts`: the compiler needs none, and release.yml runs this in
  // the job that holds publish rights.
  execFileSync('npm', ['install', '--no-audit', '--no-fund', '--ignore-scripts', `typescript@${tscVersion}`], {
    cwd: project,
    stdio: 'pipe',
  });
  // The same consumer against each entry (#1158): the full one is the default
  // plus the planner and Zarr, so everything the default declares must
  // type-check through it too.
  const consumer = (entry) =>
    [
      `import init, { open, type ColourRange, type FullDetail, type MessageInfo, type MessageProbe, type RenderedRaster } from '${entry}';`,
      '',
      '// Typed against the published names, so a method whose declared return',
      '// type names nothing fails here rather than in a consumer.',
      'export async function use(',
      '  bytes: Uint8Array,',
      '): Promise<[MessageInfo, MessageProbe | null, FullDetail | null]> {',
      '  await init();',
      '  const handle = open(bytes);',
      '  return [handle.message(0), handle.probeMessage(0, 45.5, 120), handle.probeFullDetail(0, 45.5, 120)];',
      '}',
      '',
      '// A field decoded to draw is not a field (#1082). Each misuse below must be',
      "// a type error: under --strict an `@ts-expect-error` with no error under it",
      '// is itself an error, so a declaration that let one through fails here.',
      'export function display(bytes: Uint8Array): void {',
      '  const handle = open(bytes);',
      '  const full = handle.decode(0, {});',
      '  const coarse = handle.decodeDisplay(0, { reduce: 1 });',
      '  handle.renderDisplay(coarse, full.stats(), {}, false);',
      '  handle.warpDisplay(coarse, {});',
      '  handle.paletteDisplay(coarse, full.stats(), {});',
      '  // @ts-expect-error a display field cannot be probed',
      '  handle.probe(coarse, 45.5, 120);',
      '  // @ts-expect-error nor contoured',
      '  handle.contours(coarse, new Float64Array([]), {});',
      '  // @ts-expect-error nor combined',
      "  handle.combine(coarse, full, 'a_minus_b');",
      '  // @ts-expect-error nor handed to the shader accessors',
      '  handle.shaderValues(coarse, {});',
      '  // @ts-expect-error and a full field is not a display field either',
      '  handle.renderDisplay(full, full.stats(), {}, false);',
      '  // @ts-expect-error a reduction is not a decode option',
      '  handle.decode(0, { reduce: 1 });',
      '}',
      '',
      '// Both drawing calls report the colour range they used (#1156): a render',
      '// is an object with its bytes in `rgba`, and a palette carries `range`.',
      'export function ranges(bytes: Uint8Array): [Uint8Array, number | null, ColourRange, number] {',
      '  const handle = open(bytes);',
      '  const field = handle.decode(0, {});',
      '  const rendered: RenderedRaster = handle.render(field, {}, false);',
      '  const rgba: Uint8Array = rendered.rgba;',
      '  const low: number | null = handle.palette(field, {}).range.usedMin;',
      '  const coarse = handle.decodeDisplay(0, { reduce: 1 });',
      '  const shown: ColourRange = handle.paletteDisplay(coarse, field.stats(), {}).range;',
      '  // @ts-expect-error a render is no longer the bare bytes',
      '  const bare: Uint8Array = handle.render(field, {}, false);',
      '  void bare;',
      '  return [rgba, low, shown, handle.renderDisplay(coarse, field.stats(), {}, false).range.below];',
      '}',
      '',
      '// One shape with the Node addon (#1073): a slice and a line are one object',
      '// each, every class closes, and `using` releases a handle at the block end.',
      'export function shape(bytes: Uint8Array): number {',
      '  using handle = open(bytes);',
      '  using slice = handle.decodeSlice({ variable: 0, yDim: 0, xDim: 1, sliceIndices: [0, 0] });',
      "  handle.decodeLine({ variable: 0, alongDim: 0, sliceIndices: [0, 0] }, { dtype: 'f64' });",
      '  // @ts-expect-error the slice is one object, not four arguments',
      '  handle.decodeSlice(0, 0, 1, [0, 0]);',
      '  // @ts-expect-error a key the request does not take',
      '  handle.decodeSlice({ variable: 0, yDim: 0, xDim: 1, sliceIndex: [0, 0] });',
      '  const field = handle.decode(0);',
      '  field.close();',
      '  return slice.ni();',
      '}',
      '',
    ].join('\n');
  writeFileSync(join(project, 'consumer.ts'), consumer('@fieldglass/wasm'));
  writeFileSync(join(project, 'consumer_full.ts'), consumer('@fieldglass/wasm/full'));
  // What `tsc` says about one consumer file, or '' when it type-checks.
  const typeCheck = (file) => {
    try {
      execFileSync(
        join(project, 'node_modules', '.bin', 'tsc'),
        [
          '--noEmit',
          '--strict',
          '--skipLibCheck',
          'false',
          '--module',
          'nodenext',
          '--moduleResolution',
          'nodenext',
          // `esnext`, not a dated target: wasm-bindgen declares each handle's
          // `[Symbol.dispose]()`, which TypeScript types only from the
          // `esnext.disposable` lib. A consumer on an older `lib` sees two
          // errors in this file that are wasm-bindgen's, not this package's.
          '--target',
          'esnext',
          file,
        ],
        { cwd: project, stdio: 'pipe' },
      );
      return '';
    } catch (e) {
      return `${e.stdout ?? ''}${e.stderr ?? ''}`.trim();
    }
  };
  const tscOutput = typeCheck('consumer.ts');
  check(
    `the published declarations type-check under tsc ${tscVersion} --strict`,
    tscOutput === '',
    `\n${tscOutput}`,
  );
  const tscFullOutput = typeCheck('consumer_full.ts');
  check(
    `the same consumer type-checks against @fieldglass/wasm/full`,
    tscFullOutput === '',
    `\n${tscFullOutput}`,
  );

  await init({ module_or_path: await readFile(wasmPath) });

  // GRIB2, the message-addressed half.
  const grib = new Uint8Array(
    await readFile(
      join(repoRoot, 'crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2'),
    ),
  );
  const handle = open(grib);
  check('format', handle.format() === 'grib2', handle.format());
  check('addressing', handle.addressing() === 'messages', handle.addressing());
  check('message count', handle.count() === 1, handle.count());
  // Not band-limited: the key is there, and it is `null` (#574).
  const info = handle.message(0);
  check(
    'an absent field is a present null',
    'truncation' in info && info.truncation === null,
    info.truncation,
  );
  // GRIB2 has no P1 octet, and its identification says so by not having the
  // key at all, rather than a null (#773).
  check(
    'the identification is tagged grib2 and carries no P1 octet',
    info.identification?.edition === 'grib2' &&
      !('p1Octet' in info.identification) &&
      !('p1Octet' in info),
    JSON.stringify(info.identification),
  );

  const field = handle.decode(0, {});
  check('raster', field.ni() === 16 && field.nj() === 31, `${field.ni()}x${field.nj()}`);
  check('parameter', field.parameter() === 'Temperature', field.parameter());
  check('units', field.units() === 'K', field.units());
  const stats = field.stats();
  check('every cell present', stats.validCount === 496, stats.validCount);
  check(
    'values are plausible temperatures',
    stats.min > 270 && stats.min < 271 && stats.max > 311 && stats.max < 312,
    `${stats.min}..${stats.max}`,
  );
  field.free();
  handle.free();

  // NetCDF, the variable-addressed half — the package claims three formats and
  // this is the one that only arrived in #662.
  const nc = new Uint8Array(
    await readFile(join(repoRoot, 'crates/fieldglass-netcdf/tests/fixtures/cf_packed_data.nc')),
  );
  const dataset = open(nc);
  check('netcdf format', dataset.format() === 'netcdf', dataset.format());
  check('netcdf addressing', dataset.addressing() === 'variables', dataset.addressing());
  const [variable] = dataset.variables();
  const slice = dataset.decodeSlice(
    {
      variable: variable.index,
      yDim: 0,
      xDim: 1,
      sliceIndices: new Array(variable.dims.length).fill(0),
    },
    {},
  );
  check('netcdf raster', slice.ni() === 4 && slice.nj() === 3, `${slice.ni()}x${slice.nj()}`);
  // Physical units, so the CF unpacking survived into the package.
  const ncStats = slice.stats();
  check(
    'netcdf values are physical units',
    ncStats.min >= 250 && ncStats.max <= 875,
    `${ncStats.min}..${ncStats.max}`,
  );
  slice.free();
  dataset.free();

  // -------------------------------------------------------------------------
  // The full entry, `@fieldglass/wasm/full` (#1158)
  // -------------------------------------------------------------------------

  // Resolved through the installed package's `exports`, as a consumer's import
  // is, so a map that names the wrong file or none fails here.
  const resolve = createRequire(join(project, 'consumer.js')).resolve;
  check(
    'the default entry resolves to the root module',
    resolve('@fieldglass/wasm') === join(modules, 'fieldglass_wasm.js'),
    resolve('@fieldglass/wasm'),
  );
  let fullEntry = '';
  try {
    fullEntry = resolve('@fieldglass/wasm/full');
  } catch (e) {
    fullEntry = `does not resolve: ${e.message}`;
  }
  check(
    '@fieldglass/wasm/full resolves to the full module',
    fullEntry === join(modules, 'full', 'fieldglass_wasm.js'),
    fullEntry,
  );
  check(
    'its .wasm has an exports subpath',
    packed.exports['./full/fieldglass_wasm_bg.wasm'] === './full/fieldglass_wasm_bg.wasm',
  );
  check('its declarations are what the subpath names', packed.exports['./full']?.types === './full/fieldglass_wasm.d.ts');

  // Each entry declares only what it contains. The planner's and Zarr's names
  // are read off the generated files the full build embeds, so a type added to
  // either is covered without a list here to keep in step; the functions and
  // classes are wasm-bindgen's own, so they are named.
  const featureTypes = ['api.fetchplan.generated.d.ts', 'api.zarr.generated.d.ts'].flatMap((file) =>
    [
      ...readFileSync(join(repoRoot, 'crates/fieldglass-wasm/src', file), 'utf8').matchAll(
        /^export (?:interface|type) (\w+)/gm,
      ),
    ].map((m) => m[1]),
  );
  check(
    'the planner and Zarr declarations are found',
    ['PlanItem', 'Query', 'SourceSpec', 'Dialect', 'WindowPlan', 'WindowRequest', 'ChunkRef'].every((n) =>
      featureTypes.includes(n),
    ),
    featureTypes.join(', '),
  );
  const fullDts = await readFile(join(modules, 'full', 'fieldglass_wasm.d.ts'), 'utf8');
  const declares = (text, name) =>
    new RegExp(`^export (?:interface|type|class|function) ${name}\\b`, 'm').test(text);
  const bindings = ['parseWgrib2Idx', 'parseEcmwfIndex', 'verifyEnvelope', 'candidates', 'MessageManifest', 'openStore'];
  for (const name of [...featureTypes, ...bindings]) {
    check(`the default .d.ts does not declare ${name}`, !declares(dts, name));
    check(`the full .d.ts declares ${name}`, declares(fullDts, name));
  }
  check('the full .d.ts declares the default one\'s types too', declares(fullDts, 'MessageInfo'));

  // The same two facts as a TypeScript consumer meets them: the full entry's
  // names type-check, and asking the default entry for them is an error.
  writeFileSync(
    join(project, 'full.ts'),
    [
      "import init, { open, openStore, parseWgrib2Idx, httpRangeHeader, type MessageInfo, type PlanItem, type WindowPlan, type WindowRequest } from '@fieldglass/wasm/full';",
      '',
      '// The planner, Zarr and the decoder, from the one entry.',
      'export async function use(',
      '  idx: string,',
      '  metadata: Uint8Array,',
      '  grib: Uint8Array,',
      '  request: WindowRequest,',
      '): Promise<[PlanItem[], string | null, WindowPlan, MessageInfo]> {',
      '  await init();',
      "  const items = parseWgrib2Idx('key', idx).select({ abbreviation: 'TMP' });",
      "  const plan = openStore(new Map([['zarr.json', metadata]])).planWindow(request);",
      '  return [items, httpRangeHeader(items[0].range), plan, open(grib).message(0)];',
      '}',
      '',
    ].join('\n'),
  );
  const fullTsc = typeCheck('full.ts');
  check(`the full entry's declarations type-check under tsc ${tscVersion} --strict`, fullTsc === '', `\n${fullTsc}`);
  writeFileSync(
    join(project, 'default_has_no_planner.ts'),
    "import { openStore, parseWgrib2Idx, type PlanItem, type WindowPlan } from '@fieldglass/wasm';\nexport type T = [typeof openStore, typeof parseWgrib2Idx, PlanItem, WindowPlan];\n",
  );
  const defaultTsc = typeCheck('default_has_no_planner.ts');
  for (const name of ['openStore', 'parseWgrib2Idx', 'PlanItem', 'WindowPlan']) {
    check(
      `importing ${name} from the default entry is a type error`,
      defaultTsc.includes(`has no exported member '${name}'`) ||
        defaultTsc.includes(`has no exported member named '${name}'`),
      `\n${defaultTsc}`,
    );
  }

  // And it runs: a plan from the committed GFS sidecar, and a store opened
  // from its metadata, through the installed tree.
  const full = await import(pathToFileURL(fullEntry).href);
  await full.default({ module_or_path: await readFile(join(modules, 'full', 'fieldglass_wasm_bg.wasm')) });
  const sidecar = full.parseWgrib2Idx(
    'gfs.t00z.pgrb2.0p25.f000',
    readFileSync(join(repoRoot, 'crates/fieldglass-fetchplan/tests/fixtures/gfs_0p25.idx'), 'utf8'),
  );
  const [rh] = sidecar.select({ abbreviation: 'RH', levelText: '2 mb' });
  const header = rh === undefined ? 'no RH 2 mb record' : full.httpRangeHeader(rh.range);
  check('the full entry plans a fetch', header === 'bytes=76032983-76033192', header);
  sidecar.free();
  const store = full.openStore(
    new Map([
      [
        'zarr.json',
        new Uint8Array(
          readFileSync(join(repoRoot, 'crates/fieldglass-zarr/tests/fixtures/windows/window_v3/zarr.json')),
        ),
      ],
    ]),
  );
  check(
    'the full entry opens a Zarr store',
    store.variables().some((v) => v.name === 't2m'),
    store.variables().map((v) => v.name).join(', '),
  );
  store.free();
} finally {
  rmSync(project, { recursive: true, force: true });
}

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
