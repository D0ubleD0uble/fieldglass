# Test fixture provenance

## `netcdf_classic_dummy.nc`, `netcdf4_hdf5_dummy.nc`

Sourced from the Unidata `netcdf4-python` test corpus
(<https://github.com/Unidata/netcdf4-python/tree/master/test>) — `netcdf_dummy_file.nc`
and `issue1152.nc`, respectively. Used to exercise minimal CDF-1 classic and
NetCDF-4 / HDF5 backings.

## `ersst_v5_187001_cdf1.nc`

NOAA Extended Reconstructed Sea Surface Temperature (ERSST) v5, January 1870
monthly mean — a real published climate-science product. Sourced verbatim from
NOAA NCEI:

- URL: <https://www.ncei.noaa.gov/pub/data/cmb/ersst/v5/netcdf/ersst.v5.187001.nc>
- DOI: <https://doi.org/10.7289/V5T72FNM>
- Reference: Huang, B., et al. (2017), *Extended Reconstructed Sea Surface
  Temperature, Version 5 (ERSSTv5)*, J. Climate, 30, 8179–8205.
- License (per file metadata): "No constraints on data access or use."

The file is the un-modified upstream byte stream (`CDF\x01` magic, classic
CDF-1, 4 dimensions, 6 variables including `sst` and `ssta` at 2°×2°
resolution, 38 CF-1.6 / ACDD-1.3 global attributes).

## `ersst_v5_187001_cdf2.nc`, `ersst_v5_187001_cdf5.nc`

Re-encoded copies of `ersst_v5_187001_cdf1.nc` produced by the canonical
Unidata `netCDF4` Python library (which wraps `libnetcdf`'s `nccopy -k`):

```text
NETCDF3_64BIT_OFFSET   →  ersst_v5_187001_cdf2.nc   (CDF-2: 64-bit var begins)
NETCDF3_64BIT_DATA     →  ersst_v5_187001_cdf5.nc   (CDF-5: 64-bit nelems / dim lengths / vsize)
```

The values, dimensions, attributes, and variable structure are identical to
the upstream NOAA file — only the on-disk encoding differs. This lets the
header parser exercise the rare CDF-2 / CDF-5 width paths against real
model-derived content rather than hand-crafted bytes. Reproduced via the
`build_fixtures.py` script in this directory.

> CDF-5's *extended numeric types* (`UByte`, `UShort`, `UInt`, `Int64`,
> `UInt64`) are not exercised by these fixtures because the source CDF-1 file
> contains none — those types are covered by the unit tests in
> `crates/fieldglass-netcdf/src/classic.rs`.

## Value-decode oracles (`*.values.json`)

`netcdf_classic_dummy.nc.values.json` and `ersst_v5_187001_cdf1.nc.values.json`
are the value-decode targets for classic NetCDF value decode (#108). Each
records, per variable, what the canonical Unidata `netCDF4` library (which
wraps `libnetcdf`) decodes from the on-disk bytes: `nc_type`, shape,
dimensions, fill value, present/missing counts, value statistics, and a few
anchored samples in C (row-major / on-disk) order. Samples are the *raw*
on-disk values (fills included) so a decoder can match the exact sequence,
including masked positions. Once #108 reads each variable from its `begin`
offset, the decoded array must reproduce these numbers.

The two fixtures together cover the decode matrix: every `nc_type`
(`char` / `int` / `float` / `double`), every layout (scalar, fixed 1-D,
multi-dimensional, and unlimited-dimension *record* variables — empty here
since `numrecs = 0`), default fills (`crs` = `NC_FILL_INT`), explicit
`_FillValue`s (`z` = -9999.9), and real masked climate data (ERSST `sst`:
5032 of 16020 points are the -999 fill). The ERSST CDF-2 / CDF-5 fixtures
decode to byte-identical values, so the single CDF-1 oracle covers all three.

Regenerate with `python3 tools/regenerate-netcdf-oracles.py` from the repo
root (needs `netCDF4`); the committed JSON means the Rust suite needs no
netCDF4 at runtime. `tests/classic_value_targets.rs` pins the type/shape
matrix the decode builds on; the value numbers are checked once #108 lands.
