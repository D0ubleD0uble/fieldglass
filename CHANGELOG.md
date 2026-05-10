# Changelog

All notable changes to Fieldglass are documented here. The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versioning follows the [VS Code pre-release convention](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#prerelease-extensions): odd-minor versions (`0.1.x`, `0.3.x`, …) ship to the Marketplace pre-release channel; stable releases use the next even minor (`0.2.x`, `0.4.x`, …).

## [Unreleased]

### Added

- **NetCDF classic header parser** — pure-Rust reader covering CDF-1 (32-bit offsets), CDF-2 (64-bit offsets), and CDF-5 (64-bit sizes / extended numeric types). Exposes dimensions (with the unlimited / record dim flagged), global attributes, and per-variable type / dim-refs / attributes via a new `NetcdfReader` and the napi `open_netcdf` entry point.
- **HDF5 / NetCDF-4 detection + superblock probe** — files are validated, the superblock version is reported, and the metadata view surfaces a clear "deep parsing not yet implemented" notice. Deep HDF5 traversal is a deliberate scope cut tracked in a follow-up issue.
- **NetCDF metadata view** — `.nc` / `.nc4` / `.netcdf` files now render their dimensions, global attributes, and variables instead of "no messages found." Long attribute values are truncated with the full text on hover.
- **CDF-5 magic-byte detection** — `detect_from_bytes` now recognizes `CDF\x05` in addition to `CDF\x01` / `CDF\x02`.
- **GRIB1 2-D grid rendering in a dedicated tab.** Clicking a metadata row
  expands an inline panel between that row and the next, exposing a
  per-message *Render* button. Pressing *Render* decodes the message via the
  existing napi `decode_grid` and opens a new editor tab beside the table
  that paints the values into a `<canvas>` using a baked-in 256-entry
  **viridis** colormap (no colormap library dependency). A vertical
  colorbar shows the data min/max (computed from the grid itself, excluding
  bitmap-masked points). Each render gets its own tab so messages can be
  compared side-by-side. Render is button-triggered — selecting a row only
  expands the panel, it does not auto-decode — to keep the metadata-only
  path fast.
- **Bitmap-masked points render as transparent (alpha = 0)** so missing data
  reads as "no value" against the editor background. The render-pane legend
  documents this policy.
- **Webview Content-Security-Policy.** Scripts are now enabled (required to
  request a render and paint a canvas) and the webview ships an explicit,
  restrictive CSP: `default-src 'none'; script-src 'nonce-<per-page>';
  style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource}
  blob: data:`. No `'unsafe-eval'`, no inline scripts without a nonce. The
  policy and rationale are documented inline in `provider.ts`.

- **GRIB1 BDS complex / second-order packing variant detection.** When a
  message uses complex packing (BDS flag bit 1 = 1), `BdsHeader` now
  exposes a typed `complex_extended` struct with N1 + the seven
  extended-flag bits (matrix-of-values, secondary-bitmap, group-width,
  general-extended, boustrophedonic, two-orders-of-SPD, plus-one-in-SPD)
  plus a derived `order_of_spd()` and `packing_type_label()` that mirrors
  eccodes' `packingType` (`grid_second_order`, `grid_second_order_SPD3`,
  `grid_second_order_row_by_row`, etc.).
- **GRIB1 `grid_second_order` decoder** for the general-extended family
  (`secondOrderOfDifferentWidth=1`, `secondaryBitmapPresent=0`,
  `generalExtended2ordr=1`). Lives at
  `crates/fieldglass-grib1/src/packing/second_order.rs` and handles
  `orderOfSPD ∈ 0..=3` plus boustrophedonic row-scan reordering. The
  control flow mirrors `DataG1SecondOrderGeneralExtendedPacking::unpack`
  in eccodes' source, with the byte-aligned section sizing rule from
  `Spd::compute_byte_count` / `UnsignedBits::compute_byte_count`. Pinned
  end-to-end against a `grib_get_data` snapshot of an ECMWF MARS-derived
  fixture: 29,040 grid points, every anchored sample matches eccodes to
  within 1e-3.
- **GRIB1 packing-mode compatibility table** in the README, distinguishing
  metadata coverage (every variant) from decode/render coverage (simple
  packing only today).
- **ECMWF GRIB1 test fixture** at
  `crates/fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1` (56 KB,
  one message, `grid_second_order` SPD-2). Used by
  `tests/decode_ecmwf_complex.rs` to pin the variant-detection wiring;
  will become the oracle for the second-order decoder in a follow-up.
- **GRIB2 Indicator Section parsing** — `fieldglass-grib2` now parses Section 0 (16 bytes), enumerating messages by walking 64-bit total-length offsets and surfacing edition, discipline (WMO Code Table 0.0 lookup), and total length. New `open_grib2` napi function dispatches `.grb2` / `.grib2` files to the tabular viewer instead of the previous "no messages found" fallback. Sections 1–7 remain follow-ups.

### Changed

- Render is performed in *grid coordinates* — no map reprojection is applied,
  so polar stereographic and Lambert conformal grids show the data in scan
  order. Geographic reprojection is tracked as a separate follow-up.

### Fixed

- **GRIB1 PDS decimal scale factor `D` decoded as sign-magnitude** (per WMO
  spec) instead of two's-complement. Octet 27 high bit is the sign, low 15
  bits are the magnitude — reading the pair as a plain `i16` turned small
  negatives like `D = -2` (wire `0x8002`) into `-32766`, which silently
  multiplied every decoded value by `10^32766` (→ `±inf`). Both shipped
  fixtures happen to use `D = 0`, so the bug was invisible until
  cross-checking real ECMWF surface fields against eccodes. End-to-end
  regression in `tests/decode_ecmwf_complex.rs` patches the fixture's
  PDS to `D = -2` and pins the result against
  `grib_set -s decimalScaleFactor=-2` (eccodes 2.34.1).
- README feature matrix: replaced GitHub-only `$\color{red}{\textsf{Not yet}}$` LaTeX color hack with `❌ Not yet` so the table renders correctly inside the VS Code Marketplace listing as well as on GitHub (#25).
- Codecov badge in the README showed `unknown` because the coverage workflow's tokenless upload was being rejected (`Token required - not valid tokenless upload`) and silently swallowed by `fail_ci_if_error: false`. Switched the upload to OIDC (`use_oidc: true` plus `id-token: write` permission), which is the recommended tokenless path on `codecov-action@v5` for trusted runs (#24).
- **NetCDF CDF-5 variable header: `dimid` now read as 8-byte `NON_NEG`** (the
  CDF-5 width) instead of 4-byte. The previous code unconditionally read
  4 bytes, then ran the rest of the parse 4 bytes off and tripped the
  `att_list` ABSENT-with-non-zero-count guard partway through the var list.
  Surfaced by the new ERSST CDF-5 fixture (real NOAA NCEI data re-encoded
  by the canonical Unidata `netCDF4` library); unit tests in
  `classic.rs` happened to use synthetic CDF-1 fixtures so missed it.

## [0.1.0] — 2026-05-09

First public release, on the Marketplace pre-release channel. Read-only metadata viewer for GRIB1; GRIB2 and NetCDF detection only.

### Added

- **GRIB1 metadata viewer** — IS, PDS, GDS section parsing, with WMO ON388 lookups for parameter, originating centre, and level type. Tabular webview shows one row per message.
- **GRIB1 grid descriptions** — Lat/Lon, Gaussian, Polar Stereographic, Lambert Conformal projections.
- **GRIB1 Binary Data Section decoder** (`Grib1Reader::decode_message_values`, exposed via napi `decode_grid`) — produces per-point values respecting the optional Bit Map Section. Not yet wired to a 2-D visualization.
- **Format detection** for GRIB1, GRIB2, NetCDF (classic + NetCDF-4 / HDF5) from magic bytes.
- **File associations** for `.grb`, `.grib`, `.grib1`, `.grb1`, `.grb2`, `.grib2`, `.nc`, `.nc4`, `.netcdf`, plus an option-priority "Fieldglass Viewer" for arbitrary files via *Reopen Editor With…*.
- **Hex/ASCII fallback view** for files whose contents don't match a recognized format.
- **Pre-commit framework** orchestrating `cargo fmt`, `cargo clippy`, `tsc`, file-hygiene polish, `shellcheck`, `actionlint`, `gitleaks` on commit; `cargo test`, `cargo deny check`, `npm audit`, `semgrep` on push.
- **CI security suite** — Semgrep SAST, CodeQL (JS/TS + Rust, `security-extended`), Dependabot across `cargo` / `npm` / `github-actions`. Semgrep + CodeQL are visibility-gated and self-activate on flipping the repo public.
- **Test fixtures** for GRIB1 (CMC wind), GRIB2 (eccodes reduced Gaussian), and NetCDF (Unidata classic + HDF5 samples).
- **`SECURITY.md`** with the private vulnerability reporting workflow and scope.
- **Per-platform Marketplace packaging** — release workflow now builds one `.vsix` per `(linux|win32|darwin)` × `(x64|arm64)` target so users only download the binary they need.
- **`workflow_dispatch` dry-run** for the release workflow — manual runs build + package the matrix without publishing, so cross-build regressions can be caught before tagging.
- Dual licensing under **MIT OR Apache-2.0**, including a Marketplace-friendly `extension/LICENSE.md` summary.

### Changed

- CI: dropped the redundant `Build native (linux-x64 smoke test)` job. The native binary is already built (and now explicitly verified) inside `Build extension`.
- CI: cache cargo builds with `Swatinem/rust-cache` and Node.js downloads via `actions/setup-node`'s built-in `cache: npm` to cut warm-cache CI time.
- CI: switched `npm install` to `npm ci` in workflows so dependency installs respect the lockfile.
- pre-commit: aligned the local Semgrep rule set with the CI workflow by adding `p/owasp-top-ten` and `p/github-actions`; added a root-level `npm audit` hook to cover `@napi-rs/cli`.
- Marketplace listing metadata: `bugs.url`, `preview: true` (Preview badge while pre-release), and a `Visualization` category for discoverability.
- Versioning: dropped the `-beta.1` semver pre-release suffix in favour of the VS Code pre-release convention. The Marketplace rejects semver pre-release tags; future betas will be `0.1.1`, `0.1.2`, … with stable jumping to `0.2.0`.

### Notable display fixes

- GRIB1 GDS lat/lon decode now correctly handles WMO sign-and-magnitude encoding (was previously two's-complement, producing bogus bounds like `-8298.608°` for a valid `-90°` south pole).
- Level rendering is unit-aware: pressure levels show as `200 hPa`, height-above-ground as `2 m`, layer types as ranges (`100 – 85 kPa`), with `—` for fixed-surface types where the value byte has no meaning.
- Forecast time display respects the WMO time-range indicator: `+24h`, `0–24h accum`, `analysis`, etc., instead of collapsing to a raw P1 byte.

### Known limitations

See the README "Known limitations" section.

[Unreleased]: https://github.com/D0ubleD0uble/fieldglass/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/D0ubleD0uble/fieldglass/releases/tag/v0.1.0
