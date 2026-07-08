#!/usr/bin/env python3
"""Build the NetCDF-4 nested-group resolution fixture and its oracle (#219).

Dimension-scale resolution (#174) originally read the root group only, so a file
that puts its variables under groups — Sentinel-5P and other ESA L2
(``/PRODUCT/...``), NASA GPM IMERG (``/Grid/...``), OMI-style HDF-EOS — showed an
empty variables table. #219 descends into every group and presents the objects
with path-qualified names (``/PRODUCT/qa_value``).

This writes a small but representative grouped file with the canonical Unidata
``netCDF4`` library (the real HDF5 group + ``DIMENSION_LIST`` machinery) plus a
sibling oracle JSON the Rust test pins against. It deliberately covers:

  * a **root** dimension + coordinate variable (``time``) that stays bare-named,
  * a nested group ``/PRODUCT`` with its own dimensions (``scanline`` /
    ``ground_pixel``) and coordinate + data variables,
  * a variable whose ``DIMENSION_LIST`` mixes a dimension from an **ancestor**
    group (root ``time``) with its own group's dimensions — the netCDF scoping
    rule (a dimension is visible to its group and all descendants),
  * a **two-level** nested group (``/PRODUCT/SUPPORT_DATA``) whose variable is
    path-qualified through both levels.

The oracle path-qualifies each object exactly as the Rust reader does: a
root-group object keeps its bare name; anything under group ``G`` is ``G``'s
leading-slash path plus ``/name``. A variable's dimension names use the path of
the group that *defines* each dimension, so an inherited dimension keeps its
ancestor-group name.

Run from the repo root (needs ``netCDF4``):

    python3 tools/build_netcdf4_grouped_fixture.py
"""
from __future__ import annotations

import json
from pathlib import Path

import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf4_grouped.nc"


def build(path: Path) -> None:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as f:
        f.title = "fieldglass NetCDF-4 nested-group fixture"

        # Root: a dimension + coordinate variable, inherited by descendants.
        f.createDimension("time", 2)
        time = f.createVariable("time", "f8", ("time",))
        time.units = "seconds since 2020-01-01 00:00:00"
        time.standard_name = "time"
        time[:] = [0.0, 3600.0]

        # /PRODUCT: its own scanline/ground_pixel grid + variables.
        prod = f.createGroup("PRODUCT")
        prod.createDimension("scanline", 3)
        prod.createDimension("ground_pixel", 4)

        lat = prod.createVariable("latitude", "f4", ("scanline", "ground_pixel"))
        lat.units = "degrees_north"
        lat.standard_name = "latitude"
        lon = prod.createVariable("longitude", "f4", ("scanline", "ground_pixel"))
        lon.units = "degrees_east"
        lon.standard_name = "longitude"
        # qa_value mixes the ROOT `time` dimension (ancestor scoping) with the
        # group's own dimensions.
        qa = prod.createVariable(
            "qa_value", "f4", ("time", "scanline", "ground_pixel"),
            fill_value=np.float32(-1.0),
        )
        qa.units = "1"
        lat[:] = np.arange(3 * 4, dtype="f4").reshape(3, 4)
        lon[:] = (np.arange(3 * 4, dtype="f4") + 100.0).reshape(3, 4)
        qa[:] = np.arange(2 * 3 * 4, dtype="f4").reshape(2, 3, 4)

        # /PRODUCT/SUPPORT_DATA: a two-level-deep variable using PRODUCT's dims.
        support = prod.createGroup("SUPPORT_DATA")
        alt = support.createVariable("surface_altitude", "f4", ("scanline", "ground_pixel"))
        alt.units = "m"
        alt[:] = (np.arange(3 * 4, dtype="f4") * 10.0).reshape(3, 4)


# numpy dtype name -> the netCDF type name the Rust reader reports.
NC_TYPE_NAME = {
    "int8": "byte", "uint8": "ubyte", "int16": "short", "uint16": "ushort",
    "int32": "int", "uint32": "uint", "int64": "int64", "uint64": "uint64",
    "float32": "float", "float64": "double",
}


def group_prefix(grp: netCDF4.Group) -> str:
    """The leading-slash path used to qualify a group's children (`""` for the
    root), matching the Rust reader's rule."""
    return "" if grp.path == "/" else grp.path


def qualify(grp: netCDF4.Group, name: str) -> str:
    prefix = group_prefix(grp)
    return name if not prefix else f"{prefix}/{name}"


def walk_groups(grp: netCDF4.Group):
    """Yield every group in the file, depth-first from the root."""
    yield grp
    for child in grp.groups.values():
        yield from walk_groups(child)


def oracle(path: Path) -> dict:
    with netCDF4.Dataset(path, "r") as f:
        dimensions = []
        variables = []
        for grp in walk_groups(f):
            coord_names = {n for n in grp.variables if n in grp.dimensions}
            for name, dim in grp.dimensions.items():
                dimensions.append({
                    "name": qualify(grp, name),
                    "length": len(dim),
                    "unlimited": dim.isunlimited(),
                })
            for name, var in grp.variables.items():
                # Each dimension name is qualified by the group that *defines* it,
                # so an inherited dimension keeps its ancestor-group path.
                dims = [qualify(d.group(), d.name) for d in var.get_dims()]
                variables.append({
                    "name": qualify(grp, name),
                    "nc_type": NC_TYPE_NAME[var.dtype.name],
                    "dimensions": dims,
                    "is_coordinate": name in coord_names,
                })
        return {
            "source": (
                f"netCDF4 {netCDF4.__version__} "
                f"(libnetcdf {netCDF4.__netcdf4libversion__}, "
                f"HDF5 {netCDF4.__hdf5libversion__})"
            ),
            "format": f.data_model,
            "note": "nested-group resolution with path-qualified names (#219)",
            "dimensions": dimensions,
            "variables": variables,
        }


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    path = FIXTURES_DIR / NAME
    build(path)
    data = oracle(path)
    (FIXTURES_DIR / f"{NAME}.oracle.json").write_text(json.dumps(data, indent=2) + "\n")
    size = path.stat().st_size
    print(f"wrote {path} ({size} B) + oracle "
          f"[{len(data['variables'])} vars across groups, {data['source']}]")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
