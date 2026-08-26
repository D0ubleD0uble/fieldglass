# Decision records

Short notes that capture a cross-cutting design choice and the reasoning
behind it — the kind of decision that shapes more than one issue or crate and
is worth not re-litigating from scratch later.

Each record states the context, the decision, and the consequences as they were
understood when it was written. A record is a snapshot, not a living spec: when
a decision is revisited, add a new record rather than rewriting an old one.

| Record | Decision |
| --- | --- |
| [`0001-grib2-compressed-packing-codecs.md`](0001-grib2-compressed-packing-codecs.md) | Which decoder to use for the GRIB2 compressed packings (5.40 JPEG 2000 / 5.41 PNG / 5.42 CCSDS). |
| [`0002-netcdf-slice-selection-and-rendering.md`](0002-netcdf-slice-selection-and-rendering.md) | How NetCDF N-D variables pick a 2-D slice and reach the screen (CF axis detection, slice picker, synthesised `latlon` geometry reusing the GRIB warp). |
| [`0003-netcdf4-dimension-scale-resolution.md`](0003-netcdf4-dimension-scale-resolution.md) | How NetCDF-4 / HDF5 dimension scales (`DIMENSION_LIST`, `CLASS`, `NAME`, `_Netcdf4Dimid`) resolve to named dimensions + coordinate variables so the HDF5 backing renders. |
| [`0004-netcdf-projected-grid-geolocation.md`](0004-netcdf-projected-grid-geolocation.md) | How projected NetCDF grids (WRF Lambert, GOES geostationary) geolocate by reconstructing the projection and reusing the warp; adds a shared geostationary projector (also GRIB2 §3.90). |
| [`0005-byte-access-and-the-remote-seam.md`](0005-byte-access-and-the-remote-seam.md) | How Fieldglass reaches bytes it does not hold: prefetch the ranges an operation needs and decode synchronously, behind a `ByteSource` trait that `Vec<u8>` implements; plus the constraint decoders must keep and the GRIB `.idx` sidecar. |
| [`0006-hosts-are-bindings-over-a-plain-data-api.md`](0006-hosts-are-bindings-over-a-plain-data-api.md) | How a host (napi, wasm, and later PyO3, CLI, C) is built: a `fieldglass` umbrella crate exposes a `Session` over plain, serde-derivable data types with a conformance suite, so each host is buffer handoff plus error mapping and nothing of the engine. |
| [`0007-netcdf-unit-notation.md`](0007-netcdf-unit-notation.md) | Whether file-authored NetCDF `units` are typeset like the spec-table units of GRIB: yes, because CF requires them to be UDUNITS-parsable — but notation only (`m/s` → `m s⁻¹`, `um` → `µm`), never renaming (`meters` stays `meters`), with time encodings refused by rule and the whole corpus pinned in a snapshot. |
