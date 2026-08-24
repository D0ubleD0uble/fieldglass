# Planned — Level 3: composition

After milestones 10 and 11. Compare with [`../03-composition.md`](../03-composition.md).
GRIB1 and the NetCDF reader are unchanged except where drawn.

## GRIB2 grid templates gain HEALPix

`GridTemplate` gets one variant (#442). Everything else in the GRIB2 message
is as today.

```mermaid
classDiagram
    direction LR
    class HealpixTemplate {
        <<planned #442>>
        +u32 nside
        +Ordering ring_or_nested
    }
    GridTemplate --> HealpixTemplate
```

## Core owns the geometry; `MessageMeta` becomes a view

The centre of #464. Every format's typed grid description converts into one
`GridGeometry`; the hosts' DTOs are built *from* it and nothing in Rust reads
them back.

```mermaid
classDiagram
    direction LR
    class GridGeometry {
        <<enum, planned #464>>
    }
    GridGeometry --> LatLonParams
    GridGeometry --> GaussianParams
    GridGeometry --> MercatorParams
    GridGeometry --> RotatedLatLonParams
    GridGeometry --> LambertParams
    GridGeometry --> PolarStereoParams
    GridGeometry --> TransverseMercatorParams
    GridGeometry --> LambertAzimuthalParams
    GridGeometry --> GeostationaryParams
    GridGeometry --> Lookup : #437
    GridGeometry --> Spectral : truncation
    GridGeometry --> Unsupported : label

    class Lookup {
        <<planned #437>>
        +Arc~SpatialIndex~ index
        +(ni, nj) raster shape rule
    }

    GridTemplate ..> GridGeometry : From (grib2)
    GridDescription ..> GridGeometry : From (grib1)
    RenderableVariable ..> GridGeometry : From (netcdf, CF + WRF + 2-D coords)
    GridGeometry ..> MessageMeta : view (napi)
    GridGeometry ..> Georef : view (wasm)
```

The synthesised grids (spectral today, HEALPix after #443) keep their pattern:
the decode seam resamples onto a regular lat/lon grid and the field carries a
`GridGeometry::LatLon` for that grid, so probe, contours, CSV, and overlays
need no special case.

## NetCDF: curvilinear grids

#445 adds the third geolocation model alongside ADR-0004's two. The reader
reads the two auxiliary coordinate variables and hands them to the spatial
index; the render pipeline still sees `Vec<Option<f64>>` plus a geometry.

```mermaid
classDiagram
    class RenderableVariable
    class CurvilinearCoords {
        <<planned #445>>
        +Vec~f64~ lat2d
        +Vec~f64~ lon2d
    }
    RenderableVariable o-- CurvilinearCoords : CF coordinates =
    CurvilinearCoords ..> SpatialIndex : builds
```

## The two host boundaries after #464

Both hosts bind one `Session` in the `fieldglass` umbrella crate (ADR-0006);
their DTOs are derived from its serde types, and only the buffer handoff is
hand-written. napi keeps its caches (the extension wiggles a picker and
expects a free repaint). wasm keeps none: the host owns every field it
decoded and passes it back for render, probe, and contours, so memory is the
app's decision.

```mermaid
classDiagram
    class Grib2Handle {
        napi
        -Mutex~HashMap~ decoded
        -Mutex~HashMap~ synthesized
    }
    class WasmHandle {
        <<planned #460>>
        no cache
        +count() u32
        +message(i) JSON
        +decode(i, opts) Field
        +warp(field, opts) Field
        +render(field, opts) RGBA
        +probe(field, lat, lon)
        +contours(field, levels)
    }
    class Field {
        <<planned #460>>
        +Float32Array values
        +Uint8Array mask
        +u32 ni, nj
        +Georef grid
        +Stats stats
    }
    class Georef {
        <<planned #460>>
        +String kind
        +bounds_lonlat
        +Option~String~ proj4
        +x0, y0, dx, dy
        +bool periodic_x
        +scan flags
    }
    class Session {
        <<planned #464, crate fieldglass>>
        +open(bytes)
        +message(i)
        +decode(i, opts) Field
        +warp / render / probe / contours / overlay / csv
    }
    class Palette {
        <<planned, core>>
        +[u8; 256*4] lut
        +f64 t0, t1 (transformed domain)
        +ScaleMode scale
        +[u8; 4] masked_rgba
        +normalise(v) f32
        +paint(values, mask, w, h) RGBA
    }
    Grib2Handle *-- Session
    WasmHandle *-- Session
    Session *-- Grib2Reader
    Session ..> Palette : palette(opts)
    Palette ..> RenderedGrid : CPU painter (oracle)
    WasmHandle ..> Palette : to GPU as LUT texture + uniforms
    Grib2Handle ..> MessageMeta : view of GridGeometry
    Grib2Handle ..> RenderedGrid
    WasmHandle ..> Field : host owns
    Field *-- Georef
    WasmHandle ..> Field : takes back by reference
```

`RenderedGrid` / `TargetRaster` gain caller-controlled `width` × `height` for
the box targets (#465); the default stays the source `ni × nj`.

Colour exists once. `Palette` is what the CPU painter already builds
internally (a 256-entry LUT plus the scale rule), extracted as an API type.
`render()` paints through it, and a GPU host uploads the same table and
applies the same two-line normalisation in a shader snippet the package
ships, so the CPU output is the oracle for the GPU output.
