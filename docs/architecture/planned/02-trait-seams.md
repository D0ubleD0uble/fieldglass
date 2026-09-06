# Planned — Level 2: trait seams

After milestones 7, 10, and 11. Compare with [`../02-trait-seams.md`](../02-trait-seams.md).
Only the seams that change are drawn; `Grib1Packing` and the
`TargetProjection` / `PreparedTarget` / `ForwardMap` family are unchanged and
stay in the guarded diagram.

## Grid geometry becomes a type; the inverse stays a closure

Today the nine per-family warp setups live in `napi` and take a flat 65-field
`MessageMeta`. After #460 (which creates it) and #464 (which moves napi onto
it) `core` owns `GridGeometry`, one variant per family, built straight from
the typed GDS. The seam `warp` consumes is unchanged: `SourceGrid::inverse_at`
is already a closure, so `GridGeometry::inverse_at()` is a `match` per variant
returning that closure, not a new trait. Behind it are two kinds of inverse: a
formula (the projectors, closed-form functions for lat/lon and Mercator) and a
lookup (`SpatialIndex`, #437) for grids that are only a list of cell centres.

```mermaid
classDiagram
    class GridGeometry {
        <<planned #460 then #464; the questions shipped #571>>
        +inverse_at() Inverse closure for SourceGrid
        +forward(i, j) Option~(lat, lon)~
        +lonlat_bbox() where the data is
        +render_window() what a render frames
        +reprojectable(scan) bool
        +is_periodic_x() bool
        +contour_seam_wraps() bool
        +resampling() Any | NearestOnly
        +proj4() Option~String~
    }
    class Scan {
        <<shipped #571, crate fieldglass-core>>
        +i_negative, j_positive, j_consecutive
        +flips_source_rows(requested) bool
    }
    Scan ..> GridGeometry : passed to reprojectable()
    class PlanarGridProjector {
        <<trait>>
        shipped #486
        +forward_xy(lat, lon) required
        +inverse(lat, lon) provided once for all planar grids
        +accepts(lat, lon) hook, default true
        +snap_eps() SnapEps hook, default Cells(1e-9)
    }
    class SpatialIndex {
        <<planned #437>>
        cell centres, k-d tree or HEALPix buckets
        nearest cell only: no fractional index, no bilinear
    }

    GridGeometry ..> PlanarGridProjector : Lambert, polar stereo, transverse Mercator, Lambert azimuthal
    GridGeometry ..> GaussianProjector
    GridGeometry ..> RotatedLatLonProjector
    GridGeometry ..> GeostationaryProjector
    GridGeometry ..> latlon_inverse : closed form
    GridGeometry ..> mercator_inverse : closed form
    GridGeometry ..> SpatialIndex : Lookup variant
    PlanarGridProjector <|.. LambertProjector
    PlanarGridProjector <|.. PolarStereoProjector
    PlanarGridProjector <|.. TransverseMercatorProjector
    PlanarGridProjector <|.. LambertAzimuthalProjector
```

Two things this fixes on the way. The four planar `inverse()` bodies were the
same 30 to 50 lines each (guards, forward, `(x − origin) / spacing`, edge
snap); the forward direction was already a provided method on
`PlanarGridProjector`, and the inverse joined it in #486, so the edge-snap rule
exists once. What genuinely differed went into two hooks rather than four
near-copies: `accepts` for a point the projection cannot place at all, and
`snap_eps` for the edge tolerance — `SnapEps::Cells` for a round trip that
closes to float noise, `SnapEps::Metres` for Lambert azimuthal, whose authalic
series carries a real millimetre-scale error, so its tolerance is a property of
the ground rather than of the cell. And `GridIndex` is fractional because the raster grids are: a lookup
grid returns the nearest cell centre, where the fractional part and the
"next column" neighbour mean nothing, so `GridGeometry::Lookup` reports
`NearestOnly` and `warp` refuses or degrades bilinear against it rather than
blending across a tripolar fold.

**The acceptance test for any inverse, formula or lookup, is that a grid places
its own points.** Walk every `(i, j)`, geolocate it, invert it, and get `(i, j)`
back. It needs no external oracle — the grid's own forward map is the answer —
and it is what `.eccodes.ref.json` cannot supply, since that pins only the
forward direction. `crates/fieldglass-core/tests/grid_round_trip.rs` runs it
across all nine families; a new one belongs there the day it is written.

The check existed before that file and still missed two bugs, which is the part
worth remembering: `assert_round_trips` in `projection.rs` had the right shape at
eight call sites, but each built a small synthetic grid, and the polar
stereographic one started at 27°N. It could not have found #488, where a north
polar grid refused everything south of the equator, because no grid it tested
went there. Coverage is a property of the fixture. A lookup grid's version of
the same trap is a tripolar fold or a swath edge that the test grid happens not
to contain — so the fixtures for #444 and #445 should be the awkward ones, not
the convenient ones.

The consumers of the lookup, in the order they are filed:

| Consumer | Issue | Where the cell centres come from |
| --- | --- | --- |
| NetCDF 2-D coordinate (curvilinear) grids | #445 | CF `coordinates` → two 2-D lat/lon variables |
| HEALPix §3.150 | #442, #443 | `pix2ang` in core; synthesised onto lat/lon at decode, like spectral |
| GRIB2 §3.204 NCEP curvilinear | #418 | lat/lon carried as two extra fields |
| ICON §3.101 unstructured | #420, #419 | out-of-band grid file, ADR pending |

## Byte access grows its planned implementers

`ByteSource` exists (#438, NetCDF classic migrated). The remote transports are
**not** Rust implementers: ADR-0005 puts fetching in the host. What Rust gains
is a source over ranges the host already fetched.

```mermaid
classDiagram
    class ByteSource {
        <<trait>>
        +prefetch(ranges)
        +read(range) Cow
        +size() u64
    }
    class PrefetchedRanges {
        <<planned #247 #252 #114>>
        sparse map of fetched ranges; read() outside them is an error
    }
    ByteSource <|.. Vec
    ByteSource <|.. PrefetchedRanges
```

## Fetch planning is a seam of its own

`fieldglass-fetchplan` (#461) reads a manifest and returns ranges. Every
cloud-native convention is one dialect. The crate is syntax only: matching a
sidecar's `TMP` / `2 m above ground` to a WMO parameter needs the NCEP table
in `fieldglass-grib2` (#426), and that dependency would drag the decoder and
its codecs into a pure planner, so semantic matching is a trait the umbrella
implements.

**The GRIB half has landed**, and the current
[`02-trait-seams.md`](../02-trait-seams.md) is where it is documented; what
stays here is the Zarr half, which lands with the codec crate (#246) so both
are tested against the same fixtures. Four things about the shipped half are
now facts rather than plans:

* **No `core` edge, and then one.** The crate was drawn depending on `core`;
  it turned out to need nothing from it *except* the one thing that matters —
  `ByteRange`, the type `ByteSource::prefetch` takes. So a plan range is
  `PlanRange` (`Exact` / `OpenEnded` / `Whole`, because a `.idx` states no
  length) and `PlanRange::close(object_size)` returns `core`'s `ByteRange`.
  Two spellings of one concept was the alternative, and the diagram drift
  guard flagged the name collision before it shipped.
* **`Manifest::messages` is a provided method**, not a per-dialect one:
  collapsing the records that share a byte range is a property of the plan and
  not of the grammar it was read from.
* **`NoResolver` is part of the surface.** Without an implementer that resolves
  nothing, the syntactic path — matching the sidecar's own words, which is what
  a user pasting a `.idx` line has — could not be exercised without the
  umbrella, and the crate would not be testable alone.
* **Ambiguity needs two knobs, not one.** `Query::qualifiers` can only *add*
  requirements, and NBM publishes a plain deterministic field beside its
  probabilistic ones under the same abbreviation and level. `Query::unqualified`
  is how that record is named; without it, it is the one member of an ambiguous
  set nothing can ask for.

```mermaid
classDiagram
    class Manifest {
        <<trait, shipped #461>>
        +items() Vec~PlanItem~
        +select(query, &dyn ParameterResolver) Vec~PlanItem~
        +messages() Vec~PlanItem~ (provided)
    }
    class ParameterResolver {
        <<trait, shipped #461>>
        +resolve(abbrev, level_str) Option~ParameterId~
    }
    class TableResolver {
        <<shipped, crate fieldglass>>
        grib2 tables (#426) inverted into an index
    }
    class NoResolver {
        <<shipped>>
        resolves nothing; the syntactic path
    }
    ParameterResolver <|.. TableResolver
    ParameterResolver <|.. NoResolver
    class PlanItem {
        <<shipped #461>>
        +String key
        +PlanRange range
        +Option~u32~ sub_index
        +Expect expect (parameter, level, forecast from the sidecar line)
    }
    class Expect {
        <<shipped #461>>
        the plan is a claim: verify_envelope checks magic and §0 length
    }
    Manifest <|.. Wgrib2Idx
    Manifest <|.. EcmwfIndex
    Manifest <|.. ZarrV3
    Manifest <|.. ZarrV2
    Manifest <|.. KerchunkRefs
    Manifest ..> PlanItem : produces
    PlanItem *-- Expect
```

## Decode options

#463 adds a `DecodeOptions { resolution_reduction }` to the GRIB2 decode path.
It is not a trait: only 5.40 honours it, the rest return `Unsupported` for a
non-zero value, and a message with a bitmap refuses it. The point of drawing
it is that a reduced field carries a **derived** `GridGeometry`, never the
message's own GDS.

**The decode half of this has landed** (`fieldglass-grib2`,
`Grib2Reader::decode_message_raster_with`), and three things about it are now
facts rather than plans:

* The derived geometry is `GridGeometry::subsampled(r)` in `core`, not a
  `From(gds, r)` in grib2 — it is a question about a grid, and both editions
  will want it. It declines a Gaussian grid, because every other
  Gauss–Legendre node is not the half-order quadrature and there is no
  `GaussianParams` describing the kept rows.
* The refusals are five, not two: any packing but 5.40, a §6 bitmap, a reduced
  grid, alternate-row or `j`-consecutive scanning, and a family with no
  derivable coarse geometry. The two scanning orders are the ones the plan
  missed — both are undone *after* the packing decodes, and a wavelet low-pass
  of a scrambled raster has no undo.
* The "render and warp only" rule is carried by `DisplayRaster`, whose
  `exact_values()` answers `Some` only at reduction zero. `Option`, not a new
  error variant, following #633's precedent: the caller's response to "these
  are not the message's numbers" is to ask again at reduction zero, not to
  branch on a code.

**The host half is not built, and the sequence below is what it would be.**
It turns on a decision nobody has made: `Session::render`, `warp` and `palette`
all take `&Field`, and so do `probe`, `contours` and `combine`. A
`DisplayField` that the first three accept and the last three reject needs
either a sealed trait bound on the render methods — which every binding then
has to monomorphise anyway, since neither wasm-bindgen nor napi crosses a
generic — or a parallel `render_display` / `warp_display` / `palette_display`
on the host surface. ADR-0006 decision 2 ("no generics, lifetimes, or trait
objects") bears on the first and `api_rules.rs` enforces it for types; it says
nothing about method bounds, which is precisely the gap. Until that is settled
the capability is reachable from the format crate and from nothing else.

```mermaid
sequenceDiagram
    participant H as host
    participant S as Session (fieldglass)
    participant M as Grib2Message
    participant J as rust_j2k
    participant G as GridGeometry (From, in grib2)
    H->>S: decode(i, { reduce: r })
    S->>M: decode_with(DecodeOptions { reduce: r })
    M->>J: decode_with(bytes, resolution_reduction = r)
    J-->>M: ni/2^r × nj/2^r samples
    M->>G: from(gds, r) — first point kept, dx·2^r, last point recomputed
    M-->>S: values, mask, derived geometry
    S-->>H: DisplayField
    Note over H,S: render / warp only — probe, csv, contours, stats take a Field
```

## Presentation is data, not a seam

`Palette` (#485, the painter's LUT + scale rule, extracted as an API type) is
deliberately not a trait: there is one painter, and a GPU host consumes the
painter's table rather than implementing a second colour path. See
[`03-composition.md`](03-composition.md).

## Verification (milestone 7)

Not a runtime seam, but a boundary worth drawing: which functions carry a
Verus proof. The proofs live in `fieldglass-verify`, outside the workspace,
and restate the kernel functions with pre/post-conditions.

```mermaid
flowchart LR
    subgraph kernel["decode kernel (~600 LOC)"]
        t1a["grib1::unpack_simple_values #199"]
        t1b["apply_spd_inverse / decode_complex_spatial_diff #200"]
        t1c["grib2::decode_complex_groups #201"]
        t2a["decode_inline_bitmap / parse_bitmap #202"]
        t2b["hdf5 unshuffle #203"]
        t3["classic read_slab / record_size #204"]
    end
    verify["fieldglass-verify #205"] -. proves .-> kernel
    classDef planned stroke-dasharray: 6 4
    class t1a,t1b,t1c,t2a,t2b,t3,verify planned
```
