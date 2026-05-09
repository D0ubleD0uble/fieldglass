# Fieldglass

A Visual Studio Code extension for viewing meteorological binary data files (GRIB1, GRIB2, NetCDF) directly in the editor. Built on a Rust native module with no Python dependencies.

[Latest release](https://github.com/D0ubleD0uble/fieldglass/releases/latest)

## Status

Phase 1 of the project is in progress: read-only metadata viewing for GRIB1, with grid-data decoding implemented in Rust but not yet visualized. GRIB2, NetCDF, metadata editing, and 2-D field rendering are on the roadmap. See [PLAN.md](PLAN.md) for the full phase breakdown.

## Feature matrix

| Feature | GRIB1 | GRIB2 | NetCDF |
|---|:---:|:---:|:---:|
| Format detection from magic bytes | ✅ | ✅ | ✅ |
| File-extension association (`.grb` / `.grib*` / `.nc*`) | ✅ | ✅ | ✅ |
| Open via *Reopen Editor With…* for unrecognized files | ✅ | ✅ | ✅ |
| Indicator / header section parsing | ✅ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| Per-message metadata (parameter, level, time, forecast period) | ✅ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| Grid description (lat/lon, Gaussian, polar stereo, Lambert) | ✅ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| WMO ON388 lookups (parameter, centre, level type) | ✅ | $\color{red}{\textsf{Not yet}}$ | n/a |
| Tabular metadata viewer | ✅ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| Binary data section decoding (Rust API) | ✅ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| Metadata editing | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |
| 2-D grid rendering with colormap | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ | $\color{red}{\textsf{Not yet}}$ |

Format-agnostic features:

- Hex and ASCII fallback view for files whose contents are not a recognized format. ✅
- Files without a recognized extension can still be opened through *Reopen Editor With… → Fieldglass Viewer*.

## Installation

Pre-built binaries for all supported platforms are bundled inside a single `.vsix` package. The extension selects the correct binary at runtime based on the host platform and architecture.

Supported platforms:

- Linux x64 (glibc), Linux arm64 (glibc)
- macOS x64, macOS arm64
- Windows x64, Windows arm64

### macOS

1. Download the latest `fieldglass-x.y.z.vsix` from the [releases page](https://github.com/D0ubleD0uble/fieldglass/releases/latest).
2. Open VS Code, run "Extensions: Install from VSIX..." from the command palette, and select the downloaded file. Alternatively, from a terminal:
   ```sh
   code --install-extension fieldglass-x.y.z.vsix
   ```
3. Reload the VS Code window.

### Linux

1. Download the latest `fieldglass-x.y.z.vsix` from the [releases page](https://github.com/D0ubleD0uble/fieldglass/releases/latest).
2. Open VS Code, run "Extensions: Install from VSIX..." from the command palette, and select the downloaded file. Alternatively, from a terminal:
   ```sh
   code --install-extension fieldglass-x.y.z.vsix
   ```
3. Reload the VS Code window.

### Windows

1. Download the latest `fieldglass-x.y.z.vsix` from the [releases page](https://github.com/D0ubleD0uble/fieldglass/releases/latest).
2. Open VS Code, run "Extensions: Install from VSIX..." from the command palette, and select the downloaded file. Alternatively, from PowerShell or Command Prompt:
   ```powershell
   code --install-extension fieldglass-x.y.z.vsix
   ```
3. Reload the VS Code window.

## Usage

Open any file with a supported extension. VS Code will use Fieldglass as the default editor and render a metadata table for each message in the file. To open an unrecognized file, right-click the file in the Explorer and choose "Open With...", then select "Fieldglass Viewer".

## Development

### Prerequisites

- Rust (stable toolchain)
- Node.js 22 or newer
- Visual Studio Code 1.85 or newer

### Repository layout

| Path | Purpose |
|---|---|
| `crates/fieldglass-core` | Format-agnostic traits and shared metadata types. |
| `crates/fieldglass-grib1` | GRIB1 parser, organized by section (`is.rs`, `pds.rs`, `gds.rs`, `bds.rs`) and WMO table lookups (`tables.rs`). |
| `crates/fieldglass-grib2` | GRIB2 reader stub. |
| `crates/fieldglass-netcdf` | NetCDF reader stub. |
| `crates/fieldglass-napi` | Node.js bindings exposed via napi-rs. The only crate that knows about Node. |
| `extension/` | TypeScript VS Code extension. Registers a custom read-only editor and renders a webview. |

### Initial setup

```sh
git clone git@github.com:D0ubleD0uble/fieldglass.git
cd fieldglass
npm install
```

The root `npm install` installs `@napi-rs/cli` (used to build the native module) and runs an `npm prepare` step that activates the repository's git hooks (see [Pre-commit hooks](#pre-commit-hooks) below).

### Building the native module

The compiled binary must be present in `extension/bin/` for the extension to load. From the repository root:

```sh
cd crates/fieldglass-napi
npx napi build --platform --release --output-dir ../../extension/bin
```

This produces a file such as `extension/bin/fieldglass.linux-x64-gnu.node` along with `extension/bin/index.d.ts`. Repeat after changing any Rust code.

### Building the extension

```sh
cd extension
npm install
npm run compile
```

For continuous compilation during development, run `npm run watch` instead.

### Running the extension

Open the repository in VS Code and press `F5`. An Extension Development Host window will launch with Fieldglass loaded. Open any supported file in that window to test changes.

### Tests

Run the full Rust test suite:

```sh
cargo test
```

Run tests for a single crate or a single test by name substring:

```sh
cargo test -p fieldglass-grib1
cargo test -p fieldglass-grib1 parse_pds
```

### Linting

```sh
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
```

The `fieldglass-napi` crate also enables `#![deny(clippy::all)]`, so warnings there are hard errors regardless.

### Pre-commit hooks

The repository uses the [`pre-commit`](https://pre-commit.com/) framework. Its config is at [`.pre-commit-config.yaml`](.pre-commit-config.yaml). The framework auto-fetches and isolates the pinned versions of `shellcheck`, `actionlint`, `gitleaks`, and `semgrep`, so you only need `pre-commit` itself plus the Rust + Node toolchains.

One-time setup (per clone):

```sh
pip install --user pre-commit          # or: pipx install pre-commit
cd /path/to/fieldglass
npm install                            # auto-runs `pre-commit install --hook-type pre-commit --hook-type pre-push`
```

Optional but recommended (the hooks gracefully report-and-fail if missing):

```sh
cargo install --locked cargo-deny      # advisory / license / source policy
```

What runs:

| Stage | Hook | What it does |
|---|---|---|
| `pre-commit` | `cargo fmt --check`, `cargo clippy -- -D warnings`, `tsc --noEmit` | Fast lints — usually under 3 s on incremental builds. |
| `pre-commit` | `check-yaml`, `check-json`, `check-toml`, `end-of-file-fixer`, `trailing-whitespace`, `check-merge-conflict`, `check-added-large-files` | File-hygiene polish. |
| `pre-commit` | `shellcheck`, `actionlint`, `gitleaks` | Lint shell scripts, GitHub Actions YAML, and scan staged diff for secrets. |
| `pre-push` | `cargo test --workspace`, `cargo deny check`, `npm audit --omit=dev`, `semgrep scan` | Slower correctness + security checks. |

Bypass with `git commit --no-verify` / `git push --no-verify` when you really must — CI (below) runs the same checks at full strength regardless.

### Continuous integration

The `pre-commit` job in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) installs the same toolchain (Rust, Node, Python, `cargo-deny`) and runs `pre-commit run --all-files` for both the commit and push stages — so local hooks and CI run *exactly* the same checks, no drift. A second job builds the native module and compiles the extension as a smoke test. Tagged versions matching `v*` trigger a release build that compiles the native module for all six supported targets, packages the `.vsix`, and publishes it to GitHub Releases.

### Security and static analysis

Three additional Tier-1 scanners run on every push and PR (results land in the repo's **Security** tab):

- **[Semgrep](https://semgrep.dev/)** SAST in [`.github/workflows/semgrep.yml`](.github/workflows/semgrep.yml) — pattern-based security rules across Rust + TypeScript (`p/default`, `p/security-audit`, `p/owasp-top-ten`, `p/rust`, `p/typescript`, `p/secrets`). Same scan also runs locally on `git push`.
- **[CodeQL](https://codeql.github.com/)** semantic analysis in [`.github/workflows/codeql.yml`](.github/workflows/codeql.yml) — JavaScript/TypeScript and Rust, with the `security-extended` query suite.
- **[Dependabot](https://docs.github.com/en/code-security/dependabot)** in [`.github/dependabot.yml`](.github/dependabot.yml) — weekly version + security update PRs across `cargo`, `npm` (extension + root), and `github-actions`.

## Adding a new GRIB1 metadata field

1. Parse the field in the relevant section module under `crates/fieldglass-grib1/src/`.
2. Expose the value on the section struct.
3. Populate it on `MessageMeta` in `crates/fieldglass-napi/src/lib.rs`.
4. Add the corresponding camelCase field to the `MessageMeta` interface in `extension/src/provider.ts`.
5. Render it in the webview table in the same file.

The napi-rs bindings automatically convert `snake_case` Rust field names to `camelCase` TypeScript fields.

## License

Fieldglass is dual-licensed under either of:

- MIT License ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)

at your option. This is the same dual-licensing convention used by most of the Rust ecosystem; downstream consumers can pick whichever fits their needs.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Fieldglass by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
