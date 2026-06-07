#!/usr/bin/env python3
"""Build the HDF5 deep-parse test fixtures and their structural/value oracles.

The HDF5 traversal chain (#37 object-header walker, #38 group/link traversal,
#39 dataspace + datatype, #40 attributes, #121 value decode — under #33) needs
small, controlled fixtures exercising both on-disk group layouts plus the
datatype / dataspace / storage matrix. This script writes two fixtures with
``h5py`` (which wraps libhdf5) and a sibling ``*.h5.oracle.json`` for each:

  * ``hdf5_v1_symboltable.h5`` — ``libver='earliest'``: superblock v0, **v1**
    object headers, **symbol-table** groups (local heap + B-tree v1). The
    legacy layout #38 must handle.
  * ``hdf5_v2_linkinfo.h5`` — ``libver='v110'``: superblock v3, **v2** object
    headers (``OHDR``), **link-info** groups, plus a chunked+gzip+shuffle
    dataset (#121) and a dataset with enough attributes to force **dense**
    attribute storage (fractal heap + B-tree v2, #40).

Both carry the datatype matrix (#39: signed int LE + BE, float32, float64,
fixed-length string), the dataspace matrix (scalar, simple 1-D/2-D, and an
unlimited/`H5S_UNLIMITED` max dim), global + per-dataset attributes (#40), and
contiguous storage with an explicit fill value (#121).

``track_times=False`` keeps object headers free of modification timestamps so
the fixtures are reproducible. Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_fixtures.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import numpy as np

FIXturesDir = Path("crates/fieldglass-netcdf/tests/fixtures")
FIXED_STR = h5py.string_dtype("ascii", 8)


def populate(f: h5py.File, *, dense_and_chunked: bool) -> None:
    """Write the shared object matrix into an open file."""
    # --- global (root-group) attributes: #40 ---
    f.attrs["title"] = np.bytes_(b"fieldglass HDF5 fixture")
    f.attrs["version"] = np.int32(5)
    f.attrs["scale"] = np.float64(0.25)

    def ds(name, data=None, **kw):
        kw.setdefault("track_times", False)
        return f.create_dataset(name, data=data, **kw)

    # --- datatype matrix (#39) + contiguous storage (#121) ---
    ds("temp_i32", np.arange(12, dtype="<i4").reshape(3, 4))  # int32 LE, simple 2-D
    ds("temp_be_i32", np.arange(5, dtype=">i4"))              # int32 BE  (byte order)
    ds("temp_f32", (np.arange(8) * 1.5).astype("<f4"))       # float32, simple 1-D
    f64 = ds("temp_f64", np.linspace(0.0, 1.0, 6, dtype="<f8"))  # float64, simple 1-D
    ds("scalar_i32", data=np.int32(42))                      # scalar dataspace
    ds("label", data=np.array(b"degC", dtype=FIXED_STR))     # fixed-length string

    # --- dataspace: unlimited / H5S_UNLIMITED max dim (#39) ---
    ds("record", np.arange(4, dtype="<f4"), maxshape=(None,))

    # --- fill value, no data written → reads all-fill (#121) ---
    ds("masked", shape=(6,), dtype="<f4", fillvalue=np.float32(-999.0))

    # --- per-dataset attributes (#40): numeric + string ---
    f64.attrs["units"] = np.bytes_(b"meters")
    f64.attrs["valid_min"] = np.float64(0.0)
    f64.attrs["valid_max"] = np.float64(1.0)

    if dense_and_chunked:
        # chunked + deflate + shuffle (#121: filter pipeline)
        ds(
            "chunked",
            np.arange(100, dtype="<f4").reshape(10, 10),
            chunks=(5, 5),
            compression="gzip",
            compression_opts=4,
            shuffle=True,
        )
        # >8 attributes forces dense attribute storage (#40)
        dense = ds("dense_attrs", np.arange(3, dtype="<i4"))
        for i in range(12):
            dense.attrs[f"attr_{i:02d}"] = np.int32(i)


def numpy_dtype_oracle(dt: np.dtype) -> dict:
    kind = {"i": "fixed-point signed", "u": "fixed-point unsigned",
            "f": "floating-point", "S": "string (fixed-length)"}.get(dt.kind, dt.kind)
    order = {"<": "little-endian", ">": "big-endian",
             "=": "native", "|": "not-applicable"}[dt.byteorder]
    return {"class": kind, "size_bytes": dt.itemsize, "byte_order": order}


def sample_indices(n: int) -> list[int]:
    if n == 0:
        return []
    if n <= 5:
        return list(range(n))
    return sorted({0, 1, n // 2, n - 2, n - 1})


def attr_oracle(attrs: h5py.AttributeManager) -> list[dict]:
    out = []
    for name in attrs:
        v = attrs[name]
        dt = attrs.get_id(name).dtype
        entry = {"name": name, "datatype": numpy_dtype_oracle(np.dtype(dt))}
        if np.dtype(dt).kind == "S":
            entry["value"] = (v.tobytes() if hasattr(v, "tobytes") else v).decode("latin-1").rstrip("\x00")
        else:
            arr = np.atleast_1d(v)
            entry["value"] = arr.reshape(-1).tolist() if arr.size > 1 else float(arr.reshape(-1)[0])
        out.append(entry)
    return out


def dataset_oracle(d: h5py.Dataset) -> dict:
    raw = np.asarray(d[()]).reshape(-1) if d.shape != () else np.atleast_1d(np.asarray(d[()]))
    o: dict = {
        "kind": "dataset",
        "datatype": numpy_dtype_oracle(d.dtype),
        "dataspace": {
            "class": "scalar" if d.shape == () else "simple",
            "rank": len(d.shape),
            "dims": list(d.shape),
            "max_dims": [(-1 if m is None else m) for m in (d.maxshape or ())],
        },
        "storage": {
            "layout": "chunked" if d.chunks else "contiguous",
            "chunks": list(d.chunks) if d.chunks else None,
            "filters": [n for n, on in (("shuffle", d.shuffle), ("deflate", d.compression == "gzip")) if on],
        },
        "fill_value": (float(d.fillvalue) if np.issubdtype(d.dtype, np.number) else None),
        "attributes": attr_oracle(d.attrs),
    }
    if d.dtype.kind == "S":
        o["text"] = b"".join(np.atleast_1d(d[()]).reshape(-1).tolist()).decode("latin-1").rstrip("\x00")
        return o
    fill = d.fillvalue if np.issubdtype(d.dtype, np.number) else None
    present = raw[raw != fill] if fill is not None else raw
    o["values"] = {
        "count": int(raw.size),
        "present_count": int(present.size),
        "missing_count": int(raw.size - present.size),
        "samples": {str(i): float(raw[i]) for i in sample_indices(raw.size)},
    }
    if present.size:
        o["values"].update(min=round(float(present.min()), 8),
                           max=round(float(present.max()), 8),
                           mean=float(present.mean()))
    return o


def build(name: str, libver: str, *, dense_and_chunked: bool) -> None:
    path = FIXturesDir / name
    with h5py.File(path, "w", libver=libver) as f:
        populate(f, dense_and_chunked=dense_and_chunked)

    raw = path.read_bytes()
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver={libver!r}",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "object_header_style": "v2_linkinfo" if b"OHDR" in raw else "v1_symboltable",
            "raw_markers": {
                "OHDR_v2_object_header": b"OHDR" in raw,
                "SNOD_symbol_table": b"SNOD" in raw,
                "FRHP_fractal_heap_dense_attrs": b"FRHP" in raw,
            },
            "global_attributes": attr_oracle(f.attrs),
            "root_children": [
                {"name": n, "type": "group" if isinstance(f[n], h5py.Group) else "dataset"}
                for n in f
            ],
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[sb_v{oracle['superblock_version']}, {oracle['object_header_style']}, "
          f"FRHP={oracle['raw_markers']['FRHP_fractal_heap_dense_attrs']}]")


def main() -> int:
    if not FIXturesDir.is_dir():
        raise SystemExit("run from the repo root")
    build("hdf5_v1_symboltable.h5", "earliest", dense_and_chunked=False)
    build("hdf5_v2_linkinfo.h5", "v110", dense_and_chunked=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
