# Changelog

All notable changes to Fieldglass are documented here. The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versioning follows the [VS Code pre-release convention](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#prerelease-extensions): odd-minor versions (`0.1.x`, `0.3.x`, …) ship to the Marketplace pre-release channel; stable releases use the next even minor (`0.2.x`, `0.4.x`, …).

## [Unreleased]

### Added

- **The render panel's projection picker gains Web Mercator, orthographic, and polar-stereographic targets.** Beyond `source` and `equirectangular`, the picker now offers three more reprojection targets, all sharing the source inverse map through a new `TargetProjection` trait + generic `warp` in `fieldglass-core::warp` (the per-target difference is just the output pixel → `(lat, lon)` map). **Web Mercator** distributes rows linearly in Mercator Y (poleward stretch) with the latitude extent clamped to the ±85.0511° valid band; like equirectangular it honours the manual lat/lon window and echoes its extent back. **Orthographic** ("globe view") fits the visible hemisphere to the raster as a disc centred on a chosen point (Snyder USGS PP-1395 §20), masking the back hemisphere. **Polar stereographic** renders the conformal true-shape view centred on a pole with the equator at the disc rim (Snyder §21). The two azimuthal targets are parameterised by a small preset selector rather than free-form numeric inputs — orthographic centre (`atlantic` (default, 0°N 0°E), `pacific`, `north_pole`, `south_pole`) and polar hemisphere (`north` (default), `south`) — and report no lat/lon-box extent. The napi `RenderOptions` gains `projectionPreset`. Closes #71.

- **The render panel can overlay coastlines and a lat/lon graticule on the projected grid.** A new `Overlay` toolbar control toggles continent coastlines (bundled Natural Earth 1:110m, public domain, under `extension/media/`) and a graticule with adjustable spacing, anchoring what region a field covers. The forward projection stays entirely in Rust: `fieldglass-core::warp::PreparedTarget` gains a `lonlat_to_pixel` map (the analytic inverse of `pixel_to_lonlat`) for all four targets, and a new `fieldglass-core::overlay::project_polylines` clips geographic polylines to the visible domain — dropping the orthographic back hemisphere / polar far hemisphere and splitting at the antimeridian seam. The napi surface gains a generic, geometry-only `Grib{1,2}Handle.projectOverlay(messageIndex, options, latlon, ringLengths) -> { xy, segLengths }` that reprojects `(lat, lon)` polylines into the rendered raster's pixel space without decoding values, so toggling never re-decodes. The webview strokes the returned runs on a separate vector canvas layered over the image — crisp at display resolution rather than baked into the upscaled raster — and toggling a layer off clears it with no round-trip. The render contract (`RenderOptions`/`RenderedGrid`) is unchanged; the overlay is purely additive, and because `projectOverlay` knows nothing about "coastline" specifically, a future user-defined-shape layer reuses the same call. The overlay also projects onto the **source** projection: since `paint_source` lays grid point `(i, j)` straight into output pixel `(i, j)`, the grid's own inverse map doubles as the source forward map (new `fieldglass-core::overlay::SourceOverlayTarget`), so coastlines/graticule show on the unwarped grid too — across every grid type. Closes #72.

- **GRIB1 polar stereographic source grids now reproject into equirectangular.** The render panel's `Equirectangular` target now supports `polar_stereo` source grids in addition to lat/lon, Gaussian, and Lambert. Closes the GRIB1 half of #45. New `PolarStereoParams` / `PolarStereoProjector` in `fieldglass-core::projection` implement the sphere polar-aspect formulas (Snyder USGS PP-1395 §21, true scale fixed at 60° per GRIB convention) with both north- and south-pole orientations. The napi `MessageMeta` surface gains `polarStereoLov` / `polarStereoDxMetres` / `polarStereoDyMetres` / `polarStereoSouthPole` fields populated from the GRIB1 §3 GDS. Warp setup detects when the projection pole falls inside the grid extent and expands the equirectangular bounds to the full 360° of longitude (clamping latitude to the relevant pole) so hemispheric grids render correctly. GRIB2 §3.20 polar stereographic templates remain pending the §3 template-coverage track under #70.

### Added

- **The equirectangular render now accepts a manual lat/lon window.** The render panel gains a `Bounds` Auto/Manual control (mirroring the existing range control): in Manual mode you set `lat min/max` and `lon min/max` and the warp renders exactly that window. Inputs pre-fill with the bounds Rust computed, so Manual mode starts from the auto extent. The napi `RenderOptions` gains `boundsLatMin`/`boundsLatMax`/`boundsLonMin`/`boundsLonMax` (all four required; a partial or inverted box silently falls back to the computed bounds), and `RenderedGrid` echoes back the extent actually used as `usedLatMin`/`usedLatMax`/`usedLonMin`/`usedLonMax`. `lonMin`/`lonMax` may fall outside [-180, 180] to describe an antimeridian-spanning window; pass the echoed values back verbatim to reproduce a view.

### Fixed

- **`GridDescription::bounds()` now reports the real opposite corner for polar stereographic and Lambert GRIB1 grids.** These projections encode only the first grid point (La1/Lo1) in the GDS — there is no La2/Lo2 — so `bounds()` previously returned a `(0, 0)` placeholder for the last corner, which surfaced as a bogus grid extent in metadata. The corner is now derived from the projection (forward-project the origin to plane metres, step `(N-1)·D`, inverse-project, normalise longitude to `(-180, 180]`). A new `PlanarGridProjector` trait in `fieldglass-core::projection` carries the shared corner/bbox geometry for both planar projectors; the Lambert and polar stereographic warp setups now derive their target bounds through it instead of reimplementing the four-corner loop.
- **Equirectangular bounds for planar grids now follow the curved edges, not just the corners.** A Lambert/polar-stereographic grid edge is straight in projected metres but a curve in lat/lon, and its lat/lon extrema usually fall mid-edge — most visibly the point of an edge closest to the projection pole, which maximises latitude. The four-corner bounding box missed these: the CMC polar grid's corners cap at 60.5°N while its top edge reaches ~80.6°N, so the auto extent (and any UI pre-fill from it) clipped the northern data. `PlanarGridProjector::lonlat_bbox` now samples the grid perimeter (512 points/edge) instead of the corners.
- **Antimeridian-crossing source grids no longer warp into a near-global equirectangular box.** When a Lambert or polar-stereographic grid straddled the ±180° dateline (e.g. the CMC regional grid, whose corners span +177° to −135°), the four-corner bounding box took a naive longitude min/max and ballooned to ~312° wide — squeezing the data into a sliver of a mostly-empty raster. `PlanarGridProjector::lonlat_bbox` now unwraps corner longitudes to a continuous interval before the min/max and recenters it on [-180, 180], producing a tight span across the dateline. Grids that genuinely surround the projection pole still take the existing full-360° `pole_inside` override.

## [0.1.2] — 2026-05-17

Third pre-release. GRIB2 moves from "header-only" to full §0–§7 parsing plus simple-packing value decode; the render panel gains a projection picker, a Rust-side render pipeline (reader handles, paint-ready RGBA, viridis colormap entirely in Rust), and a webview wire-format fix that restores the canvas painting end-to-end.

### Added

- GRIB2 §1 Identification Section parsing — exposes originating centre, sub-centre, master/local table versions, reference time, production status, and processed-data type per message.
- GRIB2 §2 Local Use Section parsing — surfaces the byte range so centre-specific decoders can pick up the opaque payload later.
- GRIB2 §3 Grid Definition Section parsing for templates 3.0 (regular lat/lon), 3.30 (Lambert Conformal), and 3.40 (Gaussian lat/lon — both regular and reduced). Other templates surface as `unsupported(3.N)` so file enumeration still works.
- GRIB2 §4 Product Definition Section parsing for templates 4.0 (analysis or forecast at a horizontal level/layer at a point in time), 4.8 (average / accumulation / extreme values over a time interval), and 4.11 (individual ensemble forecast over a time interval). Other templates surface as `unsupported(4.N)`.
- WMO Code Table 1.2 / 1.3 / 1.4 / 3.1 / 3.2 / 4.3 / 4.4 / 4.5 / 4.6 / 4.10 lookups (reference-time significance, production status, processed-data type, grid template, earth shape, generating-process type, time-range unit, fixed surface, ensemble type, statistical processing) plus a curated subset of Code Tables 4.1/4.2 (parameter triples by discipline + category) and Common Code Table C-1 (originating centres) in `fieldglass-grib2`.
- Reference time, originating centre, grid type / dimensions / corner coordinates, and now parameter name + units + level + forecast time populate per-message rows in the GRIB2 metadata viewer.
- Two new GRIB2 fixtures: `gfs_c255_latlon.grib2` (NCEP GFS, template 3.0) and `eta_lambert_msg0.grib2` (NOAA Eta, template 3.30) — see `crates/fieldglass-grib2/tests/fixtures/NOTICE.md` for provenance.

- eccodes reference snapshots for every bundled GRIB2 fixture — checked-in `.eccodes.ref.json` files capture `grib_dump -j` output for a curated subset of WMO keys; the new `tests/eccodes_reference.rs` integration test cross-checks our parser against each snapshot on every run with zero runtime dependencies. Regenerate via `python3 tools/regenerate-eccodes-snapshots.py` after upgrading eccodes or adding a fixture.
- GRIB2 §5 Data Representation Section parsing for template 5.0 (simple packing): reference value (IEEE float), binary scale factor `E`, decimal scale factor `D`, bits per value, and original-field type. Other templates surface as `unsupported(5.N)`.
- GRIB2 §6 Bit-Map Section parsing — indicator byte (inline / no bitmap / reuse-previous / predefined) and inline-bitmap unpack into `Vec<bool>`. Reuse-previous and predefined indicators are surfaced as `UnsupportedSection` errors with the code in the message.
- GRIB2 §7 Data Section parsing + simple-packing decoder. `Grib2Reader::decode_message_values` returns `Vec<Option<f64>>` mirroring the GRIB1 API; constant-field (`bits_per_value == 0`) and bitmap-aware decoding are both covered.
- napi `decode_grid` now dispatches by magic-byte detection so the existing 2-D render pipeline picks up GRIB2 messages with no UI changes — simple-packed messages render end-to-end.
- New fixture `regular_latlon_surface.grib2` (1.2 KiB ECMWF 2-m temperature on a 16×31 lat/lon grid) for the simple-packing decode integration test.
- **Render-panel reprojection picker.** The 2-D render now exposes two pickers — projection target (`Source projection` / `Equirectangular`) and resampling (`Nearest` / `Bilinear`) — and warps lat/lon, Gaussian, and Lambert source grids through their native projection into a north-up equirectangular canvas when chosen. Bilinear masks cells whose 4-neighbour stencil includes a bitmap-masked source point.
- **Rust-side render pipeline (closes #41 + the structural half of #45).** Reader handles (`Grib1Handle` / `Grib2Handle`) are now persistent across napi calls: parse once, reuse for every subsequent decode / render / metadata call. The provider stores one handle per document; `decodeGrid` returns `(Float64Array, Uint8Array)` directly (no boxed `Array<number | null>` repack), and the new `renderGrid` composes decode + warp + viridis colormap entirely in Rust, returning a paint-ready RGBA `Buffer` the webview blits to canvas via `putImageData`. The TS-side colormap LUT + paint loop is gone — they live in `fieldglass-core::colormap` now.
- New modules `crates/fieldglass-core/src/{projection,warp,colormap}.rs` — projection math (lat/lon, Gaussian via Gauss-Legendre nodes, Lambert Conformal per Snyder USGS PP-1395), inverse-warp pipeline with bilinear/nearest resampling, and viridis colormap painting. 25 new unit tests covering Gauss-Legendre node accuracy, Lambert round-trip, bilinear edge cases, colormap clamps, and flip-y row inversion.
- New napi structs `RenderOptions`, `RenderedGrid`, `DecodedGrid` + handle classes `Grib1Handle` / `Grib2Handle` replacing the standalone `openGrib1` / `openGrib2` / `decodeGrid` / `setP1` entries.
- **Render-panel integration tests** in `extension/src/test/suite/render.test.ts` — pin the wire contract that the `gridReady` payload depends on (Uint8Array survives `webview.postMessage`; raw Node Buffer does not), cover the full render-pipeline path through `Grib1Handle.renderGrid` + `Grib2Handle.renderGrid` for GRIB1 (`cmc_wind_300_2010052400_p012.grib`) and GRIB2 (`regular_latlon_surface.grib2`) fixtures, and pin `openNetcdf` against the classic NetCDF (`netcdf_classic_dummy.nc`) DatasetMeta contract — one regression test per user-visible file format.

### Changed
- `MessageMeta` (napi) gains optional `productionStatus` / `dataType` fields; existing GRIB1 callers see them as `null`.
- GRIB2 `Grib2Message` now carries every section through §7: required `gds: GridDefinitionSection`, `pds: ProductDefinitionSection`, `drs: DataRepresentationSection` plus byte ranges for §6 BMS and §7 DS. `Grib2Reader::from_bytes` validates the full §0–§7 walk per the WMO spec.
- Render-panel projection caption now names the source projection explicitly. The picker readouts read `source: latlon 240×121 → latlon (no reprojection)` (source projection) or `source: latlon 240×121 → equirectangular (nearest)` (equirectangular), so the right-hand side of the arrow always tells you the target — for the default picker that's the actual source projection (`latlon`, `lambert`, `gaussian`, etc.) rather than the generic "source projection".

### Removed
- Webview legend caption beneath the render canvas (`"Rendered server-side (Rust). …"`) — implementation detail; not user-facing information.

### Fixed
- **Render canvas was blank after #73.** `Grib{1,2}Handle.renderGrid` returns RGBA as a napi `Buffer`; when posted via `webview.postMessage`, VS Code's serializer (`extHostWebviewMessaging.ts::getTypedArrayType`) switches on `value.constructor.name` and only accepts the standard TypedArray names. Node `Buffer` (whose `constructor.name === "Buffer"`) is not on that list, so the bytes silently fell back to `Buffer.prototype.toJSON()` and the webview received `{type:"Buffer", data:[…]}` — a plain object, not a typed array. The panel script's `new Uint8ClampedArray(payload.rgba.buffer ?? payload.rgba, …)` then produced a zero-length array, and `new ImageData(rgba, w, h)` threw silently (no `try/catch` on the blit), leaving status stuck at `"Rendering…"` and the canvas blank. `Grib1Handle.renderGrid` and `Grib2Handle.renderGrid` were both affected; every grid type was affected (the temperature messages in `ecmwf_lfpw.grib1` that surfaced the bug were just what the user happened to click). Fixed by wrapping the napi `Buffer` as a plain `Uint8Array` view (`new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength)`) in the new exported `buildGridReadyMessage` helper in `provider.ts`, which sets `constructor.name === "Uint8Array"` so the VS Code serializer ships it as a binary reference and the webview revives it as a real Uint8Array. Pinned by `render.test.ts`'s round-trip tests.

## [0.1.1] — 2026-05-10

Second pre-release. GRIB2 and NetCDF move from "magic-byte detection only" to header-parsed metadata viewers; GRIB1 gains 2-D grid rendering and second-order packing decode for the ECMWF default (SPD-2).

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

[Unreleased]: https://github.com/D0ubleD0uble/fieldglass/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/D0ubleD0uble/fieldglass/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/D0ubleD0uble/fieldglass/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/D0ubleD0uble/fieldglass/releases/tag/v0.1.0
