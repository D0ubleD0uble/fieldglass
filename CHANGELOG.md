# Changelog

All notable changes to Fieldglass are documented here. The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versioning follows the [VS Code pre-release convention](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#prerelease-extensions): odd-minor versions (`0.1.x`, `0.3.x`, …) ship to the Marketplace pre-release channel; stable releases use the next even minor (`0.2.x`, `0.4.x`, …).

## [Unreleased]

### Added

- **NetCDF classic header parser** — pure-Rust reader covering CDF-1 (32-bit offsets), CDF-2 (64-bit offsets), and CDF-5 (64-bit sizes / extended numeric types). Exposes dimensions (with the unlimited / record dim flagged), global attributes, and per-variable type / dim-refs / attributes via a new `NetcdfReader` and the napi `open_netcdf` entry point.
- **HDF5 / NetCDF-4 detection + superblock probe** — files are validated, the superblock version is reported, and the metadata view surfaces a clear "deep parsing not yet implemented" notice. Deep HDF5 traversal is a deliberate scope cut tracked in a follow-up issue.
- **NetCDF metadata view** — `.nc` / `.nc4` / `.netcdf` files now render their dimensions, global attributes, and variables instead of "no messages found." Long attribute values are truncated with the full text on hover.
- **CDF-5 magic-byte detection** — `detect_from_bytes` now recognizes `CDF\x05` in addition to `CDF\x01` / `CDF\x02`.

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
