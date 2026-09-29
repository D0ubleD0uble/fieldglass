#!/usr/bin/env python3
"""Print the report tier: wall time beside wasm and beside the reference tools.

    python3 crates/fieldglass-perf/report.py --corpus <dir> --native native.json \\
        --wasm wasm.json --wasm-simd wasm-simd.json \\
        [--manifest manifests/era5.json --cache ~/.cache/fieldglass-perf]

Nothing here gates. Wall time on a shared runner is noise; what this adds is a
*ratio*, which survives the noise far better than either number alone, and the
answer to the question a gated number cannot give: is this close to the best
anyone does?

**The reference tools** read the same bytes, from memory, doing the same work:

| Input | Reference | Work |
|---|---|---|
| GRIB1, GRIB2 | eccodes (`codes_get_values`) | decode the message |
| NetCDF | netCDF4 (`Dataset(memory=…)`) | read one plane, masked and scaled |
| Zarr | zarr-python over a `MemoryStore` | read one plane |
| ERA5 Zarr | zarr-python and xarray over the same chunks | scrub the frames |

**Discounting the binding.** A Python call costs something before any work is
done: argument conversion, the ctypes or Cython boundary, building a NumPy array.
Each tool's floor is measured on the smallest input it can be given — one value
through the same call — and subtracted before the ratio is taken. Both ratios
are printed, but only where the reference took at least `FLOOR_MULTIPLE` times its
floor: below that the subtraction is dominated by the floor's own noise (the
first draft printed a ratio of ten million for a decode eccodes finished inside
its floor), and the row says "binding-dominated" instead.

**Not compared.** Spectral GRIB1: eccodes returns the coefficients and does no
synthesis, while `Session::decode` synthesises a 0.5° grid, so the two are not
the same work. ERA5 GRIB1 is compared with a caveat the table repeats: eccodes
returns the 542,080 points of the reduced N320 grid, `Session::decode` expands
them to its 1280 × 640 regular grid.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import statistics
import sys
import tempfile
import time
from pathlib import Path

import eccodes
import netCDF4
import numpy as np
import xarray
import zarr
from zarr.core.buffer import default_buffer_prototype
from zarr.storage import MemoryStore

# The same as the Rust side, so the two columns time the same work.
ITERATIONS = 5
SLICE_PLANE = 3

# A reference time under this many binding floors is mostly binding.
FLOOR_MULTIPLE = 3


def median_ms(fn, iterations: int = ITERATIONS) -> float:
    fn()  # warm: first-call imports and caches are not the steady state
    samples = []
    for _ in range(iterations):
        start = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - start) * 1e3)
    return statistics.median(samples)


def memory_store(files: dict[str, bytes]) -> MemoryStore:
    prototype = default_buffer_prototype()
    return MemoryStore({key: prototype.buffer.from_bytes(data) for key, data in files.items()}, read_only=True)


def store_files(root: Path) -> dict[str, bytes]:
    return {p.relative_to(root).as_posix(): p.read_bytes() for p in root.rglob("*") if p.is_file()}


# ── Binding floors ────────────────────────────────────────────────────────────


def floors() -> dict[str, float]:
    """Each tool's cost for one value through the call a row times."""
    out = {}
    h = eccodes.codes_grib_new_from_samples("regular_ll_sfc_grib2")
    eccodes.codes_set(h, "Ni", 1)
    eccodes.codes_set(h, "Nj", 1)
    eccodes.codes_set_values(h, np.array([1.0]))
    one = eccodes.codes_get_message(h)
    eccodes.codes_release(h)

    def grib():
        handle = eccodes.codes_new_from_message(one)
        eccodes.codes_get_values(handle)
        eccodes.codes_release(handle)

    out["eccodes"] = median_ms(grib, 50)

    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "one.nc"
        with netCDF4.Dataset(path, "w") as ds:
            ds.createDimension("x", 1)
            ds.createVariable("v", "f4", ("x",))[:] = 1
        data = path.read_bytes()
    out["netCDF4"] = median_ms(lambda: netCDF4.Dataset("one", memory=data)["v"][:], 50)

    store = zarr.storage.MemoryStore()
    zarr.create_array(store, shape=(1,), dtype="f4")[:] = 1
    out["zarr-python"] = median_ms(lambda: zarr.open_array(store, mode="r")[:], 50)
    out["xarray"] = out["zarr-python"]
    return out


# ── Corpus ────────────────────────────────────────────────────────────────────


def reference_corpus(corpus: Path) -> dict[str, tuple[str, float]]:
    """`{input: (tool, ms)}` for every input a reference tool reads."""
    index = json.loads((corpus / "corpus.json").read_text(encoding="utf-8"))
    out: dict[str, tuple[str, float]] = {}
    for name, entry in sorted(index["inputs"].items()):
        fmt = entry["format"]
        if name.startswith("grib1-spectral"):
            continue  # eccodes does not synthesise; see the module docs
        if fmt in ("grib1", "grib2"):
            message = (corpus / entry["file"]).read_bytes()

            def decode(message=message):
                handle = eccodes.codes_new_from_message(message)
                eccodes.codes_get_values(handle)
                eccodes.codes_release(handle)

            out[f"{name}/decode"] = ("eccodes", median_ms(decode))
        elif fmt == "netcdf":
            data = (corpus / entry["file"]).read_bytes()
            out[f"{name}/slice"] = (
                "netCDF4",
                median_ms(lambda data=data: netCDF4.Dataset("m", memory=data)["t"][SLICE_PLANE]),
            )
        elif fmt == "zarr":
            store = memory_store(store_files(corpus / entry["dir"]))
            out[f"{name}/slice"] = (
                "zarr-python",
                median_ms(lambda store=store: zarr.open_group(store, mode="r")["t"][SLICE_PLANE]),
            )
    return out


# ── Real data ─────────────────────────────────────────────────────────────────


def reference_real(manifest_path: Path, cache: Path) -> dict[str, list[tuple[str, float]]]:
    """Median ms per frame for each real-data container, per reference tool."""
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    def obj(digest: str) -> bytes:
        return (cache / digest).read_bytes()

    out: dict[str, list[tuple[str, float]]] = {}

    section = manifest["zarr"]
    files = {key: obj(digest) for key, digest in section["objects"].items()}
    files.update({key: json.dumps(doc).encode() for key, doc in section["documents"].items()})
    files.update({key: base64.b64decode(text) for key, text in section["binary"].items()})
    store = memory_store(files)
    frames = section["frames"]
    array = zarr.open_group(store, mode="r", zarr_format=2)[section["variable"]]
    zarr_ms = median_ms(lambda: [array[k] for k in range(frames)], 3) / frames
    dataset = xarray.open_zarr(store, consolidated=False, zarr_format=2, decode_times=False)
    xr_ms = median_ms(lambda: [dataset[section["variable"]].isel(time=k).values for k in range(frames)], 3) / frames
    out["era5-zarr"] = [("zarr-python", zarr_ms), ("xarray", xr_ms)]

    section = manifest["netcdf"]
    data = obj(section["object"])
    ds = netCDF4.Dataset("era5", memory=data)
    out["era5-netcdf"] = [
        ("netCDF4", median_ms(lambda: [ds[section["variable"]][k] for k in range(section["frames"])], 3) / section["frames"])
    ]

    messages = [obj(m["object"]) for m in manifest["grib1"]["messages"]]

    def grib():
        for message in messages:
            handle = eccodes.codes_new_from_message(message)
            eccodes.codes_get_values(handle)
            eccodes.codes_release(handle)

    out["era5-grib1"] = [("eccodes", median_ms(grib, 3) / len(messages))]
    return out


# ── Printing ──────────────────────────────────────────────────────────────────


def ratio(ours: float, theirs: float) -> str:
    return f"{ours / theirs:.2f}×" if theirs > 0 else "—"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--native", type=Path, required=True, help="the `report` binary's JSON")
    parser.add_argument("--wasm", type=Path, required=True)
    parser.add_argument("--wasm-simd", type=Path, required=True)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--cache", type=Path)
    args = parser.parse_args(argv)

    native = json.loads(args.native.read_text(encoding="utf-8"))
    wasm = json.loads(args.wasm.read_text(encoding="utf-8"))["scenarios"]
    simd = json.loads(args.wasm_simd.read_text(encoding="utf-8"))["scenarios"]
    floor = floors()
    reference = reference_corpus(args.corpus)

    lines = [
        f"### Wall time (median of {native['iterations']}, not gated)",
        "",
        "| Scenario | native ms | wasm ms | wasm / native | +simd128 ms | +simd128 / native |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for scenario, ms in native["native"].items():
        if scenario not in wasm or ms < 0.05:
            continue
        w, s = wasm[scenario]["ms"], simd[scenario]["ms"]
        lines.append(f"| `{scenario}` | {ms:.2f} | {w:.2f} | {ratio(w, ms)} | {s:.2f} | {ratio(s, ms)} |")

    lines += [
        "",
        "### Against the reference tools (same bytes, from memory)",
        "",
        "Binding floors, one value through the same call: "
        + ", ".join(f"{tool} {ms:.3f} ms" for tool, ms in floor.items() if tool != "xarray")
        + ".",
        "",
        "| Scenario | Reference | fieldglass ms | reference ms | ratio | ratio less the binding floor |",
        "|---|---|---:|---:|---:|---:|",
    ]
    for scenario, (tool, theirs) in reference.items():
        ours = native["native"].get(scenario)
        if ours is None:
            continue
        discounted = (
            ratio(ours, theirs - floor[tool]) if theirs >= FLOOR_MULTIPLE * floor[tool] else "binding-dominated"
        )
        lines.append(f"| `{scenario}` | {tool} | {ours:.2f} | {theirs:.2f} | {ratio(ours, theirs)} | {discounted} |")

    if native["real"]:
        if not (args.manifest and args.cache):
            print("error: the native report has real-data rows but no --manifest/--cache to compare them with", file=sys.stderr)
            return 1
        real_reference = reference_real(args.manifest, args.cache)
        lines += [
            "",
            "### ERA5, 2 m temperature, 2020-01-01 00Z–11Z",
            "",
            "eccodes returns the GRIB1 field's 542,080 reduced-grid points; fieldglass expands them to a",
            "1280 × 640 regular grid, so that ratio includes work eccodes does not do.",
            "",
            "| Container | Frames | Cells per frame | Bytes read | Bound | Requests | Open ms | ms per frame | Reference ms per frame |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---|",
        ]
        for row in native["real"]:
            refs = ", ".join(f"{tool} {ms:.1f} ({ratio(row['frame_ms'], ms)})" for tool, ms in real_reference[row["name"]])
            lines.append(
                f"| `{row['name']}` | {row['frames']} | {row['cells']:,} | {row['bytes']:,} | {row['bound_bytes']:,} | "
                f"{row['requests']} | {row['open_ms']:.2f} | {row['frame_ms']:.1f} | {refs} |"
            )

    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"{text}\n\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
