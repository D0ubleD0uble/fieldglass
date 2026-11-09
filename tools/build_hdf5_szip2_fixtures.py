#!/usr/bin/env python3
"""Fetch two HDF5 files whose szip chunks the HDF Group's SZIP 2.x wrote (#820).

Since #794, ``fieldglass_aec::sz::decompress`` refuses a 32- or 64-bit szip
stream with a whole byte left after the end the output's length implies. That
end point was taken from libaec's encoder, and every other szip fixture here
was written by libaec. These two files were written before libaec existed, by
libhdf5 linked against the HDF Group's own SZIP library, so they show where
that encoder ends its stream.

Both come from the HDF5 source repository, where they are test inputs:

- ``tools/test/testfiles/h5repack_szip.h5``: one ``int32`` dataset, szip NN.
  Its bytes have not changed since svn r12917 (2006-11-15).
- ``test/testfiles/le_data.h5``: written by ``test/gen_cross.c`` on a
  little-endian host for the cross-platform read test. Its bytes have not
  changed since svn r20855 (2011-05-17). It has two szip ``float32``
  datasets, one little-endian and one big-endian (the MSB option), and other
  filters this reader does not decode.

libaec's first release, 0.1.0, was announced in 2013. The provenance is
checked by git blob id: the builder refuses a download whose blob id is not
the one the file has carried since the dates above. HDF5 is under the
3-clause BSD license (``fixtures/LICENSE-HDF5.txt``).

The files are committed unchanged. The oracle lists only the datasets whose
whole pipeline is szip, with the h5py read-back of every value and each
chunk's file offset, stored size and filter mask, so a test can cut out each
chunk's stream.

Run from the repo root (needs h5py and numpy; downloads about 80 KB)::

    python3 tools/build_hdf5_szip2_fixtures.py
"""
from __future__ import annotations

import hashlib
import json
import urllib.request
from pathlib import Path

import h5py

HERE = Path(__file__).resolve().parent.parent
FIXTURES = HERE / "crates" / "fieldglass-netcdf" / "tests" / "fixtures"

# HDF5 `develop` on 2026-11-09. The blob ids below are what pins the bytes; the
# commit only fixes where they are read from.
COMMIT = "348d507bdb77645f9f59f8d2b2e9bc3b276151a8"
H5REPACK_URL = (
    "https://raw.githubusercontent.com/HDFGroup/hdf5/"
    "348d507bdb77645f9f59f8d2b2e9bc3b276151a8/tools/test/testfiles/h5repack_szip.h5"
)
LE_DATA_URL = (
    "https://raw.githubusercontent.com/HDFGroup/hdf5/"
    "348d507bdb77645f9f59f8d2b2e9bc3b276151a8/test/testfiles/le_data.h5"
)

SOURCES = [
    {
        "fixture": "hdf5_szip2_h5repack.h5",
        "url": H5REPACK_URL,
        "path": "tools/test/testfiles/h5repack_szip.h5",
        "blob": "b16d16907a8120827e2221351c7bb94d6afbabaf",
        "unchanged_since": "svn r12917, git 8f74e81758, 2006-11-15",
    },
    {
        "fixture": "hdf5_szip2_cross_le.h5",
        "url": LE_DATA_URL,
        "path": "test/testfiles/le_data.h5",
        "blob": "e79ab47b416f1cf42e80c5c62307346deb4ec812",
        "unchanged_since": "svn r20855, git 89339ccce5, 2011-05-17",
    },
]


def git_blob_id(data: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def fetch_all() -> dict[str, bytes]:
    # One single-argument urlopen per literal URL, so the URL a reviewer reads
    # is the one fetched (and semgrep can see it is a constant).
    with urllib.request.urlopen(H5REPACK_URL) as r:
        h5repack = r.read()
    with urllib.request.urlopen(LE_DATA_URL) as r:
        le_data = r.read()
    return {H5REPACK_URL: h5repack, LE_DATA_URL: le_data}


def dataset_oracle(d: h5py.Dataset) -> dict | None:
    dcpl = d.id.get_create_plist()
    pipeline = [dcpl.get_filter(i) for i in range(dcpl.get_nfilters())]
    if [f[0] for f in pipeline] != [4]:
        return None
    mask, ppb, bpp, pps = pipeline[0][2]
    chunks = [d.id.get_chunk_info(i) for i in range(d.id.get_num_chunks())]
    return {
        "dtype": d.dtype.str,
        "shape": list(d.shape),
        "chunks": list(d.chunks),
        "szip": {
            "options_mask": mask,
            "pixels_per_block": ppb,
            "bits_per_pixel": bpp,
            "pixels_per_scanline": pps,
        },
        "chunk_byte_offsets": [c.byte_offset for c in chunks],
        "chunk_sizes": [c.size for c in chunks],
        "chunk_filter_masks": [c.filter_mask for c in chunks],
        "values": [float(v) for v in d[...].ravel()],
    }


def main() -> None:
    fetched = fetch_all()
    for src in SOURCES:
        data = fetched[src["url"]]
        blob = git_blob_id(data)
        if blob != src["blob"]:
            raise SystemExit(f"{src['url']}: blob {blob}, expected {src['blob']}")
        path = FIXTURES / src["fixture"]
        path.write_bytes(data)
        with h5py.File(path, "r") as f:
            objects = {}
            for name in f:
                if isinstance(f[name], h5py.Dataset):
                    o = dataset_oracle(f[name])
                    if o is not None:
                        objects[name] = o
        oracle = {
            "source": f"h5py {h5py.version.version} (libhdf5 {h5py.version.hdf5_version})",
            "origin": {
                "repository": "https://github.com/HDFGroup/hdf5",
                "commit": COMMIT,
                "path": src["path"],
                "git_blob": src["blob"],
                "unchanged_since": src["unchanged_since"],
            },
            "note": "written by libhdf5 with the HDF Group SZIP 2.x encoder, "
            "before libaec existed; only the datasets whose pipeline is szip alone",
            "objects": objects,
        }
        (FIXTURES / f"{src['fixture']}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path.name} ({len(data)} B, blob {blob}): {sorted(objects)}")


if __name__ == "__main__":
    main()
