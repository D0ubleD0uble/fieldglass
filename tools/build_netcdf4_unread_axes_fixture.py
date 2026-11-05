#!/usr/bin/env python3
"""Build the unread-axis-attribute fixture (#1687).

``netcdf4_unread_axes.nc`` is written with ``h5py``, the h5netcdf layout
(dimension scales, every ``str`` attribute a variable-length string), and then
damaged in two ways the reader keeps the file through:

  * the projected coordinates ``y`` and ``x``, in metres, lose their
    ``units`` and ``standard_name``: the length in each of those four
    global-heap IDs is one more than its object holds, which the reader
    refuses (#1569). Read by their names alone, ``y`` and ``x`` are latitude
    and longitude;
  * two time axes have an attribute stored as an array of two fixed-length
    strings, declared three long, so the data is short: ``time``'s ``units``
    and ``step``'s ``calendar``. The reader keeps each as its first element,
    as 0.6.0 read it, and names it as read in part (#1689).

``tas(time, y, x)`` and ``pr(step, y, x)`` are drawn over them. Nothing else
in the file is damaged.

Run from the repo root (needs ``h5py`` and ``numpy``), under a memory limit as
every HDF5 fixture builder is:

    ( ulimit -v 8000000; python3 tools/build_netcdf4_unread_axes_fixture.py )
"""

from __future__ import annotations

import struct
from pathlib import Path

import h5py
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf4_unread_axes.nc"

TIME_VALUES = [0.0, 365.0]
STEP_VALUES = [0.0, 6.0]
Y_VALUES = [-1000.0, 1000.0]
X_VALUES = [-2000.0, 0.0, 2000.0]

# The strings whose heap IDs are damaged: `y` and `x`'s units and standard
# names, each its own object.
DAMAGED = [b"m", b"projection_y_coordinate", b"projection_x_coordinate"]

# Each attribute stored as two fixed-length strings, declared three long:
# (dataset, attribute, the two strings).
SHORT = [
    ("time", "units", [b"days since 2000-01-01", b"days since 2000-01-01"]),
    ("step", "calendar", [b"standard", b"julian"]),
]


def build(path: Path) -> None:
    # The fixed libver keeps the file stable across h5py releases, and gives
    # version-1 object headers, which carry no checksum to patch.
    with h5py.File(path, "w", libver=("earliest", "v108")) as f:
        f.attrs["title"] = "fieldglass unread axis attributes"
        axes = {}
        for name, values in (
            ("time", TIME_VALUES),
            ("step", STEP_VALUES),
            ("y", Y_VALUES),
            ("x", X_VALUES),
        ):
            axis = f.create_dataset(name, data=np.array(values))
            axis.make_scale(name)
            axes[name] = axis
        for dataset, attribute, strings in SHORT:
            axes[dataset].attrs[attribute] = np.array(strings)
        axes["step"].attrs["units"] = "hours since 2000-01-01"
        for name in ("y", "x"):
            axes[name].attrs["units"] = "m"
            axes[name].attrs["standard_name"] = f"projection_{name}_coordinate"
        shape = (len(Y_VALUES), len(X_VALUES))
        base = np.arange(np.prod(shape), dtype="f4").reshape(shape)
        for name, first, units in (("tas", "time", "K"), ("pr", "step", "mm")):
            v = f.create_dataset(name, data=np.stack([base + 280, base + 281]))
            v.attrs["units"] = units
            for i, dim in enumerate((first, "y", "x")):
                v.dims[i].attach_scale(axes[dim])


def collections(data: bytes) -> list[tuple[int, dict[int, bytes]]]:
    """Every global-heap collection: its address and its objects by index."""
    found = []
    at = data.find(b"GCOL")
    while at != -1:
        assert data[at + 4] == 1, "a version-1 collection"
        end = at + struct.unpack_from("<Q", data, at + 8)[0]
        objects = {}
        o = at + 16
        while o + 16 <= end:
            index, _, _, size = struct.unpack_from("<HHIQ", data, o)
            if index == 0:
                break
            objects[index] = data[o + 16 : o + 16 + size]
            o += 16 + (size + 7) // 8 * 8
        found.append((at, objects))
        at = data.find(b"GCOL", end)
    return found


def damage(data: bytearray) -> None:
    """Point each heap ID of a `DAMAGED` string one byte past its object."""
    patched = 0
    for address, objects in collections(bytes(data)):
        for index, text in objects.items():
            if text not in DAMAGED:
                continue
            heap_id = struct.pack("<IQI", len(text), address, index)
            at = data.find(heap_id)
            assert at != -1 and data.find(heap_id, at + 1) == -1, text
            data[at : at + 16] = struct.pack("<IQI", len(text) + 1, address, index)
            patched += 1
    assert patched == 4, f"{patched} heap IDs patched"


def pad8(n: int) -> int:
    return (n + 7) // 8 * 8


def shorten(data: bytearray) -> None:
    """Declare each `SHORT` array three long, current and maximum size."""
    for _, attribute, strings in SHORT:
        name = attribute.encode() + b"\0"
        body = bytes(strings[0]) + bytes(strings[1])
        matches = []
        at = data.find(name)
        while at != -1:
            start = at - 8
            version, _, name_size, type_size, space_size = struct.unpack_from(
                "<BBHHH", data, start
            )
            if version == 1 and name_size == len(name):
                space = start + 8 + pad8(name_size) + pad8(type_size)
                value = space + pad8(space_size)
                if bytes(data[value : value + len(body)]) == body:
                    matches.append(space)
            at = data.find(name, at + 1)
        assert len(matches) == 1, (attribute, matches)
        space = matches[0]
        space_version, rank, flags = struct.unpack_from("<BBB", data, space)
        assert (space_version, rank) == (1, 1), (space_version, rank)
        dims = space + 8
        assert struct.unpack_from("<Q", data, dims)[0] == 2
        struct.pack_into("<Q", data, dims, 3)
        if flags & 1:
            assert struct.unpack_from("<Q", data, dims + 8)[0] == 2
            struct.pack_into("<Q", data, dims + 8, 3)


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    path = FIXTURES_DIR / NAME
    build(path)
    data = bytearray(path.read_bytes())
    damage(data)
    shorten(data)
    path.write_bytes(bytes(data))
    print(f"wrote {path} ({len(data)} bytes), h5py {h5py.__version__} "
          f"(HDF5 {h5py.version.hdf5_version})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
