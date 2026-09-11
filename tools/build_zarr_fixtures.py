#!/usr/bin/env python3
"""Build the Zarr decode fixtures and their value oracles (#246).

Writes two things, from one source array so both crates see the same data:

  * ``crates/fieldglass-zarr/tests/fixtures/`` — one real store per codec
    combination, written by ``zarr-python``, plus ``oracle.json`` naming the
    values every chunk must decode to. This is what proves the codec chain: the
    chunk bytes were produced by the reference implementation, not by a second
    copy of this repository's arithmetic.
  * ``crates/fieldglass-fetchplan/tests/fixtures/zarr/`` — the *metadata*
    documents of the same stores, one object holding their chunks laid end to
    end, and three kerchunk reference documents addressing it. Addressing is
    that crate's question, decoding is the other's, and neither should have to
    reach into the other's fixture directory to be tested.

The stores are deliberately tiny — a 4x6 float32 array in 2x3 chunks — because
what is under test is the framing, not the volume. Two exceptions are big
enough on purpose: ``blosc_multiblock`` is large enough that blosc cuts it into
several blocks (so the block-offset table, the per-block split rule and the
short last block are all exercised), and ``blosc_bitshuffle_ragged`` has an
element count that is not a multiple of eight, which is the case blosc leaves
un-shuffled.

Run from the repo root. Needs ``zarr`` and ``numcodecs``:

    python3 tools/build_zarr_fixtures.py                # the whole corpus
    python3 tools/build_zarr_fixtures.py --plan-only    # the planner's half

``--plan-only`` rebuilds the ``fieldglass-fetchplan`` fixtures from the
committed stores without rewriting the corpus, which matters because gzip
stamps a timestamp into each chunk it writes: a full run diffs every gzip case
even when nothing changed.
"""
from __future__ import annotations

import base64
import json
import math
import shutil
import struct
import sys
from pathlib import Path

import numcodecs
import numpy as np
import zarr
from zarr.codecs import (
    BloscCodec,
    BytesCodec,
    Crc32cCodec,
    GzipCodec,
    ShardingCodec,
    TransposeCodec,
    ZstdCodec,
)

ZARR_FIXTURES = Path("crates/fieldglass-zarr/tests/fixtures")
PLAN_FIXTURES = Path("crates/fieldglass-fetchplan/tests/fixtures/zarr")

# The array every store holds, unless a case says otherwise. Values that are
# exactly representable in float32 and distinct, so a mis-ordered chunk is a
# wrong number rather than a plausible one.
BASE = (np.arange(24, dtype="<f4") * 0.5).reshape(4, 6)
CHUNKS = (2, 3)


# Files under a fixture directory that this script does not write and must not
# delete. `NOTICE.md` records where every fixture came from, which is a
# committed document about the corpus rather than part of it — and a `rmtree`
# that took it out left the corpus undocumented until someone noticed the
# deletion in `git status`.
KEEP = {"NOTICE.md"}


def clean(path: Path) -> None:
    """Empty a fixture directory of everything this script writes."""
    if not path.exists():
        path.mkdir(parents=True)
        return
    for entry in path.iterdir():
        if entry.name in KEEP:
            continue
        if entry.is_dir():
            shutil.rmtree(entry)
        else:
            entry.unlink()


def write_v2(root: Path, name: str, *, data, chunks, compressor, filters=None, order="C", dtype=None):
    """One Zarr v2 array in its own store directory."""
    store = root / name
    if store.exists():
        shutil.rmtree(store)
    array = zarr.create_array(
        store=str(store),
        shape=data.shape,
        chunks=chunks,
        dtype=dtype or data.dtype,
        zarr_format=2,
        compressors=[compressor] if compressor is not None else None,
        filters=[filters] if filters is not None else None,
        order=order,
    )
    array[...] = data
    return store


def write_v3(root: Path, name: str, *, data, chunks, shards=None, serializer=None, compressors=None, filters=None):
    """One Zarr v3 array in its own store directory."""
    store = root / name
    if store.exists():
        shutil.rmtree(store)
    kwargs = {}
    if serializer is not None:
        kwargs["serializer"] = serializer
    if compressors is not None:
        kwargs["compressors"] = compressors
    if filters is not None:
        kwargs["filters"] = filters
    if shards is not None:
        kwargs["shards"] = shards
    array = zarr.create_array(
        store=str(store),
        shape=data.shape,
        chunks=chunks,
        dtype=data.dtype,
        zarr_format=3,
        **kwargs,
    )
    array[...] = data
    return store


def chunk_records(data, chunks, keys, *, ramp) -> list[dict]:
    """Each chunk file, with the values it must decode to.

    The expected values are computed from the source array and the chunk grid,
    never by reading the store back: an oracle produced by the code under test
    would agree with a wrong decoder.

    ``ramp`` replaces the value list with the arithmetic sequence that produced
    it. One fixture is 200,000 elements — large on purpose, so blosc cuts it
    into several blocks — and writing those out would make the committed oracle
    two megabytes to prove a property about a block table.
    """
    records = []
    grid = [-(-n // c) for n, c in zip(data.shape, chunks)]
    for index, key in zip(np.ndindex(*grid), keys):
        block = np.zeros(chunks, dtype=data.dtype)
        region = tuple(
            slice(i * c, min((i + 1) * c, n)) for i, c, n in zip(index, chunks, data.shape)
        )
        piece = data[region]
        block[tuple(slice(0, s) for s in piece.shape)] = piece
        flat = block.ravel()
        record = {"index": list(index), "key": key}
        if ramp is None:
            record["values"] = [float(v) for v in flat]
        else:
            start, step = ramp
            expected = start + step * np.arange(flat.size, dtype="f8")
            assert np.array_equal(flat.astype("f8"), expected), "the ramp must describe the data"
            record["ramp"] = {"start": start, "step": step, "count": int(flat.size)}
        records.append(record)
    return records


def shard_records(data, shard_shape, inner_shape, keys, written) -> list[dict]:
    """Each shard file, with the values of every inner chunk it holds.

    ``written`` is the region of the array that was actually assigned. An inner
    chunk outside it was never written, so the shard index carries the
    empty-chunk sentinel for it and the decoder must report `None` rather than
    a block of the fill value — the two are different answers, and only the
    index can tell them apart.
    """
    records = []
    grid = [-(-n // c) for n, c in zip(data.shape, shard_shape)]
    inner_grid = [s // i for s, i in zip(shard_shape, inner_shape)]
    for index, key in zip(np.ndindex(*grid), keys):
        inner = []
        for sub in np.ndindex(*inner_grid):
            origin = [
                o * s + j * i for o, s, j, i in zip(index, shard_shape, sub, inner_shape)
            ]
            region = tuple(slice(o, o + i) for o, i in zip(origin, inner_shape))
            present = all(
                o + i <= w for o, i, w in zip(origin, inner_shape, written)
            )
            if not present:
                inner.append(None)
                continue
            inner.append([float(v) for v in data[region].ravel()])
        records.append({"index": list(index), "key": key, "inner": inner})
    return records


def collect(
    store: Path,
    data,
    chunks,
    *,
    edition: int,
    sharded: bool = False,
    ramp=None,
    inner_shape=None,
    written=None,
) -> dict:
    """The oracle entry for one store: where its chunks are and what they hold.

    The chunk *bytes* are not copied in here — the store on disk is the fixture,
    and a second base64 copy of it in the oracle would be a second thing to keep
    in step.
    """
    meta_name = "zarr.json" if edition == 3 else ".zarray"
    arrays = sorted(p for p in store.rglob(meta_name) if p.parent != store)
    if not arrays:
        # `create_array` with a store path puts the array at the store root.
        arrays = [store / meta_name]
    meta_path = arrays[0]
    array_dir = meta_path.parent

    chunk_files = sorted(
        p
        for p in array_dir.rglob("*")
        if p.is_file() and p.name not in {meta_name, ".zarray", ".zattrs"}
    )
    keys = [str(p.relative_to(store)).replace("\\", "/") for p in chunk_files]
    records = (
        shard_records(data, chunks, inner_shape, keys, written)
        if sharded
        else chunk_records(data, chunks, keys, ramp=ramp)
    )
    return {
        "metadata": str(meta_path.relative_to(store)).replace("\\", "/"),
        "sharded": sharded,
        "chunks": records,
    }


def main() -> None:
    # The planner's fixtures derive from the committed stores, so refreshing
    # them needs no rewrite of the corpus — and rewriting the corpus is not
    # free: gzip stamps a timestamp into every chunk it writes, so a full run
    # produces a diff in every gzip case whether or not anything changed.
    if "--plan-only" in sys.argv:
        clean(PLAN_FIXTURES)
        write_plan_fixtures()
        print(f"wrote the planner's addressing fixtures to {PLAN_FIXTURES}")
        return
    # The store corpus (#658) is its own flag for the reason `--plan-only` is:
    # rewriting the codec corpus churns every gzip chunk, and the stores do not
    # depend on it.
    if "--stores-only" in sys.argv:
        write_store_fixtures()
        return
    # The NetCDF twin of the CF stores (#704), on its own flag for the same
    # reason: writing it should not churn the stores beside it.
    if "--twin-only" in sys.argv:
        write_cf_twin()
        return

    clean(ZARR_FIXTURES)
    clean(PLAN_FIXTURES)
    oracle: dict[str, dict] = {}

    blosc_cases = {
        f"blosc_{cname}_{shname}": numcodecs.Blosc(cname=cname, clevel=5, shuffle=shuffle)
        for cname in ("blosclz", "lz4", "lz4hc", "zlib", "zstd")
        for shname, shuffle in (
            ("noshuffle", numcodecs.Blosc.NOSHUFFLE),
            ("shuffle", numcodecs.Blosc.SHUFFLE),
            ("bitshuffle", numcodecs.Blosc.BITSHUFFLE),
        )
    }
    v2_cases = {
        "raw": (None, None, "C"),
        "fortran_order": (None, None, "F"),
        "zlib": (numcodecs.Zlib(level=6), None, "C"),
        "gzip": (numcodecs.GZip(level=6), None, "C"),
        "zstd": (numcodecs.Zstd(level=3), None, "C"),
        "lz4": (numcodecs.LZ4(acceleration=1), None, "C"),
        "shuffle_zlib": (numcodecs.Zlib(level=6), numcodecs.Shuffle(elementsize=4), "C"),
        **{name: (codec, None, "C") for name, codec in blosc_cases.items()},
    }
    for name, (compressor, filters, order) in v2_cases.items():
        store = write_v2(
            ZARR_FIXTURES,
            name,
            data=BASE,
            chunks=CHUNKS,
            compressor=compressor,
            filters=filters,
            order=order,
        )
        oracle[name] = collect(store, BASE, CHUNKS, edition=2)

    # Big enough that blosc splits it into several blocks, so the block-offset
    # table, the per-block split rule and the un-split short last block are all
    # exercised. One chunk, so the whole thing is one blosc buffer.
    big = (np.arange(200_000, dtype="<f4") * 0.25).reshape(400, 500)
    store = write_v2(
        ZARR_FIXTURES,
        "blosc_multiblock",
        data=big,
        chunks=big.shape,
        compressor=numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE),
    )
    oracle["blosc_multiblock"] = collect(
        store, big, big.shape, edition=2, ramp=(0.0, 0.25)
    )

    # An element count that is not a multiple of eight: blosc's bit shuffle
    # declines the block outright, and a decoder that transposed the aligned
    # head would corrupt it.
    ragged = (np.arange(150, dtype="<f4") * 3.0).reshape(10, 15)
    store = write_v2(
        ZARR_FIXTURES,
        "blosc_bitshuffle_ragged",
        data=ragged,
        chunks=ragged.shape,
        compressor=numcodecs.Blosc(cname="lz4", clevel=5, shuffle=numcodecs.Blosc.BITSHUFFLE),
    )
    oracle["blosc_bitshuffle_ragged"] = collect(store, ragged, ragged.shape, edition=2)

    # Big-endian storage: the byte order is a property of the file, and a
    # decoder that assumed little would read every value as noise.
    big_endian = BASE.astype(">f4")
    store = write_v2(
        ZARR_FIXTURES,
        "big_endian",
        data=big_endian,
        chunks=CHUNKS,
        compressor=numcodecs.Zlib(level=6),
        dtype=">f4",
    )
    oracle["big_endian"] = collect(store, big_endian, CHUNKS, edition=2)

    # An integer type, so the value path is exercised somewhere other than a
    # float: `int16` is what packed satellite and reanalysis archives store.
    ints = (np.arange(24, dtype="<i2") - 12).reshape(4, 6)
    store = write_v2(
        ZARR_FIXTURES,
        "int16",
        data=ints,
        chunks=CHUNKS,
        compressor=numcodecs.Blosc(cname="zstd", clevel=5, shuffle=numcodecs.Blosc.SHUFFLE),
    )
    oracle["int16"] = collect(store, ints, CHUNKS, edition=2)

    v3_cases = {
        "v3_bytes": {"serializer": BytesCodec(endian="little"), "compressors": []},
        "v3_gzip": {"serializer": BytesCodec(endian="little"), "compressors": [GzipCodec(level=5)]},
        "v3_zstd": {"serializer": BytesCodec(endian="little"), "compressors": [ZstdCodec(level=3)]},
        "v3_blosc": {
            "serializer": BytesCodec(endian="little"),
            "compressors": [BloscCodec(cname="zstd", clevel=3, shuffle="shuffle")],
        },
        "v3_crc32c": {
            "serializer": BytesCodec(endian="little"),
            "compressors": [Crc32cCodec()],
        },
        "v3_big_endian": {"serializer": BytesCodec(endian="big"), "compressors": []},
        "v3_transpose": {
            "serializer": BytesCodec(endian="little"),
            "compressors": [],
            "filters": [TransposeCodec(order=(1, 0))],
        },
    }
    for name, kwargs in v3_cases.items():
        store = write_v3(ZARR_FIXTURES, name, data=BASE, chunks=CHUNKS, **kwargs)
        oracle[name] = collect(store, BASE, CHUNKS, edition=3)

    # Sharding: a 4x6 shard of 2x3 inner chunks, so the index holds four
    # entries and the inner grid is genuinely two-dimensional.
    store = write_v3(
        ZARR_FIXTURES,
        "v3_shard",
        data=BASE,
        chunks=CHUNKS,
        shards=(4, 6),
        compressors=[GzipCodec(level=5)],
    )
    oracle["v3_shard"] = collect(
        store, BASE, (4, 6), edition=3, sharded=True, inner_shape=CHUNKS, written=(4, 6)
    )

    # A sharded array with a chunk that was never written, so the index's
    # all-ones sentinel appears in a real fixture rather than only in a
    # hand-built one. Written through a plain `__setitem__` of a region, which
    # is how a partly-filled shard actually arises.
    sparse_store = ZARR_FIXTURES / "v3_shard_sparse"
    if sparse_store.exists():
        shutil.rmtree(sparse_store)
    sparse = zarr.create_array(
        store=str(sparse_store),
        shape=(4, 6),
        chunks=CHUNKS,
        shards=(4, 6),
        dtype="float32",
        zarr_format=3,
        fill_value=0.0,
    )
    sparse[0:2, 0:3] = BASE[0:2, 0:3]
    expected = np.zeros((4, 6), dtype="<f4")
    expected[0:2, 0:3] = BASE[0:2, 0:3]
    oracle["v3_shard_sparse"] = collect(
        sparse_store,
        expected,
        (4, 6),
        edition=3,
        sharded=True,
        inner_shape=CHUNKS,
        written=(2, 3),
    )

    (ZARR_FIXTURES / "oracle.json").write_text(
        json.dumps(oracle, indent=1, sort_keys=True) + "\n", encoding="utf-8"
    )

    write_plan_fixtures()
    write_store_fixtures()

    print(f"wrote {len(oracle)} store fixtures to {ZARR_FIXTURES}")
    print(f"wrote the planner's addressing fixtures to {PLAN_FIXTURES}")


# --- Whole stores (#658) -----------------------------------------------------
#
# The codec corpus above is one root array per store, because what it tests is
# a chunk. These are whole stores — groups, nested groups, shared dimensions,
# absent chunks, ragged edges, shards, consolidated metadata and not, and
# xarray's CF encoding — because what they test is the walk and the region
# read. Everything expected is recorded from zarr-python and xarray, never from
# the crate under test.

STORES = ZARR_FIXTURES / "stores"


def json_number(value):
    """A float the committed JSON can hold: NaN and the infinities as the
    strings Zarr itself spells them with."""
    value = float(value)
    if math.isnan(value):
        return "NaN"
    if math.isinf(value):
        return "Infinity" if value > 0 else "-Infinity"
    return value


def attribute_entry(value) -> dict:
    """One attribute as the walker's model should hold it: numbers kept as
    numbers, text as text, anything else as its JSON."""
    if isinstance(value, bool) or value is None:
        return {"opaque": json.dumps(value)}
    if isinstance(value, (int, float)):
        return {"numbers": [json_number(value)]}
    if isinstance(value, str):
        return {"text": value}
    if isinstance(value, list) and value and all(
        isinstance(v, (int, float)) and not isinstance(v, bool) for v in value
    ):
        return {"numbers": [json_number(v) for v in value]}
    return {"opaque": json.dumps(value, separators=(",", ":"), sort_keys=True)}


def presented_attributes(attrs: dict, edition: int, fill, kind: str) -> dict:
    """The attributes the walker should present, with the two things xarray
    does to `_FillValue` undone — written out here independently of the Rust:

    * v2 keeps `_FillValue` as the array's `fill_value`, so a numeric one is
      presented as the attribute when the store states none;
    * v3 writes a float `_FillValue` as base64 of its little-endian bytes.
    """
    out = {name: attribute_entry(value) for name, value in attrs.items()}
    if edition == 2 and "_FillValue" not in attrs and fill is not None:
        out["_FillValue"] = {"numbers": [json_number(fill)]}
    if edition == 3 and kind == "f" and isinstance(attrs.get("_FillValue"), str):
        raw = base64.b64decode(attrs["_FillValue"])
        fmt = {8: "<d", 4: "<f"}.get(len(raw))
        if fmt is not None:
            out["_FillValue"] = {"numbers": [json_number(struct.unpack(fmt, raw)[0])]}
    return out


def element(dtype) -> dict:
    kind = {"f": "float", "i": "int", "u": "uint", "b": "bool"}[dtype.kind]
    return {"kind": kind, "bits": dtype.itemsize * 8}


def describe_store(root: Path, edition: int, *, left_out=(), unreadable=(), physical=False) -> dict:
    """What zarr-python sees in a store: its groups, and each array's layout,
    attributes and values, plus a region that crosses chunk boundaries."""
    group = zarr.open_group(str(root), mode="r", zarr_format=edition, use_consolidated=False)
    groups = {"": {k: attribute_entry(v) for k, v in dict(group.attrs).items()}}
    arrays = {}
    for path, node in sorted(group.members(max_depth=None), key=lambda item: item[0]):
        if isinstance(node, zarr.Group):
            groups[path] = {k: attribute_entry(v) for k, v in dict(node.attrs).items()}
            continue
        if path in left_out:
            continue
        attrs = dict(node.attrs)
        if edition == 3:
            names = node.metadata.dimension_names
            dimensions = list(names) if names is not None else None
        else:
            dimensions = attrs.get("_ARRAY_DIMENSIONS")
        fill = node.fill_value
        fill = None if fill is None else float(fill)
        values = node[...]
        region = tuple(slice(1 if n > 2 else 0, n) for n in node.shape)
        arrays[path] = {
            "shape": list(node.shape),
            "chunks": list(node.shards or node.chunks),
            "element": element(node.dtype),
            "dimensions": dimensions,
            "attributes": presented_attributes(attrs, edition, fill, node.dtype.kind),
            "values": [json_number(v) for v in values.ravel()],
            "region": {
                "ranges": [[r.start, r.stop] for r in region],
                "values": [json_number(v) for v in values[region].ravel()],
            },
        }
    if physical:
        import xarray as xr

        dataset = xr.open_zarr(str(root), zarr_format=edition, consolidated=True)
        for name in list(dataset.data_vars) + list(dataset.coords):
            decoded = dataset[name].values.astype("f8").ravel()
            arrays[name]["physical"] = [None if math.isnan(v) else float(v) for v in decoded]
    return {
        "edition": edition,
        "groups": groups,
        "arrays": arrays,
        "left_out": sorted(left_out),
        "unreadable": sorted(unreadable),
    }


def cf_dataset():
    """The dataset the two CF stores and their NetCDF twin are written from,
    and the CF encoding they share: a packed int16 with a scale, an offset, a
    `_FillValue` and a masked cell, and a float with a -9999 sentinel."""
    import xarray as xr

    lat = np.array([10.0, 20.0, 30.0])
    lon = np.array([0.0, 1.0, 2.0, 3.0])
    t = np.array(
        [[250.0, 251.5, np.nan, 253.0], [260.25, 270.0, 280.0, 290.5], [240.0, 241.0, 242.0, 243.0]]
    )
    a = np.array([[1.0, np.nan, 3.0, 4.0], [5.0, 6.0, np.nan, 8.0], [9.0, 10.0, 11.0, 12.0]], "f4")
    dataset = xr.Dataset(
        {"t": (("lat", "lon"), t), "a": (("lat", "lon"), a)},
        coords={"lat": lat, "lon": lon},
    )
    encoding = {
        "t": {"dtype": "int16", "scale_factor": 0.25, "add_offset": 200.0, "_FillValue": -32767, "chunks": (2, 2)},
        "a": {"_FillValue": -9999.0, "chunks": (2, 2)},
    }
    return dataset, encoding


def write_cf_twin() -> None:
    """The CF stores' dataset again, as NetCDF-4 (#704).

    One xarray dataset written as a Zarr store and as a NetCDF file is the one
    input that shows the two containers place a slice by the same rules: the
    same variables, the same axes detected, the same geometry, the same values.
    Zarr's `chunks` encoding key is NetCDF's `chunksizes`.
    """
    dataset, encoding = cf_dataset()
    netcdf_encoding = {
        name: {("chunksizes" if k == "chunks" else k): v for k, v in enc.items()}
        for name, enc in encoding.items()
    }
    path = ZARR_FIXTURES / "cf_twin.nc"
    dataset.to_netcdf(str(path), engine="netcdf4", encoding=netcdf_encoding)
    print(f"wrote the CF stores' NetCDF twin to {path}")


def write_store_fixtures() -> None:
    import xarray as xr

    if STORES.exists():
        shutil.rmtree(STORES)
    STORES.mkdir(parents=True)
    oracle: dict[str, dict] = {}

    # The same nested layout in three spellings: v2 with `.` keys and
    # consolidated, v2 with `/` keys and not, and v3 consolidated. `temp` is
    # 5x7 in 2x3 chunks — ragged on both axes — and only its top-left 4x5 is
    # written, so whole chunks are absent and one is only partly written.
    temp = (np.arange(35, dtype="<f4") * 0.5).reshape(5, 7)
    for edition, name, consolidated, separator in (
        (2, "v2_nested", True, "."),
        (2, "v2_slash", False, "/"),
        (3, "v3_nested", True, None),
    ):
        root = STORES / name
        group = zarr.open_group(str(root), mode="w", zarr_format=edition)
        group.attrs["title"] = name
        extra = {}
        if edition == 3:
            extra["dimension_names"] = ["y", "x"]
        if separator is not None:
            extra["chunk_key_encoding"] = {"name": "v2", "separator": separator}
        array = group.create_array(
            "temp", shape=(5, 7), chunks=(2, 3), dtype="<f4", fill_value=-1.0, **extra
        )
        if edition == 2:
            array.attrs["_ARRAY_DIMENSIONS"] = ["y", "x"]
        array.attrs["units"] = "K"
        array[:4, :5] = temp[:4, :5]
        sub = group.create_group("sub")
        sub.attrs["level"] = 850
        inner_extra = {"dimension_names": ["z"]} if edition == 3 else {}
        if separator is not None:
            inner_extra["chunk_key_encoding"] = {"name": "v2", "separator": separator}
        inner = sub.create_array(
            "inner", shape=(4,), chunks=(2,), dtype="<i2", fill_value=0, **inner_extra
        )
        if edition == 2:
            inner.attrs["_ARRAY_DIMENSIONS"] = ["z"]
        inner[...] = np.arange(4, dtype="<i2") - 2
        if consolidated:
            zarr.consolidate_metadata(str(root))
        oracle[name] = describe_store(root, edition)

    # v3 with v2-style keys, unconsolidated, a NaN fill and no dimension
    # names: the absent half reads NaN, and the axes get the walker's own names.
    root = STORES / "v3_v2keys"
    group = zarr.open_group(str(root), mode="w", zarr_format=3)
    keyed = group.create_array(
        "k",
        shape=(4, 6),
        chunks=(2, 3),
        dtype="<f4",
        fill_value=float("nan"),
        chunk_key_encoding={"name": "v2", "separator": "."},
    )
    keyed[:2, :] = BASE[:2, :]
    oracle["v3_v2keys"] = describe_store(root, 3)

    # Sharded, ragged, and sparse at both levels: shards of 4x4 holding 2x2
    # inner chunks over a 5x7 array, with one corner written in the first shard
    # and one cell in the last.
    root = STORES / "v3_sharded"
    group = zarr.open_group(str(root), mode="w", zarr_format=3)
    sharded = group.create_array(
        "s",
        shape=(5, 7),
        chunks=(2, 2),
        shards=(4, 4),
        dtype="<f4",
        fill_value=-1.0,
        dimension_names=["y", "x"],
    )
    sharded[:2, :3] = temp[:2, :3]
    sharded[4, 6] = 9.0
    zarr.consolidate_metadata(str(root))
    oracle["v3_sharded"] = describe_store(root, 3)

    # What fails, and how far: a codec this crate refuses (bz2), a type it does
    # not read (`<U4`), and two arrays giving dimension `x` different lengths.
    # The first is listed and fails on read; the other two are left out; `ok`
    # beside them is unaffected.
    root = STORES / "v2_problems"
    group = zarr.open_group(str(root), mode="w", zarr_format=2)
    ok = group.create_array("ok", shape=(4,), chunks=(2,), dtype="<f4", fill_value=0.0)
    ok[...] = BASE[0, :4]
    bz = group.create_array(
        "bz", shape=(4,), chunks=(2,), dtype="<f4", compressors=[numcodecs.BZ2(level=5)]
    )
    bz[...] = BASE[1, :4]
    names = group.create_array("names", shape=(2,), chunks=(2,), dtype="<U4")
    names[...] = np.array(["ab", "cd"])
    for label, length in (("x1", 3), ("x2", 5)):
        clash = group.create_array(label, shape=(length,), chunks=(length,), dtype="<i2")
        clash.attrs["_ARRAY_DIMENSIONS"] = ["x"]
        clash[...] = length
    zarr.consolidate_metadata(str(root))
    oracle["v2_problems"] = describe_store(
        root, 2, left_out={"names", "x2"}, unreadable={"bz"}
    )

    # xarray's CF encoding, in both editions: a packed int16 with a scale,
    # an offset, a `_FillValue` and a masked cell, and a float with a -9999
    # sentinel. The physical values are xarray's own decode of the same store.
    dataset, encoding = cf_dataset()
    for edition in (2, 3):
        root = STORES / f"cf_v{edition}"
        dataset.to_zarr(str(root), zarr_format=edition, consolidated=True, encoding=encoding, mode="w")
        oracle[f"cf_v{edition}"] = describe_store(root, edition, physical=True)

    (ZARR_FIXTURES / "stores_oracle.json").write_text(
        json.dumps(oracle, indent=1, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(oracle)} whole-store fixtures to {STORES}")


# The object the kerchunk fixtures address. Chunks are laid end to end with a
# gap between them, because a real reference document points into a file that
# was never a Zarr store — a NetCDF4 or GRIB archive, whose chunks are
# separated by headers this crate never sees. A uniform stride would let a
# planner that multiplied the chunk index by a length pass anyway; a gap makes
# the stated offset the only way to be right.
CHUNK_GAP = 7

# Fills the gaps. Not zero: a run of zeroes is what an off-by-one range lands
# in and decodes as plausible-looking data, whereas this is not valid zstd and
# fails loudly.
GAP_BYTE = 0xA5

# The URL the reference documents name. Opaque to the planner — it hands the
# string back for the host to fetch — so what it points at only has to be
# realistic, not reachable. The seam test resolves it to ``temp.bin`` beside it.
OBJECT_URL = "s3://example-bucket/temp.bin"


def concatenate_chunks(store: Path, keys: list[str]) -> tuple[bytes, dict[str, tuple[int, int]]]:
    """Lay a store's chunk objects end to end, with a gap between them.

    Returns the object's bytes and, per key, the ``(offset, length)`` a
    reference document has to state to address that chunk inside it.
    """
    blob = bytearray()
    placement: dict[str, tuple[int, int]] = {}
    for key in keys:
        chunk = (store / key).read_bytes()
        placement[key] = (len(blob), len(chunk))
        blob += chunk
        blob += bytes([GAP_BYTE]) * CHUNK_GAP
    return bytes(blob), placement


def write_plan_fixtures() -> None:
    """The planner's half: metadata documents and kerchunk reference documents.

    Addressing is ``fieldglass-fetchplan``'s question and decoding is
    ``fieldglass-zarr``'s, and neither should have to reach into the other's
    fixture directory to be tested. So the metadata documents are copied here,
    and the chunks are concatenated into one object the reference documents
    address by byte range.
    """
    for name, source_name, meta_name in (
        ("v2_zarray.json", "raw", ".zarray"),
        ("v3_zarr.json", "v3_bytes", "zarr.json"),
    ):
        meta = next(iter(sorted((ZARR_FIXTURES / source_name).rglob(meta_name))))
        # `zarr-python` writes these without a trailing newline, and the repo's
        # end-of-file hook adds one. Written with it here so the hook and this
        # script agree; a trailing newline is not part of the JSON.
        text = meta.read_text(encoding="utf-8").rstrip("\n") + "\n"
        (PLAN_FIXTURES / name).write_text(text, encoding="utf-8")

    # Over the **zstd** store rather than the raw one on purpose. The seam this
    # fixture exists to test is "the range fetchplan planned holds a chunk the
    # zarr crate can decode", and raw chunks are little-endian float32 — a test
    # over those would pass against a reader that never called the codec crate
    # at all.
    store = ZARR_FIXTURES / "zstd"
    keys = [f"{j}.{i}" for j in range(2) for i in range(2)]
    blob, placement = concatenate_chunks(store, keys)
    (PLAN_FIXTURES / "temp.bin").write_bytes(blob)

    metadata = {
        ".zgroup": json.dumps({"zarr_format": 2}),
        "temp/.zarray": (store / ".zarray").read_text(encoding="utf-8"),
        "temp/.zattrs": json.dumps({"_ARRAY_DIMENSIONS": ["y", "x"]}),
    }
    chunks = {
        f"temp/{key}": [OBJECT_URL, offset, length]
        for key, (offset, length) in placement.items()
    }

    # The plain form: every URL written out in full.
    (PLAN_FIXTURES / "kerchunk_refs.json").write_text(
        json.dumps({"version": 1, "refs": {**metadata, **chunks}}, indent=1) + "\n",
        encoding="utf-8",
    )

    # The templated form, which is what kerchunk actually emits over a single
    # archive: the URL is written once under `templates` and every chunk names
    # it as `{{u}}`. Same ranges, so the two documents must plan identically.
    (PLAN_FIXTURES / "kerchunk_templates.json").write_text(
        json.dumps(
            {
                "version": 1,
                "templates": {"u": OBJECT_URL},
                "refs": {
                    **metadata,
                    **{
                        key: ["{{u}}", offset, length]
                        for key, (_url, offset, length) in chunks.items()
                    },
                },
            },
            indent=1,
        )
        + "\n",
        encoding="utf-8",
    )

    # A `gen` block: references generated from a jinja2 expression over a
    # dimension rather than written out. The planner refuses this by name — it
    # is a template language, not a manifest grammar — and this fixture is what
    # holds it to refusing rather than silently planning the `refs` it can see.
    (PLAN_FIXTURES / "kerchunk_gen.json").write_text(
        json.dumps(
            {
                "version": 1,
                "templates": {"u": OBJECT_URL},
                "gen": [
                    {
                        "key": "temp/{{i}}.0",
                        "url": "{{u}}",
                        "offset": "{{i * 31}}",
                        "length": "24",
                        "dimensions": {"i": {"stop": 2}},
                    }
                ],
                "refs": metadata,
            },
            indent=1,
        )
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
