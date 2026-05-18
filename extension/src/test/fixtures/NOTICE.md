# Extension-test fixture provenance

The fixtures in this directory are duplicates of upstream `crates/*/tests/fixtures/`
binaries so the VS Code integration tests in `extension/src/test/suite/`
have a self-contained corpus that doesn't reach across crate boundaries
at runtime. The canonical NOTICE for each file lives next to its source
of truth in the crate; see those for full provenance and licensing.

## `cmc_wind_300_2010052400_p012.grib`

Single-message GRIB1 file from the Canadian Meteorological Centre regional
model (wind speed at 300 hPa, polar-stereographic 60 km grid). Originally
from the [pygrib sample data set](https://github.com/jswhit/pygrib/tree/master/sampledata),
MIT-licensed, J. Whitaker. Canonical copy: `crates/fieldglass-grib1/tests/fixtures/NOTICE.md`.

## `regular_latlon_surface.grib2`

Single-message GRIB2 from the public ECMWF eccodes test data corpus —
2-metre temperature on a 16×31 regular lat/lon grid, GDS template **3.0**,
PDS template **4.0**, DRS template **5.0 (simple packing)**. Apache 2.0
(eccodes redistribution). Canonical copy:
`crates/fieldglass-grib2/tests/fixtures/NOTICE.md`.

## `netcdf_classic_dummy.nc`

Minimal CDF-1 classic NetCDF from the Unidata `netcdf4-python` test corpus
(<https://github.com/Unidata/netcdf4-python/tree/master/test>). Canonical
copy: `crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.
