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

## Multi-level B-tree v2 fixture (`hdf5_btreev2_multilevel.h5`)

A synthetic `h5py` (libhdf5) file whose `many_attrs` dataset carries 700 dense
attributes — enough that the attribute name-index **version-2 B-tree grows to
depth 2** (an internal-node tree, not a single leaf). It targets the multi-level
B-tree walk: the doubling-table heap support added for the GOES-16 file (#187)
handles a file's *storage*, but a metadata-heavy file's *index* spills into
internal B-tree nodes first, which the reader previously refused. A real
operational file (e.g. ERA5 / MERRA-2 / CMIP6, #123) hits this before it needs
child indirect heap blocks. Built and oracle-dumped by
`tools/build_hdf5_fixtures.py` (`track_times=False` for reproducibility).

Each attribute is `a{i:04d} -> int32 i`, so the sibling `*.h5.oracle.json` records
the rule, the attribute count, the measured B-tree depth, and a few sampled
values rather than dumping 700 entries; `tests/hdf5_attributes.rs` reads every
attribute back and checks it against the generated expectation. Part of #33.

## Child-indirect fractal-heap fixture (`hdf5_child_indirect.h5`)

A synthetic `h5py` (libhdf5) file whose `many_attrs` dataset carries 512 dense
attributes of `int32[256]` (≈1 KiB each). That much dense storage fills every
direct-block row of the attribute fractal heap's doubling table and spills into a
**child indirect block** — the rows beyond `max_direct_block_size`, which the
reader previously refused with a clean `child indirect fractal-heap blocks not
supported` error. It is the real-libhdf5 backstop for the step after the
multi-level B-tree fixture: the metadata-heaviest corpus files (#123) reach this
once their *storage*, not just their *index*, outgrows one indirect block's
direct rows.

libhdf5 fills the full grid of direct blocks before it allocates a child indirect
block (the exact heap geometry — starting / max-direct block size, table width —
is libhdf5-version dependent and recorded in the oracle), so this fixture is
necessarily larger (~575 KiB) than the others; the hand-built
`crates/fieldglass-netcdf/src/hdf5/heap.rs` unit tests pin the exact byte layout
at no cost. The sibling `*.h5.oracle.json` records the attribute rule / count,
sampled names, and the parsed fractal-heap geometry (`cur_rows`,
`max_dblock_rows`, and the indirect / direct block counts that confirm a child
indirect block is populated). Built by `tools/build_hdf5_fixtures.py`
(`track_times=False` for reproducibility); `tests/hdf5_attributes.rs` reads every
attribute back. Part of #33.

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

## Projected-grid fixtures (`wrf_lambert.nc`, `wrf_polar.nc`, `wrf_mercator.nc`, `goes_geostationary.nc`)

Targets for projected-grid geolocation (#168 and #220; decision 0004) — regular
grids in a projected CRS, rendered through the analytic-inverse warp (Model A).
All are self-generated by `tools/build_netcdf_projected_fixtures.py` (run from
the repo root; needs `netCDF4` + `numpy`), so there is **no upstream provenance
or licensing constraint**. They are deliberately tiny toy grids; the official
NOAA GOES and a real `wrfout` subset belong to the bundled corpus (#123).

The coordinate geometry is generated with *independent* NumPy implementations of
the standard projection formulas — Snyder Lambert Conformal Conic, Snyder polar
stereographic, spherical Mercator, and the GOES-R PUG fixed-grid algorithm — so
the Rust projectors reproducing it is a genuine cross-language check rather than
a tautology.

- `wrf_lambert.nc` (classic NetCDF-3) is a WRF `wrfout`-style file: the Lambert
  projection lives in **global attributes** (`MAP_PROJ = 1`, `TRUELAT1` /
  `TRUELAT2`, `STAND_LON`, `MOAD_CEN_LAT`, `DX` / `DY`), and the 2-D `XLAT` /
  `XLONG` arrays are precomputed conveniences whose `(0, 0)` corner fixes the grid
  origin. The fixture adopts the projector's spherical Earth radius (6 371 229 m);
  real `wrfout` uses 6 370 000 m — the same ~0.02 % approximation the GRIB Lambert
  path already makes.
- `wrf_polar.nc` and `wrf_mercator.nc` (#220) are the same `wrfout` shape with
  `MAP_PROJ = 2` (polar stereographic: `DX`/`DY` true at `TRUELAT1`, oriented
  along `STAND_LON`, hemisphere from `TRUELAT1`'s sign) and `MAP_PROJ = 3`
  (Mercator: uniform projected metres, geolocated from the `XLAT`/`XLONG`
  corner coordinates alone).
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

## Real GOES-16 ABI fixture (`goes16_abi_cmip.nc`)

The first **real operational** NetCDF-4 / HDF5 file in the corpus (#123) — a
small subset of a genuine NOAA GOES-16 ABI L2 Cloud & Moisture Imagery product:

- Product: ABI L2 CMIP, Mesoscale sector 1, band 13 (10.3 µm IR), GOES-East.
- Source object (immutable, in the public NOAA archive):
  `s3://noaa-goes16/ABI-L2-CMIPM/2023/001/18/`
  `OR_ABI-L2-CMIPM1-M6C13_G16_s20230011800281_e20230011800350_c20230011800425.nc`
- License: a work of the U.S. Government — **public domain**, no copyright. NOAA
  requests attribution to "NOAA/NESDIS, GOES-R Series."

Built by `tools/build_goes_real_fixture.py` (run from the repo root; needs
`netCDF4` + `numpy`; downloads the source object once). The script keeps a
`24 × 24` center window of the 500×500 grid plus the `goes_imager_projection`
grid mapping, the scaled-`int16` `x` / `y` scan-angle coordinates, and the
`CMI` / `DQF` fields; the dozens of ancillary scalar metadata variables are
dropped to keep the fixture byte-small. The raw on-disk `int16` / `int8` codes
are copied verbatim (auto-scaling off), so the genuine CF `scale_factor` /
`add_offset` / `valid_range` / `_FillValue` attributes and the real GRS80 /
sub-satellite-longitude projection parameters survive unchanged. `CMI` keeps the
real chunked + deflate storage, so the HDF5 value path decodes a real compressed
field end to end.

Unlike the synthetic `goes_geostationary.nc`, the attributes here are rich enough
that their dense storage spills into a fractal heap with an **indirect root
block** (a doubling table of direct blocks) — the structure real attribute-heavy
NetCDF-4 files use, exercised here for the first time.

The sibling `goes16_abi_cmip.nc.oracle.json` records the resolved projection
parameters, the `(i, j) ↔ (lat, lon)` geolocation (computed by an *independent*
NumPy transcription of the GOES-R PUG fixed-grid algorithm, so the Rust projector
reproducing it is a cross-language check), and the `CMI` brightness temperatures
`netCDF4` decodes (CF-unpacked, in Kelvin). `tests/goes_real_world.rs` asserts
the HDF5 backing, the dimension/variable resolution, the geolocation, and the
chunked-field value decode against it.

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

## Real NOAA OISST v2.1 fixture (`oisst_avhrr_v2.nc`)

The second **real operational** NetCDF-4 / HDF5 file in the corpus (#123) — a
tiny window subset of a genuine NOAA/NCEI Optimum Interpolation Sea Surface
Temperature analysis:

- Product: OISST v2.1, AVHRR, daily 1/4° global, 2025-01-01.
- Source object (immutable, in the public NOAA CDR archive):
  `s3://noaa-cdr-sea-surface-temp-optimum-interpolation-pds/`
  `data/v2.1/avhrr/202501/oisst-avhrr-v02r01.20250101.nc`
- License: a NOAA Climate Data Record produced by NOAA/NCEI — a work of the
  U.S. Government, **public domain**, no copyright. Attribution: NOAA/NCEI.

Built by `tools/build_oisst_real_fixture.py` (run from the repo root; needs
`netCDF4` + `numpy`; downloads the source object once). The script keeps a
`32 × 32` Hudson Bay window (rows 592–624, cols 1112–1144) of the global grid —
a January high-latitude scene chosen so all three real behaviours appear at
once: land and sea ice fill the `sst` mask (~1/3 of the window), while the rest
carries near-freezing water and the `ice` field genuine sea-ice concentrations.
It retains the `sst` and `ice` fields plus the `time` / `zlev` / `lat` / `lon`
coordinate variables; the dozens of ancillary attributes that name the full grid
extent are dropped or noted as a window subset in `history`. The raw on-disk
`int16` codes are copied verbatim (auto-scaling off), so the genuine CF
`scale_factor` / `add_offset` / `valid_min` / `valid_max` / `_FillValue`
attributes survive unchanged, and `sst` / `ice` keep the real chunked + deflate
+ **shuffle** storage, so the HDF5 value path decodes a real compressed field end
to end.

It complements the geostationary `goes16_abi_cmip.nc` with a different slice of
the stack: a **regular 1/4° lat/lon** analysis grid (vs the GOES fixed scan
grid), the deflate + **shuffle** filter chain (GOES used deflate alone), CF
unpacking driven by scalar `valid_min` / `valid_max` (GOES used the two-element
`valid_range`), and a 4-D `(time, zlev, lat, lon)` variable with singleton
`time` / `zlev`. Its 25 retained global attributes still exceed libhdf5's
8-attribute compact threshold, so the metadata spills into **dense** storage
(fractal heap `FRHP` + B-tree v2 `BTHD`) — the layout the #33 robustness work
hardened, exercised here on a real file.

The sibling `oisst_avhrr_v2.nc.oracle.json` records the regular-grid geolocation
(corner + 0.25° spacing) and, per packed field, the masking + scaling `netCDF4`
produces (auto mask+scale on): present / missing counts, value statistics, and
anchored per-index samples. `tests/oisst_real_world.rs` asserts the HDF5
backing, the dimension / variable resolution, the regular-grid coordinates, and
the chunked + deflate + shuffle value decode against it.
