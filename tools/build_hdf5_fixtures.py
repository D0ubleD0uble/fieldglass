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
the fixtures are reproducible. Run from the repo root (needs ``h5py``, and
``hdf5plugin`` for the zstd fixture — libhdf5 cannot write filter 32015
without it):

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
            "filters": [n for n, on in (("shuffle", d.shuffle), ("deflate", d.compression == "gzip"),
                                        ("fletcher32", d.fletcher32)) if on],
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
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
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
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
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
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[{n_attrs} attrs × int32[{vlen}], FRHP cur_rows={heap['cur_rows']} "
          f"max_dblock_rows={heap['max_dblock_rows']} FHIB={heap['indirect_block_count']}]")


def build_v4_chunk_index(name: str) -> None:
    """Datasets whose version-4 chunk indexes are the two the reader decodes but
    the ``v110`` fixture does not already cover: a **single chunk** (chunk shape
    == dataset shape) and a fixed-shape multi-chunk **fixed array** with
    unfiltered chunks. Written under ``libver='latest'`` so libhdf5 selects the
    version-4 layout message and these newer indexes. (The ``v110`` fixture
    already covers a *filtered* fixed array and an extensible array.)

    ``single_chunk_filtered`` exercises the **version-5** layout message: libhdf5
    2.0 writes a filtered single chunk with a data-layout message version 5 (not
    the version 4 the unfiltered case uses). For the chunked class version 5 is
    encoded byte-for-byte like version 4, so it decodes through the same path.
    Values are ``arange`` so the oracle is a rule."""
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass v4 chunk-index fixture")

        def ds(nm, data, **kw):
            kw.setdefault("track_times", False)
            return f.create_dataset(nm, data=data, **kw)

        # Single Chunk index (type 1): the whole dataset is one chunk.
        ds("single_chunk", np.arange(16, dtype="<f4").reshape(4, 4), chunks=(4, 4))
        # Single Chunk, filtered → SINGLE_INDEX_WITH_FILTER: size + mask inline.
        ds(
            "single_chunk_filtered",
            np.arange(16, dtype="<f4").reshape(4, 4),
            chunks=(4, 4),
            compression="gzip",
            compression_opts=4,
            shuffle=True,
        )
        # Fixed Array index (type 3), unfiltered: fixed-shape 8×8 in 4×4 chunks.
        ds("fixed_array", np.arange(64, dtype="<f4").reshape(8, 8), chunks=(4, 4))

    raw = path.read_bytes()
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "version-4 data layout; single-chunk and fixed-array chunk indexes (#216)",
            "raw_markers": {"FAHD_fixed_array_header": b"FAHD" in raw},
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[v4 single-chunk + fixed-array, FAHD={oracle['raw_markers']['FAHD_fixed_array_header']}]")


def build_extensible_array(name: str) -> None:
    """Datasets with one unlimited dimension, whose chunks libhdf5 indexes with a
    version-4 **Extensible Array**. Written under ``libver='latest'``. Two sizes
    exercise the two tiers the reader decodes (values are ``arange``):

    - ``ea_direct`` (150 chunks): spans super blocks 0–3, whose data-block
      addresses libhdf5 stores directly in the extensible-array index block (no
      secondary block) — data blocks of growing size (16, 32, 32, 64, …).
    - ``ea_secondary`` (280 chunks): large enough that libhdf5 allocates a
      **secondary block** for super block 4, which the reader walks to reach the
      data-block addresses beyond the index block's direct slots.

    (The v110 fixture's ``record`` already covers the index-block-only case: one
    chunk addressed directly in the index block with no data blocks.)"""
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass extensible-array fixture")
        for nm, nchunks in (("ea_direct", 150), ("ea_secondary", 280)):
            n = nchunks * 4  # chunk edge 4
            d = f.create_dataset(nm, shape=(n,), maxshape=(None,), chunks=(4,),
                                 dtype="<f4", track_times=False)
            d[:] = np.arange(n, dtype="<f4")

    raw = path.read_bytes()
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "version-4 extensible-array chunk index, direct + secondary blocks (#216)",
            "raw_markers": {
                "EAHD_header": b"EAHD" in raw,
                "EADB_data_block": b"EADB" in raw,
                "EASB_secondary_block": b"EASB" in raw,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[EA direct + secondary, EASB={oracle['raw_markers']['EASB_secondary_block']}]")


def build_extensible_array_filtered(name: str) -> None:
    """Datasets with one unlimited dimension **and a filter** (gzip + shuffle),
    whose chunks libhdf5 indexes with a version-4 **Extensible Array** whose
    elements are *filtered* (client id 1): address + on-disk size + filter mask,
    not the address-only elements the unfiltered array stores. Written under
    ``libver='latest'``. Three sizes exercise every path a filtered element is
    read (values are ``arange``):

    - ``ea_filtered_iblock`` (4 chunks): all elements live directly in the
      extensible-array index block — no data blocks. Covers the filtered
      index-block element read in isolation.
    - ``ea_filtered_direct`` (150 chunks): spans super blocks 0–3 whose data
      blocks libhdf5 addresses directly from the index block (no secondary
      block). Covers filtered elements inside data blocks.
    - ``ea_filtered_secondary`` (280 chunks): large enough that libhdf5
      allocates a **secondary block** for super block 4, reached via the index
      block's secondary-block pointer.

    (The unfiltered counterparts live in ``hdf5_ea_chunk_index.h5``.)"""
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass filtered extensible-array fixture")
        for nm, nchunks in (("ea_filtered_iblock", 4), ("ea_filtered_direct", 150),
                            ("ea_filtered_secondary", 280)):
            n = nchunks * 4  # chunk edge 4
            d = f.create_dataset(nm, shape=(n,), maxshape=(None,), chunks=(4,),
                                 dtype="<f4", compression="gzip", compression_opts=4,
                                 shuffle=True, track_times=False)
            d[:] = np.arange(n, dtype="<f4")

    raw = path.read_bytes()
    # Every extensible-array header in this file must be a *filtered* one
    # (client id 1, the byte after the 4-byte signature + 1-byte version); a
    # client id 0 would mean libhdf5 dropped the filter and this fixture no
    # longer exercises the filtered path.
    i, saw_ea = 0, False
    while (i := raw.find(b"EAHD", i)) >= 0:
        saw_ea = True
        client_id = raw[i + 5]
        if client_id != 1:
            raise SystemExit(
                f"{name}: extensible-array header at {i} has client id {client_id}, "
                f"expected 1 (filtered) — libhdf5 did not filter these chunks")
        i += 1
    if not saw_ea:
        raise SystemExit(f"{name}: no EAHD extensible-array header — libhdf5 chose "
                         f"a different chunk index (check maxshape / chunks)")
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "version-4 filtered extensible-array chunk index (client id 1), "
                    "index-block + direct + secondary blocks (#216)",
            "raw_markers": {
                "EAHD_header": b"EAHD" in raw,
                "EADB_data_block": b"EADB" in raw,
                "EASB_secondary_block": b"EASB" in raw,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle "
          f"[filtered EA iblock + direct + secondary, EASB={oracle['raw_markers']['EASB_secondary_block']}]")


def build_implicit_index(name: str) -> None:
    """Datasets whose version-4 chunk index is the **Implicit** index (type 2):
    libhdf5 uses it for a fixed-shape, unfiltered, *early-allocated* chunked
    dataset, storing every chunk of the chunk grid contiguously from one base
    address with no on-disk index structure. The high-level h5py API always
    defers allocation (which yields a Fixed Array), so the datasets are created
    through the low-level API with the space-allocation time set to *early*.

    Two shapes exercise the reader: a square multi-chunk grid (``implicit``,
    8×8 in 4×4 → four whole chunks) and one whose edge chunks hang past the
    dataset bounds (``implicit_partial``, 5×7 in 4×4 → a 2×2 grid of full-size
    chunks the reader must clip on scatter). Values are ``arange`` so the oracle
    is a rule."""
    path = FIXturesDir / name

    def implicit_ds(f, nm, shape, chunks, dtype="<f4"):
        # Early space allocation + fixed dims + no filters ⇒ libhdf5 selects the
        # implicit index. Requires the low-level dataset-creation path.
        space = h5py.h5s.create_simple(shape)  # max dims == shape (fixed)
        dcpl = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
        dcpl.set_chunk(chunks)
        dcpl.set_alloc_time(h5py.h5d.ALLOC_TIME_EARLY)
        # Match the high-level fixtures' `track_times=False`: keep the object
        # header free of modification timestamps so the fixture is reproducible.
        dcpl.set_obj_track_times(False)
        tid = h5py.h5t.py_create(np.dtype(dtype), logical=True)
        dsid = h5py.h5d.create(f.id, nm.encode(), tid, space, dcpl=dcpl)
        d = h5py.Dataset(dsid)
        d[...] = np.arange(int(np.prod(shape)), dtype=dtype).reshape(shape)
        return d

    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass implicit chunk-index fixture")
        implicit_ds(f, "implicit", (8, 8), (4, 4))
        implicit_ds(f, "implicit_partial", (5, 7), (4, 4))

    raw = path.read_bytes()
    # Positively confirm the implicit index. These datasets are fixed-shape and
    # multi-chunk, so the only index libhdf5 could pick *instead* of implicit is
    # a Fixed Array (single chunk is ruled out by having several chunks; the
    # Extensible Array and v2-B-tree indexes require an unlimited dimension). The
    # Fixed Array writes a "FAHD" header / "FADB" data block, and neither has any
    # other reason to appear in this file, so their absence pins the index as
    # implicit. (An "EAHD" check is kept as a cheap belt-and-suspenders; a bare
    # "BTHD" is deliberately *not* checked — that signature also marks dense
    # link / attribute storage and would false-fail if the fixture grew.)
    for marker in (b"FAHD", b"FADB", b"EAHD"):
        if marker in raw:
            raise SystemExit(
                f"{name}: unexpected {marker.decode()} marker — libhdf5 did not "
                f"use the implicit index (check alloc-time / filters / max dims)")
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "version-4 implicit chunk index (early-allocated, unfiltered) (#216)",
            "raw_markers": {
                "no_FAHD_fixed_array": b"FAHD" not in raw,
                "no_EAHD_extensible_array": b"EAHD" not in raw,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle [v4 implicit index, no FAHD/EAHD]")


def build_v2_btree(name: str) -> None:
    """Datasets with **more than one unlimited dimension**, the case libhdf5
    indexes with a version-4 **version-2 B-tree** (chunk index type 5) rather than
    a fixed or extensible array (which assume at most one growth dimension).
    Written under ``libver='latest'``. Each record carries the chunk's scaled
    (chunk-grid) coordinate directly, so the reader multiplies it by the chunk
    edge to place the chunk. Two datasets cover the reader's two record shapes
    (values are ``arange``):

    - ``bt2`` (4×4 in 2×2 chunks): unfiltered chunks → type-10 records (address +
      scaled offsets).
    - ``bt2_filtered`` (4×4 gzip+shuffle in 2×2 chunks): filtered chunks →
      type-11 records (address + on-disk size + filter mask + scaled offsets).

    A larger unfiltered grid (``bt2_multi`` 8×8 in 2×2 → 16 chunks) keeps the
    B-tree exercised with several records per leaf without depending on a specific
    node fanout."""
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass v2 B-tree chunk-index fixture")

        def bt2_ds(nm, shape, chunks, **kw):
            kw.setdefault("track_times", False)
            d = f.create_dataset(nm, shape=shape, maxshape=(None,) * len(shape),
                                 chunks=chunks, dtype="<f4", **kw)
            d[...] = np.arange(int(np.prod(shape)), dtype="<f4").reshape(shape)
            return d

        bt2_ds("bt2", (4, 4), (2, 2))
        bt2_ds("bt2_multi", (8, 8), (2, 2))
        bt2_ds("bt2_filtered", (4, 4), (2, 2),
               compression="gzip", compression_opts=4, shuffle=True)

    raw = path.read_bytes()
    # Positively confirm the v2 B-tree chunk index: every chunk-index B-tree
    # header (BTHD) must be type 10 (unfiltered) or 11 (filtered), and neither a
    # fixed nor extensible array header may appear — those would mean libhdf5 chose
    # a different index (check maxshape has >1 unlimited dimension).
    for marker in (b"FAHD", b"EAHD"):
        if marker in raw:
            raise SystemExit(
                f"{name}: unexpected {marker.decode()} marker — libhdf5 did not use "
                f"the v2 B-tree index (check maxshape / chunks)")
    saw_chunk_bt2 = False
    i = 0
    while (i := raw.find(b"BTHD", i)) >= 0:
        if raw[i + 5] in (10, 11):
            saw_chunk_bt2 = True
        i += 1
    if not saw_chunk_bt2:
        raise SystemExit(f"{name}: no chunk-index BTHD (type 10 or 11) — libhdf5 "
                         f"chose a different chunk index")
    with h5py.File(path, "r") as f:
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": raw[raw.index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "version-4 v2 B-tree chunk index (>1 unlimited dimension), "
                    "unfiltered (type 10) + filtered (type 11) records (#216)",
            "raw_markers": {
                "BTHD_type10_unfiltered_chunks": any(
                    raw[j + 5] == 10 for j in _find_all(raw, b"BTHD")),
                "BTHD_type11_filtered_chunks": any(
                    raw[j + 5] == 11 for j in _find_all(raw, b"BTHD")),
                "no_FAHD_fixed_array": b"FAHD" not in raw,
                "no_EAHD_extensible_array": b"EAHD" not in raw,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(raw)} B) + oracle [v4 v2-B-tree chunk index, "
          f"type10={oracle['raw_markers']['BTHD_type10_unfiltered_chunks']}, "
          f"type11={oracle['raw_markers']['BTHD_type11_filtered_chunks']}]")


def fletcher32_oracle(payload: bytes) -> int:
    """The fletcher32 checksum libhdf5 computes over `payload`.

    Asks libhdf5 rather than reimplementing it: store the payload as a single
    chunk with fletcher32 and no compressor, so the stored chunk is the payload
    verbatim followed by the four checksum bytes, then read those back. Used to
    generate the vectors pinned in `hdf5/filter.rs`."""
    import struct
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        fn = Path(tmp) / "oracle.h5"
        with h5py.File(fn, "w") as f:
            f.create_dataset("v", data=np.frombuffer(payload, dtype="u1"),
                             chunks=(len(payload),), fletcher32=True, track_times=False)
        with h5py.File(fn, "r") as f:
            ci = f["v"].id.get_chunk_info(0)
            raw = fn.read_bytes()[ci.byte_offset:ci.byte_offset + ci.size]
    if len(raw) != len(payload) + 4 or raw[:-4] != payload:
        raise SystemExit("fletcher32 is not a plain 4-byte suffix on this libhdf5")
    return struct.unpack("<I", raw[-4:])[0]


def build_fletcher32(name: str) -> None:
    """Datasets carrying the **fletcher32** checksum filter (id 3, #412).

    fletcher32 is not a compressor: it appends a four-byte checksum to each
    stored chunk, which reading verifies and strips. A file whose pipeline is
    ``fletcher32 + deflate`` therefore failed outright before #412, even though
    the actual compression was one already decoded.

    The datasets all hold the *same* values, so the test can assert directly
    that a checksummed pipeline decodes to what the plain one does:

      * ``plain_deflate`` — deflate alone, the comparison baseline.
      * ``f32_deflate`` — deflate + fletcher32, the case the issue is about.
      * ``f32_only`` — fletcher32 alone: no compressor, so the stored chunk is
        the raw element bytes plus exactly four.
      * ``f32_shuffle_deflate`` — the full netCDF-4 stack.
      * ``f32_shuffle`` — shuffle + fletcher32 with **no** compressor, which
        libhdf5 does write. This is the dataset that makes a passthrough
        implementation visibly wrong rather than accidentally right: fletcher32
        is written last, so it reverses first, and a reader that returned the
        chunk unchanged would hand unshuffle four extra bytes. 132 divides
        evenly by the 4-byte element, so unshuffle regroups 33 elements instead
        of 32 and returns wrong values with no error. With deflate in the
        middle the extra bytes are simply absorbed by the zlib decoder, so the
        compressed pipelines do *not* expose the bug.
      * ``f32_odd`` — an odd-length chunk, so the checksum's trailing-byte
        branch runs. It is also the only single-chunk dataset here (7 B raw,
        11 B stored), which covers the **single-chunk** index: that path takes
        the chunk's length from the layout message's filtered size rather than
        from the chunk shape, and a reader using the shape would read 7 bytes,
        split at 3, and fail the checksum. The others hold two chunks and go
        through a fixed array.

    A separate builder rather than an extension of the two general fixtures:
    adding datasets to those would shift every later dataset's decode index and
    rewrite their oracles for a change that has nothing to do with them."""
    path = FIXturesDir / name
    values = np.arange(64, dtype="<f4").reshape(8, 8) * 0.5
    odd = np.arange(7, dtype="u1")

    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass fletcher32 fixture")

        def ds(nm, data, **kw):
            return f.create_dataset(nm, data=data, track_times=False, **kw)

        ds("plain_deflate", values, chunks=(4, 8), compression="gzip", compression_opts=4)
        ds("f32_deflate", values, chunks=(4, 8), compression="gzip", compression_opts=4,
           fletcher32=True)
        ds("f32_only", values, chunks=(4, 8), fletcher32=True)
        ds("f32_shuffle_deflate", values, chunks=(4, 8), compression="gzip",
           compression_opts=4, shuffle=True, fletcher32=True)
        ds("f32_shuffle", values, chunks=(4, 8), shuffle=True, fletcher32=True)
        ds("f32_odd", odd, chunks=(7,), fletcher32=True)

    # Fail loudly if libhdf5 did not actually apply the filter, or applied it
    # somewhere other than last: either would leave a fixture that passes the
    # test while proving nothing about fletcher32.
    with h5py.File(path, "r") as f:
        for nm in ("f32_deflate", "f32_only", "f32_shuffle_deflate", "f32_shuffle", "f32_odd"):
            plist = f[nm].id.get_create_plist()
            ids = [plist.get_filter(i)[0] for i in range(plist.get_nfilters())]
            if not ids or ids[-1] != 3:
                raise SystemExit(f"{nm}: expected fletcher32 (3) last, got {ids}")
        # With no compressor the suffix is directly visible: stored == raw + 4.
        raw_bytes = int(np.prod(f["f32_only"].chunks)) * f["f32_only"].dtype.itemsize
        stored = f["f32_only"].id.get_chunk_info(0).size
        if stored != raw_bytes + 4:
            raise SystemExit(f"f32_only: stored chunk {stored} B, expected {raw_bytes + 4} B "
                             f"— fletcher32 is not a 4-byte suffix on this libhdf5")

        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), libver='latest'",
            "superblock_version": path.read_bytes()[path.read_bytes().index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "fletcher32 checksum filter (id 3) alone and composed with "
                    "deflate and shuffle; `plain_deflate` holds the same values "
                    "with no checksum, as the comparison baseline (#412)",
            "raw_markers": {
                "f32_only_stored_chunk_bytes": stored,
                "f32_only_raw_chunk_bytes": raw_bytes,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle "
          f"[fletcher32; f32_only chunk {raw_bytes} B raw -> {stored} B stored]")


def build_zstd(name: str) -> None:
    """Datasets compressed with the **zstd** filter (id 32015, #413).

    zstd landed in netcdf-c 4.9 and is DKRZ's recommended compressor for new
    climate archives, so it turns up in recent NetCDF-4 output. h5py cannot
    write it on its own — libhdf5 needs the plugin `hdf5plugin` supplies — so
    this builder is the one here with a dependency beyond h5py/numpy.

    Same shape as the fletcher32 fixture, and for the same reason: all the
    datasets hold identical values so the test compares them directly rather
    than against transcribed numbers.

      * ``plain_deflate`` — deflate alone, the comparison baseline.
      * ``zstd`` — zstd alone.
      * ``shuffle_zstd`` — the composition netcdf-c actually writes, and the
        one that pins the *order*: zstd is applied after shuffle, so reading
        must undo zstd first. A reader that ran them the other way round would
        fail loudly here rather than quietly returning rearranged bytes.
      * ``zstd_fletcher32`` — zstd under a checksum (#412), so the two filters
        added most recently are exercised together.
      * ``zstd_high`` — level 19, a different frame layout from the default.
    """
    import hdf5plugin  # noqa: PLC0415 - optional, only this fixture needs it

    path = FIXturesDir / name
    values = np.arange(64, dtype="<f4").reshape(8, 8) * 0.5

    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass zstd fixture")

        def ds(nm, **kw):
            return f.create_dataset(nm, data=values, chunks=(4, 8), track_times=False, **kw)

        ds("plain_deflate", compression="gzip", compression_opts=4)
        ds("zstd", **hdf5plugin.Zstd(clevel=3))
        ds("shuffle_zstd", shuffle=True, **hdf5plugin.Zstd(clevel=3))
        ds("zstd_fletcher32", fletcher32=True, **hdf5plugin.Zstd(clevel=3))
        ds("zstd_high", **hdf5plugin.Zstd(clevel=19))

    # Fail the build rather than emit a fixture that proves nothing: confirm
    # libhdf5 really applied filter 32015, and that `shuffle_zstd` carries both
    # filters in the order the test's ordering claim depends on.
    with h5py.File(path, "r") as f:
        for nm in ("zstd", "shuffle_zstd", "zstd_fletcher32", "zstd_high"):
            plist = f[nm].id.get_create_plist()
            ids = [plist.get_filter(i)[0] for i in range(plist.get_nfilters())]
            if 32015 not in ids:
                raise SystemExit(f"{nm}: zstd (32015) not applied, pipeline is {ids}")
        plist = f["shuffle_zstd"].id.get_create_plist()
        ids = [plist.get_filter(i)[0] for i in range(plist.get_nfilters())]
        if ids != [2, 32015]:
            raise SystemExit(f"shuffle_zstd: expected [2, 32015], got {ids}")

        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                      f"hdf5plugin {hdf5plugin.version}, libver='latest'",
            "superblock_version": path.read_bytes()[
                path.read_bytes().index(b"\x89HDF\r\n\x1a\n") + 8],
            "note": "zstd filter (id 32015) alone and composed with shuffle and "
                    "fletcher32; `plain_deflate` holds the same values without "
                    "zstd, as the comparison baseline (#413)",
            "raw_markers": {
                "shuffle_zstd_pipeline": ids,
                "zstd_stored_chunk_bytes": f["zstd"].id.get_chunk_info(0).size,
            },
            "objects": {n: dataset_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle [zstd 32015; "
          f"shuffle_zstd pipeline {ids}]")


def _bundled_lib(stem: str) -> str:
    """Path of a shared library the h5py wheel bundles (``h5py.libs/``)."""
    import glob
    import os

    libdir = Path(h5py.__file__).parent.parent / "h5py.libs"
    found = sorted(glob.glob(os.fspath(libdir / f"{stem}-*.so*")))
    if not found:
        raise SystemExit(f"no bundled {stem} under {libdir}: this h5py wheel has no szip")
    return found[0]


def bundled_libaec_version() -> str:
    """The libaec release the h5py wheel's libsz is built from.

    The wheel does not export a version call, so read it from the source path
    compiled into the library (``/tmp/libaec-v1.1.4/src/vector.c``)."""
    import re

    raw = Path(_bundled_lib("libaec")).read_bytes()
    m = re.search(rb"libaec-v(\d+\.\d+\.\d+)", raw)
    if not m:
        raise SystemExit("cannot find the libaec version in the bundled library")
    return m.group(1).decode()


def szip_compress(raw: bytes, mask: int, bpp: int, ppb: int, pps: int) -> bytes:
    """One chunk as libhdf5's szip filter stores it: a 4-byte little-endian
    uncompressed size, then the stream from the wheel's own libsz
    (``SZ_BufftoBuffCompress``, the call ``H5Zszip.c`` makes)."""
    import ctypes
    import struct

    class SzCom(ctypes.Structure):
        _fields_ = [("options_mask", ctypes.c_int), ("bits_per_pixel", ctypes.c_int),
                    ("pixels_per_block", ctypes.c_int), ("pixels_per_scanline", ctypes.c_int)]

    libsz = ctypes.CDLL(_bundled_lib("libsz"))
    params = SzCom(mask, bpp, ppb, pps)
    cap = ctypes.c_size_t(len(raw) * 40 + 1024)
    dst = ctypes.create_string_buffer(cap.value)
    rc = libsz.SZ_BufftoBuffCompress(dst, ctypes.byref(cap), raw, ctypes.c_size_t(len(raw)),
                                     ctypes.byref(params))
    if rc != 0:
        raise SystemExit(f"SZ_BufftoBuffCompress failed ({rc}) for {(mask, bpp, ppb, pps)}")
    return struct.pack("<I", len(raw)) + dst.raw[:cap.value]


def szip_oracle(d: h5py.Dataset) -> dict:
    """Full read-back of one dataset through libhdf5, plus the pipeline and
    per-chunk filter masks the file really carries, so the Rust test compares
    every value and can tell which datasets exercise what."""
    plist = d.id.get_create_plist()
    filters = [plist.get_filter(i) for i in range(plist.get_nfilters())]
    chunks = [d.id.get_chunk_info(i) for i in range(d.id.get_num_chunks())]
    szip = next(cd for fid, _, cd, _ in filters if fid == 4)
    mask, ppb, bpp, pps = szip
    values = np.asarray(d[()]).reshape(-1)
    return {
        "dtype": d.dtype.str,
        "precision_bits": d.id.get_type().get_precision(),
        "shape": list(d.shape),
        "chunks": list(d.chunks),
        "pipeline": [{"id": fid, "cd_values": list(cd)} for fid, _, cd, _ in filters],
        "szip": {"options_mask": mask, "pixels_per_block": ppb, "bits_per_pixel": bpp,
                 "pixels_per_scanline": pps, "rsi": -(-pps // ppb),
                 "padded": pps % ppb != 0},
        "chunk_filter_masks": [c.filter_mask for c in chunks],
        # Where each stored chunk starts, so a test can alter one in place.
        "chunk_byte_offsets": [c.byte_offset for c in chunks],
        "chunk_stored_sizes": [c.size for c in chunks],
        "values": [v.item() for v in values],
    }


def write_szip_oracle(path: Path, note: str, extra: dict | None = None) -> dict:
    with h5py.File(path, "r") as f:
        objects = {n: szip_oracle(f[n]) for n in f if isinstance(f[n], h5py.Dataset)}
    oracle = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}, "
                  f"libaec {bundled_libaec_version()} as bundled libsz)",
        "note": note,
        **(extra or {}),
        "objects": objects,
    }
    (FIXturesDir / f"{path.name}.oracle.json").write_text(
        json.dumps(oracle, indent=1) + "\n", encoding="utf-8")
    return objects


def build_szip(name: str) -> None:
    """Datasets compressed with the **szip** filter (id 4, #421), as libhdf5
    writes them.

    libhdf5 2.0 in the h5py wheel writes szip itself: the wheel bundles libaec's
    libsz. ``compression_opts=(coding, pixels_per_block)`` is all a writer
    chooses; libhdf5's ``set_local`` fills in the rest of ``cd_values``, which
    it stores as ``(mask, pixels_per_block, bits_per_pixel,
    pixels_per_scanline)``: bits per pixel from the datatype's precision, and
    pixels per scanline from the chunk's fastest dimension, capped at 128
    blocks, or the whole chunk when that dimension is shorter than a block.

    Each dataset covers one row of the issue's matrix. The oracle is the h5py
    read-back of every value, and the builder refuses to write a fixture whose
    parameters are not the ones its name promises.
    """
    path = FIXturesDir / name
    rng = np.random.default_rng(421)

    def ramp(n, dtype, scale=1.0, offset=0.0):
        # Smooth with a little noise, so every coding option turns up.
        x = np.arange(n) * scale + offset + rng.integers(-3, 4, n)
        return x.astype(dtype)

    # name: (data, chunks, coding, ppb, extra kwargs, expected cd_values)
    specs = {
        "i2_ppb16": (ramp(512, "<i2", 5, -900).reshape(16, 32), (8, 32), "nn", 16, {},
                     (169, 16, 16, 32)),
        "i2_ppb10": (ramp(240, "<i2", 3, -300).reshape(6, 40), (6, 40), "nn", 10, {},
                     (169, 10, 16, 40)),
        "i4be_ppb32": (ramp(512, ">i4", 1000, -200000).reshape(8, 64), (8, 64), "nn", 32, {},
                       (177, 32, 32, 64)),
        "f4_ppb18": ((np.sin(np.arange(144) / 9.0) * 300).astype("<f4").reshape(4, 36),
                     (4, 36), "nn", 18, {}, (169, 18, 32, 36)),
        "f8_ppb8": ((np.cos(np.arange(128) / 7.0) * 1e5).astype("<f8").reshape(8, 16),
                    (4, 16), "nn", 8, {}, (169, 8, 64, 16)),
        "f8_ppb18": ((np.arange(100) * 0.125 - 3.0).astype("<f8").reshape(5, 20),
                     (5, 20), "nn", 18, {}, (169, 18, 64, 20)),
        "u1_ppb8": ((np.arange(256) // 3 % 200).astype("u1").reshape(8, 32), (8, 32), "nn", 8,
                    {}, (169, 8, 8, 32)),
        "shuffle_szip": (ramp(256, "<i4", 17, 5000).reshape(8, 32), (8, 32), "nn", 16,
                         {"shuffle": True}, (169, 16, 32, 32)),
        # 25 is not a multiple of 16: every scanline carries 7 pad pixels.
        "pps_not_multiple": (ramp(100, "<i2", 11, 40).reshape(4, 25), (4, 25), "ec", 16, {},
                             (141, 16, 16, 25)),
        # 1,500 > 8 × 128, so a scanline is 1,024 pixels and the chunk's 3,000
        # end part-way through the third.
        "partial_scanline": (ramp(3000, "<i2", 1, -1500).reshape(2, 1500), (2, 1500), "nn", 8,
                             {}, (169, 8, 16, 1024)),
        # The same scanline cap on byte planes: 5,000 and 1,500 pixels in one
        # chunk, capped at 4,096 and 1,024 per scanline, so the last scanline
        # is partial by more than a block. libsz pads it to a whole scanline
        # all the same; a decoder that expected only the last block completed
        # refused these (#421 review).
        "f4_pps_capped": ((np.arange(5000) % 7).astype("<f4"), (5000,), "nn", 32, {},
                          (169, 32, 32, 4096)),
        "f8_pps_capped": ((np.arange(1500) % 11 * 0.25).astype("<f8"), (1500,), "nn", 8, {},
                          (169, 8, 64, 1024)),
        # Random bytes do not compress, so libhdf5 stores the chunk as it is
        # and sets bit 0 of its filter mask (szip is an optional filter).
        "incompressible": (rng.integers(0, 256, 64, dtype="u1").reshape(8, 8), (8, 8), "ec", 8,
                           {}, (141, 8, 8, 8)),
    }

    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass szip fixture")
        for nm, (data, chunks, coding, ppb, kw, _) in specs.items():
            f.create_dataset(nm, data=data, dtype=data.dtype, chunks=chunks, compression="szip",
                             compression_opts=(coding, ppb), track_times=False, **kw)
        # A 16-bit integer in a 4-byte container: libhdf5 codes it with 16 bits
        # per pixel, so the szip pixel is half the element (decision D2).
        # Non-negative values only: the reader does not apply a fixed-point
        # precision yet, and this dataset is about the filter.
        t = h5py.h5t.STD_I32LE.copy()
        t.set_precision(16)
        p16 = f.create_dataset("i4_precision16", shape=(8, 16), dtype=h5py.Datatype(t),
                               chunks=(8, 16), compression="szip", compression_opts=("nn", 8),
                               track_times=False)
        p16[...] = (np.arange(128) * 211 % 30000).reshape(8, 16)
    specs["i4_precision16"] = (None, None, None, None, None, (169, 8, 16, 16))

    # Refuse a fixture that does not test what its name says.
    with h5py.File(path, "r") as f:
        for nm, spec in specs.items():
            plist = f[nm].id.get_create_plist()
            filters = [plist.get_filter(i) for i in range(plist.get_nfilters())]
            cd = next((cd for fid, _, cd, _ in filters if fid == 4), None)
            if cd != spec[-1]:
                raise SystemExit(f"{nm}: szip cd_values {cd}, expected {spec[-1]}")
            masks = [f[nm].id.get_chunk_info(i).filter_mask
                     for i in range(f[nm].id.get_num_chunks())]
            want = 1 if nm == "incompressible" else 0
            if set(masks) != {want}:
                raise SystemExit(f"{nm}: chunk filter masks {masks}, expected all {want}")
        if [f["shuffle_szip"].id.get_create_plist().get_filter(i)[0] for i in range(2)] != [2, 4]:
            raise SystemExit("shuffle_szip: expected the pipeline [shuffle, szip]")

    objects = write_szip_oracle(
        path, "szip filter (id 4) as libhdf5 writes it; `values` is the h5py read-back "
              "of every element (#421)")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle [{len(objects)} szip datasets]")


def build_szip_hand(name: str) -> None:
    """szip chunks libhdf5 reads but its writer never produces (#421).

    Each chunk is compressed by the wheel's own libsz (``szip_compress``) and
    stored with ``write_direct_chunk``; libhdf5 then reads every one back, and
    that read-back is the oracle.

      * ``rsi1_i2`` and ``rsi1_f4``: fewer pixels per scanline than per block,
        so every scanline is one block (RSI 1) that is mostly padding. libhdf5's
        ``set_local`` never picks such a scanline (it falls back to the whole
        chunk), so the builder writes the dataset with a legal one and then
        patches ``pixels_per_scanline`` in the stored ``cd_values``. The file is
        ``libver='earliest'`` for that reason: version-1 object headers carry
        no checksum, so the patched message stays valid. ``rsi1_f4`` is one
        pixel per scanline at 32 pixels per block, the case where libsz
        decodes into a padded copy 32 times the output.
      * ``deflate_szip``: szip applied *after* deflate, a length-changing
        filter, so the prefix is the deflate stream's length, not the chunk's.
        libhdf5 writes this pipeline, but szip never shrinks a deflate stream,
        so as an optional filter it is skipped on every chunk. The chunks are
        compressed here instead, with filter mask 0.
    """
    import struct
    import zlib

    path = FIXturesDir / name
    rsi1_i2 = (np.arange(40, dtype="<i2") * 7 - 50).reshape(8, 5)
    rsi1_f4 = (np.arange(64, dtype="<f4") * 0.75 - 4.0).reshape(4, 16)
    deflate_szip = (np.arange(128) // 3).astype("u1")

    with h5py.File(path, "w", libver="earliest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass hand-built szip fixture")
        f.create_dataset("rsi1_i2", data=rsi1_i2, chunks=(8, 5), compression="szip",
                         compression_opts=("nn", 16), track_times=False)
        f.create_dataset("rsi1_f4", data=rsi1_f4, chunks=(4, 16), compression="szip",
                         compression_opts=("nn", 32), track_times=False)
        dcpl = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
        dcpl.set_chunk((64,))
        dcpl.set_deflate(4)
        dcpl.set_szip(h5py.h5z.SZIP_NN_OPTION_MASK, 8)
        dcpl.set_obj_track_times(False)
        h5py.h5d.create(f.id, b"deflate_szip", h5py.h5t.STD_U8LE,
                        h5py.h5s.create_simple((128,)), dcpl=dcpl)
        written = {nm: f[nm].id.get_create_plist().get_filter(0 if nm != "deflate_szip" else 1)[2]
                   for nm in ("rsi1_i2", "rsi1_f4", "deflate_szip")}

    # Patch pixels_per_scanline below pixels_per_block.
    patches = {"rsi1_i2": 5, "rsi1_f4": 1}
    raw = bytearray(path.read_bytes())
    for nm, pps in patches.items():
        old = struct.pack("<4I", *written[nm])
        if raw.count(old) != 1:
            raise SystemExit(f"{nm}: cd_values {written[nm]} not unique in the file")
        at = raw.index(old)
        raw[at:at + 16] = struct.pack("<4I", *written[nm][:3], pps)
    path.write_bytes(bytes(raw))

    with h5py.File(path, "r+") as f:
        for nm, data in (("rsi1_i2", rsi1_i2), ("rsi1_f4", rsi1_f4)):
            mask, ppb, bpp, pps = f[nm].id.get_create_plist().get_filter(0)[2]
            if pps != patches[nm] or pps >= ppb:
                raise SystemExit(f"{nm}: patch did not land, cd_values {(mask, ppb, bpp, pps)}")
            f[nm].id.write_direct_chunk((0, 0), szip_compress(data.tobytes(), mask, bpp, ppb, pps), 0)
        mask, ppb, bpp, pps = written["deflate_szip"]
        for i in range(2):
            part = deflate_szip[i * 64:(i + 1) * 64].tobytes()
            f["deflate_szip"].id.write_direct_chunk(
                (i * 64,), szip_compress(zlib.compress(part, 4), mask, bpp, ppb, pps), 0)

    with h5py.File(path, "r") as f:
        for nm, data in (("rsi1_i2", rsi1_i2), ("rsi1_f4", rsi1_f4),
                         ("deflate_szip", deflate_szip)):
            if not np.array_equal(f[nm][()], data):
                raise SystemExit(f"{nm}: libhdf5 does not read back what was written")

    objects = write_szip_oracle(
        path, "szip chunks compressed with the wheel's libsz and stored with "
              "write_direct_chunk: scanlines shorter than a block (cd_values patched) "
              "and szip after deflate; `values` is the h5py read-back (#421)")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle [{len(objects)} hand-built szip datasets]")


def build_szip_growth(name: str) -> None:
    """A ``[szip, deflate]`` chunk whose szip stream is 24 times the chunk
    (#813).

    The reader bounds a codec behind a length-changing filter by that
    filter's worst-case growth, and szip's is large: libsz pads every
    scanline to whole blocks, so a scanline of one pixel at 32 pixels per
    block is coded as a 32-pixel block. ``i2_growth`` is one 4,096-byte
    ``<i2`` chunk of 32767s at 32 pixels per block, entropy coding without
    the NN preprocessor (which would predict a constant chunk exactly), with
    pixels per scanline
    patched to 1 as in ``build_szip_hand``; its szip stream is 99,076 bytes,
    which deflate stores in about a kilobyte. A bound of the chunk plus an
    eighth plus 4 KiB (8,704 bytes) would refuse it, and libhdf5 reads it,
    so this pins that the reader's szip factor is large enough. The file is
    ``libver='earliest'`` for the patch, as ``hdf5_szip_hand.h5`` is.
    """
    import struct
    import zlib

    path = FIXturesDir / name
    data = np.full(2048, np.iinfo("<i2").max, dtype="<i2")
    with h5py.File(path, "w", libver="earliest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass szip chunk that grows 24 times")
        dcpl = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
        dcpl.set_chunk((2048,))
        # EC, not NN: with NN a constant chunk predicts exactly and compresses.
        dcpl.set_szip(h5py.h5z.SZIP_EC_OPTION_MASK, 32)
        dcpl.set_deflate(4)
        dcpl.set_obj_track_times(False)
        h5py.h5d.create(f.id, b"i2_growth", h5py.h5t.STD_I16LE,
                        h5py.h5s.create_simple((2048,)), dcpl=dcpl)
        written = f["i2_growth"].id.get_create_plist().get_filter(0)[2]
    raw = bytearray(path.read_bytes())
    old = struct.pack("<4I", *written)
    if raw.count(old) != 1:
        raise SystemExit(f"i2_growth: cd_values {written} not unique in the file")
    at = raw.index(old)
    raw[at:at + 16] = struct.pack("<4I", *written[:3], 1)
    path.write_bytes(bytes(raw))

    with h5py.File(path, "r+") as f:
        d = f["i2_growth"]
        plist = d.id.get_create_plist()
        if [plist.get_filter(i)[0] for i in range(plist.get_nfilters())] != [4, 1]:
            raise SystemExit("i2_growth: the pipeline is not [szip, deflate]")
        mask, ppb, bpp, pps = plist.get_filter(0)[2]
        if (ppb, bpp, pps) != (32, 16, 1):
            raise SystemExit(f"i2_growth: cd_values {(mask, ppb, bpp, pps)}")
        stream = szip_compress(data.tobytes(), mask, bpp, ppb, pps)
        if len(stream) <= data.nbytes + data.nbytes // 8 + 4096:
            raise SystemExit(f"i2_growth: the szip stream ({len(stream)} B) no longer "
                             "grows past the chunk plus an eighth plus 4 KiB")
        d.id.write_direct_chunk((0,), zlib.compress(stream, 4), 0)
    with h5py.File(path, "r") as f:
        if not np.array_equal(f["i2_growth"][()], data):
            raise SystemExit("i2_growth: libhdf5 does not read back what was written")
    write_szip_oracle(
        path, "one 4,096-byte <i2 chunk, pipeline [szip, deflate], pixels per scanline "
              f"patched to 1 at 32 per block: its szip stream is {len(stream)} bytes; "
              "`values` is the h5py read-back (#813)")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle, szip stream {len(stream)} B")


def build_szip_long_stream(name: str) -> None:
    """A 64-bit szip chunk whose stream codes more pixels than the chunk holds
    (#794, #421).

    ``f8_long_stream`` is one 4x8 ``<f8`` chunk (256 bytes) with the right
    size prefix, 256, and no filter before szip, so every length rule the
    reader applies passes. Its stream, though, is libsz's encoding of 64
    values, twice the chunk. 64-bit pixels are coded as byte planes laid out
    by the output's length, so decoding 256 bytes of a 512-byte stream puts
    most bytes in the wrong place. libhdf5 reads it without complaint: the
    oracle records that read-back beside the 32 values the chunk should hold,
    and the builder checks the two differ. The reader must refuse the chunk.
    """
    import struct

    path = FIXturesDir / name
    values = (np.arange(64, dtype="<f8") * 0.5 - 7.0)
    chunk = values[:32].reshape(4, 8)
    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass szip chunk with a stream too long")
        f.create_dataset("f8_long_stream", data=chunk, chunks=(4, 8), compression="szip",
                         compression_opts=("nn", 8), track_times=False)
    with h5py.File(path, "r+") as f:
        d = f["f8_long_stream"]
        mask, ppb, bpp, pps = d.id.get_create_plist().get_filter(0)[2]
        if (mask, ppb, bpp, pps) != (169, 8, 64, 8):
            raise SystemExit(f"f8_long_stream: cd_values {(mask, ppb, bpp, pps)}")
        stream = szip_compress(values.tobytes(), mask, bpp, ppb, pps)[4:]
        d.id.write_direct_chunk((0, 0), struct.pack("<I", chunk.nbytes) + stream, 0)
    with h5py.File(path, "r") as f:
        read = f["f8_long_stream"][()]
        if np.array_equal(read, chunk):
            raise SystemExit("f8_long_stream: libhdf5 read the chunk correctly; "
                             "the fixture no longer shows the scramble")
        wrong = int((read.view("u1") != chunk.view("u1")).sum())
    write_szip_oracle(
        path, "one 256-byte <f8 chunk whose szip stream codes 64 values, twice the chunk; "
              "`values` is libhdf5's scrambled read-back, `source` what the chunk should "
              "hold. The reader must refuse it (#794, #421)",
        extra={"source_values": chunk.reshape(-1).tolist(), "wrong_bytes": wrong})
    print(f"wrote {path} ({path.stat().st_size} B) + oracle [{wrong} of {chunk.nbytes} bytes "
          f"differ in libhdf5's read-back]")


def _find_all(raw: bytes, sig: bytes) -> list[int]:
    out, i = [], 0
    while (i := raw.find(sig, i)) >= 0:
        out.append(i)
        i += 1
    return out


def build_phony_dims(name: str) -> None:
    """A scale-less file pinning netCDF-C's anonymous-dimension numbering (#533).

    Written by plain ``h5py``, so no dataset carries a ``DIMENSION_SCALE`` class
    and none carries a ``DIMENSION_LIST``: every axis has to be invented. The
    shapes are chosen so the numbering rule cannot be satisfied by a simpler one
    than netCDF-C's actual per-axis reuse:

      * ``a_8x8`` and ``b_8x8`` share a shape, so the second must *reuse* the
        first's pair rather than allocate more;
      * ``a_8x8``'s two axes are both 8 long and still need two dimensions,
        which rules out deduplicating by length alone;
      * ``d_6x4`` is ``c_4x6`` transposed, so it reuses the same pair in the
        other order — which rules out matching whole shapes;
      * ``e_1d7`` is 1-D, and is excluded from the render list for being so
        while its dimension still appears in the table.

    Read back through netCDF4-python, netCDF-C names these
    ``phony_dim_0..4`` = 8, 8, 4, 6, 7. Datasets are created in an order that
    differs from their alphabetical order, because netCDF-C numbers by *name*
    and a builder that agreed by accident would hide the difference.
    """
    path = FIXturesDir / name
    with h5py.File(path, "w", libver="latest") as f:
        # Deliberately not in alphabetical order.
        f.create_dataset("e_1d7", data=np.arange(7, dtype="u1"), track_times=False)
        f.create_dataset("d_6x4", data=np.arange(24, dtype="f4").reshape(6, 4),
                         track_times=False)
        f.create_dataset("c_4x6", data=np.arange(24, dtype="f4").reshape(4, 6),
                         track_times=False)
        f.create_dataset("b_8x8", data=np.arange(64, dtype="f4").reshape(8, 8),
                         track_times=False)
        f.create_dataset("a_8x8", data=(np.arange(64, dtype="f4") * 0.5).reshape(8, 8),
                         track_times=False)
    with h5py.File(path, "r") as f:
        for n in f:
            if "DIMENSION_LIST" in f[n].attrs or "CLASS" in f[n].attrs:
                raise SystemExit(f"{name}: {n} carries dimension-scale metadata; "
                                 f"this fixture must have none")
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                      f"libver='latest'",
            "note": "scale-less datasets; every axis is an invented phony dimension "
                    "(#533). Expected netCDF-C numbering: phony_dim_0..4 = 8,8,4,6,7.",
            "objects": {n: dataset_oracle(f[n]) for n in f
                        if isinstance(f[n], h5py.Dataset)},
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} + oracle")


def build_fixed_point_precision(name: str) -> None:
    """Integers that do not fill their container, and a float that is not IEEE (#795).

    An HDF5 fixed-point datatype carries a bit offset and a bit precision (file
    format spec IV.A.2.d, "Fixed-Point Property Description"): the value is the
    ``precision`` bits starting ``offset`` bits above the least significant
    bit, the bits below and above are padding (zeros or ones, per the lo_pad /
    hi_pad flags), and a signed value's sign bit is the top bit of the
    precision. h5py's high-level API cannot ask for such a type, so each one is
    built through ``h5py.h5t``: copy a standard type, ``set_precision``,
    ``set_offset``, ``set_pad``. libhdf5 converts the native values into it on
    write and back on read, so what h5py reads back is the value oracle.

    Integer datasets, all contiguous unless noted:

      * ``i32_prec16`` — signed, 16 bits at offset 0 in a little-endian 32-bit
        container: the issue's own case, negatives included.
      * ``i32_prec12_off4`` / ``i32be_prec12_off4`` — signed, 12 bits at offset
        4, little- and big-endian, down to the precision's extremes.
      * ``u32_prec12_off3`` — unsigned, so the precision's top bit is a
        magnitude, not a sign.
      * ``u16be_prec10_off5`` — unsigned, big-endian, 16-bit container.
      * ``i8_prec5_off2`` — signed, one-byte container.
      * ``i64_prec40_off8`` — signed, 64-bit container.
      * ``i32_prec12_off4_pad_ones`` — padding written as ones on both sides,
        which a mask must still discard.
      * ``i32_prec12_fill`` — chunked with only its first chunk written, so the
        rest reads as the Fill Value message's default (-7, pad ones), which is
        stored in the same packed form.
      * ``i32_prec12_masked`` — carries a ``_FillValue`` attribute of the same
        packed type (-1), so masking compares decoded values on both sides.

    The root group also carries ``attr_i32_prec12_off4`` (signed, [-20, 300])
    and ``attr_u32_prec12_off3`` (unsigned, 4095): attribute values are read by
    the same rule.

    ``f32_prec24`` is a 24-bit float in a 32-bit container (sign at bit 23,
    7-bit exponent at 16, 16-bit mantissa at 0, bias 63). h5py reads it back;
    a reader that took the container as an IEEE ``f32`` would return garbage,
    so this one is expected to be refused as unsupported rather than misread.
    """
    from h5py import h5d, h5p, h5s, h5t  # noqa: PLC0415

    path = FIXturesDir / name

    def packed(base, precision, offset, pad=h5t.PAD_ZERO):
        t = base.copy()
        t.set_precision(precision)
        t.set_offset(offset)
        t.set_pad(pad, pad)
        return t

    ints = {
        "i32_prec16": (h5t.STD_I32LE, 16, 0, h5t.PAD_ZERO, np.int32,
                       [-20, 5, -1, 300, -32768, 32767]),
        "i32_prec12_off4": (h5t.STD_I32LE, 12, 4, h5t.PAD_ZERO, np.int32,
                            [-20, 5, -1, 300, -2048, 2047]),
        "i32be_prec12_off4": (h5t.STD_I32BE, 12, 4, h5t.PAD_ZERO, np.int32,
                              [-20, 5, -1, 300, -2048, 2047]),
        "u32_prec12_off3": (h5t.STD_U32LE, 12, 3, h5t.PAD_ZERO, np.uint32,
                            [0, 5, 4095, 300, 2048, 1]),
        "u16be_prec10_off5": (h5t.STD_U16BE, 10, 5, h5t.PAD_ZERO, np.uint16,
                              [0, 1, 1023, 512]),
        "i8_prec5_off2": (h5t.STD_I8LE, 5, 2, h5t.PAD_ZERO, np.int8,
                          [-16, -1, 0, 15, 7]),
        "i64_prec40_off8": (h5t.STD_I64LE, 40, 8, h5t.PAD_ZERO, np.int64,
                            [-(2**39), 2**39 - 1, -20, 0]),
        "i32_prec12_off4_pad_ones": (h5t.STD_I32LE, 12, 4, h5t.PAD_ONE, np.int32,
                                     [-20, 5, -1, 300]),
    }

    with h5py.File(path, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass fixed-point precision fixture")

        def dcpl():
            plist = h5p.create(h5p.DATASET_CREATE)
            plist.set_obj_track_times(False)
            return plist

        for nm, (base, prec, off, pad, native, vals) in ints.items():
            arr = np.asarray(vals, dtype=native)
            d = h5d.create(f.id, nm.encode(), packed(base, prec, off, pad),
                           h5s.create_simple(arr.shape), dcpl=dcpl())
            d.write(h5s.ALL, h5s.ALL, arr)

        # Chunked, first chunk written, the rest left to the fill value.
        plist = dcpl()
        plist.set_chunk((2,))
        plist.set_fill_value(np.array(-7, dtype=np.int32))
        fill_t = packed(h5t.STD_I32LE, 12, 4, h5t.PAD_ONE)
        h5d.create(f.id, b"i32_prec12_fill", fill_t, h5s.create_simple((6,)), dcpl=plist)
        f["i32_prec12_fill"][0:2] = np.array([-20, 5], dtype=np.int32)

        # A `_FillValue` attribute of the same packed type.
        mask_t = packed(h5t.STD_I32LE, 12, 4)
        arr = np.array([-20, -1, 5], dtype=np.int32)
        d = h5d.create(f.id, b"i32_prec12_masked", mask_t, h5s.create_simple(arr.shape),
                       dcpl=dcpl())
        d.write(h5s.ALL, h5s.ALL, arr)
        a = h5py.h5a.create(d, b"_FillValue", mask_t, h5s.create(h5s.SCALAR))
        a.write(np.array(-1, dtype=np.int32))

        for attr_name, t, native, vals in [
            ("attr_i32_prec12_off4", packed(h5t.STD_I32LE, 12, 4), np.int32, [-20, 300]),
            ("attr_u32_prec12_off3", packed(h5t.STD_U32LE, 12, 3), np.uint32, [4095]),
        ]:
            arr = np.asarray(vals, dtype=native)
            a = h5py.h5a.create(f.id, attr_name.encode(), t, h5s.create_simple(arr.shape))
            a.write(arr)

        # A 24-bit float in a 32-bit container.
        ft = h5t.IEEE_F32LE.copy()
        ft.set_fields(23, 16, 7, 0, 16)
        ft.set_ebias(63)
        ft.set_precision(24)
        arr = np.array([1.5, -2.25, 0.0], dtype=np.float32)
        d = h5d.create(f.id, b"f32_prec24", ft, h5s.create_simple(arr.shape), dcpl=dcpl())
        d.write(h5s.ALL, h5s.ALL, arr)

    raw = path.read_bytes()
    with h5py.File(path, "r") as f:
        objects = {}
        for nm in f:
            d = f[nm]
            t = d.id.get_type()
            entry = {
                "class": "floating-point" if isinstance(t, h5t.TypeFloatID) else
                         "fixed-point",
                "size_bytes": t.get_size(),
                "byte_order": "big-endian" if t.get_order() == h5t.ORDER_BE
                              else "little-endian",
                "bit_offset": t.get_offset(),
                "bit_precision": t.get_precision(),
                "pad": ["one" if p == h5t.PAD_ONE else "zero" for p in t.get_pad()],
                "values": d[()].tolist(),
            }
            if isinstance(t, h5t.TypeIntegerID):
                entry["signed"] = t.get_sign() == h5t.SGN_2
            else:
                spos, epos, esize, mpos, msize = t.get_fields()
                entry.update(sign_location=spos, exponent_location=epos,
                             exponent_size=esize, mantissa_location=mpos,
                             mantissa_size=msize, exponent_bias=t.get_ebias(),
                             expected="refused: not an IEEE layout")
            if d.chunks is None:
                off = d.id.get_offset()
                entry["stored_hex"] = raw[off:off + d.id.get_storage_size()].hex()
            if "_FillValue" in d.attrs:
                entry["fill_value_attribute"] = int(d.attrs["_FillValue"])
            objects[nm] = entry
        # The packed extremes and the padding are what the fixture exists to
        # show: fail the build if libhdf5 wrote a plain full-width type.
        if objects["i32_prec12_off4_pad_ones"]["stored_hex"][:8] != "cffeffff":
            raise SystemExit("pad-ones dataset was not stored with one-padding")
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                      f"libver='latest'",
            "note": "fixed-point types with a non-default bit offset / precision and "
                    "padding, plus one non-IEEE float (#795). `values` is what h5py "
                    "reads back; `stored_hex` is the contiguous data as written.",
            "attributes": {k: np.asarray(v).tolist() for k, v in f.attrs.items()
                           if k.startswith("attr_")},
            "objects": objects,
        }
    (FIXturesDir / f"{name}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {path} ({path.stat().st_size} B) + oracle [fixed-point precision]")


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
    # Version-4 layout: single-chunk and unfiltered fixed-array chunk indexes.
    build_v4_chunk_index("hdf5_v4_chunk_index.h5")
    # Version-4 extensible array (unlimited dimension): direct + secondary blocks.
    build_extensible_array("hdf5_ea_chunk_index.h5")
    # Version-4 *filtered* extensible array (unlimited dimension + gzip/shuffle):
    # index-block, direct, and secondary-block-located data blocks.
    build_extensible_array_filtered("hdf5_ea_filtered.h5")
    # Version-4 implicit index (fixed-shape, early-allocated, unfiltered chunks).
    build_implicit_index("hdf5_implicit_index.h5")
    # Version-4 v2 B-tree index (>1 unlimited dimension): unfiltered + filtered.
    build_v2_btree("hdf5_v2_btree_index.h5")
    # fletcher32 checksum filter (id 3), alone and composed with deflate/shuffle.
    build_fletcher32("hdf5_fletcher32.h5")
    # zstd filter (id 32015). Needs `hdf5plugin`, unlike every other builder here.
    build_zstd("hdf5_zstd.h5")
    # szip filter (id 4): libhdf5 in the h5py wheel writes it with its bundled
    # libaec. The second file holds chunks its writer never produces.
    build_szip("hdf5_szip.h5")
    build_szip_hand("hdf5_szip_hand.h5")
    build_szip_long_stream("hdf5_szip_long_stream.h5")
    build_szip_growth("hdf5_szip_growth.h5")
    # Scale-less datasets: every axis is an invented anonymous dimension (#533).
    build_phony_dims("hdf5_phony_dims.h5")
    # Fixed-point bit offset / precision / padding, and a non-IEEE float (#795).
    build_fixed_point_precision("hdf5_fixed_point_precision.h5")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
