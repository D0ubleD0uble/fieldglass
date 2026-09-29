#!/usr/bin/env python3
"""Write the gated tier's inputs into a directory, with the facts each bound needs.

    python3 crates/fieldglass-perf/corpus/generate.py <out-dir>

Nothing here is committed. `run.sh` calls this into a temporary directory and
deletes it afterwards, so the corpus is whatever these lines produce: a change to
a generator shows up as a change to the gated numbers, and the digest printed at
the end (and recorded in `docs/performance.md`) says so before anyone goes
looking for a regression in the reader.

**Sizes.** Every scaling bound needs the same input at two sizes, so each case is
written twice or three times:

* `S`, a 2 degree grid (180 x 91), eight time steps;
* `L`, a 1 degree grid (360 x 181), eight time steps — four times the cells;
* `D` (array formats only), the `S` grid with 32 time steps — four times the
  variable with the same plane, which is what separates "work per cell touched"
  from "work per cell stored".

**Facts, not readings.** `corpus.json` records, for each input, the byte extents
a bound is written in terms of: a GRIB message's length and its data section, a
NetCDF plane's bytes, a Zarr plane's chunk objects. They come from the writer
(eccodes keys, `h5py` chunk records, the layout the classic format fixes) and
never from the reader being measured, which would let a bound agree with a
defect.

**Determinism.** Values are closed-form, never random, and no writer here stamps
a time: gzip does, which is why no case uses it. The versions are pinned in
`requirements.txt` and asserted below, because a different encoder writes
different bytes and every gated number would move with them.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import sys
from importlib.metadata import version
from pathlib import Path

import eccodes
import h5py
import netCDF4
import numcodecs
import numpy as np
import zarr
from zarr.codecs import BloscCodec, ZstdCodec

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
REQUIREMENTS = HERE.parent / "requirements.txt"

# (label, ni, nj, time steps). Longitudes run 0..360 exclusive, latitudes 90..-90
# inclusive, so both grids are global and a warp covers the whole field.
GRIDS = {"S": (180, 91, 8), "L": (360, 181, 8), "D": (180, 91, 32)}

# Only the arrays need `D`: a message holds one field, so there is no "rest of
# the variable" for a slice to avoid.
MESSAGE_SIZES = ("S", "L")
ARRAY_SIZES = ("S", "L", "D")

GRIB2_PACKINGS = {
    "5.0": "grid_simple",
    "5.3": "grid_complex_spatial_differencing",
    "5.40": "grid_jpeg",
    "5.41": "grid_png",
    "5.42": "grid_ccsds",
}
GRIB1_PACKINGS = {"simple": "grid_simple", "second-order": "grid_second_order"}

# Spectral inputs are sized by truncation, not by grid: `Session::decode`
# synthesises every spectral field onto the same 0.5 degree grid, so the cells
# are constant and the coefficients are what grows.
SPECTRAL_TRUNCATIONS = {"S": 63, "L": 127}
SPECTRAL_PACKINGS = {"spectral-simple": "spectral_simple", "spectral-complex": "spectral_complex"}

# Template 5.200 has no encoder in eccodes (it decodes it and refuses to write
# it), so the one committed fixture stands in at a single size. Its allocation
# row therefore has no second size to be compared against, and the doc says so.
RUNLENGTH_FIXTURE = REPO / "crates/fieldglass-grib2/tests/fixtures/runlength_regular_latlon.grib2"


def check_versions() -> dict[str, str]:
    """The pinned writer versions, or exit naming the one that differs."""
    pins = {}
    for line in REQUIREMENTS.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            name, _, pin = line.partition("==")
            pins[name.strip()] = pin.strip()
    wrong = [f"{name} {version(name)} (pinned {pin})" for name, pin in pins.items() if version(name) != pin]
    if wrong:
        sys.exit(
            "the corpus writers are not the pinned versions, so the gated numbers "
            "would describe different bytes:\n  " + "\n  ".join(wrong) +
            f"\ninstall them with: python3 -m pip install -r {REQUIREMENTS.relative_to(REPO)}"
        )
    return pins


def field(ni: int, nj: int, t: int = 0) -> np.ndarray:
    """A smooth, non-constant field in kelvin, `nj` rows by `ni` columns.

    Smooth so the compressing packings compress as they would on a real model
    field, and varying in `t` so no two planes of a variable are the same bytes.
    """
    j, i = np.meshgrid(np.arange(nj), np.arange(ni), indexing="ij")
    return (
        280.0
        + 15.0 * np.sin(i * 2.0 * np.pi / ni + 0.3 * t) * np.cos(j * np.pi / (nj - 1))
        + 0.01 * i
        + 0.5 * t
    )


def grib_message(sample: str, packing: str, ni: int, nj: int, *, grib2: bool) -> tuple[bytes, dict]:
    """One global regular lat/lon message and its extents."""
    h = eccodes.codes_grib_new_from_samples(sample)
    try:
        inc = 360.0 / ni
        eccodes.codes_set(h, "Ni", ni)
        eccodes.codes_set(h, "Nj", nj)
        eccodes.codes_set(h, "latitudeOfFirstGridPointInDegrees", 90.0)
        eccodes.codes_set(h, "longitudeOfFirstGridPointInDegrees", 0.0)
        eccodes.codes_set(h, "latitudeOfLastGridPointInDegrees", -90.0)
        eccodes.codes_set(h, "longitudeOfLastGridPointInDegrees", 360.0 - inc)
        eccodes.codes_set(h, "iDirectionIncrementInDegrees", inc)
        eccodes.codes_set(h, "jDirectionIncrementInDegrees", 180.0 / (nj - 1))
        eccodes.codes_set(h, "bitsPerValue", 16)
        # Values first, then the packing: eccodes repacks the values it holds,
        # and setting the packing on an empty message falls back to simple.
        eccodes.codes_set_values(h, field(ni, nj).ravel())
        eccodes.codes_set(h, "packingType", packing)
        if eccodes.codes_get(h, "packingType") != packing:
            sys.exit(f"eccodes wrote {eccodes.codes_get(h, 'packingType')} when asked for {packing}")
        message = eccodes.codes_get_message(h)
        # The data section: section 7 in GRIB2, the binary data section (4) in
        # GRIB1.
        section = 7 if grib2 else 4
        facts = {
            "message_length": len(message),
            "data_offset": eccodes.codes_get(h, f"offsetSection{section}"),
            "data_length": eccodes.codes_get(h, f"section{section}Length"),
        }
        if packing == "grid_ccsds":
            # What the codec-alone scenario hands `rust_aec`, read from §5 by
            # the writer rather than parsed back out by the reader.
            facts["aec"] = {
                "bits_per_sample": eccodes.codes_get(h, "bitsPerValue"),
                "block_size": eccodes.codes_get(h, "ccsdsBlockSize"),
                "rsi": eccodes.codes_get(h, "ccsdsRsi"),
                "flags": eccodes.codes_get(h, "ccsdsFlags"),
            }
        return message, facts
    finally:
        eccodes.codes_release(h)


def spectral_message(packing: str, truncation: int) -> tuple[bytes, dict]:
    h = eccodes.codes_grib_new_from_samples("sh_sfc_grib1")
    try:
        eccodes.codes_set(h, "packingType", packing)
        for key in ("J", "K", "M"):
            eccodes.codes_set(h, key, truncation)
        m = np.arange((truncation + 1) * (truncation + 2), dtype=float)
        eccodes.codes_set_values(h, np.exp(-m / 200.0) * np.cos(m))
        message = eccodes.codes_get_message(h)
        return message, {
            "message_length": len(message),
            "data_offset": eccodes.codes_get(h, "offsetSection4"),
            "data_length": eccodes.codes_get(h, "section4Length"),
            "truncation": truncation,
        }
    finally:
        eccodes.codes_release(h)


def write_netcdf(path: Path, fmt: str, ni: int, nj: int, nt: int, **var_kwargs) -> None:
    with netCDF4.Dataset(path, "w", format=fmt) as ds:
        ds.createDimension("time", nt)
        ds.createDimension("lat", nj)
        ds.createDimension("lon", ni)
        time = ds.createVariable("time", "f8", ("time",))
        time.units = "hours since 2020-01-01 00:00:00"
        time[:] = np.arange(nt)
        lat = ds.createVariable("lat", "f4", ("lat",))
        lat.units = "degrees_north"
        lat[:] = np.linspace(90.0, -90.0, nj)
        lon = ds.createVariable("lon", "f4", ("lon",))
        lon.units = "degrees_east"
        lon[:] = np.arange(ni) * (360.0 / ni)
        # Last, so in a classic file its data is the file's tail: see
        # `classic_facts`.
        t = ds.createVariable("t", "f4", ("time", "lat", "lon"), **var_kwargs)
        t.units = "K"
        for step in range(nt):
            t[step] = field(ni, nj, step)


def classic_facts(path: Path, ni: int, nj: int, nt: int) -> dict:
    """Plane extents from the layout the classic format fixes.

    Fixed-size variables are laid out in definition order with no gaps, and `t`
    is defined last with a four-byte element, so its data ends the file and
    needs no padding: plane `k` is `size - (nt - k) * plane` long bytes in.
    """
    size = path.stat().st_size
    plane = ni * nj * 4
    begin = size - nt * plane
    return {"planes": [[[begin + k * plane, plane]] for k in range(nt)]}


def hdf5_facts(path: Path) -> dict:
    """Each plane's chunk, as HDF5 itself records it."""
    with h5py.File(path, "r") as f:
        ds = f["t"]
        if ds.chunks is None or ds.chunks[0] != 1:
            sys.exit(f"{path}: `t` must be chunked one time step per chunk, got {ds.chunks}")
        planes = [[] for _ in range(ds.shape[0])]
        for i in range(ds.id.get_num_chunks()):
            info = ds.id.get_chunk_info(i)
            planes[info.chunk_offset[0]].append([info.byte_offset, info.size])
    return {"planes": planes}


def write_zarr(root: Path, zarr_format: int, ni: int, nj: int, nt: int, **array_kwargs) -> None:
    group = zarr.open_group(str(root), mode="w", zarr_format=zarr_format)

    def array(name, data, dim_names, **kwargs):
        if zarr_format == 3:
            kwargs["dimension_names"] = dim_names
        a = group.create_array(name, shape=data.shape, dtype=data.dtype, **kwargs)
        a[...] = data
        if zarr_format == 2:
            a.attrs["_ARRAY_DIMENSIONS"] = list(dim_names)
        return a

    array("lat", np.linspace(90.0, -90.0, nj).astype("<f4"), ["lat"], chunks=(nj,)).attrs["units"] = "degrees_north"
    array("lon", (np.arange(ni) * (360.0 / ni)).astype("<f4"), ["lon"], chunks=(ni,)).attrs["units"] = "degrees_east"
    t = array(
        "t",
        np.stack([field(ni, nj, k) for k in range(nt)]).astype("<f4"),
        ["time", "lat", "lon"],
        **array_kwargs,
    )
    t.attrs["units"] = "K"


def zarr_facts(root: Path, zarr_format: int, nt: int, shard: int | None) -> dict:
    """Each plane's objects: its chunk, or the shard holding it."""
    planes = []
    for k in range(nt):
        index = k if shard is None else k // shard
        key = f"t/{index}.0.0" if zarr_format == 2 else f"t/c/{index}/0/0"
        if not (root / key).is_file():
            sys.exit(f"{root}: expected the object for plane {k} at {key}")
        planes.append([key])
    objects = sorted(p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file())
    sizes = {key: (root / key).stat().st_size for key in objects}
    return {"planes": planes, "objects": sizes}


def digest(paths: list[Path], root: Path) -> str:
    h = hashlib.sha256()
    for path in sorted(paths):
        h.update(path.relative_to(root).as_posix().encode())
        h.update(b"\0")
        h.update(path.read_bytes())
    return h.hexdigest()


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit(__doc__.strip().splitlines()[0])
    out = Path(sys.argv[1])
    pins = check_versions()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    inputs: dict[str, dict] = {}

    def message_input(name, fmt, message, facts, ni, nj):
        file = f"{name}.{'grib2' if fmt == 'grib2' else 'grib1'}"
        (out / file).write_bytes(message)
        inputs[name] = {"file": file, "format": fmt, "ni": ni, "nj": nj, **facts}

    for label, packing in GRIB2_PACKINGS.items():
        for size in MESSAGE_SIZES:
            ni, nj, _ = GRIDS[size]
            message, facts = grib_message("regular_ll_sfc_grib2", packing, ni, nj, grib2=True)
            message_input(f"grib2-{label}-{size}", "grib2", message, facts, ni, nj)
    for label, packing in GRIB1_PACKINGS.items():
        for size in MESSAGE_SIZES:
            ni, nj, _ = GRIDS[size]
            message, facts = grib_message("regular_ll_sfc_grib1", packing, ni, nj, grib2=False)
            message_input(f"grib1-{label}-{size}", "grib1", message, facts, ni, nj)
    for label, packing in SPECTRAL_PACKINGS.items():
        for size, truncation in SPECTRAL_TRUNCATIONS.items():
            message, facts = spectral_message(packing, truncation)
            message_input(f"grib1-{label}-{size}", "grib1", message, facts, 720, 361)

    with RUNLENGTH_FIXTURE.open("rb") as f:
        h = eccodes.codes_grib_new_from_file(f)
    message = RUNLENGTH_FIXTURE.read_bytes()
    message_input(
        "grib2-5.200-S",
        "grib2",
        message,
        {
            "message_length": len(message),
            "data_offset": eccodes.codes_get(h, "offsetSection7"),
            "data_length": eccodes.codes_get(h, "section7Length"),
        },
        eccodes.codes_get(h, "Ni"),
        eccodes.codes_get(h, "Nj"),
    )
    eccodes.codes_release(h)

    for size in ARRAY_SIZES:
        ni, nj, nt = GRIDS[size]
        common = {"ni": ni, "nj": nj, "nt": nt, "variable": "t"}

        name = f"netcdf-classic-{size}"
        path = out / f"{name}.nc"
        write_netcdf(path, "NETCDF3_CLASSIC", ni, nj, nt)
        inputs[name] = {"file": path.name, "format": "netcdf", **common, **classic_facts(path, ni, nj, nt)}

        name = f"netcdf4-zlib-{size}"
        path = out / f"{name}.nc"
        write_netcdf(
            path, "NETCDF4", ni, nj, nt, zlib=True, complevel=4, shuffle=True, chunksizes=(1, nj, ni)
        )
        inputs[name] = {"file": path.name, "format": "netcdf", **common, **hdf5_facts(path)}

        name = f"zarr-v2-blosc-{size}"
        write_zarr(
            out / name, 2, ni, nj, nt, chunks=(1, nj, ni),
            compressors=numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE),
        )
        inputs[name] = {"dir": name, "format": "zarr", **common, **zarr_facts(out / name, 2, nt, None)}

        name = f"zarr-v3-zstd-{size}"
        write_zarr(out / name, 3, ni, nj, nt, chunks=(1, nj, ni), compressors=[ZstdCodec(level=3)])
        inputs[name] = {"dir": name, "format": "zarr", **common, **zarr_facts(out / name, 3, nt, None)}

        name = f"zarr-v3-sharded-{size}"
        write_zarr(
            out / name, 3, ni, nj, nt, chunks=(1, nj, ni), shards=(4, nj, ni),
            compressors=[BloscCodec(cname="zstd", clevel=3, shuffle="shuffle")],
        )
        inputs[name] = {"dir": name, "format": "zarr", **common, **zarr_facts(out / name, 3, nt, 4)}

    files = [p for p in out.rglob("*") if p.is_file()]
    corpus = {"pins": pins, "digest": digest(files, out), "inputs": inputs}
    (out / "corpus.json").write_text(json.dumps(corpus, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(corpus["digest"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
