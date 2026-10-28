# Fieldglass roadmap

This document says where Fieldglass is going and why. It's intent, not a
commitment. Items under Now are in progress, and everything further out will
change as we learn. The reasoning behind cost estimates and order lives in
[`docs/planning/`](docs/planning/README.md), and design decisions live in
[`docs/decisions/`](docs/decisions/README.md). Issues are linked where they
exist, and an item without a link still needs one filed.

It was adopted on 2026-07-19, and it's reviewed after each release and at the
twice-yearly WMO fast-track checkpoints in June and November. The revisions so
far are listed at the end.

## Strategy

Fieldglass is a viewer for meteorological data files that runs inside Visual
Studio Code. It's for forecasters, researchers and developers who receive
GRIB and NetCDF files and want to see what's in them without leaving the
editor or installing a C toolchain.

Four commitments shape every item below:

1. Open more of the data that exists in the wild than any other viewer.
   Breadth of file support comes first: formats, grid geometries, packings,
   compression filters, and the containers the bytes arrive in. The bar isn't
   "what Panoply or ecCodes can open" but "what a forecast center actually
   publishes".
2. Tell the user what they're looking at. A decoded number without a name,
   units and level is only half useful, so complete, readable parameter and
   code tables across centers come second.
3. Be right, and prove it where it matters. Every decoder is checked against
   ecCodes or an independent implementation, and the small arithmetic kernel
   that turns untrusted bytes into numbers is being formally verified. A format
   isn't complete until it decodes every registered template, matches an
   oracle, and its kernel verifies.
4. Pure Rust, no runtime dependencies, one install. Every codec is pure Rust
   with no build flags, and the extension ships a prebuilt binary per platform.
   That rules out wrapping C libraries, and it's what lets the project make
   claims the C-based tools can't.

Interaction features, such as zoom, animation, cross-sections and plots,
matter to users and are welcome. But they're picked up as people ask for them
and as there's time, rather than as strategy, and they don't hold up the
tracks below.

### Where the remaining value is

Up to 0.4.0, the work was organized around decoding more. That's finished: the
WMO has frozen the GRIB2 packing list, and every registered template decodes.
The value that remains sits on six tracks, and they don't substitute for one
another:

| Track | Question it answers |
|---|---|
| Geometry | Can we place this file on a map at all? |
| Tables | Does the user know what they're looking at? |
| Containers | Can we get at the bytes: filters, large files, remote stores? |
| Trust | Are the numbers right, and can we prove it? |
| Interaction | Can the user do something useful with the picture? |
| Hosts | Who can call this at all, from which runtimes and languages? |

After 0.5.0, Tables was largely done. The WMO master set and the ECMWF, DWD
and NCEP local tables all landed, so a real file names its own fields.
Geometry took the two families it was waiting on, HEALPix and curvilinear,
which leaves ICON as the largest opportunity. Containers was half built: the
filters and the `ByteSource` seam were in, but the remote transports weren't.
Trust is the track that turns "decodes everything" into "decodes everything
correctly", and it runs alongside the others by design.

Hosts is the newest track, and it's why the wasm build moved up out of Later.
That build has since landed, and what's left of the track is in Next. It isn't
another kind of feature so much as a multiplier on the other five. The format
crates already take bytes in and hand values out, so every runtime added
reaches the same decoders. It gets a track of its own because it competes with
the others for time rather than building on them.

After 0.6.0, Containers is mostly built. Zarr v2 and v3 stores open, szip
decodes, and a NetCDF slice reads only its own bytes. What's left is the
remote transports and streaming reads in the extension, both host work over
seams that already exist. Hosts gained the browser package, and the
interaction backlog is cleared except for metadata editing.

### Where verification fits

The formal verification work (#197–#205) covers the decode kernel: the few
hundred lines that turn untrusted bytes into numbers, where a malformed file
can cause wrong values, an overflow or a panic. It runs alongside the other
tracks rather than gating them, for three reasons:

- It pays off most on the code every GRIB value already passes through (bit
  reading, scaling, spatial differencing and complex-packing group expansion),
  not on new formats. So it started on shipped code and doesn't wait for the
  geometry work.
- It runs on pure functions with no I/O, and never touches the render pipeline
  or the napi boundary, so it can't conflict with feature pull requests.
- It has already paid for itself once. The groundwork found and fixed a
  `read_bits` truncation bug (#198, shipped in #233) before any proof was
  written.

The order follows how much each piece could break. First come the setup and
the bit primitives, then the three arithmetic paths every GRIB value crosses,
then the bounds-checked bitmap and unshuffle code, and classic NetCDF's
offsets last. "Every registered GRIB2 packing decodes" became true in 0.4.0.
Under commitment 3, it becomes complete when the kernel behind it verifies.

## Non-goals

- Wrapping ecCodes, netcdf-c, HDF5, GDAL or any C library. Fieldglass is pure
  Rust only.
- A general GIS. Fieldglass places meteorological fields on a map. It doesn't
  edit geometry or manage layers.
- GRIB edition 3. The WMO shelved it, and Fieldglass detects the edition byte
  and reports it cleanly.
- IEEE 128-bit floating point in GRIB2 (precision 3). There's no known data
  that uses it, and ecCodes rejects it too.
- Editing a file's values. Metadata editing is on the list, but rewriting
  values isn't.

## Now

These are small, can ship on their own, and are in progress or next to start.
They're in no particular order, and most of them close out an open milestone.

| Item | Track | Why now |
|---|---|---|
| The combined full-detail probe and the CSV's band-limit line in `fieldglass` (#815) | Hosts | Only the Node binding builds them today, so the browser package has no full-detail value for a difference map of spectral fields. Part of the [fieldglass-wasm](https://github.com/D0ubleD0uble/fieldglass/milestone/11) milestone. |
| A reduced-grid listing without every row's point count (#811) | Hosts | Listing a reduced-grid file sends every message's row lengths, about 2,560 numbers per message at O1280. Measure first, then trim the listing on both hosts if it matters. |
| The last szip work (#764, #803, #820) | Trust | szip rows in the performance corpus, the oracle script honoring `CARGO_TARGET_DIR`, and a check against szip written by the HDF Group's own library. Closes the [AEC milestone](https://github.com/D0ubleD0uble/fieldglass/milestone/13). |
| Two cleanups (#904, #934) | Trust | An ADR statement that doesn't hold for one fixture, and one helper for HDF5's all-ones values. Closes [milestone 12](https://github.com/D0ubleD0uble/fieldglass/milestone/12). |
| The GRIB1 Section 3 padding divergence recorded (#810) | Trust | The last open item beside the verification epic (#205). |
| The documentation revamp ([milestone](https://github.com/D0ubleD0uble/fieldglass/milestone/16)) | Hosts | Every page a user, integrator or contributor reads, rewritten in plain words: a Marketplace page of its own, one shape for every crate page, and a check that each README's code compiles (#1504). |

## Next

These are problems we're confident we can solve. Their shape is known, but
their timing isn't.

The readers now read files by range through `ByteSource`, and stores through
`ObjectSource`. So a remote file or store is the host filling one of those
seams, not new reader work
([ADR-0005](docs/decisions/0005-byte-access-and-the-remote-seam.md)). Local
Zarr stores shipped in 0.6.0 under the Zarr epic (#246), and a remote store is
the S3 row below.

| Item | Track | What it unlocks |
|---|---|---|
| Remote data over HTTP range requests (#247) | Containers | The first transport, and the one every other depends on. It opens a file on a public bucket without downloading it. |
| S3 dataset access (#252) | Containers | NOAA's Open Data buckets are where the archives actually are, the same objects the fixture builders already fetch anonymously. A remote Zarr store is the host filling an `ObjectSource` from such a bucket. |
| Streaming reads in the extension (#114) | Containers | The readers read by range, but the extension still reads a whole GRIB or NetCDF file into memory to keep remote and virtual workspaces working, which rules out archives of several gigabytes. |
| The UX revamp (#1052) | Interaction | Opening a file should show the field straight away, with the file's structure one glance away and every number traceable to where it came from. Each proposal waits on a go or no-go from the owner before anyone starts it. |

Verification is deliberately not listed here. It runs alongside the other
tracks by design (see [Where verification fits](#where-verification-fits)),
on shipped code in a crate outside the workspace, and it's tracked in its own
milestone rather than queued behind anything in this section.

## Later

This is direction, not commitment. Each item depends on something in Next
landing first.

- GRIB2 3.204, NCEP's local curvilinear grid (#418). It carries RTOFS ocean
  data, and ecCodes lacks the template. It would be the second user of the
  spatial-index seam.
- ICON native grids, GRIB2 3.101 (#420, ADR #419). DWD publishes ICON
  worldwide, free, at very large daily volume, and GDAL can't open it. It's the
  largest "opens what nothing else opens" item left. It needs an ADR on finding
  the mesh outside the file before any code, and stays a substantial effort
  after the seam lands.
- HDF4 and HDF-EOS2 (#248), which many NASA EOS products use. szip, which they
  often use too, already decodes.
- More hosts: PyO3 bindings and a command-line tool (#254), now that the wasm
  build has landed. The format crates already take bytes in and hand values
  out, and each host is a binding over the one `fieldglass` crate
  ([ADR-0006](docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)),
  so a new host means handing over buffers and mapping errors, not a second
  engine.
- GRIB1 completion: second-order packing under a masking bitmap, and the
  remaining predefined ON388 grids (21–26 and 61–64).
- Drawing bi-Fourier fields (5.53). It's the last template that decodes but
  doesn't draw, and it needs an inverse 2-D Fourier transform.
- More tables: CF standard names for NetCDF, and JMA, KMA and BOM local tables.
- NetCDF and HDF5 gaps: showing string and char data, paged array index
  blocks, and detecting HDF5 2.0's complex type.
- BUFR. A tree and table inspector would be unique among viewers, but it
  shares nothing with the render pipeline.

## Interaction backlog

These are picked up as there's time. None of them block, or are blocked by,
the tracks above.

GRIB1 metadata editing (#46) is the one item left. Everything else that was
here shipped in 0.6.0: zoom and pan, animation, cross-sections, the zonal
average, wind arrows, line plots, GMT color tables, and CSV export for every
grid family.

## Done

| Release | What shipped |
|---|---|
| 0.6.0 (2026-10-12) | Zarr v2 and v3 stores open in the viewer. szip-compressed HDF5 opens, through the project's own CCSDS decoder (`fieldglass-aec`), which also decodes GRIB2 5.42 in place of `rust-aec`. Animation, line plots, cross-sections, wind arrows, a zonal average, zoom and pan, and imported GMT color tables. Every ECMWF GRIB1 table. A NetCDF slice reads only its own bytes. `@fieldglass/wasm` on npm, and `fieldglass`, `-zarr`, `-fetchplan` and `-aec` on crates.io. Verus proofs over the decode kernel, including classic NetCDF's offsets. |
| 0.5.0 (2026-09-04) | Grid geometry: HEALPix (3.150), reduced Gaussian in GRIB2 (3.40), transverse Mercator (3.12), Lambert azimuthal equal-area (3.140), and curvilinear NetCDF all decode and render. The complete WMO parameter table (1,387) and Common Code Table centers (309 / 239), plus ECMWF, DWD and NCEP local tables. Contours and `lat,lon,value` CSV on the projected and reduced families. Formal verification of the decode kernel wired up with Verus. |
| 0.4.0 (2026-08-23) | Every registered GRIB2 §5 packing template decodes, in pure Rust with no build flags, including the local templates 5.40000, 5.40010, 5.50001 and 5.50002. Spherical-harmonic spectral fields render, for GRIB1 and GRIB2, a first for a viewer. |
| 0.3.0 (2026-07-18) | First crates.io release of the four library crates. |
| 0.2.0 (2026-07-02) | First stable Marketplace release: GRIB1, GRIB2 and NetCDF parse, decode and render with eight projections and map overlays. |

What works today is set out in the extension's
[feature table](extension/README.md#what-works-where) and the
[GRIB2 packing modes](README.md#grib2-packing-modes) table, which are the
source of truth.

## How this roadmap is maintained

- It's revised after each release and at each WMO fast-track checkpoint, and
  the revision list below gets a new entry every time.
- Every item in Now and Next should have an issue. Filing the missing ones is
  part of the next revision, not a follow-up.
- Cost claims for work that hasn't started are unverified until someone has
  read the code it touches. The planning notes record what was checked and
  when.
- The sources to watch for new templates, filters and conventions are listed
  in [`docs/planning/standards-watch-list.md`](docs/planning/standards-watch-list.md).

### Revisions

- 2026-07-19: adopted.
- 2026-08-26, during 0.5.0 prep: milestone 10 closed, Now refilled from the
  fieldglass-wasm milestone, and the Hosts track added.
- 2026-09-06: the umbrella crate, the browser layer and its CI gates landed,
  so those rows left Now and Next (#467).
- 2026-09-07: the workspace described as layers under
  [ADR-0010](docs/decisions/0010-a-common-array-model-and-containers-as-drivers.md),
  with the milestone 12 sweep that builds them (#677–#687) in Now.
- 2026-10-12, during 0.6.0 prep: Zarr stores, szip, the browser package and
  the interaction backlog shipped, so Now and Next were refreshed against the
  open milestones.
