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
  datasets, one little-endian and one big-endian (its filter has the MSB
  bit set), and other filters this reader does not decode.

libaec's first commit (MathisRosenhauer/libaec on GitHub) is dated
2012-05-11, after both files were written. Each file's git blob id is the
same at the commit that last wrote it and at the pinned commit below, which
is what dates the bytes; the oracle records it. The builder refuses a
download whose SHA-256 is not the one pinned here.

HDF5 is under the 3-clause BSD license. The builder also fetches HDF5's
``LICENSE`` at the same commit, checks its SHA-256, and writes it as
``fixtures/LICENSE-HDF5.txt`` with the one change of a final newline, which the
upstream file lacks and the repository's hooks would add anyway.

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

# HDF5 `develop` on 2026-11-09. The SHA-256 below is what pins the bytes; the
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
LICENSE_URL = (
    "https://raw.githubusercontent.com/HDFGroup/hdf5/"
    "348d507bdb77645f9f59f8d2b2e9bc3b276151a8/LICENSE"
)
LICENSE_SHA256 = "b384492d25c6e4a49f371b646d9a81bc0a4b97cf0c66c2f3ed1b35f78a148732"

SOURCES = [
    {
        "fixture": "hdf5_szip2_h5repack.h5",
        "url": H5REPACK_URL,
        "path": "tools/test/testfiles/h5repack_szip.h5",
        "sha256": "353a3c7504176418ed74e03b5ff8fb01ded79fa968d4216a502cca950a8f13b3",
        "blob": "b16d16907a8120827e2221351c7bb94d6afbabaf",
        "unchanged_since": "svn r12917, git 8f74e81758, 2006-11-15",
    },
    {
        "fixture": "hdf5_szip2_cross_le.h5",
        "url": LE_DATA_URL,
        "path": "test/testfiles/le_data.h5",
        "sha256": "82d037f4a4dd98970e1c5fce9b70a9fad0db0201e2ce2ce305a05505bc6f3798",
        "blob": "e79ab47b416f1cf42e80c5c62307346deb4ec812",
        "unchanged_since": "svn r20855, git 89339ccce5, 2011-05-17",
    },
]


def fetch_all() -> dict[str, bytes]:
    # One single-argument urlopen per literal URL, so the URL a reviewer reads
    # is the one fetched (and semgrep can see it is a constant).
    with urllib.request.urlopen(H5REPACK_URL) as r:
        h5repack = r.read()
    with urllib.request.urlopen(LE_DATA_URL) as r:
        le_data = r.read()
    with urllib.request.urlopen(LICENSE_URL) as r:
        license_text = r.read()
    return {H5REPACK_URL: h5repack, LE_DATA_URL: le_data, LICENSE_URL: license_text}


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
    license_text = fetched[LICENSE_URL]
    digest = hashlib.sha256(license_text).hexdigest()
    if digest != LICENSE_SHA256:
        raise SystemExit(f"{LICENSE_URL}: SHA-256 {digest}, expected {LICENSE_SHA256}")
    if not license_text.endswith(b"\n"):
        license_text += b"\n"
    (FIXTURES / "LICENSE-HDF5.txt").write_bytes(license_text)
    for src in SOURCES:
        data = fetched[src["url"]]
        digest = hashlib.sha256(data).hexdigest()
        if digest != src["sha256"]:
            raise SystemExit(f"{src['url']}: SHA-256 {digest}, expected {src['sha256']}")
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
                "sha256": src["sha256"],
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
        print(f"wrote {path.name} ({len(data)} B): {sorted(objects)}")


if __name__ == "__main__":
    main()
