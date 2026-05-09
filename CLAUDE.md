# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Fieldglass is a VS Code extension for viewing meteorological binary data formats (GRIB1, GRIB2, NetCDF). It is a Cargo workspace of Rust crates compiled into a Node.js native module via napi-rs, consumed by a TypeScript VS Code extension that registers a custom read-only editor for `.grb`, `.grib*`, `.nc*` files.

`PLAN.md` is the source of truth for phased scope: Phase 1 GRIB1 metadata viewer (current), Phase 2 metadata editing, Phase 3 grid data visualization, Phase 4 GRIB2, Phase 5+ NetCDF.

## Architecture

The system is layered so the parsing core has zero knowledge of Node.js or VS Code:

- `crates/fieldglass-core` — format-agnostic types (`Metadata`, `Parameter`, `Level`, `GridDefinition`), the `FormatReader` / `DataMessage` traits, magic-byte format detection (`detect_from_bytes`), and `FieldglassError`. **All format crates depend on it; it depends on nothing else in the workspace.**
- `crates/fieldglass-grib1` — full GRIB1 parser. Section-per-file: `is.rs` (Indicator), `pds.rs` (Product Definition), `gds.rs` (Grid Description), `bds.rs` (Binary Data — stub), plus `tables.rs` (WMO ON388 lookups for parameter/centre/level type) and `reader.rs` (top-level `Grib1Reader::from_bytes` that scans messages by walking IS total-length offsets).
- `crates/fieldglass-grib2`, `crates/fieldglass-netcdf` — stubs awaiting later phases.
- `crates/fieldglass-napi` — `cdylib` exposing N-API functions (`detect_bytes`, `open_grib1`) and the flat `MessageMeta` struct that crosses the JS/Rust boundary. Keep this crate thin: format logic belongs in the format crates.
- `extension/` — TypeScript VS Code extension. `provider.ts` registers `FieldglassEditorProvider` (a `CustomReadonlyEditorProvider`) for two view types: `fieldglass.viewer` (default for known extensions) and `fieldglass.viewer.any` (option-priority, matches `*` so users can open arbitrary files). It reads bytes via `vscode.workspace.fs.readFile` (NOT a native fs path — this matters for remote/virtual workspaces) and renders an HTML table in a webview with `enableScripts: false`.

### Native binary loading

The compiled `.node` is loaded lazily by `loadNative()` in `extension/src/provider.ts`. The filename is computed from `process.platform` + `process.arch` + ABI suffix (`-gnu` linux, `-msvc` win, none macOS), e.g. `fieldglass.linux-x64-gnu.node`. Binaries live in `extension/bin/` and must be present there for the platforms the extension ships to. A missing binary surfaces as a VS Code error toast — the extension still activates so non-matching platforms don't crash.

### Adding a new GRIB1 message field

The data flow for any new metadata field is: parse it in the relevant section module → expose it on the section struct → use it in `crates/fieldglass-napi/src/lib.rs` to populate `MessageMeta` → add the camelCase field on the `MessageMeta` interface in `extension/src/provider.ts` → render it in `renderHtml`. napi-rs auto-converts `snake_case` Rust field names to `camelCase` in the generated TS bindings.

## Commands

Workspace is at the repo root (`Cargo.toml` lists members); the VS Code extension is a separate npm package at `extension/`.

### Rust

```bash
cargo build                                   # build all crates
cargo build -p fieldglass-grib1               # build a single crate
cargo test                                    # run all tests
cargo test -p fieldglass-grib1                # tests for one crate
cargo test -p fieldglass-grib1 parse_pds      # single test by name substring
cargo clippy --all-targets -- -D warnings     # lints (napi crate has #![deny(clippy::all)])
```

### Native module (napi-rs)

The native binary must be rebuilt and placed in `extension/bin/` whenever Rust code changes:

```bash
cd extension
npx napi build --platform --release --output-dir bin --manifest-path ../crates/fieldglass-napi/Cargo.toml
```

This produces e.g. `extension/bin/fieldglass.linux-x64-gnu.node` plus `extension/bin/index.d.ts` (TS types). To cross-compile for distribution, add `--target <triple>` and run once per platform.

### Extension

```bash
cd extension
npm install
npm run compile          # tsc → out/
npm run watch            # tsc --watch during development
```

To test in VS Code: open the repo in VS Code and press F5 (an Extension Development Host launches with the extension loaded — see `.vscode/`). Open any `.grb`/`.grib`/`.nc` file, or use "Reopen Editor With… → Fieldglass Viewer" for arbitrary files.

## Conventions

- The core crate must remain free of format-specific imports and free of `napi` types. Cross the boundary only in `fieldglass-napi`.
- `FormatReader` / `DataMessage` traits in `core/src/reader.rs` currently use static-style signatures (no `&self`); when implementing new format crates, follow whatever shape ends up there rather than inferring from the trait names.
- WMO lookup tables (`fieldglass-grib1/src/tables.rs`) are the single source of truth for parameter/centre/level-type human-readable names — extend the tables rather than hardcoding strings at the napi or TS layer.
- Webview HTML in `provider.ts` runs with `enableScripts: false`. If you need interactivity later, switch the option and add a CSP — don't quietly enable scripts.
