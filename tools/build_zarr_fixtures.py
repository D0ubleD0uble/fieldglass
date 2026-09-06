#!/usr/bin/env python3
"""Build the Zarr decode fixtures and their value oracles (#246).

Writes two things, from one source array so both crates see the same data:

  * ``crates/fieldglass-zarr/tests/fixtures/`` — one real store per codec
    combination, written by ``zarr-python``, plus ``oracle.json`` naming the
    values every chunk must decode to. This is what proves the codec chain: the
    chunk bytes were produced by the reference implementation, not by a second
    copy of this repository's arithmetic.
  * ``crates/fieldglass-fetchplan/tests/fixtures/zarr/`` — the *metadata*
    documents of the same stores, plus a kerchunk reference document over them.
    Addressing is that crate's question, decoding is the other's, and neither
    should have to reach into the other's fixture directory to be tested.

The stores are deliberately tiny — a 4x6 float32 array in 2x3 chunks — because
what is under test is the framing, not the volume. Two exceptions are big
enough on purpose: ``blosc_multiblock`` is large enough that blosc cuts it into
several blocks (so the block-offset table, the per-block split rule and the
short last block are all exercised), and ``blosc_bitshuffle_ragged`` has an
element count that is not a multiple of eight, which is the case blosc leaves
un-shuffled.

Run from the repo root. Needs ``zarr`` and ``numcodecs``:

    python3 tools/build_zarr_fixtures.py
"""
from __future__ import annotations

import json
import shutil
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


def clean(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)


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

    # ── The planner's half: metadata documents and a kerchunk index over them.
    for name, edition in (("v2", 2), ("v3", 3)):
        source = ZARR_FIXTURES / ("raw" if edition == 2 else "v3_bytes")
        meta_name = "zarr.json" if edition == 3 else ".zarray"
        meta = next(iter(sorted(source.rglob(meta_name))))
        (PLAN_FIXTURES / f"{name}_{meta_name.lstrip('.')}.json").write_text(
            meta.read_text(encoding="utf-8"), encoding="utf-8"
        )

    # A kerchunk reference document over the v2 store: the two shapes a `refs`
    # value takes — an inline string for the metadata, and a
    # `[url, offset, length]` triple for each chunk.
    raw_store = ZARR_FIXTURES / "raw"
    zarray = (raw_store / ".zarray").read_text(encoding="utf-8")
    refs = {
        ".zgroup": json.dumps({"zarr_format": 2}),
        "temp/.zarray": zarray,
        "temp/.zattrs": json.dumps({"_ARRAY_DIMENSIONS": ["y", "x"]}),
    }
    offset = 0
    for j in range(2):
        for i in range(2):
            size = CHUNKS[0] * CHUNKS[1] * 4
            refs[f"temp/{j}.{i}"] = ["s3://example-bucket/temp.bin", offset, size]
            offset += size
    (PLAN_FIXTURES / "kerchunk_refs.json").write_text(
        json.dumps({"version": 1, "refs": refs}, indent=1) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(oracle)} store fixtures to {ZARR_FIXTURES}")
    print(f"wrote the planner's metadata fixtures to {PLAN_FIXTURES}")


if __name__ == "__main__":
    main()
