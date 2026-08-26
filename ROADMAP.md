# Fieldglass roadmap

*Adopted 2026-07-19. Revised 2026-08-26 during 0.5.0 prep: milestone 10 closed,
**Now** repopulated from the fieldglass-wasm milestone, and a **Hosts** track
added. Reviewed after each release and at the twice-yearly WMO fast-track
checkpoints (June and November).*


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
decodes. The value that remains sits on six tracks that are not
interchangeable:

| Track | Question it answers |
|---|---|
| **Geometry** | Can we place this file on a map at all? |
| **Tables** | Does the user know what they are looking at? |
| **Containers** | Can we get at the bytes: filters, large files, remote stores? |
| **Trust** | Are the numbers right, and can we prove it? |
| **Interaction** | Can the user do something useful with the picture? |
| **Hosts** | Who can call this at all — which runtimes and languages? |

Read after 0.5.0, the picture has moved. **Tables** is largely done: the WMO
master set plus ECMWF, DWD and NCEP local tables all landed, so a real file
names its own fields. **Geometry** took the two families it was gated on —
HEALPix and curvilinear — leaving ICON as the single largest opportunity.
**Containers** is half-built: the filters and the `ByteSource` seam are in, the
remote transports are not. **Trust** is the track that turns "decodes
everything" into "decodes everything correctly", and is a parallel effort by
design.

**Hosts** is new to this list, and it is the reason the wasm build sits in
**Now**. It is not a sixth kind of feature so much as a multiplier on the other
five: the format crates are pure byte-in, values-out engines already, so every
runtime added reaches the same decoders. It earns a track of its own because it
trades against them for attention rather than composing with them.

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
[fieldglass-wasm: browser host surface](https://github.com/D0ubleD0uble/fieldglass/milestone/11)
milestone. Milestone 10 closed on 2026-08-26 with the grid-geometry, local-table
and byte-access work done; this is the promotion of "New host surfaces" out of
**Later**, now that the seam it was gated on ([ADR-0005](docs/decisions/0005-byte-access-and-the-remote-seam.md),
`ByteSource`, #438) has landed.

Most of the milestone waits on one issue, so the three below are what is
genuinely startable. The rest are in **Next**.

| Item | Track | Why now |
|---|---|---|
| `fieldglass-wasm`, a synchronous browser façade (#460) | Hosts | The root: five other issues in the milestone wait on it. ADR-0005 keeps decode synchronous precisely so this is possible — blocking inside a read cannot work in wasm. |
| `fieldglass-fetchplan`, manifests in, byte ranges out (#461) | Containers | Unblocked (#417 and #426 are closed). Turns a `.idx` sidecar into the byte ranges an operation needs, which is what makes a multi-GB archive openable without downloading it. |
| Reduced-resolution decode for JPEG 2000 fields (#463) | Containers | Independent of the byte-access seam entirely, so it can run alongside. 5.40 carries its own resolution levels; decoding fewer is free bytes and free time. |

## Next

Problems we are confident we can solve; shape known, timing not.

| Item | Track | What it unlocks |
|---|---|---|
| ~~HEALPix grids, GRIB2 3.150 (#416)~~ **Done.** | Geometry | Resampled onto a lat/lon grid at decode, like spectral, so every downstream path works unchanged. |
| ~~Curvilinear NetCDF grids (#218)~~ **Done.** | Geometry | Both shapes ship: a k-d tree over cell centres as unit vectors, with the tripolar and swath corpus of #444. |
| Verify the core decode arithmetic (#199, #200, #201) | Trust | Simple-packing scaling, inverse spatial differencing, and complex-packing group expansion: the three paths every GRIB value passes through. |
| Verify bitmap decoders and HDF5 unshuffle (#202, #203) | Trust | Bounds-safety on the two paths that index by untrusted counts. |
| ~~GRIB2 local parameter tables: ECMWF (#424), DWD (#425), NCEP (#426)~~ **Done.** | Tables | 2,826 ECMWF parameters, 213 DWD, 479 NCEP; one generator per PR as planned. |
| ~~Transverse Mercator (3.12, #422) and Lambert azimuthal equal-area (3.140, #423)~~ **Done.** | Geometry | Both landed, each checked against an outside oracle - PROJ for 3.12, eccodes for 3.140. |

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
- **Further host surfaces.** PyO3 bindings and a CLI (#254), after the wasm
  build in **Now**. The format crates are already pure byte-in, values-out
  engines, and each host is a binding over one `fieldglass` umbrella crate
  ([ADR-0006](docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)),
  so a new host is buffer handoff and error mapping, not a second engine.
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
GRIB1 metadata editing (#46) · contours and long-format CSV for reduced grids
(the projected families landed in #470; a reduced grid still needs its per-row
longitudes to place a point).

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
