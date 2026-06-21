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

## HDF5 deep-parse fixtures (`hdf5_v1_symboltable.h5`, `hdf5_v2_linkinfo.h5`)

Synthetic HDF5 files built with `h5py` (wraps libhdf5) as targets for the
NetCDF-4 / HDF5 deep-parse chain — object-header walker (#37), group/link
traversal (#38), dataspace + datatype decoders (#39), attribute decoder (#40),
and dataset value decode (#121), under the #33 umbrella. Built and
oracle-dumped by `tools/build_hdf5_fixtures.py` (run from the repo root; needs
`h5py`). `track_times=False` keeps object headers timestamp-free for
reproducibility.

The two files deliberately exercise the **two on-disk group layouts** #38 must
handle:

- `hdf5_v1_symboltable.h5` (`libver='earliest'`): superblock v0, **v1** object
  headers, **symbol-table** groups (local heap + B-tree v1 → `SNOD` nodes), no
  `OHDR` signature. The legacy layout. Also carries a chunked + gzip + shuffle
  dataset (`compressed`) whose chunk index is a **version-1 B-tree** (Data Layout
  v3) — the storage path #121 value decode reads end to end (B-tree chunk walk +
  filter-pipeline reverse). The v2 fixture's `chunked` dataset uses the newer
  version-4 chunk index instead, so the two cover both index styles.
- `hdf5_v2_linkinfo.h5` (`libver='v110'`): superblock v3, **v2** object headers
  (`OHDR`), **link-info** groups, a chunked + gzip + shuffle dataset (#121
  filter pipeline), and a 12-attribute dataset that forces **dense** attribute
  storage (fractal heap `FRHP` + B-tree v2, #40).

Both carry the same matrix: the datatype set (#39: signed int little- and
big-endian, `float32`, `float64`, fixed-length string), the dataspace set
(scalar, simple 1-D / 2-D, and an unlimited `H5S_UNLIMITED` max dim — stored
chunked, as HDF5 requires), global + per-dataset attributes (#40, numeric and
string), contiguous storage, and an unwritten dataset with an explicit fill
value (#121).

Each fixture has a sibling `*.h5.oracle.json` (the decode/parse target): the
superblock version, object-header style, raw layout markers (`OHDR` / `SNOD` /
`FRHP`), global attributes, the root-group child list (== `h5dump -n`), and per
dataset the datatype, dataspace (dims + max dims), storage layout + filters,
fill value, attributes, and value statistics + samples. These are what the
chain must reproduce; deep parsing isn't implemented yet, so they're staged
references, with `tests/hdf5_deep_parse_targets.rs` pinning the layout facts
verifiable today (superblock + `OHDR`/`SNOD`/`FRHP` markers).

> The bundled real NetCDF-4 file `netcdf4_hdf5_dummy.nc` remains the "library
> wrote it" example; these two add controlled coverage of both group layouts
> and the datatype / storage / attribute matrix.

## NetCDF-4 dimension-scale fixture (`netcdf4_dimscale.nc`)

A small NetCDF-4 file written with the canonical Unidata `netCDF4` library (which
wraps `libnetcdf` / libhdf5) as the target for dimension-scale resolution
(#174, under #33; decision 0003). Unlike the two `hdf5_*` fixtures — which are
pure `h5py` and carry **no** dimension scales — this lays down the real
`CLASS = "DIMENSION_SCALE"` / `DIMENSION_LIST` / `_Netcdf4Dimid` machinery, so it
exercises the semantic layer that maps HDF5 dimension scales to named netCDF
dimensions and resolves each variable's ordered dimension list. Built by
`tools/build_netcdf4_dimscale_fixture.py` (run from the repo root; needs
`netCDF4`).

It covers every classification the resolver makes: an **unlimited** dimension
with a coordinate variable (`time`), regular coordinate variables (`lat` /
`lon`), a **pure dimension** with no coordinate variable (`nv` — the
`"This is a netCDF dimension but not a netCDF variable."` placeholder), a
multi-dimensional **data variable** whose `DIMENSION_LIST` must resolve to
ordered names (`temperature(time, lat, lon)`), and a variable that references the
pure dimension (`lat_bnds(lat, nv)`).

The sibling `netcdf4_dimscale.nc.oracle.json` is `ncdump -h` in JSON form: per
dimension its length and unlimited flag; per variable its netCDF type, ordered
dimension names, and whether it is a coordinate variable. `nc_type` is the
canonical netCDF type name (matching the Rust reader's `NcType::name()`), not the
numpy alias. `tests/hdf5_dimension_scales.rs` pins the resolver against it.

## Projected-grid fixtures (`wrf_lambert.nc`, `goes_geostationary.nc`)

Targets for projected-grid geolocation (#168; decision 0004) — regular grids in a
projected CRS, rendered through the analytic-inverse warp (Model A). Both are
self-generated by `tools/build_netcdf_projected_fixtures.py` (run from the repo
root; needs `netCDF4` + `numpy`), so there is **no upstream provenance or
licensing constraint**. They are deliberately tiny toy grids; the official NOAA
GOES and a real `wrfout` subset belong to the bundled corpus (#123).

The coordinate geometry is generated with *independent* NumPy implementations of
the standard projection formulas — Snyder Lambert Conformal Conic and the GOES-R
PUG fixed-grid algorithm — so the Rust projectors reproducing it is a genuine
cross-language check rather than a tautology.

- `wrf_lambert.nc` (classic NetCDF-3) is a WRF `wrfout`-style file: the Lambert
  projection lives in **global attributes** (`MAP_PROJ = 1`, `TRUELAT1` /
  `TRUELAT2`, `STAND_LON`, `MOAD_CEN_LAT`, `DX` / `DY`), and the 2-D `XLAT` /
  `XLONG` arrays are precomputed conveniences whose `(0, 0)` corner fixes the grid
  origin. The fixture adopts the projector's spherical Earth radius (6 371 229 m);
  real `wrfout` uses 6 370 000 m — the same ~0.02 % approximation the GRIB Lambert
  path already makes.
- `goes_geostationary.nc` (NetCDF-4 / HDF5) is a GOES ABI-style file: a CF
  `grid_mapping` variable `goes_imager_projection`
  (`grid_mapping_name = "geostationary"`, GRS80 ellipsoid, `sweep_angle_axis =
  "x"`, GOES-East sub-satellite longitude) and 1-D `x` / `y` *radian* scan-angle
  coordinate variables stored as **scaled `int16`** (the real GOES encoding),
  exercising CF `scale_factor` / `add_offset`.

Each has a sibling `*.oracle.json` with the resolved projection parameters and
sampled `(i, j) ↔ (lat, lon)` geolocation. `tests/projected_grids.rs` resolves
the projection from the on-disk metadata and asserts the `fieldglass_core`
projector reproduces the oracle.

## CF packed-data fixture (`cf_packed_data.nc`)

Target for CF **data-variable** unpacking (#184): `scale_factor` / `add_offset`
+ `valid_range` applied to the rendered field, the way GOES `Rad`, MERRA-2, and
ERA5 store it as scaled `int16`. The companion projected fixtures above already
exercise the CF convention on *coordinate* arrays; this packs the data plane
itself.

Self-generated by `tools/build_netcdf_cf_packed_fixture.py` (run from the repo
root; needs `netCDF4` + `numpy`), so there is **no upstream provenance or
licensing constraint** — a tiny `3 × 4` toy grid. `temp(lat, lon)` is a scaled
`int16` carrying `scale_factor = 0.0625` (a power of two, hence exact in both
float32 and float64), `add_offset = 250`, `_FillValue = -9999`, and
`valid_range = [0, 10000]`, plus 1-D `lat`/`lon` coordinates.

The sibling `cf_packed_data.nc.oracle.json` records both the raw on-disk codes
(only `_FillValue` masked) and the physical values `netCDF4` produces with auto
mask+scale on — the CF unpacking the Rust decode + `unpack_cf_data` must
reproduce. `tests/cf_packed_data.rs` asserts both stages against it.

## CF `missing_value` fixtures (`missing_value_classic.nc`, `missing_value_nc4.nc`)

Target for `missing_value` masking: libnetcdf masks a point equal to either
`_FillValue` **or** the CF `missing_value` attribute. `temp(y, x)` is an `int16`
marking gaps with a distinct `_FillValue` and a scalar `missing_value`, points
hitting each. The same logical field is bundled in both on-disk encodings —
classic NetCDF-3 (`…_classic.nc`) and NetCDF-4 / HDF5 (`…_nc4.nc`) — so each
decode backing is exercised end-to-end.

Self-generated by `tools/build_netcdf_missing_value_fixture.py` (run from the
repo root; needs `netCDF4` + `numpy`), so there is **no upstream provenance or
licensing constraint** — a tiny `2 × 3` toy grid. The shared
`missing_value.oracle.json` records the masked array `netCDF4` produces with
auto-mask on; `tests/missing_value.rs` asserts both backings reproduce it.
