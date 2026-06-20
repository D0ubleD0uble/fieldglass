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
