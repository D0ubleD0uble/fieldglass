#!/usr/bin/env python3
"""Write `era5.json`: the real-data manifest, pinned by hash.

    python3 crates/fieldglass-perf/manifests/build_era5.py

Run once, by a maintainer, when the manifest should change. It reads the
public ARCO-ERA5 bucket, finds the objects and byte ranges the report tier needs,
downloads each once to hash it, and writes the manifest. The downloads are not
kept: `tools/fetch_perf_data.py` fetches into the cache, and verifies against the
hashes written here.

**One field, three containers.** 2 m temperature, 2020-01-01 00Z–11Z, twelve
hourly frames, as the bucket holds it three ways:

* **Zarr** — `ar/full_37-1h-0p25deg-chunk-1.zarr-v3`. Despite the name this is a
  Zarr **v2** store (`.zarray`, blosc lz4), one time step per chunk, 0.25°.
  Its metadata is rewritten daily as ERA5T is appended, so the array's own
  `.zarray` and the store's `.zmetadata` cannot be pinned by hash. The manifest
  therefore carries a small *authored* subset store instead: the variable's
  `.zarray` with its shape cut to the twelve frames and the chunk keys renumbered
  from 0, a twelve-value `time` array, and `.zgroup`. The chunks and the
  coordinate arrays are the bucket's own bytes, fetched and hashed.
* **NetCDF classic** — `raw/date-variable-single_level/2020/01/01/2m_temperature/surface.nc`,
  the whole day's file (24 frames, packed `int16`). The NetCDF reader takes the
  whole file, so the whole file is what a host must fetch, and the manifest
  says so rather than pretending otherwise.
* **GRIB** — `raw/ERA5GRIB/HRES/Month/2020/202001_hres_sfc.grb2`. Despite the
  extension these are GRIB **edition 1** messages, on the native N320 reduced
  Gaussian grid rather than 0.25°, padded to multiples of 120 bytes. The month
  file is 11.7 GB with no sidecar index, so this walks its message headers (a
  64-byte range per message) to find parameter 167 (`2t`) for the twelve hours,
  and pins each message's range. The same variable and hours, a different grid:
  the cross-format comparison is of the same work, not the same cells.

The time index is read from the store's own `time` array rather than computed
from a calendar: a hand-computed index was off by 43 years the first time.

ERA5 is Copernicus Climate Change Service information, used under the
Copernicus licence. No ERA5 bytes are committed; this writes URLs and hashes.
"""

from __future__ import annotations

import base64
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

import numcodecs
import numpy as np

HERE = Path(__file__).resolve().parent
OUT = HERE / "era5.json"

BUCKET = "https://storage.googleapis.com/gcp-public-data-arco-era5/"
ZARR = BUCKET + "ar/full_37-1h-0p25deg-chunk-1.zarr-v3/"
NETCDF = BUCKET + "raw/date-variable-single_level/2020/01/01/2m_temperature/surface.nc"
GRIB = BUCKET + "raw/ERA5GRIB/HRES/Month/2020/202001_hres_sfc.grb2"

VARIABLE = "2m_temperature"
FRAMES = 12
# 2020-01-01T00:00, in the store's own units ("hours since 1900-01-01").
START_HOURS = 1051896
# ECMWF GRIB1 table 128, parameter 167: 2 metre temperature.
GRIB_PARAM = (128, 167)

# The largest share of the cache the report tier may need; `fetch_perf_data.py`
# refuses a manifest larger than its cap rather than evicting what it just fetched.
CACHE_CAP_BYTES = 256 * 1024 * 1024


def get(url: str, start: int | None = None, length: int | None = None) -> bytes:
    request = urllib.request.Request(url)
    if start is not None:
        request.add_header("Range", f"bytes={start}-{start + length - 1}")
    with urllib.request.urlopen(request) as response:  # the bucket's fixed URLs above
        data = response.read()
    if length is not None and len(data) != length:
        sys.exit(f"{url} [{start}+{length}]: got {len(data)} bytes")
    return data


def size_of(url: str) -> int:
    request = urllib.request.Request(url, method="HEAD")
    with urllib.request.urlopen(request) as response:
        return int(response.headers["Content-Length"])


class Objects:
    """The manifest's object table, keyed by content hash."""

    def __init__(self) -> None:
        self.table: dict[str, dict] = {}

    def add(self, url: str, start: int | None = None, length: int | None = None) -> str:
        data = get(url, start, length)
        digest = hashlib.sha256(data).hexdigest()
        entry = {"url": url, "length": len(data)}
        if start is not None:
            entry["offset"] = start
        self.table[digest] = entry
        print(f"  {len(data):>10,}  {url.removeprefix(BUCKET)}" + (f" @{start}" if start is not None else ""))
        return digest


def zarr_subset(objects: Objects) -> dict:
    zarray = json.loads(get(ZARR + f"{VARIABLE}/.zarray"))
    time_zarray = json.loads(get(ZARR + "time/.zarray"))
    per_chunk = time_zarray["chunks"][0]
    chunk = START_HOURS // per_chunk
    compressor = time_zarray["compressor"]
    times = np.frombuffer(numcodecs.get_codec(compressor).decode(get(ZARR + f"time/{chunk}")), dtype="<i8")
    hits = np.nonzero(times == START_HOURS)[0]
    if len(hits) != 1:
        sys.exit(f"time chunk {chunk} does not hold {START_HOURS} exactly once")
    first = chunk * per_chunk + int(hits[0])
    print(f"zarr: 2020-01-01T00 is time index {first}")

    if zarray["chunks"][0] != 1 or zarray["zarr_format"] != 2:
        sys.exit(f"the store's layout changed: {zarray}")
    zarray["shape"][0] = FRAMES
    keys = {f"{VARIABLE}/{k}.0.0": objects.add(ZARR + f"{VARIABLE}/{first + k}.0.0") for k in range(FRAMES)}
    for coordinate in ("latitude", "longitude"):
        for key in (".zarray", ".zattrs", "0"):
            keys[f"{coordinate}/{key}"] = objects.add(ZARR + f"{coordinate}/{key}")
    keys[f"{VARIABLE}/.zattrs"] = objects.add(ZARR + f"{VARIABLE}/.zattrs")

    time_values = np.arange(START_HOURS, START_HOURS + FRAMES, dtype="<i8")
    return {
        "variable": VARIABLE,
        "frames": FRAMES,
        "first_time_index": first,
        "objects": keys,
        "documents": {
            ".zgroup": {"zarr_format": 2},
            f"{VARIABLE}/.zarray": zarray,
            "time/.zarray": {
                "chunks": [FRAMES], "compressor": None, "dtype": "<i8", "fill_value": None,
                "filters": None, "order": "C", "shape": [FRAMES], "zarr_format": 2,
            },
            "time/.zattrs": {
                "_ARRAY_DIMENSIONS": ["time"], "calendar": "proleptic_gregorian",
                "units": "hours since 1900-01-01 00:00:00",
            },
        },
        "binary": {"time/0": base64.b64encode(time_values.tobytes()).decode()},
    }


def grib_messages(objects: Objects) -> dict:
    size = size_of(GRIB)
    found: dict[int, tuple[int, int]] = {}
    offset = 0
    walked = 0
    while len(found) < FRAMES:
        head = get(GRIB, offset, 64)
        if head[:4] != b"GRIB" or head[7] != 1:
            sys.exit(f"{GRIB} @{offset}: not a GRIB1 message start")
        length = int.from_bytes(head[4:7], "big")
        pds = head[8:]
        table, param, level_type = pds[3], pds[8], pds[9]
        year = (pds[24] - 1) * 100 + pds[12]
        month, day, hour = pds[13], pds[14], pds[15]
        if (table, param) == GRIB_PARAM and level_type == 1 and (year, month, day) == (2020, 1, 1) and hour < FRAMES:
            found[hour] = (offset, length)
        # ECMWF pads each message to a multiple of 120 bytes.
        offset += -(-length // 120) * 120
        walked += 1
        if walked > 2000:
            sys.exit("walked 2000 messages without finding twelve hours of 2t")
    print(f"grib: walked {walked} message headers")
    return {
        "size": size,
        "messages": [
            {"hour": hour, "offset": start, "object": objects.add(GRIB, start, length)}
            for hour, (start, length) in sorted(found.items())
        ],
    }


def main() -> int:
    objects = Objects()
    zarr = zarr_subset(objects)
    grib = grib_messages(objects)
    netcdf = {"variable": "t2m", "frames": FRAMES, "object": objects.add(NETCDF)}
    total = sum(entry["length"] for entry in objects.table.values())
    if total > CACHE_CAP_BYTES:
        sys.exit(f"the manifest needs {total:,} bytes, over its own cache cap of {CACHE_CAP_BYTES:,}")
    manifest = {
        "schema": 1,
        "name": "era5-2t-2020-01-01",
        "attribution": "Contains modified Copernicus Climate Change Service information (ERA5), via ARCO-ERA5 on Google Cloud Public Datasets; Copernicus licence",
        "cache_cap_bytes": CACHE_CAP_BYTES,
        "total_bytes": total,
        "objects": dict(sorted(objects.table.items())),
        "zarr": zarr,
        "netcdf": netcdf,
        "grib1": grib,
    }
    OUT.write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT.name}: {len(objects.table)} objects, {total:,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
