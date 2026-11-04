# Manual test plan — everything that changed since v0.6.0

**Baseline: v0.6.0 (2026-10-31). Covers 0.6.1.** This plan is rewritten each
cycle to cover what landed since the last tag. Add a section as each
user-facing feature merges, rather than trying to reconstruct the cycle from
the log at prep time. The 0.6.0 plan found three defects that way, and all
three were things a reader of the diff would not have thought to look for.

`samples/README.md` has a per-file "does it look right" checklist for coverage
that shipped **at or before** 0.6.0. This plan does not repeat it beyond the
regression floor below.

0.6.1 is a fix release. Most of its changes are in what a file's metadata
says: NetCDF-4 attributes stored as variable-length strings, the calendar of a
time axis, the label of a GRIB1 time range, and what the browser package's
`verifyMessage` accepts. A changed attribute can move a picture, though: a file
written by h5py or h5netcdf now has its `grid_mapping`, `coordinates`, `axis`
and `positive`, which 0.6.0 never saw. So §4 is the one section here about
placement, and its checks are priority.

---

## How to add to this plan

One section per feature, named for the issue. Under each, group checks by the
**file they need**, so a file is opened once and every check it can serve runs
together. For each check, say what to look at and what failure looks like.
"Renders correctly" is not a check anyone can fail honestly.

Before writing a check, **open the fixture and confirm it can show the thing**.
A constant field, a single-message file, or a domain that never reaches the
latitude in question will all pass a visual check while proving nothing.

Mark a check **priority** when it covers a path no automated test can judge:
something resampled, interpolated, or placed by a lookup rather than a formula.
Those are where the last cycle's defects were.

State explicitly where a check **cannot** be made by eye, and name the test that
covers it instead, rather than leaving a reader to attempt a judgment the data
cannot support.

A check marked **known** shows a gap that is already filed or reviewed and does
not block the release. Record what you see; a known check fails only if it
shows something other than what it describes.

---

## 0. Setup (once)

```sh
# From the repo root, at the commit being released (RELEASING.md's
# $RELEASE_SHA, the prep merge), not whatever master holds now.
git fetch origin && git checkout "$RELEASE_SHA"

# The napi CLI comes from the root package.json, so install it first.
npm ci --ignore-scripts

# Build the native module into the extension so the dev host runs current Rust,
# then compile the TypeScript.
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm ci && npm run compile )

# Full samples.
tools/fetch_samples.sh

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js

# Placement: read 200 seeded points per message or slice through the native
# module and through eccodes / xarray, and compare the cell, the value and the
# mask. The preflight only proves a file draws; this proves it draws in the
# right place. Needs the pre-commit venv's python3 (eccodes, xarray, netCDF4,
# zarr, scipy). About a quarter of an hour, most of it the RTOFS grid. Exit 0
# is clean; a FAILURES block is a decode or placement bug until shown to be the
# oracle's. The MRMS -999 sentinel and eccodes' j-consecutive lookup are listed
# in the output as documented exceptions, not failures.
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples )
```

Then `F5` to launch the dev host, and in it File > Open Folder > `samples`.

To drive the dev host headless or on a virtual display, set
`files.simpleDialog.enable: true` and `window.dialogStyle: custom` in its
settings first. Without them the native Export dialogs never appear.

Paths below are relative to the repo root. `F:` marks a committed crate fixture
(always present); `S:` marks a `samples/` file (needs the fetch above); `B:`
marks a file the next step builds into `samples/0.6.0/` or `samples/0.6.1/`.

### 0.6.0 beside it

Several checks below compare with what 0.6.0 showed for the same file. Install
the released 0.6.0 from the Marketplace into a profile of its own, so it does
not replace anything in your usual one, and open the same folder there:

```sh
code --profile fieldglass-0.6.0 --install-extension fieldglass.fieldglass@0.6.0
code --profile fieldglass-0.6.0 samples
```

If the Marketplace is out of reach, build the `v0.6.0` tag in a worktree
outside this checkout the way the setup above builds this one, and launch its
dev host instead.

### Build the files this cycle's checks need

Every committed GRIB file holds **one** message, and so does every file
`fetch_samples.sh` writes. That includes `cmc_wind_300_2010052400_p012.grib`,
which is wind *speed* at 300 hPa, not a u/v pair. Arrows, Compare on GRIB and
the reload checks all need two or more messages, and no committed NetCDF or Zarr
file has a real time or level axis to animate. The first block builds those
files into `samples/0.6.0/`; the regression floor uses some of them. The second
builds this cycle's files into `samples/0.6.1/`. Both folders are ignored by
git. They need `curl`, and `numpy`, `xarray`, `zarr` 3, `netCDF4`, `h5netcdf`
and `h5py` in the `python3` you run them with. The pre-commit venv has all but
`h5netcdf`. If yours lacks any, make a throwaway environment for this step:
`python3 -m venv /tmp/fg-venv && /tmp/fg-venv/bin/pip install xarray 'zarr>=3' netCDF4 h5netcdf h5py`,
then run the Python steps below with `/tmp/fg-venv/bin/python` in place of
`python3`.

```sh
mkdir -p samples/0.6.0 && cd samples/0.6.0

# Real u/v pairs at 10 m: GFS states earth-relative components, HRRR grid-
# relative ones. NOMADS keeps a few days, so use yesterday's 00Z run.
D=$(date -u -d yesterday +%Y%m%d)
curl -fsSL -o gfs_wind.grib2 "https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl?dir=/gfs.${D}/00/atmos&file=gfs.t00z.pgrb2.0p25.f000&var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on"
curl -fsSL -o hrrr_wind.grib2 "https://nomads.ncep.noaa.gov/cgi-bin/filter_hrrr_2d.pl?dir=/hrrr.${D}/conus&file=hrrr.t00z.wrfsfcf00.grib2&var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on"

# NCEP/NCAR reanalysis air temperature: 366 daily global fields for 2024, and
# 12 monthly means on 17 pressure levels. NetCDF-4, deflated, public domain.
curl -fsSL -o ncep_air_daily_2024.nc https://psl.noaa.gov/thredds/fileServer/Datasets/ncep.reanalysis/Dailies/surface/air.sig995.2024.nc
curl -fsSL -o ncep_air_levels_ltm.nc https://psl.noaa.gov/thredds/fileServer/Datasets/ncep.reanalysis/Monthlies/pressure/air.mon.ltm.1991-2020.nc

# The daily file again as Zarr, written by xarray: a consolidated v2 store
# and an unconsolidated v3 store.
python3 -c "
import xarray as xr
ds = xr.open_dataset('ncep_air_daily_2024.nc')
ds.to_zarr('air_v2.zarr', zarr_format=2, consolidated=True, mode='w')
ds.to_zarr('air_v3.zarr', zarr_format=3, consolidated=False, mode='w')
"

# A failed download can leave an HTML error page under the right name. This
# must print nothing.
for f in gfs_wind.grib2 hrrr_wind.grib2; do [ "$(head -c4 "$f")" = GRIB ] || echo "BAD $f"; done
python3 -c "
import netCDF4
for f in ['ncep_air_daily_2024.nc', 'ncep_air_levels_ltm.nc']:
    d = netCDF4.Dataset(f)
    assert d['air'].shape[0] in (366, 12), f
"
cd ../..

# Multi-message GRIB files made from committed fixtures, by rewriting a few
# octets. eccodes reads every one of them as the comments say.
python3 - <<'EOF'
from pathlib import Path

out = Path("samples/0.6.0")
g1 = Path("crates/fieldglass-grib1/tests/fixtures")
g2 = Path("crates/fieldglass-grib2/tests/fixtures")


def section(msg, number):
    at = 16
    while msg[at:at + 4] != b"7777":
        if msg[at + 4] == number:
            return at
        at += int.from_bytes(msg[at:at + 4], "big")
    raise SystemExit(f"no section {number}")


def patched(src, edits):
    b = bytearray(src)
    for at, value in edits:
        b[at] = value
    return bytes(b)


def write(name, *messages):
    (out / name).write_bytes(b"".join(messages))


# CMC's 300 hPa wind speed twice: a u/v "pair" at 45 degrees to the grid.
cmc = (g1 / "cmc_wind_300_2010052400_p012.grib").read_bytes()
write("cmc_pair.grib", cmc, cmc)

# The committed GFS message as NCEP local parameters 0-2-250 and 0-2-251,
# which no table names (eccodes reads both as `unknown`): a pair whose names
# say nothing (#1142).
gfs = (g2 / "gfs_c255_latlon.grib2").read_bytes()
s4 = section(gfs, 4)
write("gfs_unnamed_pair.grib2",
      patched(gfs, [(s4 + 9, 2), (s4 + 10, 250)]),
      patched(gfs, [(s4 + 9, 2), (s4 + 10, 251)]))

# HRRR temperature stating grid-relative, earth-relative, grid-relative
# components (template 3.30 flags, bit 5), and against a lat/lon grid.
hrrr = (g2 / "hrrr_complex_spd_lambert.grib2").read_bytes()
flags = section(hrrr, 3) + 14 + 32
grid_rel = patched(hrrr, [(flags, hrrr[flags] | 0x08)])
earth_rel = patched(hrrr, [(flags, hrrr[flags] & 0xF7)])
write("hrrr_frames.grib2", grid_rel, earth_rel, grid_rel)
write("two_grids.grib2", grid_rel, (g2 / "regular_latlon_surface.grib2").read_bytes())

# A T383 spectral field twice, to Compare.
t383 = (g2 / "spectral_simple_t383.grib2").read_bytes()
write("spectral_pair.grib2", t383, t383)

# ECMWF 2 m temperature with the PDS rewritten: as committed; table 140
# id 229 (swh); DWD's own table 129; Meteo-France with ECMWF as sub-center;
# level type 250; level type 255.
t2m = (g1 / "j_consecutive_latlon.grib1").read_bytes()
p = 8
write(
    "grib1_names.grib1",
    t2m,
    patched(t2m, [(p + 3, 140), (p + 8, 229)]),
    patched(t2m, [(p + 4, 78), (p + 3, 129)]),
    patched(t2m, [(p + 4, 85), (p + 25, 98), (p + 3, 140), (p + 8, 229)]),
    patched(t2m, [(p + 9, 250)]),
    patched(t2m, [(p + 9, 255)]),
)

# GRIB2 temperature with discipline 191, then 99.
latlon = (g2 / "regular_latlon_surface.grib2").read_bytes()
write("grib2_codes.grib2", patched(latlon, [(6, 191)]), patched(latlon, [(6, 99)]))

# Ensemble members 7 and 8 (template 4.1 octet 36), in both orders and doubled.
member = (g2 / "ensemble_member_4_1.grib2").read_bytes()
m8 = patched(member, [(section(member, 4) + 35, 8)])
write("members_7_8.grib2", member, m8)
write("members_8_7.grib2", m8, member)
write("members_8_8.grib2", m8, m8)

# A color table in a model Fieldglass refuses.
(out / "hsv.cpt").write_text("# COLOR_MODEL = HSV\n0 0-1-1 1 360-1-1\n")
EOF

# Every file above decodes and renders headlessly, and lands in the right place.
node tools/preflight_samples.js samples/0.6.0
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples/0.6.0 )
```

Then this cycle's files. They are built from committed fixtures and from
`samples/0.6.0/ncep_air_levels_ltm.nc` above, so they need no download. The
two budget files are 26 MiB each; everything else is small.

```sh
python3 - <<'EOF'
from pathlib import Path

import netCDF4
import numpy as np

out = Path("samples/0.6.1")
out.mkdir(parents=True, exist_ok=True)
g1 = Path("crates/fieldglass-grib1/tests/fixtures")
nc = Path("crates/fieldglass-netcdf/tests/fixtures")

# One time axis per calendar, each under its own 19 x 36 global field that
# warms 3 K a step. Every axis holds the same numbers of days since
# 2000-01-01, which the standard calendar dates 2000-01-01, 2000-02-29,
# 2000-12-26 and 2000-12-31. A `calendar` written with setncattr_string is an
# NC_STRING attribute, which 0.6.0 dropped.
calendars = [
    # name, calendar, stored as NC_STRING?
    ("noleap", "noleap", True),
    ("360day", "360_day", False),
    ("julian", "julian", True),
    ("standard", "standard", True),
    ("padded", " Gregorian ", False),
    ("none", None, False),
    ("list", ["standard", "gregorian"], True),
]
lat = np.arange(-90.0, 91.0, 10.0)
lon = np.arange(0.0, 360.0, 10.0)
days = [0.0, 59.0, 360.0, 365.0]
d = netCDF4.Dataset(out / "calendars.nc", "w", format="NETCDF4")
d.createDimension("lat", lat.size)
d.createDimension("lon", lon.size)
for name, units in (("lat", "degrees_north"), ("lon", "degrees_east")):
    v = d.createVariable(name, "f8", (name,))
    v.units = units
    v[:] = lat if name == "lat" else lon
base = 250 + 40 * np.cos(np.radians(lat))[:, None] + 5 * np.sin(np.radians(lon))[None, :]


def time_axis(dim, units, values, calendar, as_string):
    d.createDimension(dim, len(values))
    t = d.createVariable(dim, "f8", (dim,))
    t.units = units
    t.standard_name = "time"
    if as_string:
        t.setncattr_string("calendar", calendar)
    elif calendar is not None:
        t.calendar = calendar
    t[:] = values


def field(name, dim, n):
    v = d.createVariable(name, "f4", (dim, "lat", "lon"))
    v.units = "K"
    v.long_name = f"air temperature on time axis {dim}"
    v[:] = np.stack([base + 3 * i for i in range(n)])
    return v


for name, calendar, as_string in calendars:
    time_axis(f"time_{name}", "days since 2000-01-01", days, calendar, as_string)
    field(f"tas_{name}", f"time_{name}", len(days))
# The list's units are a list too.
d["tas_list"].delncattr("units")
d["tas_list"].setncattr_string("units", ["K", "kelvin"])
# A standard calendar counted from year 1. CF dates it Julian before
# 1582-10-15, so day 0 is 0001-01-01. These days never reach 1582, so
# this field cannot tell the Julian reading from the proleptic one.
time_axis("time_early", "days since 0001-01-01", days, "standard", True)
field("tas_early", "time_early", len(days))
d.close()

# The committed ECMWF 2 m temperature (GRIB1, hours) with its center and time
# range rewritten: PDS octet 5 is the center, 18 the time unit, 19 and 20 P1
# and P2, 21 the time-range indicator. Off ECMWF, the parameter is WMO table 2
# code 11, temperature.
t2m = (g1 / "j_consecutive_latlon.grib1").read_bytes()
p = 8


def stamp(center, unit, p1, p2, code):
    b = bytearray(t2m)
    b[p + 4] = center
    b[p + 17], b[p + 18], b[p + 19], b[p + 20] = unit, p1, p2, code
    if center != 98:
        # Drop ECMWF's local PDS extension (octets 29-52), which another
        # center's local definition would misread, and fix both lengths.
        b[p + 3], b[p + 8] = 2, 11
        b[p + 2] = 28
        del b[p + 28:p + 52]
        b[4:7] = len(b).to_bytes(3, "big")
    return bytes(b)


ECMWF, NCEP, JMA, DWD = 98, 7, 34, 78
HOUR, MIN15, MIN30 = 1, 13, 14
messages = [
    (ECMWF, HOUR, 6, 0, 0),      # 0
    (ECMWF, HOUR, 24, 24, 113),  # 1
    (ECMWF, HOUR, 0, 24, 113),   # 2
    (ECMWF, HOUR, 6, 18, 123),   # 3
    (ECMWF, HOUR, 6, 18, 130),   # 4
    (ECMWF, HOUR, 30, 54, 133),  # 5
    (ECMWF, HOUR, 24, 48, 133),  # 6
    (ECMWF, HOUR, 18, 6, 130),   # 7
    (ECMWF, HOUR, 6, 18, 8),     # 8
    (NCEP, HOUR, 6, 18, 133),    # 9
    (NCEP, HOUR, 6, 18, 128),    # 10
    (NCEP, HOUR, 6, 18, 137),    # 11
    (JMA, HOUR, 6, 18, 129),     # 12
    (DWD, HOUR, 6, 0, 11),       # 13
    (DWD, HOUR, 6, 0, 13),       # 14
    (DWD, HOUR, 6, 18, 133),     # 15
    (ECMWF, MIN30, 1, 0, 0),     # 16
    (ECMWF, MIN15, 8, 0, 0),     # 17
    (ECMWF, MIN15, 0, 1, 123),   # 18
]
(out / "grib1_time_ranges.grib1").write_bytes(b"".join(stamp(*m) for m in messages))

# netcdf4_string_attributes.nc with one global-heap object's index repeated:
# the object holding `noleap` (time:calendar) takes the index of the one
# holding `K` (temperature:units). An object is a 2-byte index, 2-byte
# reference count, 4 reserved bytes, an 8-byte size and its data padded to 8.
src = bytearray((nc / "netcdf4_string_attributes.nc").read_bytes())
objects = {}
at = src.find(b"GCOL")
while at != -1:
    end = at + int.from_bytes(src[at + 8:at + 16], "little")
    o = at + 16
    while o + 16 <= end:
        index = int.from_bytes(src[o:o + 2], "little")
        size = int.from_bytes(src[o + 8:o + 16], "little")
        if index == 0:
            break
        objects[bytes(src[o + 16:o + 16 + size])] = o
        o += 16 + (size + 7) // 8 * 8
    at = src.find(b"GCOL", end)
src[objects[b"noleap"]:objects[b"noleap"] + 2] = src[objects[b"K"]:objects[b"K"] + 2]
# In its own folder: netCDF-C refuses its attributes, so the parity sweep,
# which reads every file in samples/0.6.1 with netCDF4, would stop on it.
(out / "damaged").mkdir(exist_ok=True)
(out / "damaged" / "repeated_heap_index.nc").write_bytes(src)

# Past the 16 MiB of string attribute text one file is read for: two 9 MiB
# NC_STRING attributes. In budget_variable.nc they are on the variable
# `notes`, and `time`, listed after it, has its calendar as NC_STRING. In
# budget_global.nc they are global attributes, and `time` is the same. The
# second 9 MiB attribute does not fit and is left out, which leaves 7 MiB, so
# the calendar is `noleap` padded to 8 MiB: too big for what is left, so the
# budget cuts it. A short calendar would fit and be read (#1568).
nine = "x" * (9 << 20)
for name, where in (("budget_variable.nc", "variable"), ("budget_global.nc", "global")):
    d = netCDF4.Dataset(out / name, "w", format="NETCDF4")
    d.setncattr_string("title", f"string attributes past the budget, on a {where}")
    d.createDimension("time", 2)
    d.createDimension("lat", lat.size)
    d.createDimension("lon", lon.size)
    for axis, units in (("lat", "degrees_north"), ("lon", "degrees_east")):
        v = d.createVariable(axis, "f8", (axis,))
        v.units = units
        v[:] = lat if axis == "lat" else lon
    notes = d.createVariable("notes", "i4", ())
    notes.comment = "a small NC_CHAR attribute, which the budget leaves alone"
    owner = notes if where == "variable" else d
    owner.setncattr_string("blob_a", nine)
    owner.setncattr_string("blob_b", nine)
    t = d.createVariable("time", "f8", ("time",))
    t.units = "days since 2000-01-01"
    t.setncattr_string("calendar", "noleap".ljust(8 << 20))
    t[:] = [0.0, 365.0]
    v = d.createVariable("tas", "f4", ("time", "lat", "lon"))
    v.units = "K"
    v[:] = np.stack([base, base + 3])
    d.close()
EOF

# The same data written again by h5netcdf, which stores every text attribute
# as a variable-length string: `units`, `long_name`, `axis`, `positive`,
# `coordinates`, `grid_mapping` and `grid_mapping_name` among them. 0.6.0
# read none of those from these copies. decode_cf=False keeps the values and
# attributes as stored. Needs h5netcdf (see above).
python3 - <<'EOF'
from pathlib import Path

import xarray as xr

out = Path("samples/0.6.1")
nc = Path("crates/fieldglass-netcdf/tests/fixtures")
for src, name in (
    (nc / "goes16_abi_cmip.nc", "goes_h5netcdf.nc"),
    (nc / "rtofs_tripolar_arctic.nc", "rtofs_h5netcdf.nc"),
    (Path("samples/0.6.0/ncep_air_levels_ltm.nc"), "air_levels_h5netcdf.nc"),
):
    with xr.open_dataset(src, decode_cf=False) as ds:
        ds = ds.load()
    # The parity sweep pins the original's climatology_bounds as a variable
    # with no image axes; the copy drops it rather than adding a second pin.
    if "climatology_bounds" in ds:
        ds = ds.drop_vars("climatology_bounds")
        ds["time"].attrs.pop("climatology", None)
    ds.to_netcdf(out / name, engine="h5netcdf")
EOF

# calendars.nc again as a consolidated Zarr v2 store.
python3 -c "
import xarray as xr
xr.open_dataset('samples/0.6.1/calendars.nc', decode_cf=False).to_zarr(
    'samples/0.6.1/calendars.zarr', zarr_format=2, consolidated=True, mode='w')
"

# calendar_shapes.zarr (#1630). It has lat and lon so the sweep below finds
# image axes to place.
python3 - <<'EOF'
import json
from pathlib import Path

import numpy as np

# One time array per shape a `calendar` can take, each under a 4 x 6 field
# that warms 3 K a step.
root = Path("samples/0.6.1/calendar_shapes.zarr")
shapes = [
    # name, the calendar attribute as JSON text
    ("null", "null"),
    ("true", "true"),
    ("object", '{"a": 1}'),
    ("one", '["noleap"]'),
    ("number", "5"),
]
days = np.array([0.0, 59.0, 360.0, 365.0])
lat = np.array([-30.0, -10.0, 10.0, 30.0])
lon = np.array([0.0, 60.0, 120.0, 180.0, 240.0, 300.0])
base = np.add.outer(np.arange(4.0), np.arange(6.0)) + 250


def array(shape):
    return {"zarr_format": 2, "shape": shape, "chunks": shape, "dtype": "<f8",
            "compressor": None, "fill_value": None, "order": "C", "filters": None}


def put(name, shape, attrs, chunk_key, values):
    d = root / name
    d.mkdir(parents=True, exist_ok=True)
    (d / ".zarray").write_text(json.dumps(array(shape)))
    (d / ".zattrs").write_text(attrs)
    (d / chunk_key).write_bytes(values.astype("<f8").tobytes())


root.mkdir(parents=True, exist_ok=True)
(root / ".zgroup").write_text('{"zarr_format": 2}')
for name, units, values in (("lat", "degrees_north", lat), ("lon", "degrees_east", lon)):
    put(name, [values.size],
        '{"_ARRAY_DIMENSIONS": ["%s"], "units": "%s"}' % (name, units), "0", values)
for name, calendar in shapes:
    put(f"time_{name}", [4],
        '{"_ARRAY_DIMENSIONS": ["time_%s"], "units": "days since 2000-01-01", '
        '"calendar": %s}' % (name, calendar), "0", days)
    put(f"tas_{name}", [4, 4, 6],
        '{"_ARRAY_DIMENSIONS": ["time_%s", "lat", "lon"], "units": "K"}' % name,
        "0.0.0", np.stack([base + 3 * i for i in range(4)]))
EOF

# Message table (#1628): one message that draws, the Bi-Fourier one that
# can't, and the Lambert one, then 20 more copies of the first so the page
# is taller than the editor and scrolls (23 messages, numbered 0 to 22). It
# goes in a subfolder, since its middle message does not render and the
# preflight would say so.
mkdir -p samples/0.6.1/rows
LATLON=crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2
{
  cat "$LATLON" \
      crates/fieldglass-grib2/tests/fixtures/bifourier_ellipse_ieee32.grib2 \
      extension/src/test/fixtures/eta_lambert_msg0.grib2
  for _ in $(seq 20); do cat "$LATLON"; done
} > samples/0.6.1/rows/message_rows.grib2

# Every file decodes and renders headlessly, and lands in the right place:
# the preflight prints no `RENDER FAILED` line, and the parity sweep ends
# with `0 failed`. The damaged file is in a subfolder neither reads.
node tools/preflight_samples.js samples/0.6.1
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples/0.6.1 )
```

To see that a built file holds what its section needs, `h5dump -A` prints
every attribute of a NetCDF-4 file, and `grib_ls -p
centre,timeRangeIndicator,P1,P2,unitOfTimeRange samples/0.6.1/grib1_time_ranges.grib1`
lists each GRIB1 message's center and time range. eccodes reads all 19 of them
as the comments in the build say. `h5dump` reads `repeated_heap_index.nc`'s
temperature `units` as `K` and its time `calendar` as empty, and netCDF4-python
refuses those attributes; §6 is about what Fieldglass does instead.

`.h5` files do not open in Fieldglass by double-click: right-click the file,
*Open With…*, *Fieldglass Viewer*.

### Files by section

Each file is opened once; this is where its checks are.

| File | Sections |
|---|---|
| `S: samples/gfs.grib2` | 1 |
| `S: samples/mirs_swath.nc` | 1 |
| `S: samples/oisst.nc` | 1 |
| `B: samples/0.6.0/gfs_wind.grib2` | 1 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 1, 2 |
| `B: samples/0.6.0/air_v2.zarr` | 2 |
| `B: samples/0.6.0/ncep_air_levels_ltm.nc` | 2, 4 |
| `B: samples/0.6.0/cmc_pair.grib`, `grib1_names.grib1` | 7 |
| `B: samples/0.6.1/calendars.nc`, `calendars.zarr` | 2 |
| `B: samples/0.6.1/calendar_shapes.zarr` | 11 |
| `B: samples/0.6.1/goes_h5netcdf.nc`, `rtofs_h5netcdf.nc`, `air_levels_h5netcdf.nc` | 4 |
| `B: samples/0.6.1/budget_variable.nc`, `budget_global.nc` | 5 |
| `B: samples/0.6.1/damaged/repeated_heap_index.nc` | 6 |
| `B: samples/0.6.1/grib1_time_ranges.grib1` | 7 |
| `B: samples/0.6.1/rows/message_rows.grib2` | 10 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`, `bifourier_ellipse_ieee32.grib2` | 1 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_string_attributes.nc`, `h5py_string_attributes.h5` | 3 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc`, `netcdf4_hdf5_dummy.nc` | 3 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc`, `netcdf4_string_lists.nc` | 11 |
| `F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc`, `rtofs_tripolar_arctic.nc` | 4 |
| `F: crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_cf_ro.grib2`, `ecmwf_ifs_0p25_mx2t3.grib2` | 7 |
| `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` | 1 |
| The release dry run's npm tarball | 8, 9 |

---

## 1. Regression floor

Carried forward each cycle. This is the minimum for a release whose diff is
small, and it is *not* a substitute for the per-feature sections below it.

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
- [ ] **Controls a grid can't use are hidden, and the panel says why**
      (#1036). On `gfs.grib2` with Projection on Source projection (next
      step), Resampling and the projection settings (Bounds, Center lon and
      lat, Hemisphere, Central meridian) are hidden: each appears only with a
      projection it acts on. Every other control shows, and there is no "Not
      available for this grid" line under the caption. On
      `S: samples/mirs_swath.nc`, Zonal average and Resampling are hidden;
      the line under the caption reads `Not available for this grid:
      Resampling, Zonal average`, starts closed, and opens (click, or Tab to
      it and press Enter) to `Resampling: this grid places each cell by
      its own coordinates, so the map always shows the nearest cell.` and
      `Zonal average: this grid's rows aren't circles of latitude.` Nothing
      is grayed out.
- [ ] **Resampling shows only when the projection warps** (#1091). On
      `gfs.grib2` with Projection on Source projection there is no
      Resampling picker. Choose Equirectangular: it appears. Set it to
      Bilinear, switch to Source projection and back: it is still Bilinear and
      the map is the same as before. Do the same on a NetCDF slice
      (`B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`).
      Nothing else in the toolbar leaves a blank gap or stays visible when it
      should be gone. The one exception is the empty row under Bounds while
      it is on Auto: that space is reserved on purpose, so the map does not
      move under the pointer when a wheel or drag switches Bounds to Manual
      (#1228).
- [ ] **No Resampling on a grid of cell centers** (#1225). On
      `S: samples/mirs_swath.nc` (`TPW`), choose Equirectangular, then
      Mollweide: there is still no Resampling picker, the line still lists
      Resampling, and the caption ends `(nearest)`. Fail: a Resampling picker
      on either map. The warp draws such a grid nearest whatever is asked, so
      Bilinear would change nothing.
- [ ] **Contours on `gfs.grib2` in Mollweide.** No isoline streaks straight
      across the map. Use Mollweide rather than Equirectangular: an
      equirectangular raster is as wide as the grid, which lands the last pixel
      on the seam and hides the gap the bug lives in (#332).
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` — a
      spectral field still synthesizes and renders.
- [ ] **A field with no rows and columns states its own size.** The spectral
      file above reads `T63`, `extension/src/test/fixtures/healpix_n4_ring.grib2`
      reads `Nside 4`, and
      `F: crates/fieldglass-grib2/tests/fixtures/bifourier_ellipse_ieee32.grib2`
      reads `N4 M4`. Bi-Fourier decodes only to coefficients and does not
      render, so check its Size column and the note in its row, which reads
      `Render not available: Bi-Fourier synthesis isn't supported, so these
      coefficients can't be drawn.` (#1465), not "grid dimensions unknown".
- [ ] **A coarse grid reprojects at display scale** (#514). The HEALPix file
      above in **Orthographic**: the globe's edge is a smooth circle, not a
      staircase, and the coastlines follow it rather than floating over blank
      pixels. The data stays blocky — 192 pixels is all there is — and the
      source view shows that honestly.
- [ ] `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` — classic NetCDF
      metadata view renders.
- [ ] **A variable with no cells says why** (#1425). In the same file `time`
      is unlimited with no records. Click Render on `AGC_1202`: the status
      reads `nothing to draw: "time" (dimension 0 of "AGC_1202") has
      length 0`, with no `Error:` in front, no picture and no color bar.
      Export PNG says `Nothing to export: …`, not that the last render
      failed. The axis pickers and Compare stay; the empty frame, the Line
      plot box, Flip Y axis and the Colors, Color Range and Overlay rows are
      hidden (#1426). Fail: the panel stays on `Rendering…`, or calls it an
      error.
      Set Y axis to `z` (time across), then turn Compare on, then pick
      `CD_310`: each says the same of its own variable, and the panel keeps
      answering.
- [ ] Make an empty file (`: > /tmp/empty.nc`) and open it: the page says
      `Nothing to list in this file.`, not `No messages found.`, which is
      kept for GRIB (#1053).
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` —
      the canonical GRIB2 path renders.
- [ ] **Difference map, on NetCDF**: `S: samples/mirs_swath.nc` → `BT` at two
      different `Channel` indices, **Compare**. The difference renders and the
      probe reads the *combined* value, not field A (#329).
- [ ] **Difference map, on GRIB**: `B: samples/0.6.0/gfs_wind.grib2`, message 0
      (UGRD) **Compare** A − B against message 1 (VGRD). The difference renders,
      and the probe at one point reads u minus v there, not u.
- [ ] **Export PNG** from one render; the file opens and matches what was on
      screen.
- [ ] Tick **Contours** and **only**, then **Export PNG**: the file shows the
      isolines without the color field, and no color bar. In the bar's place,
      on screen and in the PNG, a key reads `Contours every 10 K · 210.4 …
      309.7 K`, with your field's interval, range and units. Type an
      interval and the key follows; blank is the automatic spacing (#1463).
      The key's range is the color range the lines are spaced over, not
      always the data's. Set Color Range to Manual with min and max inside
      your field's range (say 250 and 300 on a 210 to 310 K field): the key
      reads `Contours every 5 K · 250.0 … 300.0 K`: the lines respace over
      that range, every 5 K where they were every 10. Back to
      Auto, it reads the data's range again. Failure looks like a key that
      keeps the data's range under a manual one.
      `a_manual_range_spaces_the_automatic_contours` and
      `one_manual_end_takes_the_other_from_the_data` (both in
      `crates/fieldglass/tests/color_range_rules.rs`; the second also in
      `crates/fieldglass/src/color_range.rs`) pin the one range the
      lines and the colors share, and `composeContourKey` in
      `extension/src/test/suite/contour-key.test.ts` pins the key's text. No
      test drives the panel's respacing under a manual range, so that part
      is by eye.
      An interval too small for the range is refused, not drawn in part
      (#1484). Back on Auto, type `0.01` as the interval: no isolines are
      drawn, the key reads `No contours drawn`, on screen and in the PNG,
      and the status line reads `Contours: contour interval 0.01 would draw
      … levels, more than the 2000 allowed; choose a wider interval`, with
      about the range divided by 0.01 in place of the `…`. Type `5`: the
      lines, the key and the status line are as before. Then set Color Range
      to Manual, min `300`, max `290`: the key reads `No contours drawn`
      again. Set min back below max, back to Auto, and clear the interval.
      Failure looks like lines over the bottom of the range only, or a key
      describing lines that are not on the map. `contour-key.test.ts` (`The
      contour key claims no lines that are not drawn (#1484)`) drives both
      cases through the panel, and
      `crates/fieldglass-napi/src/range_rule_tests.rs`
      (`contours_refuse_an_interval_past_the_level_cap`) holds the refusal
      for every contour call.
      The map area is the editor's background, as on screen: dark in Dark
      Modern, white in Light Modern, with the isolines and coastlines clearly
      readable in both (the title and key stay on white). Untick **only**: the
      color bar is back and the key is gone, on screen and in the next PNG.
      Untick Contours with **only** still ticked: the field is back
      on screen and in the next PNG (#1270, #1265). The **only** box and the
      contour interval field go while Contours is off, and leave their room
      empty, so the toolbar does not move when Contours is ticked again
      (#1260). Failure looks like a blank map with Contours off, or an
      interval field still showing.
- [ ] On a file with two grids, set Compare to A − B with Field B on the other
      grid. The status line says "Error: …", and **Export PNG** says "Export
      failed: the last render failed; see the status line." and saves nothing.
      Pick a Field B on the same grid: the export works again (#1187).
- [ ] On the `gfs.grib2` render panel, Export CSV… sits beside Export PNG…
      (#1039). In 0.5.0 a GRIB field's CSV could be exported only from the
      metadata table. The save dialog opens in the file's folder on the PNG's
      name with `.csv`, `gfs_<parameter>_message-<n>.csv`, and the file holds
      the field.
- [ ] On the GRIB difference map above, hover Export CSV…: the tooltip says
      that with Compare on it exports Field A. Click it: the layout prompt
      starts "Compare is on: the CSV holds Field A, not the difference.", and
      the CSV holds message 0's values (u), not u minus v. A CSV of the
      difference is #1111.
- [ ] **Standard-calendar times are still dates** (#1548 regression). On
      `B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`, the `time` stepper
      reads `2024-01-01` at step 0 and `2024-12-31` at the last step, and the
      line under the heading carries the same date. Failure looks like
      `1963536 hours since 1800-01-01 …`: this file states no `calendar`,
      which must still count as standard.
- [ ] **GRIB1 forecasts with no time range are unchanged** (#876
      regression). `B: samples/0.6.0/cmc_pair.grib` reads `+12h` in the Fcst
      column for both messages, and `B: samples/0.6.0/grib1_names.grib1`
      reads `+0h` for all six. Failure looks like `time range 0, …` or an
      empty Fcst cell.
- [ ] **The "Variables not decoded" table still lists a file's unreadable
      variables** (#1556 regression). On
      `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc`
      the table lists `station_info` with `unsupported section: HDF5 datatype
      class 6 (compound)` and `visits` with a reason ending
      `(variable-length)`. Under it the legend reads `Fieldglass could not
      read these variables, for the reason given beside each.` and nothing
      more (0.6.0 said these variables "use an HDF5 datatype Fieldglass does
      not decode", which is no longer the only reason). Failure looks like
      the table missing, a variable in both tables, or a legend that claims
      `The rest of this file's metadata is complete` (#1571).
- [ ] **A real NetCDF-4 file reads as it did** (#1556, #1557, #1562
      regression). The heap and budget changes sit on the path every
      NetCDF-4 attribute is read through. Open `S: samples/oisst.nc` in 0.6.0
      and in 0.6.1. Both list the same 37 global attributes and the same
      attributes on `time`, `zlev`, `lat`, `lon`, `sst`, `anom`, `err` and
      `ice`, with the same values. There is no "Variables not decoded"
      table in either. The metadata page appears in about the same time in
      both, no more than a second apart. Failure looks like an attribute
      missing or changed in 0.6.1, a new "Variables not decoded" table, or a
      page that is clearly slower to open.

---

## 2. Times on a calendar other than the standard one show their number (#1033, #1548)

A time whose `calendar` is anything but `standard`, `gregorian` or
`proleptic_gregorian` (in any case, with spaces trimmed) now shows its value
and units instead of a Gregorian date. 0.6.0 dated every time on the standard
calendar, and dropped a `calendar` stored as NC_STRING altogether, so the
noleap, julian and standard axes below went through it as if they had none.

Every axis in `calendars.nc` holds days `0, 59, 360, 365`, since 2000-01-01
(since 0001-01-01 for `tas_early`), and
the field warms 3 K a step, so each step draws a different picture. On the
standard calendar those days are 2000-01-01, 2000-02-29, 2000-12-26 and
2000-12-31.

**`B: samples/0.6.1/calendars.nc`**

- [ ] `tas_noleap`: step the `time_noleap` stepper through all four. It
      reads `0 days since 2000-01-01`, `59 days since 2000-01-01`, `360 days
      since 2000-01-01`, `365 days since 2000-01-01`, and the line under the
      heading carries the same text. Failure looks like a date: 0.6.0 shows
      `2000-02-29` at step 1, a day the noleap calendar does not have.
- [ ] `tas_360day` (the calendar is NC_CHAR here): step 2 reads `360 days
      since 2000-01-01`, where 0.6.0 reads `2000-12-26` (on a 360-day
      calendar it is 2001-01-01). `tas_julian` reads numbers the same way.
- [ ] `tas_standard`, `tas_padded` (` Gregorian `, with spaces and a capital)
      and `tas_none` (no `calendar`) all read dates: `2000-01-01`,
      `2000-02-29`, `2000-12-26`, `2000-12-31`. Failure looks like numbers on
      any of the three.
- [ ] On `tas_360day` at step 2, Export PNG…: the suggested name carries the
      step as `…360-days-since-2000-01-01.png`, not a date. On
      `tas_standard` at step 2 it ends `…2000-12-26.png`.
- [ ] Tick Line plot on `tas_noleap`, click any cell and plot along
      `time_noleap`: four points, ticked along the bottom with numbers
      (`0 … 365`), and the dashed rule at the step on screen. Do the same on
      `tas_standard`: the ticks are dates. Failure looks like dates under the
      noleap plot.
- [ ] **Cross-section.** On `tas_noleap` set the axes to `time_noleap` (X)
      and `lat` (Y). The time ticks are numbers, and the caption reads
      `x: time_noleap (days since 2000-01-01) · y: lat (degrees_north)`,
      keeping the units the numbers need. On `tas_standard` with the same
      axes the ticks are dates, and the caption reads `x: time_standard ·
      y: lat (degrees_north)`, with no units after a dated axis.
- [ ] `tas_list` has `calendar` written
      as a two-string list, `"standard", "gregorian"`, and `units` as `"K",
      "kelvin"`. At step 1 the stepper reads `59 days since 2000-01-01
      (not shown as a date: its calendar holds several values)`, and the
      field's units read `"K", "kelvin"`, quoted, in the readout and under
      the color bar. That fails safe: a list is not a calendar Fieldglass
      knows, so it shows a number, never a wrong date, and the note beside
      it says why (#1604). 0.6.0 read neither attribute: dates, and no
      units. Failure looks like a date at step 1, a number with no note, or
      no units. The other axes (`tas_noleap`, `tas_standard` and the rest)
      carry no note.
- [ ] `B: samples/0.6.0/ncep_air_levels_ltm.nc` counts hours since
      1800-01-01 on the standard calendar, and CF dates that calendar on
      the Julian calendar before 1582-10-15. Its `time` stepper reads
      `0001-01-01` at step 0. Failure looks like `0000-12-30`: the
      proleptic Gregorian date, two days off at year 1 (#1574).
      `tas_early` (days since `0001-01-01`) only checks that its dates
      still read: step 0 is `0001-01-01`, and its days never reach 1582,
      so the old build read it the same way.

**`B: samples/0.6.1/calendars.zarr`** (*Fieldglass: Open Zarr Store…* from the
command palette, then pick the folder)

- [ ] The same checks hold on the Zarr copy: `tas_noleap` and `tas_360day`
      read numbers, `tas_standard`, `tas_padded` and `tas_none` read dates.
      `tas_list`'s units read `["K","kelvin"]` there, the way Zarr's JSON
      holds them, and its time reads numbers.
- [ ] `tas_list` here holds its `calendar` as the JSON list
      `["standard","gregorian"]`. At step 1 the stepper reads `59 days since
      2000-01-01 (not shown as a date: its calendar holds several values)`,
      the same words as the NetCDF copy (#1630). Failure looks like a date,
      a number with no note, or any other reason: `its calendar is not text`
      is wrong for a list of two strings, and before #1630 it was the one
      reason every odd value got. The other axes carry no note.

**`B: samples/0.6.0/air_v2.zarr`**, `air`

- [ ] The `time` stepper reads dates (`2024-01-01` at step 0), as in 0.6.0.

Whether a time counted on its own calendar reaches the right date is not
checked here, because 0.6.1 does not do it: showing these as dates on their
own calendar is later work (#1033 stays open). `calendar` reaching the wire is
held by `a_time_axis_carries_its_calendar`, `an_nc_string_calendar_reaches_the_time_axis`
and `a_zarr_time_axis_carries_its_calendar` in
`crates/fieldglass/tests/axis_values.rs`.

## 3. NetCDF-4 attributes stored as variable-length strings (#1549, #1555)

0.6.0 dropped every NC_STRING attribute without a word. They now read as text,
and a list of strings shows every element, quoted as `ncdump` prints it.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_string_attributes.nc`**

- [ ] Nothing was left out of this file, so neither the metadata page nor a
      rendered `temperature` shows a line with `not read` in it (#1604).
      Failure looks like a note on a file that lost nothing.
- [ ] Metadata page. Global attributes: `keywords` reads `"one", "two"`,
      `title` reads `fieldglass NC_STRING attribute fixture`. In the
      Variables table, `temperature` lists `long_name=température de l’air`
      (accents and the curly apostrophe intact), `units=K` and
      `flag_meanings_list="good", "suspect, flagged", "quoted \"bad\""`;
      `time` lists `calendar=noleap` and an empty `comment`. `dense` lists
      `note_00` to `note_11`, twelve of them. 0.6.0 shows none of these
      attributes.
- [ ] Render `temperature`: the color bar reads `K`, and the `time` stepper
      reads `1 days since 2000-01-01` at step 1, a number, since the calendar
      is now read as noleap. 0.6.0 reads `2000-01-02` and no units.
- [ ] The Global attributes table's Type column reads `string` for
      `title` and `keywords`, as `ncdump` does, and `char` for `history`,
      which the file writes as NC_CHAR. Failure looks like `char` beside
      `title` or `keywords` (#1570).

**`F: crates/fieldglass-netcdf/tests/fixtures/h5py_string_attributes.h5`**
(Open With…, Fieldglass Viewer)

- [ ] Global attributes: `control_list` reads `"line1\nline2", "tab\there"`,
      with the backslashes shown, not a line break or a tab; `fixed_list`
      reads `"ab", "cde"`, both elements, where 0.6.0 showed only `ab`;
      `one_element` reads `solo`, unquoted; `vlen_list` reads `"x", "yz"`.
      `temperature` lists `long_name=air temperature` and `units=K`.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_hdf5_dummy.nc`**

- [ ] Variable `v` lists `missing_value=-1.e+34f`, as text; 0.6.0 did not
      list it. It is a string in the file, so it is not a fill value and
      masks nothing. `a_string_missing_value_reads_as_text_and_masks_nothing`
      (`crates/fieldglass-netcdf/tests/hdf5_string_attributes.rs`) holds the
      masking, which this file cannot show by eye.

The values each fixture holds are checked against netCDF4-python in
`every_string_attribute_reads_as_netcdf4_python_reads_it`
(`crates/fieldglass-netcdf/tests/hdf5_string_attributes.rs`), and the escapes
in `control_characters_in_a_string_list_are_escaped`.

## 4. Files written by h5py or h5netcdf are placed and labeled (#1549, #1555)

The same NC_STRING change, seen from a file's geometry. h5netcdf writes every
text attribute as a variable-length string, so 0.6.0 saw none of `units`,
`axis`, `positive`, `coordinates`, `grid_mapping` or `grid_mapping_name` in
these copies. 0.6.1 sees them all, which moves where a field is drawn. Each
copy holds the same values as a file Fieldglass already places, so the check
is that the copy and the original agree. Open both side by side.

**`B: samples/0.6.1/goes_h5netcdf.nc`** beside
**`F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc`**, `CMI`

- [ ] **priority** Both draw as a geostationary view (the caption names the
      space view), on the same patch of the US East Coast near 38° N 75° W,
      with coastlines across the same part of the image. Click the center
      of each image, the same cell in both: the readout gives the same
      latitude, longitude and value (about `279.08 K` there). The color bars
      read `277.97 … 288.61 K`. Failure looks like the copy drawn as a
      lat/lon rectangle, or with no units: that is what 0.6.0 does with it,
      reading its scan angles as degrees.

**`B: samples/0.6.1/rtofs_h5netcdf.nc`** beside
**`F: crates/fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc`**,
`ice_coverage`

- [ ] **priority** Both are placed on the tripolar grid by their 2-D
      `Latitude` and `Longitude`, over the Arctic. Choose Orthographic and
      center it on 90° N: the ice sits over the pole in both, the same shape,
      inside the same coastlines. Click near 86.76° N 180.85° E in each: the
      same value, about `0.99894`. Equirectangular shows only a thin strip
      (the grid spans about 82° N to 90° N), so judge it on the polar view.
      Failure looks like the copy in index space with `Latitude` and
      `Longitude` listed as plain variables beside `ice_coverage`: 0.6.0
      finds no axes in the copy.

**`B: samples/0.6.1/air_levels_h5netcdf.nc`** beside
**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air`

- [ ] **priority** The map and the readout agree at any cell you click,
      with units `degC` in both (0.6.0 shows the copy with no units). The
      `level` stepper reads `1000 millibar` at step 0 in both.
- [ ] **priority** With Line plot ticked, click 30° S 25° E and choose
      `level`: in both, an upright profile with `1000` at the bottom and `10`
      at the top, spaced by log pressure. That needs the copy's `positive =
      "down"` and `units = "millibar"`, both NC_STRING. Then set the axes to
      `lat` (X) and `level` (Y): in both, the ground is at the bottom without
      touching Flip Y axis. Failure looks like the copy upside down, or its
      levels evenly spaced.
- [ ] The `time` stepper reads `0001-01-01` at step 0 in both (the year-1
      date of §2). Failure looks like a number in the copy, `0000-12-30`, or
      a different date.

The parity sweep in §0 compares 200 points of each copy with xarray, so a
cell drawn at the wrong place shows there too. What it cannot see is whether
the projection is the right one; that is what the side-by-side checks above
are for.

## 5. String attributes past the 16 MiB budget (#1556, #1562)

Fieldglass keeps at most 16 MiB of string attribute text when it lists a file. A variable
whose string attributes go past it is listed with the variables it could not
read, naming what is missing. Global attributes past it are left out, the
file opens with the rest, and a line under the global attributes names them
(#1604). Each budget file has two 9 MiB NC_STRING attributes
and a `time` whose `calendar` is NC_STRING `noleap` padded to 8 MiB, listed
after them. The second 9 MiB attribute is left out and charged nothing, which
leaves 7 MiB: the padded calendar does not fit in that, so it is cut. A short
calendar would fit and be read (#1568);
`an_oversized_global_leaves_the_variables_their_attributes` covers that, and
`a_later_calendar_the_budget_cut_is_not_read_as_standard` covers the cut.

**`B: samples/0.6.1/budget_variable.nc`**

- [ ] The file opens, and the metadata page appears in under 10 s.
      Failure looks like a page that takes longer or never appears. Global
      attributes list `title`.
- [ ] The "Variables not decoded" table lists `notes`, with a reason that
      starts `attribute "blob_b" not read:`, names the 16777216-byte limit
      and ends `The file itself is fine`, and `time`, whose reason starts
      `attribute "calendar" not read:` and goes on in the same words. The
      names are in double quotes, not backticks. 0.6.0 lists neither and
      reads `time` with no calendar.
- [ ] Render `tas`: it draws, and the `time` stepper steps by index (no
      dates, no numbers with units), since the axis's coordinate variable was
      left out. Failure looks like any date at step 1, such as `2000-12-31` or
      `2001-01-01`: that would be a noleap time read or dated as standard, which is what the budget must not cause.
- [ ] Read the reason text as a user would. It reads `attribute "blob_b"
      not read: this file has more than 16777216 bytes of HDF5 string
      attribute text in one listing, which is more than this reader holds at
      once. The file itself is fine`. Record only, not a failure: note
      whether that reads clearly.

**`B: samples/0.6.1/budget_global.nc`**

- [ ] The file opens. Global attributes list `title` and `blob_a`; `blob_b`
      is not listed. One plain line under the table names it: `Attribute
      "blob_b" not read: this file has more than 16777216 bytes of HDF5
      string attribute text in one listing, which is more than this reader
      holds at once. The file itself is fine` (#1604). Failure looks like no
      line, a line behind a "Show more", or a name in backticks.
- [ ] `blob_a` is 9 MiB of `x`. Its row shows the first 120 characters and
      opens to the first 16,384 characters, followed by `… (9.0 MiB in all)`.
      Click it open and closed, and scroll past it: each click shows or
      hides the text in under 1 s, and scrolling keeps up. Failure looks
      like a click that takes longer, a page that freezes, or a tab that
      crashes, or a note after the text that is missing or gives another
      size (#1572). Record the times either way.
- [ ] The "Variables not decoded" table lists `time`, with the reason
      `attribute "calendar" not read:` and the same words as above, and the
      legend under it says only that Fieldglass
      could not read these variables. Failure looks like a legend that says
      the rest of the metadata is complete (#1571).
- [ ] Render `tas`: as in `budget_variable.nc`, it draws and the `time`
      stepper steps by index.

Whether exactly the right attributes are cut, under each of a variable's
links, is held by `crates/fieldglass/tests/string_attribute_budget.rs`
(`the_variables_the_budget_cut_are_named_with_what_they_lack`,
`global_attributes_past_the_budget_are_left_out_and_the_file_opens`,
`a_later_calendar_the_budget_cut_is_not_read_as_standard`).

## 6. A damaged global heap is refused, not misread (#1557, #1561)

HDF5 keeps variable-length strings, and the references that name a
variable's dimensions, in a global heap, where each object has its own
number. In this copy of `netcdf4_string_attributes.nc` the object holding
`noleap` (time's `calendar`) carries the number of the one holding `K`
(temperature's `units`). 0.6.0 read neither attribute, since it dropped
every NC_STRING attribute (§3). It did read dimension references from the
heap, and took the first object with a repeated number. 0.6.1 refuses a heap
entry that names a repeated number or an object that is not there, or
whose stated length differs from what is stored, and only those. Here
`calendar` names object 4, which no longer exists after the renumbering, so
it is refused as not found.

**`B: samples/0.6.1/damaged/repeated_heap_index.nc`**

- [ ] The file opens. The "Variables not decoded" table lists `temperature`
      and `time`, each with a reason that says the file's record of the text
      is damaged: `temperature` for its `units` (`global-heap collection
      holds object index … twice`), `time` for its `calendar`
      (`global-heap object 4 not found in collection`). The reasons run
      past 120 characters, so click each one open to read its end. Neither
      variable also appears in the Variables table. The legend
      under the table reads `Fieldglass could not read these variables, for
      the reason given beside each.` Failure looks like either variable
      missing from that table, or a reason that is not about damage, or
      either variable shown without the attribute and with no word, as
      0.6.0 dropped them (#1569).
- [ ] Everything else still reads: the Global attributes table lists
      `keywords` as `"one", "two"`, and `dense` lists its twelve notes. A
      repeated number, and the object it displaced, refuse only what names
      them.
- [ ] The metadata page has no Render section, since `temperature` was the
      only variable that could be drawn. Failure looks like a Render button
      for `temperature`.

libhdf5 reads this file differently: `h5dump` gives `units` as `K` and
`calendar` as empty, and netCDF4-python refuses both attributes. The HDF5
specification says each object's number is unique within a collection, so
neither reading is certain and Fieldglass reads neither.

The same behavior, on other fixtures, is held by
`a_duplicate_object_index_skips_the_attributes_that_name_it`
(`crates/fieldglass-netcdf/tests/hdf5_string_attributes.rs`) and
`a_damaged_calendar_is_reported_and_the_axis_not_dated`
(`crates/fieldglass/tests/string_attribute_budget.rs`).

A dimension reference with the same fault makes the whole file fail to open.
No file here has one: `a_dimension_list_index_two_objects_carry_is_refused`
and `a_dimension_list_count_that_differs_from_its_object_is_refused`
(`crates/fieldglass-netcdf/tests/hdf5_dimension_scales.rs`) cover it. A string
whose stated length differs from what is stored is
`a_string_length_that_differs_from_its_object_is_skipped`
(`crates/fieldglass-netcdf/tests/hdf5_string_attributes.rs`).

## 7. GRIB1 and GRIB2 time ranges say what they are (#876, #1551, #1554, #1559, #1566)

0.6.0 showed any GRIB1 time range as a plain forecast at P1, so an average of
many daily forecasts read `+24h`. 0.6.1 names WMO codes 113 to 125, and the
local codes of ECMWF (128, 130, 133), NCEP (128 to 140), JMA (128 to 132) and
DWD (11, 13, 14), each only in its own center's files. A code no table names,
or a statistic whose products are zero or a negative time apart, shows as
`time range <code>, P1 …, P2 …`. Sub-hour units show minutes.

These labels are a lookup on the center and the code, so a wrong row in a
table is invisible to a decode test. Each line below is what eccodes says the
message holds, read through the table its center publishes.

**`B: samples/0.6.1/grib1_time_ranges.grib1`**

- [ ] **priority** The Fcst column reads, message by message:

      | # | Center | Code, P1, P2 | Fcst |
      |---|---|---|---|
      | 0 | ECMWF | 0, 6h, 0 | `+6h` |
      | 1 | ECMWF | 113, 24h, 24h | `average of +24h forecasts issued every 24h` |
      | 2 | ECMWF | 113, 0, 24h | `average of initialized analyses issued every 24h` |
      | 3 | ECMWF | 123, 6h, 18h | `average of analyses every 18h` |
      | 4 | ECMWF | 130, 6h, 18h | `average of 6–18h products issued every 12h` |
      | 5 | ECMWF | 133, 30h, 54h | `average of products every 6h from +30h to +54h` |
      | 6 | ECMWF | 133, 24h, 48h | `time range 133, P1 24h, P2 48h` |
      | 7 | ECMWF | 130, 18h, 6h | `time range 130, P1 18h, P2 6h` |
      | 8 | ECMWF | 8, 6h, 18h | `time range 8, P1 6h, P2 18h` |
      | 9 | NCEP | 133, 6h, 18h | `climatological average of 6–18h forecasts a year apart` |
      | 10 | NCEP | 128, 6h, 18h | `average of 6–18h accums issued every 24h` |
      | 11 | NCEP | 137, 6h, 18h | `average of 6–18h accums issued every 6h` |
      | 12 | JMA | 129, 6h, 18h | `variance of 6–18h products issued every 24h` |
      | 13 | DWD | 11, 6h, 0 | `+6h initialized (IDFI)` |
      | 14 | DWD | 13, 6h, 0 | `analysis` |
      | 15 | DWD | 133, 6h, 18h | `time range 133, P1 6h, P2 18h` |
      | 16 | ECMWF | 0, 1 × 30 min | `+30min` |
      | 17 | ECMWF | 0, 8 × 15 min | `+2h` |
      | 18 | ECMWF | 123, 0, 1 × 15 min | `average of analyses every 15min` |

      Messages 5, 9 and 15 share code 133 and mean three different things
      at ECMWF, NCEP and DWD. A code read under another center's table is
      the failure to look for: message 15 (DWD has no 133) must not read
      like 5 or 9. 0.6.0 reads `+P1` for every row: `+24h`
      on 1, `+18h` on 7, `+1h` on 16, `+0h` on 18.
- [ ] Open message 1's render panel: the subtitle carries `average of +24h
      forecasts issued every 24h`. Turn Compare on: the Field B picker's
      entries carry each message's Fcst text from the table, the same words.
- [ ] Every message draws the same field (the file repeats one message with
      its labels rewritten), so the picture tells you nothing here; only the
      labels do.

`forecastHours` for these GRIB1 messages is not shown anywhere in the panel.
It is `null` for the `time range …` rows, which a library user sees as a
change; `an_unnamed_time_range_is_not_shown_as_a_forecast` and
`statistics_over_runs_keep_a_calendar_unit` in
`crates/fieldglass-grib1/src/reader.rs` and the tests in
`crates/fieldglass-grib1/tests/code_tables.rs` hold it, along with every row of
each center's table against eccodes and ON388.

**`F: crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_cf_ro.grib2`** and
**`ecmwf_ifs_0p25_mx2t3.grib2`**

- [ ] These are GRIB2 statistics over a window: `cf_ro` is runoff
      accumulated over 0 to 6 h (template 4.11), `mx2t3` a maximum over 0 to
      3 h (template 4.8). The Fcst column reads `0–6h accum` for `cf_ro` and
      `0–3h max` for `mx2t3` (#1566). 0.6.0 read `+0h` for both, the window's
      start as if it were a lead time. Failure here is `+0h`, or any window
      other than those two.
- [ ] Open `mx2t3`'s render panel: the subtitle carries `0–3h max`.

For these two GRIB2 messages `forecastHours` stays `0`, the window's start, as
in 0.6.0; only the text changes. `crates/fieldglass/tests/statistical_windows.rs`
holds the text for every GRIB2 template with a time range loop.

## 8. `verifyMessage` in the browser package (#1304, #1550, #1552, #1553, #1558, #1560)

`verifyMessage` now compares an ECMWF `.index` record's `type` (`fc`, `an`,
`cf`, `pf`, `em`, `es`, `ep`) with the message's product template, reads
wgrib2's `ENS=` forms over a member whose ensemble type is missing, and knows
ECMWF by `originatingCentreCode`, its center number. None of this is in the
extension; it is the npm package's API, so this section runs a script, not a
panel.

Prerequisite, as in RELEASING.md §2: the release dry run at the prep SHA, and
the `fieldglass-wasm-npm` tarball it built. A tarball built by `pack.sh` is not
a substitute.

```sh
mkdir -p /tmp/fieldglass-verify && cd /tmp/fieldglass-verify
gh run download <dry-run id> -n fieldglass-wasm-npm -D .
npm init -y > /dev/null
npm install --no-audit --no-fund ./fieldglass-wasm-X.Y.Z.tgz
REPO=<path to the fieldglass checkout>
cat > verify.mjs <<'EOF'
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const [pkg, repo] = process.argv.slice(2);
const w = await import(join(pkg, 'full/fieldglass_wasm.js'));
w.initSync({ module: readFileSync(join(pkg, 'full/fieldglass_wasm_bg.wasm')) });
const fixture = (name) => readFileSync(join(repo, 'crates/fieldglass/tests/fixtures', name));
const sidecar = (name) => readFileSync(join(repo, 'crates/fieldglass-fetchplan/tests/fixtures', name), 'utf8');
const info = (name) => w.open(new Uint8Array(fixture(name))).message(0);
const verdict = (item, message) => {
  try {
    w.verifyMessage(item, message);
    return 'passes';
  } catch (e) {
    return `refused (${e.code}): ${e.message}`;
  }
};
// The first line of an ECMWF .index file, with its fields rewritten.
const ecmwf = (name, edit = {}) => {
  const line = JSON.stringify({ ...JSON.parse(sidecar(name).split('\n')[0]), ...edit });
  return w.parseEcmwfIndex('o', line).items()[0];
};
const wgrib2 = (ens) => w.parseWgrib2Idx('o', `1:0:d=2026090400:asn:surface:6 hour fcst:${ens}`).items()[0];

const fc = info('ecmwf_ifs_0p25_fc_asn.grib2');
const pf40 = info('ecmwf_ifs_0p25_pf_asn_40.grib2');
const cfRo = info('ecmwf_ifs_0p25_cf_ro.grib2');
const em = info('ecmwf_ifs_0p25_em_ws250.grib2');
const es = info('ecmwf_ifs_0p25_es_ws250.grib2');
const old = { ...pf40 };
delete old.originatingCentreCode;

const rows = [
  ['1 fc line, its own oper bytes', ecmwf('ecmwf_ifs_0p25_oper_fc_asn.index'), fc],
  ['2 fc line, member 40 bytes', ecmwf('ecmwf_ifs_0p25_oper_fc_asn.index'), pf40],
  ['3 fc line with number=40, member 40 bytes', ecmwf('ecmwf_ifs_0p25_oper_fc_asn.index', { number: '40' }), pf40],
  ['4 pf line, member 40 bytes', ecmwf('ecmwf_ifs_0p25_enfo_ef_pf.index'), pf40],
  ['5 pf line as cf, member 40 bytes', ecmwf('ecmwf_ifs_0p25_enfo_ef_pf.index', { type: 'cf' }), pf40],
  ['6 cf line, control bytes', ecmwf('ecmwf_ifs_0p25_enfo_ef_cf.index'), cfRo],
  ['7 cf line as pf, control bytes', ecmwf('ecmwf_ifs_0p25_enfo_ef_cf.index', { type: 'pf', number: '0' }), cfRo],
  ['8 em line, mean bytes', ecmwf('ecmwf_ifs_0p25_enfo_ep_ws250.index'), em],
  ['9 em line, spread bytes', ecmwf('ecmwf_ifs_0p25_enfo_ep_ws250.index'), es],
  ['10 wgrib2 ENS=+40, member 40', wgrib2('ENS=+40'), pf40],
  ['11 wgrib2 ENS=+39, member 40', wgrib2('ENS=+39'), pf40],
  ['12 wgrib2 ENS=+40, member 40 kept without its center number', wgrib2('ENS=+40'), old],
  ['13 wgrib2 ENS=+40, member 40 under another name, number 98', wgrib2('ENS=+40'), { ...pf40, originatingCentre: 'ECMWF' }],
];
console.log(`member 40 states center ${pf40.originatingCentreCode}`);
for (const [what, item, message] of rows) console.log(`${what}: ${verdict(item, message)}`);
EOF
node verify.mjs "$PWD/node_modules/@fieldglass/wasm" "$REPO"
grep -n originatingCentreCode node_modules/@fieldglass/wasm/full/fieldglass_wasm.d.ts
```

- [ ] The first line reads `member 40 states center 98`. 0.6.0 reads
      `undefined`.
- [ ] Rows 1, 3, 4, 6, 8, 10, 12 and 13 pass. Rows 2, 5, 7, 9 and 11 are
      refused with code `decode`, and each message names both sides:
      - 2: `the manifest promised type "fc" but the message has "Code
        Table 4.6 255, member 40"`
      - 5: `type "cf"` against `Code Table 4.6 255, member 40`
      - 7: `type "pf"` against `Code Table 4.6 255, member 0`
      - 9: `type "em"` against `Code Table 4.7 2`
      - 11: `ensemble "ENS=+39"` against `Code Table 4.6 255, member 40`

      Failure looks like any of those five passing: the 0.6.0 package passes
      all thirteen. Row 12 is a `MessageInfo` saved by an older host without
      the center number; it must still pass, by the name. Row 13 has the
      name changed and the number kept; it must pass, by the number.
- [ ] The `grep` finds `originatingCentreCode?: number | null;` in the type
      declarations, optional, so an older saved `MessageInfo` still
      type-checks.
- [ ] Record only, not a failure: note whether `promised type "fc"`
      reads clearly.

The other wgrib2 forms (`NP-ENS=0`, `ENS=? table4.6=255 pert=3`, `ENS=low-res
ctl`) and the probability types have no fixture here.
`a_wgrib2_member_with_the_type_missing_is_compared_as_wgrib2_prints_it`,
`an_ecmwf_event_probability_is_compared_by_type`,
`an_ecmwf_forecast_is_refused_on_ensemble_bytes`,
`an_ecmwf_analysis_is_refused_on_ensemble_bytes` and
`a_numbered_ecmwf_forecast_or_analysis_can_be_a_member`
(`crates/fieldglass/tests/verify_message_real_pairs.rs`), and
`an_ensemble_member_is_compared_with_the_messages` in
`crates/fieldglass/src/fetchplan.rs`, cover them. The node test
`crates/fieldglass-wasm/tests/node/fetchplan.mjs` runs the same check through
the built package in CI.

## 9. License and notice files ship in every package (#1545, #1546, #1563)

0.6.0's crates on crates.io and its npm package had no `LICENSE-MIT` or
`LICENSE-APACHE`. #1563 adds the colormap credits (turbo, © 2019 Google LLC,
Apache-2.0; ColorBrewer red–blue and brown–teal, with the line "This product
includes color specifications and designs developed by Cynthia Brewer
(https://colorbrewer.org/).") to the npm package's `NOTICE`, which until now
carried them only in the README. Skip the #1563 checks if it has not merged by
the prep SHA, and say so in the outcome.

**The dry-run npm tarball** (as in §8)

- [ ] `tar tzf fieldglass-wasm-X.Y.Z.tgz` lists `package/LICENSE-MIT`,
      `package/LICENSE-APACHE` and `package/NOTICE`. Failure looks like
      either license missing.
- [ ] `tar xzf fieldglass-wasm-X.Y.Z.tgz package/LICENSE-MIT && head -3
      package/LICENSE-MIT`: the MIT text with a copyright line, not a path
      such as `../../LICENSE-MIT` (which is what a symlink packed as text
      looks like).
- [ ] (#1563) `tar xzf fieldglass-wasm-X.Y.Z.tgz package/NOTICE`, then open
      `package/NOTICE`: it credits turbo to Google LLC under Apache-2.0, and
      carries the Cynthia Brewer sentence word for word, as the README has
      them.

**The crates** (from the repo root at the prep SHA)

- [ ] `python3 tools/check_crate_license_files.py` passes. It asks `cargo
      package --list` for every publishable crate, so it checks what cargo
      will upload. Failure prints the crate and the missing file.

After publishing, the crates.io page of any one crate (its "Files" view on
docs.rs, or `cargo download`) shows both license files. That is a
post-release check for the owner and is not part of this pass.

---

## 10. GRIB message rows show that they open and draw (#1628)

Nothing in the message table said that a row opens, and Render was only inside
an opened row. Each row that opens now has a chevron button and a pointer
cursor, and a Render button of its own. A message that can't be drawn says why
in the same place. One line above the table says what a click and Render do,
and it stays at the top as the table scrolls. The look is the owner's call, so
record what you see as well as pass or fail.

**`B: samples/0.6.1/rows/message_rows.grib2`**

- [ ] Above the table: `Click a row for its details. Render draws the
      message in a new tab, on a map projection where the grid allows one.`
      Rows 0 and 2 start with `▸` and have a Render button right after the
      message number, and so do rows 3 to 22, which are copies of row 0.
      Row 1 has no chevron and no button, and reads `Render not available:
      Bi-Fourier synthesis isn't supported, so these coefficients can't be
      drawn.` in that column, wrapped over a few lines rather than widening
      the table. Fail: the reason shows only after
      opening row 1, is cut short, or row 1 has a button.
- [ ] Hover row 0: the pointer is a hand and the row is highlighted. Hover
      row 1: it is highlighted, but the pointer is not a hand (an
      arrow, or a text cursor over text), and a click opens nothing.
- [ ] Click Render on row 0 without opening the row. A panel opens beside the
      table and draws the field, and the line under the hint reads `Opened
      render of message 0 in a new tab.` Row 0 stays closed. Fail: the row
      opens, or nothing is drawn.
- [ ] Click anywhere else on row 0: it opens to Export CSV and its note, and
      its chevron turns to `▾`. Click the chevron and it closes. Drag across
      the parameter name in row 0 to select it: the row does not open or
      close.
- [ ] Scroll the page down. The file has 23 rows, so the page is over 1,000
      px tall and scrolls in most editors; if it does not, make the editor
      shorter, for example by opening the bottom panel and dragging it up.
      The hint and the status line stay at the top over a solid background,
      and rows slide under them. Scroll until a row's chevron is partly
      hidden under the hint. Fail: the hint scrolls away, or rows show
      through it.
- [ ] Focus stays clear of the hint. Press Tab until a Render button in the
      middle of the page has focus (for example row 10's). Scroll down with
      the wheel, which leaves focus where it is, until the next row's
      chevron is partly hidden under the hint. Press Tab: focus moves to
      that chevron and the page moves so the chevron and its focus ring are
      in full view just below the hint. Pass: no part of the ring is under
      the hint. Fail: the ring is hidden or cut off.
- [ ] Keyboard only. Click the blank page above the table, then press Tab:
      focus goes to row 0's chevron, with a ring around it, then its Render
      button, then row 2's chevron and its Render, and passes row 1. It
      goes on through rows 3 to 22 the same way. Space on a focused chevron
      opens the row, and Enter closes it again. Enter on a focused Render
      draws the message, as a click does, and does not open the row. Fail: Tab misses a row or stops on row 1.
- [ ] (after #1658) In a closed row, double-click a value in one of its
      cells, such as the parameter name. The row stays closed and the word
      is selected, so it can be copied.
- [ ] (after #1658) Open row 0. Double-click a value in another row, such as
      row 2's parameter name. Row 0 stays open.
- [ ] (after #1658) With a screen reader on, Tab through the table. Each
      Render button is announced as "Render message N, button", where N is
      its row number.
- [ ] The first two checks again in Default Light Modern and Default High
      Contrast: the hover and focus colors are visible, and the Render button
      in an opened row is readable on its highlight.

---

## 11. What the reader left out, and why a time is a number (#1604, #1630)

Before, an attribute the reader could not read was dropped without a word, and
a time it would not date read as a bare number, which looks like a fault. Each
now says so on screen. The notes are one plain line of text: no "Show more",
nothing to open, and no byte offset, since the reader is told only that the
text is damaged.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc`** is
`netcdf4_string_attributes.nc` with the length in the heap ID of
`temperature`'s `long_name` and of the global `title` made one too large
(`NOTICE.md`), so the file opens without those two.

- [ ] Metadata page, Variables table. The `temperature` row lists its other
      attributes and then, under them, `Attribute "long_name" not read: the
      file's record of the text is damaged`. `long_name` is not among the
      attributes listed. No other row has a note: `time` lost nothing.
      Failure looks like `temperature` with no `long_name` and no note, which
      is what 0.6.0 shows, and a reader takes it for a file that never had
      one.
- [ ] Metadata page, Global attributes. The table still lists the globals that
      were read, `history` among them, with no `title`, and under it reads
      `Attribute "title" not read: the file's record of the text is damaged`.
      Failure looks like no line under the table, or the line saying
      something other than that the text is damaged.
- [ ] Render `temperature`. Beside the Variable picker, the same line as
      the table: `Attribute "long_name" not read: the file's record of the
      text is damaged`. The map draws as usual. Failure looks like the table
      having the note and the panel not.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_string_lists.nc`**
(its `time` has two calendars and `pr`'s `step` has two units)

- [ ] Render `tas`. At step 0 the `time` stepper reads `0 days since
      2000-01-01 (not shown as a date: its calendar holds several values)`,
      then the `(0–2)` range hint. Failure looks like a bare number or a
      date.
- [ ] Render `pr`. At step 0 the `step` stepper reads `0 "hours since
      2000-01-01", "days since 2000-01-01" (not shown as a date: its units
      hold several values)`, then `(0–1)`. Failure looks like a bare number
      or a date.
- [ ] Neither panel shows a `not read` line beside the picker, since no
      attribute was left out.

**`B: samples/0.6.1/calendar_shapes.zarr`** (*Fieldglass: Open Zarr Store…*
from the command palette, then pick the folder). Each `tas_*` field has a
4-step time axis of `0, 59, 360, 365` days since 2000-01-01, with a different
`calendar` in the store's JSON. A JSON value that is not one string is not a
calendar, so the stepper shows the number and says what the value is. Before
#1630 the first three read `its calendar holds several values`, which is
false for all but a list of two or more. Step to index 2 on each, where the standard
calendar would read 2000-12-26. Pick each `tas_*` from the Variable picker;
the range hint `(0–3)` follows the note.

- [ ] `tas_null` (`"calendar": null`): `360 days since 2000-01-01 (not shown
      as a date: its calendar is not text)`. Failure looks like `holds several
      values`, a date, or a number with no note.
- [ ] `tas_true` (`"calendar": true`): `360 days since 2000-01-01 (not shown
      as a date: its calendar is not text)`. Failure looks like `holds several
      values`, a date, or a number with no note.
- [ ] `tas_object` (`"calendar": {"a": 1}`): `360 days since 2000-01-01 (not
      shown as a date: its calendar is not text)`. Failure looks like `holds
      several values`, a date, or a number with no note.
- [ ] `tas_one` (`"calendar": ["noleap"]`): `360 days since 2000-01-01`, with
      no note. A list of one string is that string, as in NetCDF, so this is
      the noleap calendar and the number is right for it. Failure looks like
      any note, or `2000-12-26`.
- [ ] `tas_number` (`"calendar": 5`): `2000-12-26` and no note. A number names
      no calendar, so the time is dated as the standard one and a note would
      contradict the date. Failure looks like a note, or a number.

What these notes say for every combination of `units` and `calendar` (two
units, two units and a null calendar, and so on) is held by
`a_time_not_dated_for_a_list_says_why` and
`a_zarr_time_not_dated_says_what_its_attribute_is`
(`crates/fieldglass/tests/axis_values.rs`), which `tas_list` in §2 and the
store above sample by eye. The words reaching the wire as `notDated` are held
by `the_handle_says_what_was_left_out_and_why_a_time_is_not_dated`
(`crates/fieldglass-napi/src/handle.rs`), and the page text by
`extension/src/test/suite/left-out-notes.test.ts`.

---

## No manual check

`[Unreleased]` entries, or parts of them, with no check above, and why.

- A dimension reference in a global heap with a repeated number fails the
  file (#1557). Building one by hand would mean rewriting a `DIMENSION_LIST`
  reference; `a_dimension_list_index_two_objects_carry_is_refused` and
  `a_dimension_list_count_that_differs_from_its_object_is_refused`
  (`crates/fieldglass-netcdf/tests/hdf5_dimension_scales.rs`) cover it.
- The global-heap object budget, a limit on how many heap objects one file
  makes Fieldglass walk (#1562). It takes a file with millions of empty heap
  objects to reach. `a_calendar_past_the_object_budget_is_reported` and
  `a_calendar_within_the_object_budget_reads`
  (`crates/fieldglass/tests/string_attribute_budget.rs`) and
  `the_object_budget_is_shared_across_collections`
  (`crates/fieldglass-netcdf/src/hdf5/global_heap.rs`) cover it.
- `Line.coordinateCalendar` on the wire (#1548). The extension's line plot
  uses it (§2), but the conformance suite does not observe it yet, nor
  `axisValues` on a 360-day axis (#1575).
  `axis_values.rs` covers the Rust side.
- `fieldglass_grib1::forecast_hours` returning `None` for an unnamed time
  range or a zero step (#876, #1554). The panel never shows hours; the
  library change is held by the GRIB1 tests named in §7.
- The wgrib2 forms of #1304 that no committed fixture carries; see the end of
  §8.

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
