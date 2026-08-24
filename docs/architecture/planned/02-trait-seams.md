# Planned — Level 2: trait seams

After milestones 7, 10, and 11. Compare with [`../02-trait-seams.md`](../02-trait-seams.md).
Only the seams that change are drawn; `FormatReader` / `DataMessage`,
`Grib1Packing`, and the `TargetProjection` / `PreparedTarget` / `ForwardMap`
family are unchanged and stay in the guarded diagram.

## Grid geometry becomes a type, and inverse lookup becomes a seam

Today the nine per-family warp setups live in `napi` and take a flat 65-field
`MessageMeta`. After #464 `core` owns `GridGeometry`, one variant per family,
built straight from the typed GDS, and the inverse map is a seam with two kinds
of implementer: a formula (`PlanarGridProjector`, today) and a lookup
(`SpatialIndex`, #437) for grids that are only a list of cell centres.

```mermaid
classDiagram
    class GridGeometry {
        <<planned #464>>
        +inverse(lat, lon) Option~GridIndex~
        +forward(i, j) Option~(lat, lon)~
        +lonlat_bbox()
        +reprojectable() bool
        +proj4() Option~String~
    }
    class InverseMap {
        <<trait, planned #437>>
        +inverse_at(lat, lon) Option~GridIndex~
    }
    class PlanarGridProjector {
        <<trait>>
    }
    class SpatialIndex {
        <<planned #437>>
        cell centres, k-d tree or HEALPix buckets
    }

    GridGeometry --> InverseMap : dispatches to
    InverseMap <|.. LambertProjector
    InverseMap <|.. PolarStereoProjector
    InverseMap <|.. TransverseMercatorProjector
    InverseMap <|.. LambertAzimuthalProjector
    InverseMap <|.. GaussianProjector
    InverseMap <|.. RotatedLatLonProjector
    InverseMap <|.. GeostationaryProjector
    InverseMap <|.. SpatialIndex
    PlanarGridProjector --|> InverseMap : refines
```

The consumers of the lookup seam, in the order they are filed:

| Consumer | Issue | Where the cell centres come from |
| --- | --- | --- |
| NetCDF 2-D coordinate (curvilinear) grids | #445 | CF `coordinates` → two 2-D lat/lon variables |
| HEALPix §3.150 | #442, #443 | `pix2ang` in core; synthesised onto lat/lon at decode, like spectral |
| GRIB2 §3.204 NCEP curvilinear | #418 | lat/lon carried as two extra fields |
| ICON §3.101 unstructured | #420, #419 | out-of-band grid file, ADR pending |

Whether `InverseMap` is a new trait or `PlanarGridProjector` widened is #437's
call; the diagram assumes a new supertrait so the planar projectors keep their
forward maps, which `SpatialIndex` cannot offer.

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
cloud-native convention is one dialect. The GRIB dialects ship first; the Zarr
dialects land with the codec crate (#246) so both are tested against the same
fixtures.

```mermaid
classDiagram
    class Manifest {
        <<trait, planned #461>>
        +items() Vec~PlanItem~
        +select(query) Vec~PlanItem~
    }
    class PlanItem {
        <<planned #461>>
        +String key
        +Range range
        +Option~u32~ sub_index
    }
    Manifest <|.. Wgrib2Idx
    Manifest <|.. EcmwfIndex
    Manifest <|.. ZarrV3
    Manifest <|.. ZarrV2
    Manifest <|.. KerchunkRefs
    Manifest ..> PlanItem : produces
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
    participant M as Grib2Message
    participant J as rust_j2k
    participant G as GridGeometry
    H->>M: decode_with(DecodeOptions { reduce: r })
    M->>J: decode_with(bytes, resolution_reduction = r)
    J-->>M: ni/2^r × nj/2^r samples
    M->>G: derive(gds, r) — first point kept, dx·2^r, last point recomputed
    M-->>H: (values, mask, derived geometry)
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
