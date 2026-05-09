# Changelog

All notable changes to Fieldglass are documented here. The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-beta.1] — 2026-05-09

First public beta. Read-only metadata viewer for GRIB1; GRIB2 and NetCDF detection only. See [PLAN.md](PLAN.md) for the full phase roadmap.

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
- Dual licensing under **MIT OR Apache-2.0**.

### Notable display fixes

- GRIB1 GDS lat/lon decode now correctly handles WMO sign-and-magnitude encoding (was previously two's-complement, producing bogus bounds like `-8298.608°` for a valid `-90°` south pole).
- Level rendering is unit-aware: pressure levels show as `200 hPa`, height-above-ground as `2 m`, layer types as ranges (`100 – 85 kPa`), with `—` for fixed-surface types where the value byte has no meaning.
- Forecast time display respects the WMO time-range indicator: `+24h`, `0–24h accum`, `analysis`, etc., instead of collapsing to a raw P1 byte.

### Known limitations

See the README "Known limitations" section.

[Unreleased]: https://github.com/D0ubleD0uble/fieldglass/compare/v0.1.0-beta.1...HEAD
[0.1.0-beta.1]: https://github.com/D0ubleD0uble/fieldglass/releases/tag/v0.1.0-beta.1
