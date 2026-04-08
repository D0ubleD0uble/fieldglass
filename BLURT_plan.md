# Fieldglass — Blurt Scaffolding Plan

This document defines how to use the [Blurt compiler](~/Code/Blurt) to bootstrap the initial Rust workspace for the Fieldglass project. Blurt generates structure (Cargo.toml, modules, trait stubs, `todo!()` implementations) — we fill in the logic.

See `PLAN.md` for the full architectural design this scaffolding targets.

---

## Approach

Blurt can scaffold the Rust workspace crates cleanly using its `library` type with `module`, `struct`, `trait`, and `fn` constructs. It cannot generate the VS Code TypeScript extension or napi-rs bindings directly — those layers are added on top of the generated workspace.

**What Blurt generates:**
- Workspace `Cargo.toml`
- `fieldglass-core`: traits, shared types, error types
- `fieldglass-grib1`: section parsers, parameter tables, reader impl stub
- `fieldglass-grib2`: minimal stub crate
- `fieldglass-netcdf`: minimal stub crate

**What gets added manually after generation:**
- `fieldglass-napi` crate (napi-rs requires `build.rs` and specific Cargo config not expressible in Blurt)
- `extension/` — VS Code TypeScript extension
- `.github/workflows/` — cross-platform build matrix
- `scripts/postinstall.js` — binary download helper

---

## File Location

The `.blurt` source files live **outside** both the Blurt compiler repo and the grib_extension repo:

```
~/Code/rust/
├── fieldglass.blurt          # Workspace root spec (uses include)
├── specs/
│   ├── fieldglass-core.blurt
│   ├── fieldglass-grib1.blurt
│   ├── fieldglass-grib2.blurt
│   └── fieldglass-netcdf.blurt
└── fieldglass/               # Compiler output (generated project)
```

The generated project directory is `~/Code/rust/fieldglass/`.

---

## Blurt Spec Files

### `~/Code/rust/fieldglass.blurt`

```blurt
// Fieldglass — Meteorological data format viewer for VS Code
// Workspace root: includes all member crate specs

workspace Fieldglass {
    members: [fieldglass-core, fieldglass-grib1, fieldglass-grib2, fieldglass-netcdf]
}

include "specs/fieldglass-core.blurt"
include "specs/fieldglass-grib1.blurt"
include "specs/fieldglass-grib2.blurt"
include "specs/fieldglass-netcdf.blurt"
```

---

### `~/Code/rust/specs/fieldglass-core.blurt`

The stable interface contract. All format crates depend on this; nothing here depends on any format.

```blurt
project FieldglassCore {
    type: library
    version: "0.1.0"
    description: "Format-agnostic traits and shared types for the Fieldglass data viewer"
}

// =================================================================
// ERROR TYPES
// =================================================================

module error {
    enum FieldglassError {
        Io,
        Parse,
        UnsupportedFormat,
        UnsupportedSection,
        InvalidMagic,
        OutOfRange,
    }
}

// =================================================================
// SHARED METADATA TYPES
// =================================================================

module metadata {
    /// A human-readable parameter (e.g. "Temperature", "Wind Speed")
    struct Parameter {
        name: string
        abbreviation: string
        units: string
        id: i32
    }

    /// A vertical level descriptor
    struct Level {
        level_type: string
        value: f64
        units: string
    }

    /// Geographic grid geometry
    struct GridDefinition {
        grid_type: string
        ni: i32
        nj: i32
        lat_first: f64
        lon_first: f64
        lat_last: f64
        lon_last: f64
        di: f64
        dj: f64
    }

    /// All metadata for a single data message, format-agnostic.
    /// raw_fields carries format-specific extras without polluting the struct.
    struct Metadata {
        parameter: Parameter
        level: Level
        reference_time: string
        forecast_hours: i32
        originating_centre: string
        grid?: GridDefinition
    }
}

// =================================================================
// CORE TRAITS
// =================================================================

module reader {
    /// Implemented by each format crate's top-level reader
    trait FormatReader {
        fn format_name() -> string
        fn message_count() -> i32
        fn message(index: i32) -> DataMessage
    }

    /// Implemented by each format's message type
    trait DataMessage {
        fn metadata() -> Metadata
        fn grid() -> GridDefinition
        /// Decode the actual grid values — lazy, only called on demand
        fn decode_field() -> [f64]
    }
}

// =================================================================
// FORMAT DETECTION
// =================================================================

module detect {
    enum Format {
        Grib1,
        Grib2,
        NetCdf,
        Unknown,
    }

    /// Detect format from file magic bytes and extension
    fn detect_format(path: string) -> Format
}

export {
    error::FieldglassError,
    metadata::Parameter,
    metadata::Level,
    metadata::GridDefinition,
    metadata::Metadata,
    reader::FormatReader,
    reader::DataMessage,
    detect::Format,
    detect::detect_format,
}
```

---

### `~/Code/rust/specs/fieldglass-grib1.blurt`

GRIB1 parser. Sections are parsed from raw bytes; BDS is lazy.

```blurt
project FieldglassGrib1 {
    type: library
    version: "0.1.0"
    description: "GRIB edition 1 format reader implementing the Fieldglass core traits"
}

hints {
    Grib1Reader: "Implements FormatReader from fieldglass-core. Opens file, scans message offsets at init, parses PDS on demand."
    Grib1Message: "Implements DataMessage. Holds byte offset into source file, parses BDS only when decode_field() is called."
    is::parse_indicator: "Read bytes 0-3 for GRIB magic, bytes 4-6 for total length, byte 7 for edition number. Return error if edition != 1."
    pds::parse_product_definition: "GRIB1 PDS is 28+ bytes. Centre=bytes[4], table_version=bytes[3], param_id=bytes[8], level_type=bytes[9], level_value=bytes[10..11], reference time from bytes[12..17], forecast period from bytes[18..19]."
    gds::parse_grid_description: "Only parse if section 2 present flag is set in PDS byte[7]. Grid type in byte[5] of GDS."
    tables::lookup_parameter: "WMO GRIB1 parameter tables are versioned. Bundle as include_bytes! from a static table file."
}

// =================================================================
// SECTION PARSERS
// =================================================================

module is {
    struct IndicatorSection {
        total_length: i32
        edition: i32
    }

    fn parse_indicator(bytes: [u8]) -> IndicatorSection
}

module pds {
    struct ProductDefinition {
        table_version: i32
        originating_centre: i32
        generating_process: i32
        parameter_id: i32
        level_type: i32
        level_value_1: i32
        level_value_2: i32
        reference_year: i32
        reference_month: i32
        reference_day: i32
        reference_hour: i32
        reference_minute: i32
        time_unit: i32
        p1: i32
        p2: i32
        time_range: i32
        has_gds: bool
        has_bms: bool
    }

    fn parse_product_definition(bytes: [u8]) -> ProductDefinition
}

module gds {
    struct GridDescription {
        grid_type: i32
        ni: i32
        nj: i32
        lat_first: f64
        lon_first: f64
        lat_last: f64
        lon_last: f64
        di: f64
        dj: f64
    }

    fn parse_grid_description(bytes: [u8]) -> GridDescription
}

module bds {
    struct BinaryDataSection {
        scale_factor: f64
        reference_value: f64
        bits_per_value: i32
        grid_point_count: i32
    }

    /// Parse BDS header only — do not unpack values
    fn parse_bds_header(bytes: [u8]) -> BinaryDataSection

    /// Unpack all grid point values — expensive, call lazily
    fn decode_values(bytes: [u8], header: BinaryDataSection) -> [f64]
}

// =================================================================
// WMO PARAMETER TABLES
// =================================================================

module tables {
    struct ParameterEntry {
        id: i32
        table_version: i32
        name: string
        abbreviation: string
        units: string
    }

    fn lookup_parameter(id: i32, table_version: i32) -> ParameterEntry
    fn lookup_level_name(level_type: i32) -> string
    fn lookup_centre_name(centre_id: i32) -> string
}

// =================================================================
// READER
// =================================================================

module reader {
    struct Grib1Message {
        message_index: i32
        byte_offset: i32
        total_length: i32
        pds: pds::ProductDefinition
        gds?: gds::GridDescription
    }

    struct Grib1Reader {
        path: string
        messages: [Grib1Message]
    }

    fn open(path: string) -> Grib1Reader
    fn message_count(reader: Grib1Reader) -> i32
    fn read_message(reader: Grib1Reader, index: i32) -> Grib1Message
    fn decode_field(reader: Grib1Reader, index: i32) -> [f64]
}

export {
    reader::Grib1Reader,
    reader::Grib1Message,
    reader::open,
}
```

---

### `~/Code/rust/specs/fieldglass-grib2.blurt`

Minimal stub. Implemented in Phase 4.

```blurt
project FieldglassGrib2 {
    type: library
    version: "0.1.0"
    description: "GRIB edition 2 format reader — stub, implemented in Phase 4"
}

hints {
    Grib2Reader: "Will implement FormatReader from fieldglass-core. Consider wrapping the grib-rs crate (crates.io: grib) which has mature GRIB2 support."
}

module reader {
    struct Grib2Reader {
        path: string
    }

    fn open(path: string) -> Grib2Reader
}

export {
    reader::Grib2Reader,
    reader::open,
}
```

---

### `~/Code/rust/specs/fieldglass-netcdf.blurt`

Minimal stub. Implemented in Phase 5+.

```blurt
project FieldglassNetcdf {
    type: library
    version: "0.1.0"
    description: "NetCDF format reader — stub, implemented in Phase 5+"
}

hints {
    NetcdfReader: "Will implement FormatReader from fieldglass-core. Evaluate netcdf-rs or netcdf4-rs crates for underlying I/O."
}

module reader {
    struct NetcdfReader {
        path: string
    }

    fn open(path: string) -> NetcdfReader
}

export {
    reader::NetcdfReader,
    reader::open,
}
```

---

## Generation Workflow

### 1. Build the Blurt compiler (if not already built)

```bash
cd ~/Code/Blurt/compiler
cargo build --release
# Binary at: ~/Code/Blurt/compiler/target/release/blurt
```

### 2. Create the spec files

Create the directory structure and files as defined above:

```bash
mkdir -p ~/Code/rust/specs
# Write fieldglass.blurt and all specs/fieldglass-*.blurt files
```

### 3. Run a dry-run to validate the spec

```bash
~/Code/Blurt/compiler/target/release/blurt compile \
    ~/Code/rust/fieldglass.blurt \
    --output ~/Code/rust/fieldglass \
    --dry-run
```

Review the diff output to confirm the generated structure matches `PLAN.md`.

### 4. Generate the project

```bash
~/Code/Blurt/compiler/target/release/blurt compile \
    ~/Code/rust/fieldglass.blurt \
    --output ~/Code/rust/fieldglass
```

### 5. Verify the build compiles

```bash
cd ~/Code/rust/fieldglass
cargo build
```

All implementations will be `todo!()` stubs — the build confirms structure is correct.

---

## Post-Generation: Manual Additions

After generation, these components are added by hand:

### `fieldglass-napi` crate

napi-rs requires a custom `build.rs` and specific `[lib]` configuration that Blurt cannot express. Add manually:

```
fieldglass/crates/fieldglass-napi/
├── Cargo.toml        # [lib] cdylib, napi-rs deps, build-deps
├── build.rs          # extern crate napi_build; fn main() { napi_build::setup(); }
└── src/
    ├── lib.rs        # #[napi] open(), FieldglassFile wrapper
    └── types.rs      # #[napi(object)] MessageMeta, GridMeta structs
```

### `extension/` directory

Full VS Code extension scaffold. Initialize with:

```bash
cd ~/Code/rust/fieldglass
npx yo code --extensionType=typescript --extensionName=fieldglass
# Then move/restructure output into extension/
```

Then replace the generated editor provider stubs with `FieldglassEditorProvider` and `FieldglassDocument` as described in `PLAN.md`.

### Git initialization

```bash
cd ~/Code/rust/fieldglass
git init
git add .
git commit -m "Initial scaffold from Blurt"
```

---

## Expected Generated Structure

After generation + manual additions, the project should match the target layout from `PLAN.md`:

```
~/Code/rust/fieldglass/
├── Cargo.toml                        # [workspace] with all crate members
├── crates/
│   ├── fieldglass-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── metadata.rs
│   │       ├── reader.rs
│   │       └── detect.rs
│   ├── fieldglass-grib1/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── is.rs
│   │       ├── pds.rs
│   │       ├── gds.rs
│   │       ├── bds.rs
│   │       ├── tables.rs
│   │       └── reader.rs
│   ├── fieldglass-grib2/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── fieldglass-netcdf/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── fieldglass-napi/              # Added manually
│       ├── Cargo.toml
│       ├── build.rs
│       └── src/
│           ├── lib.rs
│           └── types.rs
├── extension/                        # Added manually
│   ├── src/
│   │   ├── extension.ts
│   │   ├── FieldglassEditorProvider.ts
│   │   ├── FieldglassDocument.ts
│   │   └── webview/
│   ├── package.json
│   └── tsconfig.json
└── .github/
    └── workflows/
        ├── build.yml
        └── release.yml
```

---

## Dependency Graph

After generation, manually set up inter-crate dependencies in each `Cargo.toml`:

```
fieldglass-grib1   → fieldglass-core
fieldglass-grib2   → fieldglass-core
fieldglass-netcdf  → fieldglass-core
fieldglass-napi    → fieldglass-core
                   → fieldglass-grib1
                   → fieldglass-grib2
                   → fieldglass-netcdf
```

Blurt-generated crates will have placeholder `[dependencies]` sections — fill these in after confirming the generated structure is correct.

---

## Notes on Blurt Limitations

*Anticipated before generation:*

| Concern | Limitation | Workaround |
|---|---|---|
| napi-rs bindings | No cdylib/build.rs support | Add `fieldglass-napi` crate manually |
| Workspace inter-crate deps | Blurt generates deps based on hints; verify each Cargo.toml | Edit `[dependencies]` after generation |
| Trait `impl` bodies | Generated as `todo!()` stubs | Expected — fill in during Phase 1 |
| `include_bytes!()` tables | Blurt generates a fn stub, not the actual macro call | Replace with `include_bytes!("tables/grib1_params.csv")` |
| TypeScript / Node | Blurt only targets Rust | Extension scaffold added manually |

---

## Encountered Issues During Generation

Issues discovered during actual execution of this plan, suitable as feedback for the Blurt compiler.

---

### Issue 1 — `path` is a reserved keyword everywhere

**Symptom:** Parse error `Unexpected Path, expected one of: RParen` (in `fn` signatures) and `Unexpected Path, expected one of: RBrace` (in `struct` bodies).

**Trigger:** Any use of `path` as an identifier — both as a function parameter name and as a struct field name.

**Affected spec lines (before fix):**
```blurt
fn detect_format(path: string) -> Format      // fieldglass-core
fn open(path: string) -> Grib1Reader          // fieldglass-grib1, grib2, netcdf
struct Grib1Reader { path: string }           // fieldglass-grib1
struct Grib2Reader { path: string }           // fieldglass-grib2
struct NetcdfReader { path: string }          // fieldglass-netcdf
```

**Fix:** Renamed all occurrences to `file_path`.

**Feedback for Blurt:** `path` conflicts with the project-level `path:` config key. This is surprising and easy to hit — file paths are a very common parameter name. Consider either scoping the keyword to project declarations only, or documenting reserved identifiers prominently in the error message.

---

### Issue 2 — Workspace `members` list rejects hyphenated names

**Symptom:** `Unexpected Error, expected one of: Comma, RBracket` at the first hyphen in a member name. With quoted strings: `Unexpected String("fieldglass-core"), expected one of: RBracket` (only the first element accepted).

**Trigger:** Rust crate names conventionally use hyphens (e.g. `fieldglass-core`). The Blurt parser treats hyphens as subtraction operators in identifier lists, and the quoted-string form only accepts a single member.

**Affected spec:**
```blurt
workspace Fieldglass {
    members: [fieldglass-core, fieldglass-grib1, ...]  // hyphens = parse error
    members: ["fieldglass-core", "fieldglass-grib1", ...] // only first accepted
}
```

**Fix:** Dropped the `workspace` declaration entirely and compiled each crate spec individually with separate `blurt` invocations. Created the workspace `Cargo.toml` by hand afterward.

**Feedback for Blurt:** The workspace feature is effectively non-functional for any project following standard Rust crate naming conventions. Both the unquoted (hyphen-as-minus) and quoted-string (single-item) forms fail. The multi-crate use case is important enough that this should be a priority fix. Suggested behaviour: allow quoted strings in `members` lists with full multi-item support, and/or treat identifiers in `members` context as crate name strings rather than expressions.

---

### Issue 3 — Generated `[lib] name` uses hyphens (invalid Rust identifier)

**Symptom:** When package names were corrected to use hyphens (Cargo convention), the `[lib] name` field in generated `Cargo.toml` files also received hyphens — which are not valid Rust identifiers and would cause compilation failure.

**Trigger:** Blurt generates `[lib] name = "<same as package name>"`. If the package name contains hyphens, the lib name inherits them.

**Fix:** Removed all `[lib]` sections from the generated `Cargo.toml` files. Cargo automatically derives a valid lib name by replacing hyphens with underscores, making the explicit section unnecessary.

**Feedback for Blurt:** The `[lib]` section should either be omitted from generated output (letting Cargo apply the default), or the `name` field should always be the underscore form of the package name. The current behaviour produces a `Cargo.toml` that fails to compile if the package name is hyphenated.

---

### Issue 4 — Generated `Cargo.toml` uses underscores for package name

**Symptom:** Blurt generated `name = "fieldglass_core"` (underscores) for the `[package]` name, while inter-crate `[dependencies]` we wrote used hyphens (`fieldglass-core = { path = "..." }`). Cargo reported: `no matching package found — searched: fieldglass-core, perhaps you meant: fieldglass_core`.

**Fix:** Ran `sed` to replace underscore package names with hyphen forms in all generated `Cargo.toml` files.

**Feedback for Blurt:** Rust and Cargo treat hyphens and underscores as equivalent in crate names, but the *canonical* form in `Cargo.toml` is hyphens for multi-word names. The generated `[package] name` should use the hyphenated form of the project name (converting `FieldglassCore` → `fieldglass-core`, not `fieldglass_core`) to match ecosystem conventions and avoid dependency resolution mismatches.

---

### Issue 5 — Cross-module type references not auto-imported in generated code

**Symptom:** Build error `cannot find type 'Metadata' in this scope` and `cannot find type 'GridDefinition' in this scope` in `fieldglass-core/src/reader.rs`.

**Trigger:** The generated `reader.rs` trait definitions reference `Metadata` and `GridDefinition` from `metadata.rs`, but no `use` statement was emitted.

**Affected generated file:**
```rust
// Generated reader.rs — missing imports
pub trait FormatReader {
    fn message(index: i32) -> Metadata;  // Metadata undefined
}
pub trait DataMessage {
    fn metadata() -> Metadata;           // Metadata undefined
    fn grid() -> GridDefinition;         // GridDefinition undefined
}
```

**Fix:** Manually added `use crate::metadata::{GridDefinition, Metadata};` to `reader.rs`.

**Feedback for Blurt:** When a `module` references a type defined in a sibling module within the same project, the codegen should emit the appropriate `use crate::<module>::<Type>` import. This is the most mechanical part of Rust module wiring and is unambiguous from the spec — the compiler has enough information to generate it correctly.
