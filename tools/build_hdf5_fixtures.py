#!/usr/bin/env python3
"""Build the HDF5 deep-parse test fixtures and their structural/value oracles.

The HDF5 traversal chain (#37 object-header walker, #38 group/link traversal,
#39 dataspace + datatype, #40 attributes, #121 value decode — under #33) needs
small, controlled fixtures exercising both on-disk group layouts plus the
datatype / dataspace / storage matrix. This script writes two fixtures with
``h5py`` (which wraps libhdf5) and a sibling ``*.h5.oracle.json`` for each:

  * ``hdf5_v1_symboltable.h5`` — ``libver='earliest'``: superblock v0, **v1**
    object headers, **symbol-table** groups (local heap + B-tree v1). The
    legacy layout #38 must handle. Also carries a chunked+gzip+shuffle dataset
    whose chunk index is a **version-1 B-tree** (Data Layout v3) — the value
    decode #121 reads end to end.
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


def populate(f: h5py.File, *, dense_and_chunked: bool, btree_v1_compressed: bool = False) -> None:
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
    # A non-round float `_FillValue` (its exact f32 value needs more than a few
    # decimals) so value decode must mask against the *typed* fill, not the
    # rounded display text. With the attribute present, every point reads as the
    # fill and masks to missing.
    masked = ds("masked", shape=(6,), dtype="<f4", fillvalue=np.float32(-9999.55))
    masked.attrs["_FillValue"] = np.float32(-9999.55)

    if btree_v1_compressed:
        # chunked + deflate + shuffle under libver='earliest' → the chunk index
        # is a version-1 B-tree (Data Layout v3). This is the read path #121
        # decodes: B-tree chunk walk + filter-pipeline reverse, end to end. (The
        # v110 fixture's `chunked` dataset uses a version-4 chunk index instead.)
        ds(
            "compressed",
            np.arange(64, dtype="<f4").reshape(8, 8),
            chunks=(4, 4),
            compression="gzip",
            compression_opts=4,
            shuffle=True,
        )

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


def build(name: str, libver: str, *, dense_and_chunked: bool,
          btree_v1_compressed: bool = False) -> None:
    path = FIXturesDir / name
    with h5py.File(path, "w", libver=libver) as f:
        populate(f, dense_and_chunked=dense_and_chunked,
                 btree_v1_compressed=btree_v1_compressed)

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


def btree_v2_depth(raw: bytes, btree_type: int) -> int:
    """Depth field of the first version-2 B-tree (``BTHD``) of ``btree_type``."""
    i = 0
    while (i := raw.find(b"BTHD", i)) >= 0:
        if raw[i + 5] == btree_type:
            return int.from_bytes(raw[i + 12:i + 14], "little")
        i += 1
    raise SystemExit(f"no BTHD of type {btree_type} in fixture")


def build_btreev2_multilevel(name: str, n_attrs: int) -> None:
    """A dataset carrying enough dense attributes that the attribute name-index
    version-2 B-tree grows past one level (``depth > 0``), so the reader must walk
    internal nodes — the structure real metadata-heavy NetCDF-4 / HDF5 files hit
    well before their fractal heap needs child indirect blocks. Each attribute is
    ``a{i:04d} -> int32 i`` so the oracle stays a rule plus samples, not a dump."""
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass multi-level B-tree v2 fixture")
        dense = f.create_dataset("many_attrs", data=np.arange(3, dtype="<i4"),
                                 track_times=False)
        for i in range(n_attrs):
            dense.attrs[f"a{i:04d}"] = np.int32(i)

    raw = path.read_bytes()
    depth = btree_v2_depth(raw, btree_type=8)
    oracle = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
        "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
        "dataset": "many_attrs",
        "attribute_count": n_attrs,
        "attribute_name_index_btree_v2": {"type": 8, "depth": depth},
        "attribute_value_rule": "a{i:04d} -> int32 i, for i in 0..attribute_count",
        "sampled_attributes": {f"a{i:04d}": i for i in sample_indices(n_attrs)},
    }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[{n_attrs} attrs, attr-name B-tree v2 depth={depth}]")


def fractal_heap_geometry(raw: bytes) -> dict:
    """Parse the first fractal-heap header (``FRHP``) the way ``heap.rs`` does —
    assuming 8-byte offset/length sizes (``libver='latest'``) — and report
    whether its doubling table has spilled into child indirect blocks."""
    i = raw.index(b"FRHP")
    g = lambda off, n: int.from_bytes(raw[i + off:i + off + n], "little")
    width, starting, max_direct, cur_rows = g(110, 2), g(112, 8), g(120, 8), g(140, 2)
    # max_dblock_rows = (log2(max_direct) - log2(starting)) + 2; rows beyond it
    # hold child indirect block pointers.
    max_dblock_rows = (max_direct.bit_length() - starting.bit_length()) + 2
    return {
        "table_width": width,
        "starting_block_size": starting,
        "max_direct_block_size": max_direct,
        "cur_rows": cur_rows,
        "max_dblock_rows": max_dblock_rows,
        "has_child_indirect_rows": cur_rows > max_dblock_rows,
        # FHIB count > 1 means a child indirect block is actually allocated and
        # populated (block #0 is the root indirect block).
        "indirect_block_count": raw.count(b"FHIB"),
        "direct_block_count": raw.count(b"FHDB"),
    }


def build_child_indirect(name: str, n_attrs: int, vlen: int) -> None:
    """A dataset with enough *large* dense attributes that the attribute fractal
    heap fills every direct-block row of its doubling table and spills into a
    **child indirect block** — the rows beyond ``max_direct_block_size`` that
    ``heap.rs`` must now recurse into. This is the structure the metadata-heaviest
    corpus files (#123) reach; libhdf5 fills the full grid of direct blocks (the
    exact heap geometry is libhdf5-version dependent and recorded in the oracle)
    before allocating a child indirect block, so this fixture is necessarily
    larger than the others. Each attribute is ``a{i:04d} -> int32[vlen]`` (value
    ``arange``) so the oracle stays a rule plus samples rather than a dump."""
    path = FIXturesDir / name
    base = np.arange(vlen, dtype="<i4")
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass child-indirect fractal-heap fixture")
        dense = f.create_dataset("many_attrs", data=np.arange(3, dtype="<i4"),
                                 track_times=False)
        # Each attribute gets a *distinct* value (`base + i`) so a heap-object
        # mis-mapping (e.g. aliasing two records resolved through the child
        # indirect block) shows up as a wrong value, not just a missing name.
        for i in range(n_attrs):
            dense.attrs[f"a{i:04d}"] = base + np.int32(i)

    raw = path.read_bytes()
    heap = fractal_heap_geometry(raw)
    if heap["indirect_block_count"] < 2 or not heap["has_child_indirect_rows"]:
        raise SystemExit(
            f"{name}: fractal heap did not spill into a populated child indirect "
            f"block (FHIB={heap['indirect_block_count']}, "
            f"has_child_indirect_rows={heap['has_child_indirect_rows']}); "
            f"raise n_attrs / vlen")
    oracle = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
        "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
        "dataset": "many_attrs",
        "attribute_count": n_attrs,
        "attribute_value_rule": f"a{{i:04d}} -> int32[{vlen}], value[k] = i + k",
        "attribute_value_length": vlen,
        "sampled_attributes": [f"a{i:04d}" for i in sample_indices(n_attrs)],
        "attribute_fractal_heap": heap,
    }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[{n_attrs} attrs × int32[{vlen}], FRHP cur_rows={heap['cur_rows']} "
          f"max_dblock_rows={heap['max_dblock_rows']} FHIB={heap['indirect_block_count']}]")


def main() -> int:
    if not FIXturesDir.is_dir():
        raise SystemExit("run from the repo root")
    build("hdf5_v1_symboltable.h5", "earliest", dense_and_chunked=False,
          btree_v1_compressed=True)
    build("hdf5_v2_linkinfo.h5", "v110", dense_and_chunked=True)
    # 700 attributes pushes the attribute name-index B-tree v2 to depth 2,
    # exercising both the record-count and subtree-total node-pointer fields.
    build_btreev2_multilevel("hdf5_btreev2_multilevel.h5", n_attrs=700)
    # 512 large (1 KiB) attributes overflow the attribute fractal heap's direct
    # rows into a child indirect block (depth-1 doubling-table recursion).
    build_child_indirect("hdf5_child_indirect.h5", n_attrs=512, vlen=256)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
