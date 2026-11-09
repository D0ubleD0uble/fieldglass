# Manual-testing sample corpus

Real, full operational files for eyeballing renders in the extension — the
visual counterpart to the tiny oracle-checked fixtures under
`crates/*/tests/fixtures/`. Those prove decode correctness on a handful of grid
points; these prove a whole real file *looks* right (coastlines land on
coastlines, colorbars read in real units, projections frame correctly).

Everything here except this file is git-ignored (see `.gitignore`): the data is
large and license-varied, so it stays local.

## Fetch

```sh
tools/fetch_samples.sh            # all freely-available models
tools/fetch_samples.sh gfs hrrr   # just the named ones
DATE=20260629 CYCLE=12 tools/fetch_samples.sh   # pin a run
```

It defaults to yesterday's 00Z. MRMS, NBM, ECCC and ICON keep only a day or
two, so if one 404s, re-run that model with today's `DATE`.

The script pulls from public endpoints that need no credentials: NOAA NOMADS
and AWS Open Data, ECMWF open data, the eccodes test data, the ECCC
datamart, DWD open data, and the HDF5 repository on GitHub. Auth-gated sources (MERRA-2, ERA5,
CMIP6) can't be scripted without credentials — drop those files in by hand and
the checklist below still applies.

## Open a file

Build the native module and extension once, then launch the Extension
Development Host:

```sh
# from repo root, once — build the native module (so the dev host runs current
# Rust code) into extension/bin, then compile the TypeScript:
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm run compile )

# then, to open a specific sample straight in the dev host:
code --extensionDevelopmentPath="$PWD/extension" "$PWD/samples/gfs.grib2"
```

A quick headless pre-flight (decode + reproject every sample without opening the
UI) lives at `tools/preflight_samples.js`: `node tools/preflight_samples.js`.

The pre-flight proves a file decodes and renders, not that it lands in the right
place. `tools/probe_parity_sweep.py` reads 200 seeded points per message or
slice through the native module and again through eccodes (GRIB) or xarray
(NetCDF, Zarr), and exits non-zero where the two disagree on the cell, the
value or whether it is masked:

```sh
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples samples/0.6.0 )
```

### Known oracle exceptions

The sweep lists these in its output rather than failing on them.

- **MRMS `-999`.** MRMS marks no coverage with a -999 value that is not a bitmap.
  Both decoders return it as data, so the sweep compares it as a value.
- **eccodes `find_nearest` on a j-consecutive message.** eccodes returns the
  row-major index and reads the value at that index of the column-major data,
  so the cell and value belong to another point (`geo/nearest/Regular.cc` sets
  `k = i + Ni * j` whatever the scanning mode). Reproduce with
  `samples/0.6.0/grib1_names.grib1` (built by RELEASE-TEST-PLAN.md section 0),
  message 0, at 45N 15E: `codes_grib_find_nearest` gives index 27 and value
  25.0; the geo iterator, the `values` array in file order and Fieldglass all
  give 33.0. For those messages the sweep uses eccodes' geo iterator. The sweep
  prints the installed eccodes version; this was seen on the 2.48 wheel.
- **eccodes `values` on an alternate-row message** (NBM). The key keeps storage
  order, odd rows reversed, while the nearest-point lookup puts each value under
  its own latitude and longitude, so the sweep takes values from the lookup.
- **Past the last row of a reduced grid** (`reduced_ll_wave.grib1`). eccodes
  calls any point poleward of the outermost row that holds points off its grid.
  The sweep then takes the row nearest the point's latitude. If that row has
  no points (the wave grid's ice caps), Fieldglass's pixel must be on it, and
  it counts as masked on both sides. If it is the outermost row with points,
  the sweep asks eccodes again on that row's latitude and measures the
  distances from the point.

On a reduced grid Fieldglass's cell is a column of the raster it widens the rows
into. The sweep maps that column back to the file's point through the row
lengths (`pl`) before it asks eccodes, so reduced grids get the same cell, value
and mask checks as the others. Coarse reduced grids (N32 and O32, including the
committed fixtures `reduced_gg_n32_smooth.grib1` and
`octahedral_gaussian_o32.grib2`) fail those checks today, because Fieldglass's
cell there is not always the nearest point. They stay out of the default set
until a follow-up issue decides it, and since the O1280 sample's field is
constant, the value check on reduced grids rests on the wave grid alone.

Or open the repo in VS Code, press `F5`, and open any `samples/` file in the
launched window. Click a message row to open its render panel, then use the
projection picker / colormap / overlay controls.

## Verification checklist

For every file: coastlines in the overlay should land on real coastlines (not
shifted, mirrored, or torn at the dateline), and the colorbar should read in the
variable's real physical units, not raw integer codes.

### GRIB2

| File | Packing / grid | Reproject to | Looks-right check |
|---|---|---|---|
| `gfs.grib2`   | complex + spatial-diff ([5.3](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-3.shtml)), regular lat/lon ([3.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-0.shtml)) | equirect, orthographic, Web Mercator | global field, coastlines align worldwide, no dateline tear; orthographic shows one hemisphere |
| `hrrr.grib2`  | complex + spatial-diff ([5.3](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-3.shtml)), Lambert ([3.30](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-30.shtml)) | source (shows the Lambert cone), then equirect / Web Mercator | CONUS field, coastlines over the US Gulf/Atlantic/Pacific coasts; not upside-down or mirrored |
| `nam.grib2`   | complex / JPEG 2000, Lambert ([3.30](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-30.shtml)) | equirect | regional CONUS, coastlines aligned |
| `rap.grib2`   | **JPEG 2000 ([5.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml))**, Lambert ([3.30](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-30.shtml)) | equirect | CONUS, aligned — real JPEG2000-on-Lambert |
| `nbm.grib2`   | complex + spatial-diff ([5.3](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-3.shtml)) w/ inline missing values ([mvmu=1](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_table5-5.shtml)), Lambert ([3.30](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-30.shtml)) | equirect | CONUS temperature, coastlines aligned; value range matches eccodes (267.9..315.8 K). Exercises missing-value management on a real product. |
| `mrms.grib2`  | **PNG ([5.41](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-41.shtml))**, regular lat/lon ([3.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-0.shtml)) | equirect, Web Mercator | CONUS reflectivity. Note: MRMS marks no-coverage with a **−999 sentinel** that isn't a [GRIB bitmap](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_sect6.shtml), so auto-range spans −999; set a manual range (e.g. 0..70) to see the reflectivity. |
| `ecmwf.grib2` | CCSDS / AEC ([5.42](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml)), regular lat/lon ([3.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-0.shtml)) | equirect, orthographic | global field, decodes without a libaec/native dependency, coastlines align |
| `eccc.grib2`  | JPEG 2000 ([5.40](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml)), rotated lat/lon ([3.1](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-1.shtml)) — HRDPS | equirect | rotated source unrotates so coastlines land correctly (the reprojection, not the raw tilted grid) |
| `icon.grib2`  | simple ([5.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-0.shtml)), regular lat/lon ([3.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-0.shtml)) — DWD ICON-D2 | equirect | German-domain cloud cover, coastlines along the North Sea and Baltic. The point of this one is the **message table**: center reads `Offenbach (RSMC) - DWD` and the parameter reads `CLCT_MOD / Modified cloud cover for media`, from DWD's local table rather than the WMO master set (#425). |

### Reduced grids (GRIB1)

ECMWF open data is interpolated to regular grids, so these two come from the
eccodes test data instead (Apache 2.0), each checked against an md5 the script pins.

| File | Packing / grid | Reproject to | Looks-right check |
|---|---|---|---|
| `octahedral_o1280.grib1` | simple, octahedral reduced Gaussian O1280 (eccodes `msl.octa.glob.grib1`) | equirect | ECMWF's operational grid, 2560 rows and 6.6 million points. The field is constant, so it renders flat; it is here for the probe-parity sweep's cell check at full size, not for a visual pass |
| `reduced_ll_wave.grib1` | simple with a bitmap, reduced lat/lon 0.36° (eccodes `reduced_latlon_surface.grib1`) | equirect | significant wave height: land masked, and the rows over the poles hold no points, so they stay transparent |

### NetCDF

| File | Type / grid | Reproject to | Looks-right check |
|---|---|---|---|
| `goes.nc`  | [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec), [geostationary](https://cfconventions.org/cf-conventions/cf-conventions.html#_geostationary_projection) (CF `goes_imager_projection`) | source, then equirect / orthographic | off-disk pixels stay transparent; a CONUS/meso sector frames to its own extent (not tiny in an empty hemisphere); colorbar in real units (K / radiance) |
| `wrf.nc`   | [NetCDF classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec), WRF Lambert (`MAP_PROJ` attrs) | equirect | the fetch script stages the repo's tiny (6×5) synthetic wrfout-style fixture — enough to smoke-test the WRF-attribute + Lambert path, but too small for a coastline check. Real `wrfout` files are self-generated model output with no public endpoint; drop one in by hand (overwriting `wrf.nc`) for a meaningful visual pass: regional domain, coastlines aligned |
| `oisst.nc` | NetCDF-4, regular 1/4° lat/lon (real full NOAA CDR file, pinned to 2025-01-01) | equirect | global SST, land masked (fill) transparent, colorbar in °C/K |
| `rtofs_ice.nc` | NetCDF-4, **tripolar curvilinear** (2-D `Latitude(Y,X)`/`Longitude(Y,X)`, no grid mapping) | equirect | global HYCOM sea ice, 3298×4500. In *source* projection the Arctic third is the bipolar patch — visibly folded, not a map; reprojected, it should unfold into a correct Arctic. This is also the memory/latency check for #445: 14.8 million cells build a ~400 MB index in about two seconds, once per file, and repaints after that should feel instant |
| `mirs_swath.nc` | NetCDF-4, **swath curvilinear** (per-pixel 2-D lat/lon) | equirect | NOAA-21 MiRS, 768 scanlines × 96 fields of view — a full half orbit crossing the antimeridian and the south pole. Reprojected it should read as a ribbon across the globe, not a smear: a wrong antimeridian unwrap shows up here immediately. The committed fixture is 100 scanlines of this file |

### HDF5 szip from the SZIP 2.x encoder

Two HDF5 test files from the HDF5 repository (BSD-3-Clause), written in 2006
and 2011 by libhdf5 with the HDF Group's SZIP library rather than libaec, each
checked against an md5 the script pins. They have no coordinates, so there is
no coastline check: open each szip dataset and compare its values with
`h5dump` or h5py. Both are also committed as fixtures, where
`tests/hdf5_szip.rs` compares every value (#820). The pre-flight and the
probe-parity sweep read `.nc` files only and skip these.

| File | Type | Looks-right check |
|---|---|---|
| `szip2_h5repack.h5` | `dset_szip`, `int32` 40×20, szip NN | values 0 to 799 in order |
| `szip2_le_data.h5` | `Szip_float_data_le` and `Szip_float_data_be`, `float32` 7×6 | both read the same, starting 0.333, 0.667, 1.0; the n-bit and scale-offset datasets beside them say why they are not decoded |

### Auth-gated (drop in by hand)

| File | Source | Note |
|---|---|---|
| `merra2.nc` | NASA GES DISC (Earthdata login) | [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec), metadata-heavy attributes |
| `era5.nc`   | Copernicus CDS (API key) | NetCDF-4; request a small area/step subset |
| `cmip6.nc`  | ESGF | NetCDF-4; note CC-BY-SA, do **not** promote into the committed corpus |

## Screenshot for the README

The `extension/media/screenshot.png` hero should show a **reprojected field with
coastlines**, not the metadata viewer. Good candidates: `gfs.grib2` in
orthographic, or `goes.nc` in equirectangular with the coastline overlay on.
Capture the render panel (field + colorbar + overlay visible), export at the
existing image's dimensions, and update the README alt text accordingly.
