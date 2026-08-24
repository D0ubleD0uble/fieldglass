# Fieldglass roadmap

*Adopted 2026-07-19. Revised 2026-08-23 after the 0.4.0 release. Reviewed
after each release and at the twice-yearly WMO fast-track checkpoints
(June and November).*

This document says where Fieldglass is going and why. It is intent, not a
commitment: items under **Now** are in progress; everything further out will
change as we learn. The reasoning behind cost estimates and sequencing lives in
[`docs/planning/`](docs/planning/README.md); design decisions live in
[`docs/decisions/`](docs/decisions/README.md). Issues are linked where they
exist; items without a link still need one filed.

## Strategy

Fieldglass is a viewer for meteorological data files that runs inside Visual
Studio Code. It is for forecasters, researchers, and developers who receive
GRIB and NetCDF files and want to see what is in them without leaving the
editor or installing a C toolchain.

Four commitments shape every item below:

1. **Open more of the data that exists in the wild than any other viewer.**
   Breadth of file support is the first priority: formats, grid geometries,
   packings, compression filters, and the containers the bytes arrive in. The
   bar is not "what Panoply or eccodes can open" but "what a forecast centre
   actually publishes".
2. **Tell the user what they are looking at.** A decoded number without a
   name, units, and level is half useful. Complete, human-readable parameter
   and code tables across centres are the second priority.
3. **Be right, provably where it matters.** Every decoder is cross-checked
   against eccodes or an independent implementation, and the small arithmetic
   kernel that turns untrusted bytes into numbers is being formally verified.
   A format is not "complete" until it decodes every registered template,
   matches an oracle, and its kernel verifies.
4. **Pure Rust, zero runtime dependencies, one install.** Every codec is pure
   Rust with no build flags; the extension ships a prebuilt binary per
   platform. This rules out wrapping C libraries, and it is what lets the
   project make claims the C-stack tools cannot.

Interaction features (zoom, animation, cross-sections, plots) matter to users
and are welcome, but they are demand-driven rather than strategic: they are
picked up as capacity allows and do not block the tracks above.

### Where the remaining value is

Through 0.4.0 the organising axis was decode depth. That axis is finished: the
GRIB2 packing space is frozen by the WMO and every registered template
decodes. The value that remains sits on five tracks that are not
interchangeable:

| Track | Question it answers |
|---|---|
| **Geometry** | Can we place this file on a map at all? |
| **Tables** | Does the user know what they are looking at? |
| **Containers** | Can we get at the bytes: filters, large files, remote stores? |
| **Trust** | Are the numbers right, and can we prove it? |
| **Interaction** | Can the user do something useful with the picture? |

Tables and Containers hold the cheapest user-visible wins and have not been
started. Geometry holds the largest single opportunity (ICON). Trust is the
track that turns "decodes everything" into "decodes everything correctly".

### Where verification fits

The formal-verification work (#197–#205) covers the decode kernel: the few
hundred lines that turn untrusted bytes into numbers, where a malformed file
can cause wrong values, overflow, or a panic. It is placed as a parallel track
rather than a gate, for three reasons:

- It pays off most on the code that every GRIB value already passes through
  (bit reading, scaling, spatial differencing, complex-packing group
  expansion), not on new formats. So it starts now, on shipped code, and does
  not wait for the geometry work.
- It runs on pure functions with no I/O and never touches the render
  pipeline or the napi boundary, so it cannot conflict with feature PRs.
- It has already paid for itself once: the groundwork surfaced and fixed a
  `read_bits` truncation defect (#198, shipped in #233) before any proof was
  written.

Order follows blast radius: bootstrap and bit primitives first, then the three
arithmetic paths every GRIB value crosses, then bounds-checked bitmap and
unshuffle code, then NetCDF classic offsets last. "Every registered GRIB2
packing decodes" became true in 0.4.0; under commitment 3 it becomes
*complete* when the kernel behind it verifies.

## Non-goals

- Wrapping eccodes, netcdf-c, HDF5, GDAL, or any C library. Pure Rust only.
- A general GIS. Fieldglass places meteorological fields on a map; it does
  not edit geometry or manage layers.
- GRIB edition 3. Shelved by the WMO; Fieldglass detects the edition byte and
  reports it cleanly.
- IEEE 128-bit floating point in GRIB2 (precision 3). No known data; eccodes
  rejects it too.
- Data-section editing. Metadata editing is on the list; rewriting values is
  not.

## Now

Small, independently shippable, in progress or next to start. No ordering
between them.

Tracked together in the
[Containers, tables, and verification groundwork](https://github.com/D0ubleD0uble/fieldglass/milestone/9)
milestone.

| Item | Track | Why now |
|---|---|---|
| Drop the redundant buffer copies at the napi boundary (#411) | Containers | Peak memory is about 3× file size today; partial fix for large files (#114); no design work. |
| fletcher32 checksum passthrough (#412) | Containers | A checksum, not compression; its presence fails files whose compression we already handle. |
| zstd filter (#413) | Containers | netcdf-c ≥ 4.9 default for new climate archives; pure-Rust decoder available. |
| Cache traversal results on the NetCDF reader (#414) | Containers | Every decode call re-walks the file; free in memory, the whole cost once files are remote. |
| WMO master parameter tables (#415) | Tables | 44 named GRIB2 parameters today, about 1,430 in the WMO master set. Largest visible change per hour in the plan. |
| ~~Bootstrap Verus in the workspace (#197)~~ **Done** | Trust | Enabled the verification track. The proofs sit in a crate outside the workspace, and CI asserts no Verus crate reaches the shipped graph, so the stock build and six release targets are untouched. |

## Next

Problems we are confident we can solve; shape known, timing not.

| Item | Track | What it unlocks |
|---|---|---|
| HEALPix grids, GRIB2 3.150 (#416) | Geometry | DestinE Climate DT output (IFS-NEMO, IFS-FESOM, ICON harmonised). Synthesises onto a lat/lon grid at decode, like spectral. Weeks, not days. |
| Curvilinear NetCDF grids (#218) | Geometry | Ocean tripolar and satellite-swath files. First consumer of the spatial-index seam; coordinates arrive in-band, so no acquisition policy is needed. |
| Verify the core decode arithmetic (#199, #200, #201) | Trust | Simple-packing scaling, inverse spatial differencing, and complex-packing group expansion: the three paths every GRIB value passes through. |
| Verify bitmap decoders and HDF5 unshuffle (#202, #203) | Trust | Bounds-safety on the two paths that index by untrusted counts. |
| GRIB2 local parameter tables: ECMWF (#424), DWD (#425), NCEP (#426) | Tables | Short names and local parameters — the codes at or above 192, which each centre defines for itself — for the three largest publishers. ECMWF's GRIB1 table already ships; this is the GRIB2 side. One generator per PR. |
| Transverse Mercator (3.12, #422) and Lambert azimuthal equal-area (3.140, #423) | Geometry | UK Met Office UKV; CEMS/EFAS and OSI SAF sea ice. Formula-defined, self-contained. |

## Later

Direction, not commitments. Each depends on something in **Next** landing
first.

- **GRIB2 3.204 curvilinear (NCEP local) (#418).** RTOFS ocean data; eccodes lacks
  the template. Second consumer of the spatial-index seam.
- **ICON native grids, GRIB2 3.101 (#420, ADR #419).** DWD publishes ICON worldwide, free, at
  very large daily volume, and GDAL cannot open it. The largest "opens what
  nothing else opens" item left. Needs an ADR on out-of-band mesh resolution
  before code; stays a substantial effort after the seam lands.
- **szip filter (#421).** Unlocks the NASA EOS archive (AIRS, MODIS). Shares its
  entropy coder with GRIB2 5.42 but needs a new framing layer and a change to
  an externally pinned crate. A scheduled project, not a quick win; see
  [`docs/planning/hdf5-filters.md`](docs/planning/hdf5-filters.md) before
  re-costing it.
- **Verify NetCDF classic length arithmetic (#204)**, closing the
  verification milestone (#205).
- **Remote data: HTTP range (#247), S3 (#252), Zarr (#246).** The decision is
  made — [ADR-0005](docs/decisions/0005-byte-access-and-the-remote-seam.md):
  prefetch the ranges an operation needs, decode synchronously, behind a
  `ByteSource` trait. Gated now on the trait landing (#438) and one migrated
  reader.
- **New host surfaces.** PyO3 bindings and a wasm build; the format crates
  are already pure byte-in, values-out engines. Gated on the same seam — and
  the reason ADR-0005 keeps decode synchronous, since blocking inside a read
  is impossible in wasm.
- **GRIB1 completion.** Second-order packing under a masking bitmap, and the
  remaining predefined ON388 grids (21–26, 61–64).
- **Bi-Fourier rendering (5.53).** The last template that decodes without
  rendering. Needs an inverse 2-D Fourier transform.
- **Further tables.** CF standard names for NetCDF; JMA, KMA, BOM local
  tables; originating centres and sub-centres from WMO CCT.
- **NetCDF / HDF5 gaps.** String and char data display, paged array index
  blocks, HDF5 2.0 complex-type detection.
- **BUFR.** A tree and table inspector would be unique among viewers, but it
  shares nothing with the render pipeline. After Zarr.

## Interaction backlog

Picked up as capacity allows; none of these block or are blocked by the
tracks above.

Zoom and pan (#245) · animate over time (#170) · cross-sections (#171) ·
zonal average plots (#240) · vector plots from u/v pairs (#241) · line plots
through the probe point (#172) · GMT CPT colour tables (#236) · export at a
chosen size (#403) · CSV export for the remaining grid families (#244) ·
GRIB1 metadata editing (#46) · contours for projected and reduced grids.

## Done

| Release | What shipped |
|---|---|
| 0.4.0 (2026-08-23) | Every registered GRIB2 §5 packing template decodes, pure Rust, zero build flags; local templates 5.40000, 5.40010, 5.50001, 5.50002. Spherical-harmonic spectral fields render, for GRIB1 and GRIB2, a viewer first. |
| 0.3.0 (2026-07-18) | First crates.io release of the four library crates. |
| 0.2.0 (2026-07-02) | First stable Marketplace release: GRIB1, GRIB2, and NetCDF parse, decode, and render with eight projections and map overlays. |

The [README feature matrix](README.md#feature-matrix) and the
[GRIB2 packing modes](README.md#grib2-packing-modes) table are the source of
truth for what works today.

## How this roadmap is maintained

- Revised after each release and at each WMO fast-track checkpoint. The
  revision date at the top is updated every time.
- Every item in **Now** and **Next** should have an issue; filing the missing
  ones is part of the next revision, not a follow-up.
- Cost claims for unstarted work are unverified until someone has read the
  call sites. The planning notes record what was checked and when.
- Sources to watch for new templates, filters, and conventions are listed in
  [`docs/planning/standards-watch-list.md`](docs/planning/standards-watch-list.md).
