# Architecture — Level 2: trait seams

A reader can't know at compile time which packing or projection a file uses; the
file's own type codes decide. Each trait below is the dispatch point for one of
those choices. The reader reads a code, selects the matching implementer, and
calls through the trait, so supporting a new packing, projection, or output
raster takes one new implementer and nothing else.

In each diagram the implementers point at the trait they satisfy.

## Reading and decoding

`FormatReader` walks raw bytes into a sequence of messages; `DataMessage`
unpacks one message's values when asked. The scanner and the napi layer drive
both as trait objects, iterating and decoding without naming a concrete format.

```mermaid
classDiagram
    class FormatReader {
        <<trait>>
    }
    class DataMessage {
        <<trait>>
    }
    FormatReader <|.. Grib2Reader
    DataMessage  <|.. Grib2Message
```

## GRIB1 packing

The BDS flag byte names the packing. The reader matches it to one `Grib1Packing`
implementer, which unpacks the bit-stream into the common field of values. Each
implementer is one packing the decoder understands (GRIB2's equivalent set is
the README "packing modes" table).

```mermaid
classDiagram
    class Grib1Packing {
        <<trait>>
    }
    Grib1Packing <|.. SimplePacking
    Grib1Packing <|.. ComplexPacking
    Grib1Packing <|.. IeeePacking
    Grib1Packing <|.. MatrixPacking
    Grib1Packing <|.. SphericalPacking
```

## Projection and warp

`warp` reprojects a decoded field onto an output raster. Each `TargetProjection`
prepares a `PreparedTarget`, and a `PreparedTarget` is a `ForwardMap`: it turns
an output pixel back into a source lat/lon to sample. `PlanarGridProjector` runs
the inverse for native grids, mapping a lat/lon into a row and column. Overlays
reuse the `ForwardMap` seam through `SourceOverlayTarget`.

The implementers and the traits they satisfy:

```mermaid
classDiagram
    class TargetProjection {
        <<trait>>
    }
    class PreparedTarget {
        <<trait>>
    }
    class ForwardMap {
        <<trait>>
    }
    class PlanarGridProjector {
        <<trait>>
    }

    TargetProjection <|.. WebMercator
    TargetProjection <|.. Orthographic
    TargetProjection <|.. PolarStereographic
    TargetProjection <|.. Mollweide
    TargetProjection <|.. Robinson
    TargetProjection <|.. EqualEarth
    TargetProjection <|.. TargetRaster

    PreparedTarget <|.. WebMercatorPrepared
    PreparedTarget <|.. OrthographicPrepared
    PreparedTarget <|.. PolarStereographicPrepared
    PreparedTarget <|.. MollweidePrepared
    PreparedTarget <|.. RobinsonPrepared
    PreparedTarget <|.. EqualEarthPrepared
    PreparedTarget <|.. EquirectPrepared

    ForwardMap <|.. WebMercatorPrepared
    ForwardMap <|.. OrthographicPrepared
    ForwardMap <|.. PolarStereographicPrepared
    ForwardMap <|.. MollweidePrepared
    ForwardMap <|.. RobinsonPrepared
    ForwardMap <|.. EqualEarthPrepared
    ForwardMap <|.. EquirectPrepared
    ForwardMap <|.. SourceOverlayTarget

    PlanarGridProjector <|.. LambertProjector
    PlanarGridProjector <|.. PolarStereoProjector

    PreparedTarget --|> ForwardMap : requires (supertrait)
```

The call order, where `prepare()` runs once per raster and `forward()` once per
output pixel:

```mermaid
sequenceDiagram
    participant W as warp
    participant P as TargetProjection
    participant T as PreparedTarget<br/>(: ForwardMap)
    participant F as decoded field
    W->>P: prepare(grid)
    P-->>W: PreparedTarget
    loop each output pixel
        W->>T: forward(x, y)
        T-->>W: source lat/lon
        W->>F: sample(lat/lon)
        F-->>W: value
    end
```

> Authoritative source for the realizations above:
> `grep -rE 'impl( <[^>]+>)? [A-Za-z0-9_]+ for [A-Za-z0-9_]+' crates/*/src`.
> If that set changes, this file is stale; see `README.md` in this directory
> for the drift check.
