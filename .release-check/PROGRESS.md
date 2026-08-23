# 0.4.0 manual pass — live progress

Env verified: native module built today (wave-2 APIs present), TS compiled,
headless preflight green on all 12 samples, pair files built.

39 plan checks triaged to 12 that need human eyes. The rest are backstopped by
Rust/Electron tests or by the headless preflight, which already decoded and
reprojected every sample with sane ranges.

| # | Check | Why manual | Status |
|---|-------|-----------|--------|
| 1 | Spectral simple T63 renders a coherent global field | numerics tested, "looks right" is not | **PASS** (verified at 256x128, pre-#402) |
| 2 | Spectral: contours + overlays draw | wiring only partly covered | **PASS** (contours + colormaps confirmed) |
| 3 | Spectral: re-render is fast (cache #334/#357) | perf, not unit-testable | **PASS** (at 0.5deg, post-#402) |
| 4 | Spectral complex T63 renders same way | eyes-only | **PASS** |
| 5 | gfs: probe reads value tracking colour | end-to-end UI | **PASS** |
| 6 | gfs: Mollweide seam probe, no dead stripe (#332) | Rust-tested, UI path not | **PASS** |
| 7 | gfs: contours follow colour bands + cheap repaint (#336) | perf + visual | **PASS** |
| 8 | **gfs: exported PNG colorbar mid tick = 10 under log10 (#331)** | **ZERO test coverage anywhere** | **PASS** (only reachable after #400) |
| 9 | gfs: large long-CSV stays responsive (#341) | perf | **PASS** |
| 10 | fg-pair: A-B flat 0, A/B = 1, A+B doubled | never manually run | **PASS** |
| 11 | fg-pair: probe on difference reads 0, not 250 (#329) | never manually run | **PASS** |
| 12 | fg-mixed: refused with clear grid-mismatch message (#333) | never manually run | **PASS** |

Dropped as automated-covered: bulk of §2, §4, §5 regression renders
(preflight decoded+reprojected all 12), CSV round-trips, PNG byte writing,
contour extraction, grids_match, long-CSV gating.

## BUG-1 — spectral fields cannot be reprojected from the UI (found step 1)

The render panel offers only "Source projection" for a spherical-harmonic
message, with the note "Reprojection isn't available for spherical_harmonic
grids yet." That note is false.

**Engine is fine.** `renderGrid` on `spectral_simple_t63.grib2` succeeds in
every projection, reporting `source: latlon 256x128 -> <target>`:
source, equirectangular, mollweide, orthographic all render, range
231.6-317.2 K. Rust tests already assert this
(`synth_meta_is_latlon_and_reprojectable_with_geometry`).

**UI is wrong.** `render-panel.ts:1683/1693` gates the projection picker on
`meta.reprojectable`, which comes from the raw pre-synthesis message where
`gridType = "spherical_harmonic"` and `reprojectable = false`. The synthesized
grid is latlon and does reproject. NetCDF avoids this by building a synthetic
meta with `reprojectable: true` (`provider.ts:1538`); spectral has no equivalent.

The extension already special-cases this exact case for renderability in
`messageIsRenderable` (`provider.ts:1671-1680`, added for #303) - the same
reasoning was just never applied to `reprojectable`.

**Release impact.** The CHANGELOG entry for #303 claims spectral fields
"display, **reproject**, contours, and probe like any other field". That claim
was false in the shipped UI, on the flagship feature of 0.4.0.

**RESOLVED** - PR #393, merged as 6be3e06. Gate now keys on the synthesized
grid. Two regression tests added; the spectral one was verified to fail without
the fix. Bi-Fourier correctly stays source-only. CHANGELOG entry added.

## Autonomous sweep (loop iteration 1)

Bugs found and merged this iteration:
- BUG-3 PNG export status never cleared -> PR #395 (69dedcf)
- BUG-4 contour seam periodicity decided two different ways in two coordinate
  spaces; rotated grids could have been streaked -> PR #396 (3726c7a)

Validation run, all clean:
- 67 files x 8 projections rendered; no range escapes
- probe vs painted pixels (the #332 class) across the corpus: 0 mismatches
- contours across 4 projections, all vertices in-raster
- self-combine A-B == 0 and A/B == 1 exactly; probeCombined reads the
  difference, not field A (the #329 class)
- determinism: repeat renders identical; cache not poisoned by an intervening
  different projection; fresh handle agrees with reused; colormap does not move
  the data range
- CSV both formats; NetCDF render + self-difference
- INDEPENDENT ORACLE: all 8 operational samples match eccodes 2.34.1 exactly on
  min/max/mean (complex+spd, JPEG2000, CCSDS, PNG packings)

Two false leads I had to correct in my own test scripts before believing them:
raster dims differ per projection (source is native, Mollweide is 2x), and
exportCsv returns a Buffer, not a string. The second made an entire absence
check vacuous - it threw and was swallowed by a bare catch.

### Coverage gap found (not a bug, worth closing)
`samples/` has NO eccodes reference snapshots; the 31 committed snapshots are
all for synthetic fixtures. The operational corpus is therefore not guarded
against a silent decode regression. It matches eccodes today (verified above),
so committing those snapshots would lock that in.

### Still untested (next iteration)
- Semantic truth: units / level / reference time / sign conventions. Output can
  be perfectly self-consistent and still mean the wrong thing to a reader.
- Aliasing: nearest-resampling a 7000x3500 field into a small raster. Probe and
  paint agree *because they share the map*, so both can be wrong together.

## Autonomous sweep (loop iteration 2)

Merged: BUG-5 -> PR #397 (30c96c2). `render_forecast` fell back to 0 hours for a
lead too large for i32 — filing a far-future step next to the analysis. Now
saturates. The function had zero tests despite 7 unit branches, a sign-magnitude
value and that fallback; 5 added, one verified to fail without the fix.

Validated this iteration, all clean:
- reported data range does NOT vary with raster size (checked 3 sizes x 2
  projections x 4 samples). RenderOptions has no width/height at all — rasters
  are always native resolution, so the engine never downsamples and the
  aliasing axis is closed at the API level.
- sample METADATA vs eccodes 2.34.1: parameter, units, level, level type,
  reference time, forecast lead, centre — all 8 samples correct.

### Findings for the maintainer (need a decision, not a unilateral change)

1. GRIB1 and GRIB2 normalise forecast time with two separate implementations
   that disagree:
     - unit 13 (second): GRIB1 `p1_to_hours` ROUNDS, GRIB2 `render_forecast`
       TRUNCATES.
     - unknown unit: GRIB1 returns the raw value AS HOURS (inventing a lead
       time); GRIB2 returns no hours and preserves the raw value + label in the
       display string. GRIB2's behaviour is the better one.
   This is the same "two owners of one concept" class as BUG-4, and the repo
   already shares the second-order expansion across editions (#340/#361) to
   avoid exactly this. Unifying changes GRIB1 display semantics for rare units,
   so it wants a maintainer call.

2. `samples/` still has no eccodes reference snapshots (fixtures have 31).
   The operational corpus matches eccodes today — verified twice now, values and
   metadata — so committing snapshots would lock that in as a regression guard.

### Next iteration
Longer local fuzz runs on the three cargo-fuzz targets. CI time-boxes them; a
longer run is the one autonomous avenue not yet exhausted.

## Autonomous sweep (loop iteration 3) — fuzzing

Merged:
- PR #398 (cf57795) chore(fuzz): grib2 fuzz Cargo.lock was still on rust-j2k
  0.2.0 while the workspace pins =0.3.0. #370 bumped the dep without the fuzz
  lock. Not fuzzing the wrong decoder (cargo fuzz doesn't pass --locked, so it
  re-resolves), but `cargo check --locked` failed outright there and building
  the target dirtied the tree. Proven both ways before/after.
- PR #399 (639d7da) test(fuzz): the GRIB2 target only drove
  `decode_message_values`, which by design covers only scalar-per-point
  packings. Four entry points added THIS release cycle had zero fuzz coverage:
  decode_matrix_message (#306 - the variant eccodes crashes on, so no reference
  implementation exists), decode_spectral_message (#302),
  decode_bifourier_message (#304), synthesize_spectral_message (#303).
  Cost measured, not asserted: 1448409 -> 1337612 runs per 60s = 7.6%.

Fuzz results, all clean, no crashes or artifacts:
- baseline 10 min each: GRIB2 12558 runs, GRIB1 462620, NetCDF 2459651
- extended GRIB2 target, fixture-seeded, 780s: 111179 runs
(CI budgets 120s per target; these were 5-6x longer with a far richer corpus.)

Anti-vacuity check: all four new entry points have existing test call sites
proving they succeed on the exact fixtures seeded into the corpus.

### Open questions for the maintainer (unchanged from iteration 2, plus one)
1. GRIB1 vs GRIB2 forecast-time normalisation disagree (unit 13 rounds vs
   truncates; unknown unit invents hours in GRIB1). Same class as BUG-4.
2. samples/ has no eccodes reference snapshots; fixtures have 31.
3. NEW: should the fuzz jobs pass --locked? They don't, which is exactly why
   the lockfile drifted a whole release cycle unnoticed. Adding it makes the
   next drift a loud CI failure instead of a silently dirty tree, at the cost
   of a stale lock blocking the job rather than self-healing.

## Autonomous sweep (loop iteration 4)

No bugs found this iteration. What was checked:

- GRIB1 FEATURE SWEEP (a real gap — every earlier pass used Grib2Handle almost
  exclusively). All 15 fixtures through: 4 projections, probe-vs-painted-pixel,
  contour vertices in-raster, self-combine exactness, probeCombined, CSV both
  formats. ZERO defects.
  The single flagged item, hand_matrix_of_values.grib1, is correct documented
  behaviour: the GRIB1 true matrixOfValues form is not one scalar per grid
  point, so it uses its own entry point. The error is specific (names NR/NC),
  explains why, points at decode_matrix_message, and is identical across
  renderGrid / exportCsv / probe / projectContours.

- Multi-valued CF `missing_value` (the one known remaining sliver from
  #184/#186): documented IDENTICALLY in both the classic and HDF5 paths, each
  explicitly mirroring the other. Checked the corpus: every _FillValue is
  scalar SIMPLE {(1)/(1)} and no sample declares missing_value at all, so the
  limitation has no live exposure. Not worth acting on.

- Error-wording convention: GRIB1 and GRIB2 both name the Rust entry point in
  user-facing errors ("decode it via Grib2Reader::decode_matrix_message"). That
  is consistent across the two crates, not a drift. Worth a maintainer's voice
  call someday — a VS Code user cannot call a Rust method — but the string does
  serve the crates' library consumers, so I left it.

- Focused fuzz of the four newly-covered entry points still running: corpus is
  ONLY the 8 spectral/bifourier/matrix fixtures, so mutations stay near those
  packings instead of being rejected as the wrong packing immediately.

Focused fuzz result: 849734 runs in 1081s, no crashes, no artifacts, repo clean.
Higher throughput than the mixed corpus (111179) because the seeds are 52K
rather than 2.2MB, so mutations stay near the target packings instead of being
rejected as the wrong packing on arrival.

## Loop stopped after iteration 4

Fuzz executions this session, honestly tallied:
  baseline 10-min runs      2,934,829  (GRIB2 12,558 + GRIB1 462,620 + NetCDF 2,459,651)
  extended target, mixed      111,179
  extended target, focused    849,734
  throughput A/B runs       2,786,021  (half on the pre-extension target)
  ------------------------------------
  total                    ~6,700,000  of which ~2.3M exercised the four new entry points
No crashes, no artifacts, at any point.

Diminishing returns are unambiguous: iteration 3 found 2 issues, iteration 4
found 0. The deterministic surface is covered from several independent
directions — an eccodes 2.34.1 oracle on values AND metadata, self-consistency
across ~70 files, determinism/caching, GRIB1 and GRIB2 feature parity, NetCDF,
and the fuzzing above. Continuing would be manufacturing work rather than
finding bugs.

## BUG-6 — Export PNG did nothing on any GRIB render panel (found step 1, wave-2 §1/§2d)

The Export PNG button is in the panel HTML both render panels share, so a GRIB
render offers it. Only `openNetcdfRenderPanel` (`provider.ts:1122`) listened for
the `exportPng` message; the GRIB handler (`provider.ts:722`) had branches for
ready / rerender / overlay / contour / probe and nothing else. The panel
composited the image, posted it, and it fell off the end of the handler: no save
dialog, and no `exportPngDone`, so the status sat at "Exporting PNG…" for good.

PNG export therefore never worked for any GRIB message — §1's spectral PNG, §2d,
§2d′ (check 8), and §5's export-off-a-warped-view. #331's exported-colorbar fix
was unreachable from a GRIB panel. The #243 CHANGELOG entry claims "Works for
GRIB and NetCDF renders."

Why nothing caught it: all six PNG tests call `handleExportPng` directly, so the
handler being correct hid the fact that nothing routed to it. #395 fixed the
stuck status in the NetCDF handler only, which made the fix look complete.

**FIXED** — commit c23c8e6 (branch `fix/grib-panel-png-export`). New test drives
the message through the handler the provider registers; verified failing without
the fix (68 passing + 1 failing), green with it (69 passing).

## BUG-7 / BUG-8 — exported PNG clipped its header; level named twice (found step 1)

BUG-7: the export canvas took its width from the map block alone
(`margin + raster + gap + colorbar + labels + margin`). Title and subtitle are
drawn as one unwrapped line at x = margin with no measurement and no clamp, so
any raster narrower than its own header overflowed. Spectral T63: 256 px raster
-> 382 px canvas under a ~456 px subtitle, and the reference time at the end of
that line was clipped. GFS at 1440 px hides it entirely — which is why every
earlier pass of §2d looked clean. Small grids generally hit it (the §6
second-order and run-length fixtures too).

BUG-8: for level types whose value carries no information the decoder reports
the surface name in BOTH `level` and `levelType`, and the header joined them:
"Ground or water surface Ground or water surface". Shows in the in-panel header
as well as the export, and is part of why the line was wide enough to overflow.

Fixed on branch `fix/png-export-header-width`, commit 60065b7, PR #401. Sizing
rule extracted to `exportCanvasWidth` and serialized into the panel script via
`toString()`, so the webview runs the function the tests pin rather than a copy.

Anti-vacuity note: the first injection guard PASSED with the fix reverted — the
serialized function's own parameter list reads
`exportCanvasWidth(mapBlockW, headerW, margin)`, so matching a bare call matched
the declaration. Re-anchored on the assignment and re-verified failing. Same
class as the swallowed-Buffer false lead in iteration 1.

## Design decision — render/export size (raised at step 1, from the small PNG)

Two knobs, answered differently:

1. SPECTRAL SYNTHESIS RESOLUTION — changed. `spectral_render_dims` scaled the
   grid with the truncation (2(T+1) lats, capped 361x720), so T63 rendered
   256x128 and exported a ~382 px PNG. A spectral message is a band-limited
   function, not a sampled grid: any grid at or above the minimum reproduces it
   exactly, so a denser grid is a sharper picture, not interpolation. Now a
   fixed 0.5deg (720x361) for every truncation — the old ceiling becomes the
   floor, so T >= 180 is unchanged. Measured on the T63 fixtures: synthesis
   2.2 -> 13.7 ms cold, 0.4 -> 2.9 ms warm, cached (#334); range 231.6..317.2 ->
   231.0..317.3 K (same field, extrema sampled closer). PR #402.

2. GRIDDED RENDER SIZE — deliberately NOT added. RenderOptions has no
   width/height, which is what keeps probe and paint on one map; a resampling
   stage in front of it reopens the #332/#329 class. Making small gridded fields
   export larger is a figure-rendering knob instead: issue #403, which needs
   projectOverlay/projectContours to take output dims so overlays are
   re-projected at scale rather than upscaled.

## Check 1 PASS (maintainer verdict)

Verified on the 256x128 synthesis, i.e. before #402 raised every spectral field
to 0.5deg. The finer grid is the same band-limited function sampled more closely
(range 231.6..317.2 -> 231.0..317.3 K, dims pinned by tests), so the verdict
carries, but the first render after the rebuild is worth a glance.

## MANUAL PASS COMPLETE — 12 / 12

All twelve human-eyes checks pass. Everything else in RELEASE-TEST-PLAN.md was
triaged as backstopped by Rust/Electron tests or the headless preflight.

Found and merged during the pass (all from step 1, on the flagship feature):
- #400 Export PNG did nothing on ANY GRIB render panel. The button is in the
  shared panel HTML; only the NetCDF handler listened for the message. All six
  PNG tests called the handler directly, so they passed while it was unreachable.
- #401 The exported PNG clipped its header (canvas sized from the map block
  alone), and the subtitle named the level twice when the decoder reports the
  surface name in both `level` and `levelType`.
- #402 Every spectral field now synthesizes at 0.5deg (720x361) rather than
  2(T+1) scaled to the truncation. Band-limited function, so the denser grid is
  exact, not interpolated. T63 render went from a 382 px PNG to a usable figure.
- #403 filed (not fixed): export at a chosen size for small GRIDDED fields;
  needs projectOverlay/projectContours to take output dims.

Earlier iterations found #393, #394, #395, #396, #397, #398, #399.

Still open for the maintainer, unchanged:
1. GRIB1 vs GRIB2 forecast-time normalisation disagree (unit 13 rounds vs
   truncates; unknown unit invents hours in GRIB1).
2. samples/ has no eccodes reference snapshots; fixtures have 31. The corpus
   matches eccodes today on values AND metadata, verified twice.
3. Should the fuzz jobs pass --locked? Their lockfile drifted a whole release
   cycle unnoticed (#398).
