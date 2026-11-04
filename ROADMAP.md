# Fieldglass roadmap

Fieldglass is finishing the browser package and szip work from 0.6.0, and its
next big step is opening remote and multi-gigabyte data from inside VS Code.
This page says where it's going after that, and why. Now is work in progress or
next to start. Next is work whose shape I know but whose timing I don't. Later
is direction, and it will change as I learn more.

What has already shipped is under [Done](#done). The reasoning behind cost
estimates and order lives in
[`docs/planning/`](docs/planning/README.md), and design decisions live in
[`docs/decisions/`](docs/decisions/README.md).

## Strategy

Fieldglass is a viewer for meteorological data files that runs inside Visual
Studio Code. It's for forecasters, researchers and developers who receive
GRIB, NetCDF, HDF5 and Zarr files and want to see what's in them without
leaving the editor or installing a C toolchain. The same decoders are
published as Rust crates on crates.io and as `@fieldglass/wasm` for browsers.

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
   ecCodes or another reference decoder, and the small arithmetic kernel
   that turns untrusted bytes into numbers is formally verified. A format
   isn't complete until it decodes every registered template, matches ecCodes
   or another reference decoder, and its kernel verifies.
4. Pure Rust, no runtime dependencies, one install. Every codec is pure Rust
   with no build flags, and the extension ships a prebuilt binary per platform.
   That rules out wrapping C libraries, and it's what lets the project make
   claims the C-based tools can't.

Interaction features, such as zoom, animation, cross-sections and plots,
matter to users and are welcome. None of the four commitments covers them, so
I pick them up as people ask for them and as there's time, and they don't hold
up the rest of the work.

## Non-goals

- Wrapping ecCodes, netcdf-c, HDF5, GDAL or any C library. Fieldglass is pure
  Rust only.
- A general GIS. Fieldglass places meteorological fields on a map. It doesn't
  edit geometry or manage layers.
- GRIB edition 3. The WMO shelved it, and Fieldglass detects the edition byte
  and reports it cleanly.
- IEEE 128-bit floating point in GRIB2 ([precision 3](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_table5-7.shtml)). There's no known data
  that uses it, and ecCodes rejects it too.
- Editing a file's values. Metadata editing is on the list, but rewriting
  values isn't.

## Now

Most of the work here finishes what 0.6.0 started, and the items are in no
particular order.

- A spectral field finer than the map is drawn at reduced detail, and clicking
  a point shows both the value drawn and the full-detail value. The extension
  also gives the full-detail value for a difference map of two spectral
  fields, and its CSV export notes when a field is shown at reduced detail.
  The browser package will give that difference-map value too, and may write
  the CSV note (#815).
- The message list sent when a reduced-grid file opens may get smaller (#811).
  Today the extension sends every row's point count with each message, about
  2,560 numbers per message at O1280. I'll measure whether that slows opening
  a large file, and trim the listing if it does.
- szip will be checked against a file written by the HDF Group's own SZIP
  library, the writer behind some older HDF5 archives, and will join the
  performance measurements (#820, #764). Every szip file tested so far was
  written by libaec. If the older writer ends its stream differently,
  Fieldglass may refuse valid 32- and 64-bit szip chunks in those archives.
- One known difference from ecCodes, in how a padded GRIB1 bitmap section is
  read, will be recorded (#810). When the section's padding isn't counted in
  its header, ecCodes returns more values than grid points, and Fieldglass
  reads one bit per grid point, as the GRIB1 spec says. Decoding doesn't
  change. It's the last open item in the verification epic (#205).

## Next

The extension will open GRIB, NetCDF and Zarr data on public HTTP and S3
buckets without a full download, and multi-gigabyte archives without reading
them whole into memory (#1644). The first transport is HTTP range requests to
a public bucket (#247). Anonymous S3 access (#252) follows, since it needs
those range reads, and it matters because NOAA's Open Data buckets hold the
archives. A remote Zarr store opens the same way. Streaming reads (#114)
replace reading a whole file into memory, which today makes archives of
several gigabytes a poor fit (a Zarr store is already read chunk by chunk).
Stores that need credentials aren't part of this work. The browser package can
already read parts of a remote GRIB file or Zarr store
(`@fieldglass/wasm/full`, since 0.6.0). The format readers already read a file
in pieces, so the work is in the extension, not the readers
([ADR-0005](docs/decisions/0005-byte-access-and-the-remote-seam.md)).

The UX revamp (#1052) aims to make opening a file show the field straight
away, with the file's structure one glance away and every number traceable to
where it came from. It's the item in Next closest to starting. Its proposals
include a map-first render panel, panels that follow VS Code's themes and
high-contrast mode, a full keyboard and screen-reader path, and views that
survive a reload and can be shared. I decide each proposal on its own before
anyone starts it.

## Later

Items with an issue or a clear scope come first.

- GRIB2 [3.204](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-204.shtml), NCEP's local curvilinear grid (#418). It carries RTOFS ocean
  data, and ecCodes lacks the template. It would reuse the point lookup built
  for curvilinear NetCDF.
- ICON native grids, GRIB2 [3.101](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-101.shtml) (#420, ADR #419). DWD publishes ICON
  worldwide, free, at very large daily volume, and GDAL can't open it. It's the
  largest "opens what nothing else opens" item left. The file doesn't carry
  its own mesh, so an ADR on how to find it comes before any code, and the
  work stays substantial after that.
- HDF4 and HDF-EOS2 (#248), which many NASA EOS products use. szip, which they
  often use too, already decodes.
- GRIB1 metadata editing (#46), the one interaction item 0.6.0 didn't ship.
- More hosts, the bindings that let other programs call the library: PyO3
  bindings (no issue filed yet) and a command-line tool (#254, #1074). Each
  would be a binding over the one `fieldglass` crate
  ([ADR-0006](docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)),
  so adding one means handing over buffers and mapping errors.
- GRIB1 completion: second-order packing under a masking bitmap, and the
  remaining predefined [ON388 grids](https://www.nco.ncep.noaa.gov/pmb/docs/on388/tableb.html) (21–26 and 61–64).
- Drawing bi-Fourier fields ([5.53](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-53.shtml)). It's the last template that decodes but
  doesn't draw, and it needs an inverse 2-D Fourier transform.
- NetCDF and HDF5 gaps: showing string and char data, paged array index
  blocks, and detecting HDF5 2.0's complex type.
- More tables: [CF standard names](https://cfconventions.org/Data/cf-standard-names/current/build/cf-standard-name-table.html) for NetCDF, and JMA, KMA and BOM local tables.
- BUFR, the WMO format used mainly for observations such as surface reports
  and upper-air soundings. A message lists its contents as a sequence of
  descriptors, looked up in WMO Tables B and D, and its data section holds
  the values in that order. Opening it would need a decoder of its own and
  those tables, which Fieldglass doesn't have yet. Of the existing work, the
  originating-center table carries over, because WMO's Common Code Table
  serves GRIB and BUFR alike.

## Why the work is ordered this way

### Where the remaining value is

Up to 0.4.0, the work was organized around decoding more, and that work is
done: the WMO has frozen the GRIB2 packing list, and every registered template
decodes. The value that remains sits on six tracks, and they don't substitute for one
another:

| Track | Question it answers |
|---|---|
| Geometry | Can this file be placed on a map at all? |
| Tables | Does the user know what they're looking at? |
| Containers | Can the bytes be reached: filters, large files, remote stores? |
| Trust | Are the numbers right, and is that proved? |
| Interaction | Can the user do something useful with the picture? |
| Hosts | Who can call this at all, from which runtimes and languages? |

Tables is largely done. The WMO master set and the ECMWF, DWD and NCEP local
tables have landed, so a real file names its own fields. Geometry now supports
the two families it was waiting on, HEALPix and curvilinear, which leaves ICON
as the largest opportunity.

Containers is mostly built. What's left is the remote transports and
streaming reads in the extension, which is why remote data leads Next.

Hosts is the newest track, and it has its own row because its work doesn't
depend on the other tracks. The format crates take bytes in and hand values
out, so every host added reaches the same decoders. The browser package has
landed, #815 and #811 in Now finish that package, and the remaining hosts are
in Later.

### Where verification fits

The formal verification work (#197–#205) covers the decode kernel: the few
hundred lines that turn untrusted bytes into numbers, where a malformed file
can cause wrong values, an overflow or a panic. Every piece of it is now
proved. Verification runs alongside the other tracks by design, and nothing
waits on it. It pays off most on the code every GRIB value already passes
through (bit reading, scaling, spatial differencing and complex-packing group
expansion), so it started on shipped code. It touches only pure functions with
no I/O, so it never conflicted with feature work. The groundwork also found
and fixed a `read_bits` truncation bug (#198, shipped in #233) before any
proof was written.

## Done

| Release | What shipped |
|---|---|
| 0.6.0 (2026-10-31) | Zarr [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) stores open in the viewer. szip-compressed HDF5 opens, through the project's own [CCSDS](https://public.ccsds.org/Pubs/121x0b3.pdf) decoder (`fieldglass-aec`), which also decodes GRIB2 [5.42](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) in place of `rust-aec`. Animation, line plots, cross-sections, wind arrows, a zonal average, zoom and pan, and imported GMT color tables. Every ECMWF GRIB1 table. A NetCDF slice reads only its own bytes. `@fieldglass/wasm` on npm, and `fieldglass`, `-zarr`, `-fetchplan` and `-aec` on crates.io. Verus proofs over the decode kernel, including classic NetCDF's offsets. |
| 0.5.0 (2026-09-04) | Grid geometry: HEALPix ([3.150](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-150.shtml)), reduced Gaussian in GRIB2 ([3.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-40.shtml)), transverse Mercator ([3.12](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-12.shtml)), Lambert azimuthal equal-area ([3.140](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-140.shtml)), and curvilinear NetCDF all decode and render. The complete [WMO parameter table](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_table4-2.shtml) (1,387) and [Common Code Table](https://github.com/wmo-im/CCT) centers (309 for GRIB2, 239 for GRIB1), plus ECMWF, DWD and NCEP local tables. Contours and `lat,lon,value` CSV on the projected and reduced families. Formal verification of the decode kernel wired up with Verus. |
| 0.4.0 (2026-08-23) | Every registered GRIB2 [§5](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_sect5.shtml) packing template decodes, in pure Rust with no build flags, including the local templates 5.40000, 5.40010, 5.50001 and 5.50002. Spherical-harmonic spectral fields render, for GRIB1 and GRIB2, a first for a viewer. |
| 0.3.0 (2026-07-18) | First crates.io release of the four library crates. |
| 0.2.0 (2026-07-02) | First stable Marketplace release: GRIB1, GRIB2 and NetCDF parse, decode and render with eight projections and map overlays. |

The extension's [feature table](extension/README.md#what-works-where) and the
[GRIB2 packing modes](README.md#grib2-packing-modes) table are the source of
truth for what works today, and [`CHANGELOG.md`](CHANGELOG.md) lists every
change in each release.

## How this roadmap is maintained

I revise this page after each release and at the twice-yearly WMO fast-track
checkpoints in June and November, and each revision gets an entry below. Every
item in Now and Next links an issue or an epic, and that issue is where its
current state lives. A cost claim for work that hasn't started stays
unverified until someone has read the code it touches, and the planning notes
record what was checked and when. The sources I watch for new templates,
filters and conventions are listed in
[`docs/planning/standards-watch-list.md`](docs/planning/standards-watch-list.md).

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
- 2026-10-27: rewritten in plain words, with this list moved to the end. #880
  closed and left Next, the documentation revamp milestone joined Now, and the
  UX revamp epic (#1052) joined Next.
- 2026-11-03: rewritten for readers, with the plans moved ahead of the
  reasoning, the interaction backlog folded into Later, and the
  contributor-only cleanups (#803, #904, #934) taken off the page. Day-to-day
  state moved to the issues, the verification and remote-data work became
  epics (#205, #1644), and milestones are moving to mean releases only.
