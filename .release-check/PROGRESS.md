# 0.5.0 manual pass — outcome

**Version: 0.5.0. Tagged 2026-09-04 on prep merge `77eb0e4`.**
Superseded when the next cycle's pass runs; if the version above is not the one
you are cutting, this file is history, not status.

Env: native module and TypeScript built from the tagged tree; headless preflight
green on all 15 samples.

## Result

| Section | Checks | Result |
|---|---|---|
| §1 HEALPix | 12 | pass |
| §2 Curvilinear NetCDF | 9 + 3 visual | pass |
| §3 Reduced Gaussian | 13 + 2 visual | pass |
| §4 Parameter / unit / centre naming | 10 | pass |
| §5 Projected grids | 6 | pass |
| §6 NetCDF containers | 4 | pass |
| §7 Regression floor | 10 + 1 visual | pass |
| §8 Scale-less HDF5 (added mid-cycle) | 4 | pass |
| **UI checks in a dev host** | 8 | pass |

**73 headless assertions, 0 product failures.** Backed by 1,383 Rust tests
across 94 binaries and 88 Electron tests. Six rendered images inspected
directly.

## How it was run

Most of the plan was executed headlessly through the built napi addon rather
than by eye — dimensions, decode ranges, projection summaries, probe readouts,
contour extraction and overlay registration are all checkable that way, and a
script can sweep the whole corpus where a person checks one file. Images were
rendered to PNG and inspected for the "does it look right" checks. Only the
genuinely UI-bound items were done by hand in the dev host: the metadata view,
units in the panel title and probe readout, the variable-switch relabel, table
scrolling in a narrowed pane, the coastline/contour toggle, PNG export, and
`.h5` via *Reopen Editor With…*.

The sweep scripts are worth rebuilding next cycle. Three of their early
failures were faults in the harness, not the product — reading the warp's
perimeter extent where the Bounds column reads the meta corners, a wrong
combine-op name, and a pixel where the anomaly field was exactly zero so
`A − B` equalled `A` trivially. Check the harness before believing a failure.

## What it found

Three defects, none of which a reading of the diff would have suggested:

- **#514** — reprojected renders had no minimum resolution, so a coarse grid
  drew a postage-stamp map with a staircase limb. Fixed in #530.
- **A false claim in the changelog** — the #514 entry said the floor left every
  operational grid alone. NAM (614 × 428) and RAP (451 × 337) are full
  operational grids and both move. Fixed in #531.
- **#533** — a scale-less `.h5` reported every axis as length 0, so nothing in
  it could be drawn. Fixed in #534, with ADR-0003 amended to record the rule.

## Open at release

- **#532** — a large curvilinear grid repaints in seconds: RTOFS is 6.5 s
  equirectangular, 16.8 s and 2.3 GB peak for Mollweide. §2 of the plan expects
  the second render to "feel instant". Not a correctness defect and not a
  blocker by the plan's rule, but it shipped in 0.5.0 as a known item.
