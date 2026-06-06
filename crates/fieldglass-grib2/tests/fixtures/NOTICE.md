# Test fixture provenance

## `rotated_latlon_surface.grib2`

Copied verbatim from the eccodes distribution's encoding samples
(`samples/rotated_ll_sfc_grib2.tmpl`). Single message, GDS template **3.1**
(rotated latitude/longitude) on a 16×31 grid with a rotated southern pole,
centre 98 (ECMWF). A constant 2-m temperature field (DRS 5.0, bitsPerValue 0).
eccodes and its samples are released under the Apache 2.0 license.

## `polar_stereographic_surface.grib2`

Copied verbatim from the eccodes distribution's encoding samples
(`samples/polar_stereographic_sfc_grib2.tmpl`). Single message, GDS template
**3.20** (polar stereographic) on a 16×31 grid, centre 98 (ECMWF). A constant
field (DRS 5.0, bitsPerValue 0). eccodes and its samples are released under
the Apache 2.0 license.

## `reduced_gaussian_pressure_level.grib2`

Sourced from the public ECMWF eccodes test data corpus
(<https://get.ecmwf.int/test-data/eccodes/data/>). Single message, GDS
template **3.40** (Gaussian latitude/longitude — reduced variant), centre 98
(ECMWF), reference time 2008-02-06T12:00:00Z. eccodes is released under the
Apache 2.0 license; the test data is bundled with the eccodes distribution.

## `gfs_c255_latlon.grib2`

Sourced from the public ECMWF eccodes test data corpus as `gfs.c255.grib2`
(<https://get.ecmwf.int/test-data/eccodes/data/gfs.c255.grib2>). Single
message of NCEP GFS output, GDS template **3.0** (regular latitude/longitude),
0.5° global grid (10512 points). NOAA NCEP-produced GRIB2 data is U.S.
government work and in the public domain; the eccodes corpus redistribution
is under Apache 2.0.

## `regular_latlon_surface.grib2`

Sourced verbatim from the public ECMWF eccodes test data corpus
(<https://sites.ecmwf.int/repository/eccodes/test-data/data/regular_latlon_surface.grib2>).
Single message of 2-metre temperature on a coarse 16×31 regular lat/lon
grid, GDS template **3.0**, PDS template **4.0**, **DRS template 5.0
(simple packing)**, R ≈ 270 K. Used by the §5–§7 decode tests as a
small, fully simple-packed end-to-end fixture (gfs_c255 uses complex
packing 5.3, eta_lambert / reduced_gaussian use 5.0 but at larger
grids). eccodes is released under the Apache 2.0 license.

## `eta_lambert_msg0.grib2`

First GRIB2 message extracted from `eta.grb` in the
[`pygrib` sample-data corpus](https://github.com/jswhit/pygrib/tree/master/sampledata)
(`https://raw.githubusercontent.com/jswhit/pygrib/master/sampledata/eta.grb`).
NOAA Eta-model output (NAM predecessor), GDS template **3.30** (Lambert
Conformal), 12-km CONUS grid. NOAA Eta is U.S. government work in the public
domain; pygrib redistributes the file under its 3-Clause BSD license. Only
the first message is retained to keep the fixture small (10 KB vs. 920 KB
for the original multi-message file).

## `ieee32_regular_latlon.grib2` / `ieee64_regular_latlon.grib2` (+ `ieee64_regular_latlon_expected.json`)

`regular_latlon_surface.grib2` re-encoded by eccodes 2.34.1 into the IEEE
floating-point packing (DRS template **5.4**, `grid_ieee`), at both precisions:

```
grib_set -s packingType=grid_ieee,precision=1 regular_latlon_surface.grib2 ieee32_regular_latlon.grib2
grib_set -s packingType=grid_ieee,precision=2 regular_latlon_surface.grib2 ieee64_regular_latlon.grib2
```

Template 5.4 stores each value verbatim as a big-endian IEEE float (precision
1 → 32-bit, 2 → 64-bit) with no reference/binary/decimal-scale transform.
Because the source field was already quantised by simple packing to values
that are f32-exact, the 32-bit and 64-bit fixtures decode to byte-identical
fields — both are kept so the test exercises the f32 and f64 read paths.
`ieee64_regular_latlon_expected.json` is the `grib_get_data` oracle (count,
min/max/mean, anchored samples); decode tolerance is recorded in the file.
eccodes returns `GRIB_NOT_IMPLEMENTED` for precision 3 (128-bit), and so do
we. See eccodes `grib2/template.5.4.def` + `grib_accessor_class_data_raw_packing`.
