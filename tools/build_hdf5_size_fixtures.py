#!/usr/bin/env python3
"""Build the HDF5 fixtures behind #922: earliest-format files whose Size of
Offsets and Size of Lengths differ.

A superblock states the two sizes separately. libhdf5 writes a symbol-table
entry's link-name offset and a group B-tree key at Size of Lengths, so a reader
that takes them at Size of Offsets misplaces the root group and every member
after the first.

  * ``hdf5_sizes_o8_l4.h5`` — ``fcpl.set_sizes(8, 4)``
  * ``hdf5_sizes_o4_l8.h5`` — ``fcpl.set_sizes(4, 8)``

Both use ``set_libver_bounds(EARLIEST, V18)``: without explicit bounds h5py's
``set_sizes`` writes a version-2 superblock and no version-1 B-tree. The
builder checks the superblock version byte is 0.

Each root holds enough members to split the group across several symbol-table
nodes, a dataset with enough attributes to need a header continuation, an
unlimited dimension, dimension scales (whose ``DIMENSION_LIST`` lives in the
global heap), a chunked gzip dataset and a nested group. The oracle records
what h5py reads back.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_size_fixtures.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")


def write(path: Path, offsets: int, lengths: int) -> None:
    fapl = h5py.h5p.create(h5py.h5p.FILE_ACCESS)
    fapl.set_libver_bounds(h5py.h5f.LIBVER_EARLIEST, h5py.h5f.LIBVER_V18)
    fcpl = h5py.h5p.create(h5py.h5p.FILE_CREATE)
    fcpl.set_sizes(offsets, lengths)
    fid = h5py.h5f.create(str(path).encode(), h5py.h5f.ACC_TRUNC, fapl=fapl, fcpl=fcpl)
    with h5py.File(fid) as f:
        f.attrs["scale"] = np.float64(1.5)
        f.attrs["title"] = np.bytes_(b"sizes")

        x = f.create_dataset("x", data=np.arange(4, dtype="f4"), track_times=False)
        x.make_scale("x")
        time = f.create_dataset(
            "time",
            data=np.arange(3, dtype="f8"),
            maxshape=(None,),
            chunks=(2,),
            track_times=False,
        )
        time.make_scale("time")
        v = f.create_dataset(
            "v",
            data=np.arange(12, dtype="f8").reshape(3, 4) / 4,
            maxshape=(None, 4),
            chunks=(2, 2),
            compression="gzip",
            track_times=False,
        )
        v.dims[0].attach_scale(time)
        v.dims[1].attach_scale(x)
        v.attrs["units"] = np.bytes_(b"K")

        # Created before the members that follow it, and given its attributes
        # last, so its header cannot grow in place and needs a continuation.
        many = f.create_dataset("many_attrs", data=np.arange(3, dtype="u1"), track_times=False)
        f.create_dataset("contig", data=np.arange(-3, 3, dtype="i2").reshape(2, 3), track_times=False)
        # Ten more members: a symbol-table node holds 2K = 8 entries by default.
        for i in range(10):
            f.create_dataset(f"d{i:02}", data=np.full(2, i, dtype="i4"), track_times=False)

        g = f.create_group("g")
        g.attrs["level"] = np.int32(7)
        g.create_dataset("inner", data=np.arange(5, dtype="i4") * 3, track_times=False)

        for i in range(30):
            many.attrs[f"a{i:02}"] = np.float32(i) / 2

    header = path.read_bytes()[:16]
    assert header[8] == 0, f"{path}: superblock version {header[8]}, expected 0"
    assert (header[13], header[14]) == (offsets, lengths), f"{path}: sizes {header[13:15]}"
    with h5py.File(path, "r") as f:
        chunks = h5py.h5o.get_info(f["many_attrs"].id).hdr.nchunks
    assert chunks > 1, f"{path}: many_attrs header has {chunks} chunk, expected a continuation"


def oracle(path: Path, offsets: int, lengths: int) -> dict:
    datasets: dict[str, dict] = {}
    attributes: dict[str, dict] = {}

    def attrs_of(obj) -> dict:
        out = {}
        for k, a in obj.attrs.items():
            if isinstance(a, bytes):
                out[k] = a.decode()
            elif np.ndim(a) == 0 and np.issubdtype(np.asarray(a).dtype, np.number):
                out[k] = float(a)
        return out

    with h5py.File(path, "r") as f:
        attributes["/"] = attrs_of(f)

        def visit(name: str, obj) -> None:
            if isinstance(obj, h5py.Dataset):
                path_name = name if "/" not in name else f"/{name}"
                datasets[path_name] = {
                    "shape": list(obj.shape),
                    "maxshape": [m if m is not None else -1 for m in obj.maxshape],
                    "values": [float(v) for v in obj[()].ravel()],
                    "dims": [d.label or (d[0].name.lstrip("/") if len(d) else "") for d in obj.dims],
                }
            got = attrs_of(obj)
            if got:
                attributes[f"/{name}"] = got

        f.visititems(visit)
        header_chunks = h5py.h5o.get_info(f["many_attrs"].id).hdr.nchunks

    return {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"libver=(earliest, v18), set_sizes({offsets}, {lengths})",
        "note": "earliest-format file whose Size of Offsets and Size of Lengths differ (#922)",
        "superblock_version": 0,
        "size_of_offsets": offsets,
        "size_of_lengths": lengths,
        "many_attrs_header_chunks": header_chunks,
        "datasets": datasets,
        "attributes": attributes,
    }


def main() -> None:
    for offsets, lengths in ((8, 4), (4, 8)):
        name = f"hdf5_sizes_o{offsets}_l{lengths}.h5"
        path = FIXTURES / name
        write(path, offsets, lengths)
        result = oracle(path, offsets, lengths)
        (FIXTURES / f"{name}.oracle.json").write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
        print(
            f"wrote {path} ({path.stat().st_size} B): {len(result['datasets'])} datasets, "
            f"many_attrs in {result['many_attrs_header_chunks']} header chunks"
        )


if __name__ == "__main__":
    main()
