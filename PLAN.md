# Fieldglass — Project Plan

A Visual Studio Code extension for reading and manipulating meteorological binary data formats (GRIB1, GRIB2, NetCDF), built on a Rust native module with no Python dependencies.

---

## Name

**`fieldglass`**

In atmospheric science, all data is described as *fields* — pressure field, wind field, temperature field. A field glass is a traditional optical instrument for outdoor observation. The name carries both meanings: viewing data fields through a lens. It is not tied to any specific format, making it appropriate as the project expands beyond GRIB1.

---

## Goals

- Phase 1: View and edit GRIB1 metadata inside VS Code
- Phase 2: Metadata editing with full undo/redo lifecycle
- Phase 3: Grid data visualization (2D field rendering)
- Phase 4: GRIB2 support
- Phase 5+: NetCDF and other geoscientific formats

---

## Architecture Decision: napi-rs over WebAssembly

Two approaches were evaluated:

**Option A — Rust compiled to WebAssembly**
- VS Code officially supports WASM in extensions (VS Code blog, May 2024)
- The WASI Component Model does not support low-level (C-style) pointers — passing binary file contents across the boundary requires copies
- VS Code's async APIs cannot yet be proxied into WASM (blocked on WASI 0.3 async support)
- File I/O must go through a JavaScript bridge — every large file read involves a full copy into WASM memory
- Better for vscode.dev (browser-based VS Code), but poor fit for file-format tooling today

**Option B — Rust native module via napi-rs** ✓ *chosen*
- Direct file system access with zero-copy binary parsing
- Maximum performance for large files and future grid data decoding
- napi-rs generates TypeScript types automatically from Rust signatures
- Platform-specific binaries required (linux/mac/windows × x64/arm64), but napi-rs's built-in GitHub Actions templates make this manageable
- Cannot step into native code from VS Code's debugger — accepted tradeoff
- The core Rust library stays completely independent of the bindings layer

**Why napi-rs wins for this use case:** GRIB files can be large; direct native I/O matters. Future grid visualization will require decoding large packed float arrays — native performance is critical. The WASM async gap is a real unsolved architectural problem for VS Code integrations today.

---

## GRIB Library Landscape

Before designing the parsing layer, the available Rust crates were evaluated:

| Crate | GRIB1 | GRIB2 | Notes |
|---|---|---|---|
| `grib` (grib-rs) | No | Yes | Well-maintained, GRIB2 only |
| `grib1_reader` | Partial | No | Only supports Grid 10 (RotatedLatLon); ~100 downloads/month |
| `gribberish` | No | Yes | GRIB2 only |

**Conclusion:** No existing crate adequately covers GRIB1. The `fieldglass-grib1` crate will implement its own parser, starting with metadata sections (IS, PDS, GDS). The `fieldglass-grib2` crate can wrap or be informed by `grib-rs` when that phase begins.

---

## Project Structure

```
fieldglass/
├── Cargo.toml                       # Workspace
├── PLAN.md                          # This document
│
├── crates/
│   ├── fieldglass-core/             # Format-agnostic traits and shared types
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── reader.rs            # FormatReader trait
│   │       ├── message.rs           # DataMessage trait
│   │       ├── metadata.rs          # Metadata, Parameter, Level, Grid types
│   │       ├── field.rs             # DataField (for future grid decoding)
│   │       └── error.rs             # FieldglassError
│   │
│   ├── fieldglass-grib1/            # GRIB1 format implementation
│   │   └── src/
│   │       ├── lib.rs               # impl FormatReader for Grib1Reader
│   │       ├── sections/
│   │       │   ├── is.rs            # Indicator Section (bytes 0–3, edition)
│   │       │   ├── pds.rs           # Product Definition Section (parameter, level, time)
│   │       │   ├── gds.rs           # Grid Description Section (grid geometry)
│   │       │   └── bds.rs           # Binary Data Section — parsed lazily
│   │       └── tables/
│   │           ├── parameters.rs    # WMO parameter tables (bundled as include_bytes!)
│   │           └── levels.rs        # Level type definitions
│   │
│   ├── fieldglass-grib2/            # GRIB2 — stubbed, implemented in Phase 4
│   │   └── src/lib.rs
│   │
│   ├── fieldglass-netcdf/           # NetCDF — stubbed, implemented in Phase 5+
│   │   └── src/lib.rs
│   │
│   └── fieldglass-napi/             # napi-rs bindings — only layer that knows Node
│       ├── build.rs
│       └── src/
│           ├── lib.rs               # open(), format dispatch, #[napi] exports
│           └── types.rs             # #[napi(object)] structs exposed to TypeScript
│
├── extension/                       # VS Code extension (TypeScript)
│   ├── src/
│   │   ├── extension.ts             # activate() — registers providers
│   │   └── provider.ts              # FieldglassEditorProvider (CustomReadonlyEditorProvider)
│   │                                # Phase 2+: split into provider.ts + document.ts + webview/
│   ├── out/                         # compiled JS (gitignored)
│   ├── package.json
│   └── tsconfig.json
│
└── .github/
    └── workflows/
        ├── build.yml                # Cross-compile .node binaries (6 targets)
        └── release.yml              # Bundle and publish .vsix
```

---

## Core Trait Design (`fieldglass-core`)

These traits are the stable contract between format implementations and the rest of the system. All downstream code depends on these interfaces, never on format-specific types.

```rust
// reader.rs
pub trait FormatReader: Send + Sync {
    fn format_name(&self) -> &'static str;
    fn message_count(&self) -> usize;
    fn message(&self, index: usize) -> Result<Box<dyn DataMessage>>;
    fn messages(&self) -> impl Iterator<Item = Result<Box<dyn DataMessage>>>;
}

// message.rs
pub trait DataMessage: Send + Sync {
    fn metadata(&self) -> &Metadata;
    fn grid(&self) -> Option<&GridDefinition>;
    fn decode_field(&self) -> Result<DataField>;  // lazy — only called on demand
}

// metadata.rs
pub struct Metadata {
    pub parameter: Parameter,
    pub level: Level,
    pub reference_time: DateTime,
    pub forecast_offset: Duration,
    pub originating_centre: Option<String>,
    pub raw_fields: IndexMap<String, String>,  // format-specific extras without polluting the struct
}
```

The `raw_fields` map lets each format surface format-specific metadata (GRIB1 centre/sub-centre codes, NetCDF global attributes, etc.) without adding format-specific fields to the shared struct.

---

## Format Dispatcher (`fieldglass-napi`)

The napi layer is a thin dispatcher. It never reaches past `FormatReader` after the initial `open()` call. Adding a new format means adding one crate and one match arm — nothing else changes.

```rust
// fieldglass-napi/src/lib.rs
#[napi]
pub fn open(path: String) -> Result<FieldglassFile> {
    let reader: Box<dyn FormatReader> = match detect_format(&path)? {
        Format::Grib1  => Box::new(fieldglass_grib1::open(&path)?),
        Format::Grib2  => Box::new(fieldglass_grib2::open(&path)?),
        Format::NetCdf => Box::new(fieldglass_netcdf::open(&path)?),
    };
    Ok(FieldglassFile { inner: Arc::new(reader) })
}
```

### napi-exposed types

Keep JS-facing types simple and flat — avoid exposing complex Rust structs directly:

```rust
#[napi(object)]
pub struct MessageMeta {
    pub message_index: u32,
    pub offset_bytes: u32,
    pub parameter_name: String,
    pub parameter_units: String,
    pub level_type: String,
    pub level_value: f64,
    pub reference_time: String,    // ISO 8601
    pub forecast_hours: i32,
    pub originating_centre: String,
    pub grid_type: Option<String>,
    pub format: String,            // "grib1" | "grib2" | "netcdf"
    pub raw_fields: Vec<(String, String)>,
}
```

---

## VS Code Extension

### File associations

Registered in `package.json` upfront for all planned formats:

```json
"customEditors": [{
  "viewType": "fieldglass.dataViewer",
  "displayName": "Fieldglass Data Viewer",
  "selector": [
    { "filenamePattern": "*.grib"  },
    { "filenamePattern": "*.grb"   },
    { "filenamePattern": "*.grib1" },
    { "filenamePattern": "*.grib2" },
    { "filenamePattern": "*.grb2"  },
    { "filenamePattern": "*.nc"    },
    { "filenamePattern": "*.nc4"   }
  ]
}]
```

### Editor lifecycle

- `FieldglassDocument` implements `CustomDocument`, calls `native.open(uri.fsPath)` and holds the parsed message list
- `FieldglassEditorProvider` implements `CustomEditorProvider`, creates the webview panel and wires document ↔ webview messages
- Webview renders a metadata table/form from messages posted by the provider
- For Phase 1, use `CustomReadonlyEditorProvider`; upgrade to `CustomEditorProvider` in Phase 2

### Metadata edit flow (Phase 2)

```
Webview (form edit)
  → postMessage({ type: 'edit', field, value })
  → FieldglassEditorProvider
    → native.setMetadataField(path, messageIndex, field, value)
    → patches bytes in-place, returns updated MessageMeta
  → postMessage({ type: 'update', meta }) back to webview
  → document.onDidChange fires (enables dirty indicator + save)
```

---

## GRIB1 Section Parsing

GRIB1 metadata lives entirely in sections 0–2. The Binary Data Section (section 4) is never read during metadata-only operations.

| Section | Name | Key fields |
|---|---|---|
| 0 | Indicator Section | `GRIB` magic, total length, edition = 1 |
| 1 | Product Definition Section | centre, parameter ID, table version, level type/value, reference time, forecast period |
| 2 | Grid Description Section | grid type, dimensions, lat/lon bounds, resolution |
| 3 | Bit Map Section | optional; skip for metadata |
| 4 | Binary Data Section | packed grid values — **lazy, skip until needed** |
| End | End Section | `7777` |

WMO parameter and level tables are finite and version-stable for GRIB1. They will be bundled as `include_bytes!()` and parsed at crate init time.

---

## Distribution

napi-rs's `@napi-rs/cli` manages cross-platform build matrices. The CI pipeline produces six pre-built `.node` binaries:

| Platform | Architectures |
|---|---|
| Linux (glibc) | x64, arm64 |
| macOS | x64, arm64 |
| Windows | x64, arm64 |

Pre-built binaries are attached to GitHub Releases. A `scripts/postinstall.js` downloads the correct one at install time. The `.vsix` uses VS Code's `"platformSpecific": true` packaging to include only the relevant binary per platform.

---

## Phase Plan

### Phase 0 — Hello-world extension (complete)
- [x] `fieldglass-core`: stub traits and `detect_format()` (extension-based)
- [x] `fieldglass-napi`: `detect()` exported via napi; `.node` binary built for Linux x64
- [x] Extension scaffold: `package.json`, `tsconfig.json`, `extension.ts`, `provider.ts`
- [x] `CustomReadonlyEditorProvider` renders static "Detected GRIB1 — parsing not yet implemented" webview
- [x] File associations for `.grb`, `.grib`, `.grib1`, `.grb1`, `.grb2`, `.grib2`, `.nc`, `.nc4`, `.netcdf`
- [x] Language contributions for all three format families

### Phase 1 — GRIB1 metadata reading
- [ ] `fieldglass-core`: finalise `FormatReader`, `DataMessage`, `Metadata` traits
- [ ] `fieldglass-grib1`: implement IS, PDS, GDS parsers; bundle parameter/level tables
- [ ] `fieldglass-napi`: `open()` returns populated `Vec<MessageMeta>`
- [ ] Extension: webview metadata table (replace static message with real data)
- [ ] CI: cross-compile `.node` for all 6 targets

### Phase 2 — Metadata editing
- [ ] Upgrade to `CustomEditorProvider` with `CustomDocumentEditEvent`
- [ ] `fieldglass-napi`: expose `set_metadata_field()` with byte-level patching
- [ ] Undo/redo stack via VS Code document edit events
- [ ] Webview form inputs for editable PDS fields

### Phase 3 — Grid visualization
- [ ] `fieldglass-grib1`: implement lazy BDS decoder
- [ ] `fieldglass-core`: `DataField` trait with lat/lon iterators
- [ ] Webview Canvas/WebGL renderer for 2D grid fields
- [ ] Colormap and legend controls

### Phase 4 — GRIB2
- [ ] `fieldglass-grib2`: implement `FormatReader` for GRIB2 (evaluate wrapping grib-rs)
- [ ] Add `Format::Grib2` arm to dispatcher
- [ ] Extend file associations

### Phase 5+ — NetCDF and others
- [ ] `fieldglass-netcdf`: implement `FormatReader` (evaluate netcdf-rs or nczarr)
- [ ] Same pattern: new crate, one dispatch arm, no extension changes
