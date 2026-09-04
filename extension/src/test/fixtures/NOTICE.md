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

## `eta_lambert_msg0.grib2`

First GRIB2 message of NOAA Eta-model output (NAM predecessor) on a 93×65
Lambert conformal grid, GDS template **3.30** — the planar-grid fixture behind
the long-format CSV and contour tests (#470). From the
[`pygrib` sample-data corpus](https://github.com/jswhit/pygrib/tree/master/sampledata);
U.S. government work in the public domain, redistributed by pygrib under its
3-Clause BSD license. Canonical copy:
`crates/fieldglass-grib2/tests/fixtures/NOTICE.md`.

## `netcdf_classic_dummy.nc`

Minimal CDF-1 classic NetCDF from the Unidata `netcdf4-python` test corpus
(<https://github.com/Unidata/netcdf4-python/tree/master/test>). Canonical
copy: `crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.

## `ersst_v5_187001_cdf1.nc`

NOAA Extended Reconstructed Sea Surface Temperature (ERSST) v5, January 1870
monthly mean — a real 4-D (`time × lev × lat × lon`) classic NetCDF on a regular
2°×2° lat/lon grid, used to exercise the NetCDF 2-D slice render path end-to-end.
"No constraints on data access or use." Canonical copy:
`crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.

## `spectral_complex_t63.grib1` / `spectral_simple_t63.grib1`

GRIB1 spherical-harmonic (spectral) temperature fields, GDS data representation
type 50, T63 triangular truncation — the two spectral packings. Derived from
eccodes 2.34.1's own sample (`sh_sfc_grib1.tmpl`), ECMWF, Apache 2.0. Used to
verify a grid-less message opens in the editor and reports it can't render
rather than crashing. Canonical copy:
`crates/fieldglass-grib1/tests/fixtures/NOTICE.md`.

## `netcdf4_dimscale.nc`

Synthetic NetCDF-4 / HDF5 file written by `h5py` (libhdf5) carrying the
dimension-scale convention, so the extension tests cover the HDF5 backing as
well as classic. `dataset_meta_from` returns early for HDF5, a different path
from the classic one, and the metadata-parity test (#411) needs both. Built by
`tools/build_netcdf4_dimscale_fixture.py`; canonical copy:
`crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.

## `netcdf4_unsupported_type.nc`

Synthetic NetCDF-4 / HDF5 file written by the Unidata `netCDF4` library that
mixes ordinary `double` / `float` variables with two whose HDF5 datatype is
outside the decoded subset — a compound `station_info` and a variable-length
`visits`. The extension test needs it to confirm the metadata view lists the
variables that did decode and names the two that did not, which is the whole
point of #550: one such variable used to blank the table. Built by
`tools/build_netcdf4_unsupported_type_fixture.py`; canonical copy:
`crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.

## `healpix_n4_ring.grib2`

GRIB2 §3.150 (HEALPix) at Nside 4 — 192 equal-area pixels, no `Ni`/`Nj`. Built
by `tools/build_grib2_healpix_fixtures.py`; canonical copy:
`crates/fieldglass-grib2/tests/fixtures/NOTICE.md`.

Here for the same reason the spectral fixtures are: a grid-less message must
open in the editor without crashing (the #288 class, where a napi `Option` field
JS sees as `undefined` reaches an `undefined.toFixed()`), and — unlike
bi-Fourier — must offer a Render button, because #443 resamples it onto a
lat/lon grid at decode.
