# Planned — Level 2: trait seams

After milestones 7, 10, and 11. Compare with [`../02-trait-seams.md`](../02-trait-seams.md).
Only the seams that change are drawn; `FormatReader` / `DataMessage`,
`Grib1Packing`, and the `TargetProjection` / `PreparedTarget` / `ForwardMap`
family are unchanged and stay in the guarded diagram.

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
        <<planned #460 then #464>>
        +inverse_at() Inverse closure for SourceGrid
        +forward(i, j) Option~(lat, lon)~
        +lonlat_bbox()
        +reprojectable() bool
        +resampling() Any | NearestOnly
        +proj4() Option~String~
    }
    class PlanarGridProjector {
        <<trait>>
        +forward_xy(lat, lon) required
        +inverse(lat, lon) provided, once for all planar grids
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

Two things this fixes on the way. The four planar `inverse()` bodies are
today the same ~25 lines each (guards, forward, `(x − origin) / spacing`,
edge snap); the forward direction is already a provided method on
`PlanarGridProjector`, and the inverse joins it, so the edge-snap rule exists
once. And `GridIndex` is fractional because the raster grids are: a lookup
grid returns the nearest cell centre, where the fractional part and the
"next column" neighbour mean nothing, so `GridGeometry::Lookup` reports
`NearestOnly` and `warp` refuses or degrades bilinear against it rather than
blending across a tripolar fold.

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
implements. The GRIB dialects ship first; the Zarr
dialects land with the codec crate (#246) so both are tested against the same
fixtures.

```mermaid
classDiagram
    class Manifest {
        <<trait, planned #461>>
        +items() Vec~PlanItem~
        +select(query, &dyn ParameterResolver) Vec~PlanItem~
    }
    class ParameterResolver {
        <<trait, planned #461>>
        +resolve(abbrev, level_str) Option~ParameterId~
    }
    class UmbrellaResolver {
        <<planned #464, crate fieldglass>>
        grib2 tables (#426) behind the trait
    }
    ParameterResolver <|.. UmbrellaResolver
    class PlanItem {
        <<planned #461>>
        +String key
        +Range range
        +Option~u32~ sub_index
        +Expect expect (discipline, parameter, level from the sidecar line)
    }
    class Expect {
        <<planned #461>>
        the plan is a claim: the decoder checks magic, §0 length, and these
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
