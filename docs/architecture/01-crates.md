# Architecture — Level 1: crate dependency graph

The workspace is five crates. `fieldglass-core` defines the shared traits and
geometry; each format crate decodes one container and depends only on `core`;
`fieldglass-napi` is the Node/N-API boundary and is the only crate that depends
on the format crates.

```mermaid
flowchart TD
    napi["fieldglass-napi<br/><i>N-API boundary (Node addon)</i>"]
    grib1["fieldglass-grib1<br/><i>GRIB1 decode</i>"]
    grib2["fieldglass-grib2<br/><i>GRIB2 decode</i>"]
    netcdf["fieldglass-netcdf<br/><i>NetCDF classic + HDF5 probe</i>"]
    core["fieldglass-core<br/><i>traits, projection, warp, overlay, bits</i>"]

    napi --> grib1
    napi --> grib2
    napi --> netcdf
    napi --> core
    grib1 --> core
    grib2 --> core
    netcdf --> core
```

**Why this shape holds (a decode invariant, not a coincidence):** no format
crate depends on another, and nothing below `napi` depends on `napi`. A new
decode path lands inside one format crate and reuses `core`'s projection / warp
/ overlay on the decoded `Vec<Option<f64>>` field + grid geometry — it does not
ripple outward. Reprojection eligibility keys on grid type and spacing only.
