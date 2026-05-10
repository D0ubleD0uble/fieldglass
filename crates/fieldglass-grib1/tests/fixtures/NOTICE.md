# Test fixture provenance

## `cmc_wind_300_2010052400_p012.grib`

Single-message GRIB1 file from the Canadian Meteorological Centre regional
model (wind speed at 300 hPa, polar-stereographic 60 km grid, 2010-05-24
00Z + 12 h). Originally distributed with the [pygrib sample data
set](https://github.com/jswhit/pygrib/tree/master/sampledata) (MIT-licensed,
J. Whitaker).

## `ecmwf_lfpw_msg0.grib1`

First message extracted from a 64-message ECMWF GRIB1 file
(`ecmwf_lfpw.grib1`) — geopotential at 50 hPa, 240 × 121 lat-long grid,
2006-12-10 18Z + 24 h, encoded with `grid_second_order` (SPD order 2,
boustrophedonic, general-extended). Used to pin the complex-packing
variant detection and (in a follow-up) as the decode oracle for the
second-order packing implementation.

The file was sourced from another open-source application's test corpus
and is believed to be redistributable. If you are the rights-holder and
this is in error, please [open an issue](https://github.com/D0ubleD0uble/fieldglass/issues)
and we will replace it with a synthesised equivalent.

## `ecmwf_lfpw_msg0_expected.json`

Decoder oracle: counts, min/max/mean, and 12 anchored sample values dumped
from the fixture above by `grib_get_data` (eccodes 2.34.1) on
2026-05-09. Tolerance for value comparison is recorded in the file
itself.
