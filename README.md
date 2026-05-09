# Fieldglass

[![CI](https://github.com/D0ubleD0uble/fieldglass/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/D0ubleD0uble/fieldglass/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/D0ubleD0uble/fieldglass/branch/master/graph/badge.svg)](https://codecov.io/gh/D0ubleD0uble/fieldglass)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A Visual Studio Code extension for viewing meteorological binary data files (GRIB1, GRIB2, NetCDF) directly in the editor. Built on a Rust native module — installed extensions have **no Python runtime dependency**. (Python is only used by the dev tooling, not the shipped extension.)

![Fieldglass viewer rendering a multi-message GRIB1 file](extension/media/screenshot.png)

[Latest release](https://github.com/D0ubleD0uble/fieldglass/releases/latest)

## Status

First public beta. Read-only metadata viewing for GRIB1 is in. The GRIB1 Binary Data Section decoder (with bitmap-section support) is implemented at the Rust API level via `decode_grid`, but not yet wired into a 2-D visualization in the webview. GRIB2 and NetCDF parsing, metadata editing, and field rendering are on the roadmap — see the [feature matrix](#feature-matrix) below for what works today.

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

## Known limitations

This is a beta. Things to be aware of:

- **No metadata editing in the viewer.** The Rust API has byte-level patching for the forecast period (P1) and the webview retains the full undo/redo wiring, but the editable affordance is hidden in beta until general PDS-field editing lands. For now Fieldglass is a read-only viewer.
- **No 2-D field rendering.** GRIB1 grid values are decoded by the Rust API (`Grib1Reader::decode_message_values` / napi `decode_grid`) but not yet visualized in a webview canvas.
- **GRIB2 and NetCDF: detection only.** Magic-byte detection works and routes the file to the viewer, but parsing isn't implemented yet — those files will surface "no messages found."
- **GRIB1 GDS coverage:** Lat/Lon, Gaussian, Polar Stereographic, and Lambert Conformal grids are parsed. Reduced grids, rotated/oblique projections, and predefined grids (`grid_number != 255`) are not yet supported and will render as `unsupported`.
- **Parameter table coverage:** WMO ON388 Table 2 (versions 1–3) only. ECMWF local tables (versions 128+) and other centre-specific extensions resolve as `Unknown`.
- **Large files:** the extension reads the whole file into memory via `vscode.workspace.fs.readFile` to keep remote/virtual workspaces working. Multi-GB GRIB archives are not the target use case yet.

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

- Rust (stable toolchain, edition 2024 — Rust 1.85 or newer)
- Node.js 22 or newer
- Python 3.10 or newer (only for the dev tooling — `pre-commit`, `semgrep`)
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
pipx install pre-commit              # or: pip install --user pre-commit
npm install                          # installs @napi-rs/cli and activates git hooks
```

The root `npm install` installs `@napi-rs/cli` (used to build the native module) and runs an `npm prepare` step that activates the repository's git hooks (see [Pre-commit hooks](#pre-commit-hooks) below). If `pre-commit` isn't on `PATH` the prepare step prints a hint and continues; you can install it later and re-run `npm install`.

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
pipx install pre-commit                # preferred — works everywhere
# or, on systems without pipx:
pip install --user pre-commit          # may need --break-system-packages on PEP 668 distros
# or, fully isolated venv:
python3 -m venv ~/.venvs/fieldglass && ~/.venvs/fieldglass/bin/pip install pre-commit

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

- **[Semgrep](https://semgrep.dev/)** SAST in [`.github/workflows/semgrep.yml`](.github/workflows/semgrep.yml) — pattern-based security rules across Rust + TypeScript (`p/default`, `p/security-audit`, `p/owasp-top-ten`, `p/rust`, `p/typescript`, `p/secrets`). The same scan also runs locally on `git push`.
- **[CodeQL](https://codeql.github.com/)** semantic analysis in [`.github/workflows/codeql.yml`](.github/workflows/codeql.yml) — JavaScript/TypeScript and Rust, with the `security-extended` query suite.
- **[Dependabot](https://docs.github.com/en/code-security/dependabot)** in [`.github/dependabot.yml`](.github/dependabot.yml) — weekly version + security update PRs across `cargo`, `npm` (extension + root), and `github-actions`.

The Semgrep and CodeQL workflows are gated on `github.event.repository.visibility == 'public'` because GitHub Code Scanning's SARIF upload requires either a public repo or GitHub Advanced Security. They sit dormant on private repos and self-activate the moment the repo is flipped public. Dependabot runs regardless of visibility.

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

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Fieldglass by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
