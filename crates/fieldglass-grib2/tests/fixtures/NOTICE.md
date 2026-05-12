# Test fixture provenance

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

## `eta_lambert_msg0.grib2`

First GRIB2 message extracted from `eta.grb` in the
[`pygrib` sample-data corpus](https://github.com/jswhit/pygrib/tree/master/sampledata)
(`https://raw.githubusercontent.com/jswhit/pygrib/master/sampledata/eta.grb`).
NOAA Eta-model output (NAM predecessor), GDS template **3.30** (Lambert
Conformal), 12-km CONUS grid. NOAA Eta is U.S. government work in the public
domain; pygrib redistributes the file under its 3-Clause BSD license. Only
the first message is retained to keep the fixture small (10 KB vs. 920 KB
for the original multi-message file).
