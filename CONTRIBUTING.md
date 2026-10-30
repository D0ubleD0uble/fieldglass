# Contributing to Fieldglass

Thanks for considering a contribution. Bug reports, files that don't render
correctly, and pull requests are all welcome.

## Filing issues

The fastest path to a fix is a small file that shows the problem. Use the
[Bug report](.github/ISSUE_TEMPLATE/bug_report.md) template. If the file can't
be shared, describe where it came from (the center, the format and edition,
the model that made it, and roughly how big it is) so I can find a stand-in.

For a feature, the [Feature request](.github/ISSUE_TEMPLATE/feature_request.md)
template asks for the problem you're trying to solve and its scope. Please
fill it in rather than describing only the implementation you have in mind.

For a security issue, please don't file a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/D0ubleD0uble/fieldglass/security/advisories/new)
instead.

## What you need

- Stable Rust 1.88 or newer (the code uses edition 2024)
- Node.js 22 or newer
- Python 3.10 or newer, only for the dev tooling (`pre-commit`, `semgrep`)
- Visual Studio Code 1.85 or newer

## How the repository is laid out

| Path | What it holds |
|---|---|
| `crates/fieldglass-core` | The traits and shared types the format readers are built on, with nothing format-specific. |
| `crates/fieldglass-grib1` | The GRIB1 reader, organized by section (`is.rs`, `pds.rs`, `gds.rs`, `bms.rs`, `bds.rs`), with the WMO table lookups in `tables.rs`. |
| `crates/fieldglass-grib2` | The GRIB2 reader: all eight sections, the message list, and value decoding for every packing in the [GRIB2 packing modes](README.md#grib2-packing-modes) table. |
| `crates/fieldglass-netcdf` | The NetCDF reader: the classic header and values, and the HDF5 / NetCDF-4 object tree and dataset values (contiguous, compact and chunked, with deflate, shuffle, fletcher32, zstd and szip). |
| `crates/fieldglass-zarr` | Zarr v2 and v3 stores: their metadata, consolidated or not, read through a key-value seam the host fills; kerchunk references presented as a store; and the chunk codecs, including blosc with its five compressors, the byte and bit transposes, gzip, zlib, zstd, LZ4 and v3 sharding. It does no I/O, so every document and chunk arrives as bytes the caller already has. |
| `crates/fieldglass-fetchplan` | Indexes in, byte ranges out: wgrib2 `.idx`, ECMWF `.index`, kerchunk references, and the Zarr chunk-grid arithmetic that spells a chunk's key. No I/O, no clock and no decoder. |
| `crates/fieldglass-aec` | The CCSDS 121.0 adaptive entropy coding decoder behind GRIB2's CCSDS packing and the HDF5 szip filter. It's checked against libaec 1.1.7 over a committed corpus and depends on nothing but `thiserror` ([ADR-0012](docs/decisions/0012-own-the-aec-decoder.md)). |
| `crates/fieldglass` | The host-neutral API over the GRIB, NetCDF and Zarr readers: open bytes or a store, decode a field, place it on the Earth, color it. Every host is a binding of this one ([ADR-0006](docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)). |
| `crates/fieldglass-napi` | The Node.js bindings, through napi-rs. It's the only crate that knows about Node. |
| `crates/fieldglass-wasm` | The wasm-bindgen layer for the browser, meant to run in a Web Worker. |
| `crates/fieldglass-verify` | Verus proofs of the decode kernel. It's its own workspace, deliberately outside the root one, run by `scripts/verify.sh`. |
| `crates/fieldglass-perf` | Speed and memory benchmarks, held to stated bounds. It's its own workspace too, run by `crates/fieldglass-perf/run.sh`, with results in [docs/performance.md](docs/performance.md). |
| `extension/` | The TypeScript VS Code extension. It registers a custom read-only editor and draws a webview. |

Eight of the library crates are published to crates.io with each stable
release. `fieldglass-napi` is never published, because it's a build product of
the extension rather than a library to depend on. The browser build is
published to npm as `@fieldglass/wasm`.

Every library crate except `fieldglass-napi` compiles for
`wasm32-unknown-unknown`, and CI checks that on every pull request and runs the
decode tests on `wasm32-wasip1`, so the browser build can't break unnoticed.
`fieldglass-napi` is Node's binding, so it doesn't build for wasm32 at all.
`crates/fieldglass-wasm` is the browser build: `./build.sh web` produces the ES
module and `./pack.sh` the npm package. Its own
[README](crates/fieldglass-wasm/README.md) covers how it's built and measured.

## Setting up

```sh
git clone git@github.com:D0ubleD0uble/fieldglass.git
cd fieldglass
pipx install pre-commit              # or: pip install --user pre-commit
npm install                          # installs @napi-rs/cli and activates git hooks
```

The root `npm install` installs `@napi-rs/cli`, which builds the native
module, and runs a `prepare` step that turns on the repository's git hooks
(see [The checks every commit runs](#the-checks-every-commit-runs)). If
`pre-commit` isn't on your `PATH`, the step prints a hint and carries on, so
you can install it later and run `npm install` again.

### Building the native module

The extension loads a compiled binary from `extension/bin/`. From the
repository root:

```sh
cd crates/fieldglass-napi
npx napi build --platform --release --output-dir ../../extension/bin
```

That writes a file such as `extension/bin/fieldglass.linux-x64-gnu.node`, along
with `extension/bin/index.d.ts`. Run it again after you change any Rust code.

### Building the extension

```sh
cd extension
npm install
npm run compile
```

To recompile as you edit, run `npm run watch` instead.

### Running the extension

Open the repository in VS Code and press `F5`. An Extension Development Host
window opens with Fieldglass loaded, and you can open any supported file in it
to try your change.

## Tests

Run the whole Rust test suite:

```sh
cargo test
```

Or one crate, or the tests whose names contain a word:

```sh
cargo test -p fieldglass-grib1
cargo test -p fieldglass-grib1 parse_pds
```

### Extension tests

The TypeScript side has an `@vscode/test-electron` suite that starts a real VS
Code extension host and drives the editor and the render panel end to end.
The render-panel tests send a real `gridReady` message through
`webview.postMessage`, so the messages between the extension host and the
webview go through the same serializer they do in use. The suite covers GRIB1
and GRIB2 rendering, NetCDF metadata and Zarr stores, plus a regression test
for sending a `Buffer` where a `Uint8Array` is expected.

```sh
cd extension
npm test
```

The tests need a display. CI runs them under `xvfb-run -a npm test`.

### ecCodes reference snapshots

`fieldglass-grib1` and `fieldglass-grib2` check their bundled test files
against [ecCodes](https://confluence.ecmwf.int/display/ECC), ECMWF's reference
implementation, through JSON snapshots. Every file has one except the three
that ecCodes can't decode, which the script lists with the reason. Each
snapshot sits beside its file as `tests/fixtures/{file}.eccodes.ref.json` and
is checked into git, so each crate's test (`tests/eccodes_reference.rs`) needs
nothing at run time. It compares the parser's output with the stored JSON, one
chosen ecCodes key at a time.

You only need ecCodes to regenerate the snapshots, usually after adding a test
file or upgrading the reference. Install it from your package manager
(Debian and Ubuntu: `apt install libeccodes-tools`; macOS:
`brew install eccodes`), then run:

```sh
python3 tools/regenerate-eccodes-snapshots.py
```

That regenerates both editions; `--edition 1` or `--edition 2` limits it to
one. The script runs `grib_dump -j` on each file, keeps the keys in that
edition's list (`GRIB1_KEYS` or `GRIB2_KEYS`), and rewrites each snapshot, so
the diff can be reviewed in the pull request. To check a new key, add it to
the edition's list in the script and to the match in that crate's
`tests/eccodes_reference.rs::assert_message_matches`, then run the script
again.

## Linting

```sh
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --all-features
```

The lint levels live in `[workspace.lints]` in the root `Cargo.toml`, and every
member inherits them with `lints.workspace = true`. So a plain `cargo build`,
an editor and a crates.io reader all hold the code to the same bar the hooks
do. That includes `[workspace.lints.rustdoc]`, which is why `cargo doc` is a
gate and not only a build: rustdoc's lints only warn by default, so a broken
doc link would otherwise be printed and carried past.

## The checks every commit runs

The repository uses the [`pre-commit`](https://pre-commit.com/) framework,
configured in [`.pre-commit-config.yaml`](.pre-commit-config.yaml). It fetches
and isolates pinned versions of `shellcheck`, `actionlint`, `gitleaks`,
`semgrep` and `verusfmt` for you. Beyond `pre-commit` itself, you need the Rust
and Node toolchains, Python 3 for the repository's own checks, and, to push,
`cargo-deny`.

Set it up once per clone:

```sh
pipx install pre-commit                # preferred, works everywhere
# or, on systems without pipx:
pip install --user pre-commit          # may need --break-system-packages on PEP 668 distros
# or, in its own venv:
python3 -m venv ~/.venvs/fieldglass && ~/.venvs/fieldglass/bin/pip install pre-commit

cd /path/to/fieldglass
npm install                            # runs `pre-commit install --hook-type pre-commit --hook-type pre-push`
```

The pre-push hook runs `cargo-deny` and fails without it, so install it before
your first push.

```sh
cargo install --locked cargo-deny      # advisory, license and source policy
```

The hooks run in two stages. The commit stage is the fast one:

| Group | What it checks |
|---|---|
| Formatting | `cargo fmt --check` for the workspace, the fuzz crates and the performance harness, and `verusfmt --check` for the verification crate when its files change. |
| Rust lints | `cargo clippy -D warnings` for the workspace, and again for the feature sets each host builds: the browser crate with `fetchplan` and with `zarr`, `fieldglass-core` with no default features, the format crates against a core with rendering off and against their parsing surface only, and `fieldglass` with one feature set per host shape. |
| Extension | `tsc --noEmit` and `eslint`. |
| Repository rules | That the architecture diagrams match the code, the generated API declarations match the schema, generators name their text encoding, every member inherits `[workspace.lints]`, format crates re-export the core names their signatures use, host types stay in host crates, host crates depend only on `fieldglass`, no package declares a dependency it never uses, the documented parsing surface is the one the format crates use, every workflow pins the same binaryen, each nested crate's `Cargo.lock` matches its manifest, and every verified kernel file is still verified. |
| File hygiene | `check-yaml`, `check-json`, `check-toml`, `check-merge-conflict`, `end-of-file-fixer`, `trailing-whitespace` and `check-added-large-files`. |
| Scans | `shellcheck` for shell scripts, `actionlint` for GitHub Actions, and `gitleaks` for secrets in the staged diff. |

The push stage is slower:

| Group | What it checks |
|---|---|
| Tests | `cargo test --workspace`, `fieldglass-core` with no default features, `fieldglass` with one feature set per host shape, and each format crate's `decode` example. |
| Docs | `cargo doc --no-deps` for the workspace with all features, for `fieldglass-core` and the format crates with no default features, and for `fieldglass` with each host's feature set. |
| Rust lints | `cargo clippy` for the format crates' tests and examples with no default features. |
| Tool tests | The self-tests of the repository checkers and release scripts under `tools/`. |
| Security | `cargo deny check`, a `semgrep` scan, and `npm audit --omit=dev` for the extension and the root. The two audits only report; they never fail the push. |

You can skip them with `git commit --no-verify` or `git push --no-verify`
when you really have to. CI runs the same checks at full strength either way.

## Continuous integration

The `pre-commit` job in [`.github/workflows/ci.yml`](.github/workflows/ci.yml)
installs the same toolchain (Rust, Node, Python and `cargo-deny`) and runs
`pre-commit run --all-files` for both stages, so your hooks and CI run exactly
the same checks. Other jobs in the same workflow test each published crate
from its packaged `.crate`, build for wasm32 with the bundle-size gate and the
browser-host tests, and build the extension.

Separate workflows cover the rest:

- Coverage reports to Codecov on every pull request and push to `master`.
- Fuzz runs the fuzz targets when a reader's code changes, on a pull request
  or on `master`.
- Performance holds every scenario to [docs/performance.md](docs/performance.md)
  and fails on a change that moves a gated number.
- Verify runs the Verus proofs of the decode kernel. It reports without
  blocking a merge, as [docs/verification.md](docs/verification.md) explains.
- AEC oracle rebuilds libaec, regenerates the `fieldglass-aec` corpus and fails
  on any difference.

A tag matching `v*` starts a release. It builds the native module for all six
supported platforms, packages one `.vsix` for each, and publishes them to the
VS Code Marketplace and a GitHub Release. The same release publishes the
library crates to crates.io and `@fieldglass/wasm` to npm. [RELEASING.md](RELEASING.md)
has the full checklist.

### Security scanning

Three more scanners report to the repository's Security tab. Semgrep runs on
pushes to `master`, on every pull request and weekly; CodeQL runs on pushes to
`master`, on pull requests against it and weekly; Dependabot checks for version
updates weekly and raises security updates as advisories appear.

- [Semgrep](https://semgrep.dev/), in [`.github/workflows/semgrep.yml`](.github/workflows/semgrep.yml), runs pattern-based security rules across Rust and TypeScript (`p/default`, `p/security-audit`, `p/owasp-top-ten`, `p/rust`, `p/typescript`, `p/secrets` and `p/github-actions`). The same scan runs locally on `git push`.
- [CodeQL](https://codeql.github.com/), in [`.github/workflows/codeql.yml`](.github/workflows/codeql.yml), analyzes JavaScript, TypeScript and Rust with the `security-extended` queries.
- [Dependabot](https://docs.github.com/en/code-security/dependabot), in [`.github/dependabot.yml`](.github/dependabot.yml), opens weekly version-update pull requests for `cargo` (the workspace and each `crates/*/fuzz` crate), `npm` (the extension and the root) and `github-actions`. Security-update pull requests come from the repository's Dependabot security updates setting instead, as advisories appear; the advisories show in the Security tab, and the version-update pull requests don't.

The Semgrep and CodeQL workflows only run when
`github.event.repository.visibility == 'public'`, because GitHub Code
Scanning's SARIF upload needs either a public repository or GitHub Advanced
Security. On a private repository they sit idle, and they start the moment
it's made public. Dependabot runs either way.

## Pull requests

Fieldglass works on one trunk. Feature branches are cut from `master`, and
pull requests target `master` directly. There's no long-lived release branch:
a release is a tagged commit on `master` (`vX.Y.Z`). Merging to `master` never
publishes anything; only pushing a `v*` tag does.

1. Fork the repository and create a feature branch from `master`:
   ```sh
   git fetch origin
   git switch -c my-feature origin/master
   ```
2. Make your change. The hooks above run on each commit and push.
3. Add an entry to [CHANGELOG.md](CHANGELOG.md) under the `## [Unreleased]`
   heading, adding the heading above the latest release if it isn't there. If
   your change closes an issue, put `Closes #N` in the pull request body, and
   it closes when the pull request merges.

   An entry is a few sentences saying what changed for someone using
   Fieldglass. The long form (measurements, reasoning, how it was checked) goes
   in the pull request body. Changes people see go under Added, Changed or
   Fixed. Library-only changes go under `### Rust API`, with anything that
   breaks the 0.x API in its breaking-changes list. Work with no visible effect
   gets one line under `### Internal`. The section becomes the GitHub release
   body, which GitHub caps at 125,000 characters.
4. Open the pull request against `master`:
   ```sh
   gh pr create --base master
   ```
   The required checks are `Lint + test via pre-commit`, `Build extension`,
   `Analyze (rust)`, `Analyze (javascript-typescript)` and `Semgrep SAST`.

At release time, a short-lived `release-prep/X.Y.Z` branch off `master` bumps
the versions and turns the `## [Unreleased]` heading into the release's. That
pull request merges into `master`, and the merge commit is tagged `vX.Y.Z`.
[RELEASING.md](RELEASING.md) has the full checklist, from the checks before a
release to confirming it went out.

## A few rules of the codebase

`crates/fieldglass-core` stays free of anything format-specific and of `napi`
types. The format crates depend on it, and it depends on none of them.

The WMO lookup tables live in `crates/fieldglass-grib1/src/tables.rs` (GRIB1,
WMO ON388) and `crates/fieldglass-grib2/src/tables.rs` (GRIB2, WMO FM 92 Code
Tables 0.0, 1.x, 3.x and 4.x). They're the one place parameter, center, level
and process names come from, so add to them rather than writing names into the
napi or TypeScript code.

Several tables are generated, so regenerate them rather than editing them by
hand:

| Table | File | Generated by | From |
|---|---|---|---|
| ECMWF GRIB1 local parameters | `tables_local.rs` (`fieldglass-grib1`) | `tools/gen_grib1_local_tables.py` | Every table ecCodes ships. The script also writes the ecCodes decode oracle its test reads. |
| WMO GRIB2 master tables (Code Tables 4.2, 4.4, 4.5) | `tables_wmo.rs` | `tools/gen_wmo_grib2_tables.py` | A pinned `wmo-im/GRIB2` release |
| Originating centers and sub-centers | `tables_cct.rs` (both format crates) and `center_tables.rs` (core) | `tools/gen_wmo_cct_tables.py` | A pinned `wmo-im/CCT` release |
| ECMWF and DWD GRIB2 local parameters | `tables_ecmf.rs`, `tables_edzw.rs` | `tools/gen_localconcepts_tables.py` | ecCodes' `localConcepts` |
| NCEP GRIB2 local parameters | `tables_ncep.rs` | `tools/gen_ncep_tables.py` | A pinned wgrib2 release |
| Short names for the WMO GRIB2 parameters | `tables_wmo_short.rs` | `tools/gen_wmo_short_names.py` | The center-0 rows of the same wgrib2 release, because WMO publishes no abbreviations of its own |

### Adding a GRIB metadata field

The steps are the same for either edition. Only the section module you start
in differs.

1. Parse the field in its section module:
   - GRIB1: `crates/fieldglass-grib1/src/{is,pds,gds,bms,bds}.rs`
   - GRIB2: `crates/fieldglass-grib2/src/{is,ids,lus,gds,pds,drs,bms,ds}.rs`
2. Expose the value on the section struct.
3. Add it to `MessageInfo` in `crates/fieldglass/src/api.rs`. Both editions
   fill the same `MessageInfo` in `crates/fieldglass/src/session.rs`, so a new
   field usually goes into both paths.
4. Re-record the schema and regenerate the TypeScript declarations:
   `FIELDGLASS_UPDATE_SCHEMA=1 cargo test -p fieldglass --test api_rules`, then
   `python3 tools/gen_api_declarations.py`. The extension then sees the field on
   `MessageInfo` in `extension/src/api.generated.ts`.
5. Show it in the webview table in `extension/src/provider.ts`.

The field reaches both hosts under its camelCase name, because the bindings
turn `snake_case` Rust field names into `camelCase` TypeScript ones.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind,
assume good faith, and report problems privately to the maintainer.

## Licensing

Fieldglass is dual-licensed under MIT or Apache-2.0. By submitting a
contribution, you agree that it's licensed under both, as the
[README](README.md#license) says.
