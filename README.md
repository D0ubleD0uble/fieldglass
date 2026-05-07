# Fieldglass

A Visual Studio Code extension for viewing meteorological binary data files (GRIB1, GRIB2, NetCDF) directly in the editor. Built on a Rust native module with no Python dependencies.

[Latest release](https://github.com/D0ubleD0uble/fieldglass/releases/latest)

## Status

Phase 1 of the project is in progress: read-only metadata viewing for GRIB1. GRIB2, NetCDF, metadata editing, and grid field rendering are on the roadmap. See [PLAN.md](PLAN.md) for the full phase breakdown.

## Features

| Feature | Status |
|---|---|
| Format detection from magic bytes (GRIB1, GRIB2, NetCDF classic, NetCDF-4/HDF5) | Implemented |
| File associations for `.grb`, `.grib`, `.grib1`, `.grb1`, `.grb2`, `.grib2`, `.nc`, `.nc4`, `.netcdf` | Implemented |
| Optional viewer for files without a recognized extension | Implemented |
| GRIB1 Indicator Section parsing | Implemented |
| GRIB1 Product Definition Section parsing (parameter, level, reference time, forecast period) | Implemented |
| GRIB1 Grid Description Section parsing (Latitude/Longitude, Gaussian, Polar Stereographic, Lambert Conformal) | Implemented |
| WMO ON388 lookups for parameters, originating centres, and level types | Implemented |
| Tabular metadata view for all messages in a GRIB1 file | Implemented |
| Hex and ASCII fallback view for unrecognized files | Implemented |
| GRIB1 Binary Data Section decoding | Planned |
| Metadata editing with full undo and redo lifecycle | Planned |
| Two-dimensional grid field rendering with colormap controls | Planned |
| GRIB2 reader | Planned |
| NetCDF reader | Planned |

## Supported file types

| Format | Extensions | Detection | Metadata view |
|---|---|---|---|
| GRIB Edition 1 | `.grb`, `.grib`, `.grib1`, `.grb1` | Yes | Yes |
| GRIB Edition 2 | `.grb2`, `.grib2` | Yes | Not yet |
| NetCDF (classic, 64-bit, NetCDF-4) | `.nc`, `.nc4`, `.netcdf` | Yes | Not yet |

Files without a recognized extension can still be opened through the VS Code "Reopen Editor With..." command and selecting "Fieldglass Viewer".

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

The root `npm install` installs `@napi-rs/cli`, which is invoked from the napi crate to produce the platform-specific `.node` binary.

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
cargo clippy --all-targets -- -D warnings
```

The `fieldglass-napi` crate enables `#![deny(clippy::all)]`, so warnings there are hard errors.

### Continuous integration

Every push to `master` and every pull request runs a Linux x64 smoke test that builds the native module and compiles the TypeScript extension. Tagged versions matching `v*` trigger a release build that compiles the native module for all six supported targets, packages the `.vsix`, and publishes it to GitHub Releases.

## Adding a new GRIB1 metadata field

1. Parse the field in the relevant section module under `crates/fieldglass-grib1/src/`.
2. Expose the value on the section struct.
3. Populate it on `MessageMeta` in `crates/fieldglass-napi/src/lib.rs`.
4. Add the corresponding camelCase field to the `MessageMeta` interface in `extension/src/provider.ts`.
5. Render it in the webview table in the same file.

The napi-rs bindings automatically convert `snake_case` Rust field names to `camelCase` TypeScript fields.
