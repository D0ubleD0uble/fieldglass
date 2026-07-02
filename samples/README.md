# Manual-testing sample corpus

Real, full operational files for eyeballing renders in the extension — the
visual counterpart to the tiny oracle-checked fixtures under
`crates/*/tests/fixtures/`. Those prove decode correctness on a handful of grid
points; these prove a whole real file *looks* right (coastlines land on
coastlines, colorbars read in real units, projections frame correctly).

Everything here except this file is git-ignored (see `.gitignore`): the data is
large and licence-varied, so it stays local.

## Fetch

```sh
tools/fetch_samples.sh            # all freely-available models
tools/fetch_samples.sh gfs hrrr   # just the named ones
```

The script pulls from public, no-credential endpoints (NOAA NOMADS / AWS Open
Data, ECMWF open data, ECCC datamart). Auth-gated sources (MERRA-2, ERA5,
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
| `gfs.grib2`   | complex + spatial-diff (5.3), regular lat/lon (3.0) | equirect, orthographic, Web Mercator | global field, coastlines align worldwide, no dateline tear; orthographic shows one hemisphere |
| `hrrr.grib2`  | complex + spatial-diff (5.3), Lambert (3.30) | source (shows the Lambert cone), then equirect / Web Mercator | CONUS field, coastlines over the US Gulf/Atlantic/Pacific coasts; not upside-down or mirrored |
| `nam.grib2`   | complex / JPEG 2000, Lambert (3.30) | equirect | regional CONUS, coastlines aligned |
| `rap.grib2`   | **JPEG 2000 (5.40)**, Lambert (3.30) | equirect | CONUS, aligned — real JPEG2000-on-Lambert |
| `nbm.grib2`   | complex (5.2) w/ inline missing values (mvmu=1), Lambert | — | **expected to fail**: NBM uses missing-value management 1, which reports a clean `UnsupportedSection` error rather than mis-decoding (documented out-of-scope). Confirms the guard, not a render. |
| `mrms.grib2`  | **PNG (5.41)**, regular lat/lon (3.0) | equirect, Web Mercator | CONUS reflectivity. Note: MRMS marks no-coverage with a **−999 sentinel** that isn't a GRIB bitmap, so auto-range spans −999; set a manual range (e.g. 0..70) to see the reflectivity. |
| `ecmwf.grib2` | CCSDS / AEC (5.42), regular lat/lon (3.0) | equirect, orthographic | global field, decodes without a libaec/native dependency, coastlines align |
| `eccc.grib2`  | JPEG 2000 (5.40), rotated lat/lon (3.1) — HRDPS | equirect | rotated source unrotates so coastlines land correctly (the reprojection, not the raw tilted grid) |

### NetCDF

| File | Type / grid | Reproject to | Looks-right check |
|---|---|---|---|
| `goes.nc`  | NetCDF-4, geostationary (CF `goes_imager_projection`) | source, then equirect / orthographic | off-disk pixels stay transparent; a CONUS/meso sector frames to its own extent (not tiny in an empty hemisphere); colorbar in real units (K / radiance) |
| `wrf.nc`   | NetCDF classic, WRF Lambert (`MAP_PROJ` attrs) | equirect | regional domain, coastlines aligned |
| `oisst.nc` | NetCDF-4, regular 1/4° lat/lon | equirect | global SST, land masked (fill) transparent, colorbar in °C/K |

### Auth-gated (drop in by hand)

| File | Source | Note |
|---|---|---|
| `merra2.nc` | NASA GES DISC (Earthdata login) | NetCDF-4, metadata-heavy attributes |
| `era5.nc`   | Copernicus CDS (API key) | NetCDF-4; request a small area/step subset |
| `cmip6.nc`  | ESGF | NetCDF-4; note CC-BY-SA, do **not** promote into the committed corpus |

## Screenshot for the README

The `extension/media/screenshot.png` hero should show a **reprojected field with
coastlines**, not the metadata viewer. Good candidates: `gfs.grib2` in
orthographic, or `goes.nc` in equirectangular with the coastline overlay on.
Capture the render panel (field + colorbar + overlay visible), export at the
existing image's dimensions, and update the README alt text accordingly.
