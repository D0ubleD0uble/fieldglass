#!/usr/bin/env python3
"""Probe-parity sweep of real sample files against independent readers (#1029).

The value checks against eccodes run on committed fixtures, and the files
`tools/fetch_samples.sh` downloads are only checked for "decodes and renders"
(`tools/preflight_samples.js`). A placement or scan-order bug draws a plausible
picture with the right colour range, so neither check sees it: NBM's
alternate-row scanning (#285) passed the range check. This sweep reads the same
points twice, once through the native addon's `probePixel` (what the hover
readout calls) and once through a reader that shares no code with Fieldglass,
and says where they disagree.

    python3 tools/probe_parity_sweep.py [DIR_OR_FILE ...]
        [--n 200] [--seed 1] [--slices 3] [--addon PATH] [--only SUBSTR]

With no path it sweeps `samples/` and `samples/0.6.0/` under the repo root.

For each GRIB message and each 2-D NetCDF or Zarr slice it takes N seeded
pixels of the Equirectangular, nearest raster. The probes come from
`tools/probe_parity_probe.js`, which reports `lat`, `lon`, `value`, `gridI`,
`gridJ` for each. Then:

* GRIB: `eccodes.codes_grib_find_nearest` for the cell nearest `(lat, lon)`,
  and eccodes' own decoded `values` array for the value at the cell Fieldglass
  named. Lambert, polar, rotated and Gaussian grids are eccodes' business.
* NetCDF and Zarr: xarray, with the nearest cell found from the file's own
  coordinates. A 1-D latitude/longitude pair is searched per axis; a
  curvilinear pair (2-D latitude and longitude) by nearest neighbour over the
  whole grid; a GOES-R geostationary grid by the scan-angle formulas of the
  GOES-R Product User Guide (vol. 3, 5.1.2.8.1), so even the projection maths
  is not Fieldglass's.

Two independent signals per point, so a bug has to fool both to pass:

1. the cell Fieldglass resolved the pixel to (`gridI`, `gridJ`) against the
   oracle's nearest cell, and
2. Fieldglass's value against the oracle's value at the cell Fieldglass named.

A cell that is not the oracle's nearest but is within 2% of its distance is a
tie (a point on a cell boundary, where the two readers round differently), not
a disagreement. Ties are counted and reported, and more than 10% of an item's
points being ties fails it, since a half-cell offset would sit there.

Exit status: 0 clean, 1 at least one disagreement beyond the packing's
precision, 2 the sweep itself could not run. Items it cannot check (spectral
messages, reduced grids, grids with no coordinates the file states) are listed
with the reason and do not fail the run.

Tolerances. A GRIB value must agree within 1/1000 of the packing's quantisation
step (2^E / 10^D) plus a 1e-9 relative float term: two decoders reading the same
integers differ by rounding, not by a step. A NetCDF/Zarr value is held to
1e-6 relative to the larger of the value and the file's `add_offset`, which is
the float32 rounding xarray's decode carries.

Documented exceptions (NOTICE.md, samples/README.md):

* MRMS marks no coverage with a -999 sentinel that is not a bitmap. Both
  readers return it as a data value, so it is compared as one and the count is
  reported beside the row.
* eccodes' known wrong decodes (the GRIB2 boustrophedonic reduced-grid minimum,
  CCSDS streams with SIGNED/PAD_RSI flags) live in committed fixtures, not in
  the downloaded samples. A reduced grid is skipped here for the reason below,
  so the first cannot arise; a sweep over a directory holding the second will
  report the disagreement, and that is the sweep working.

Reduced grids are skipped: Fieldglass resolves a pixel on the expanded raster,
so `gridI` is not an index into eccodes' value array.

Run it under a memory cap, like every reader of files you did not write:
    ( ulimit -v 8000000; python3 -I tools/probe_parity_sweep.py )

Python dependencies (the pre-commit venv's `python3`): eccodes (the PyPI
wheel), xarray, netCDF4, zarr 3, numpy, scipy. None is a crate dependency.
"""
from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PROBE_JS = REPO / "tools" / "probe_parity_probe.js"

GRIB_SUFFIXES = {".grib2", ".grib1", ".grib", ".grb2", ".grb"}
NC_SUFFIXES = {".nc", ".nc4"}
ZARR_MARKERS = ("zarr.json", ".zgroup", ".zmetadata", ".zarray")

# A cell within this factor of the oracle's nearest distance is a tie.
TIE_RATIO = 1.02
# More ties than this share of an item's points fails it.
TIE_SHARE_LIMIT = 0.10
# Probes per pool task for a GRIB message.
CHUNK = 50

# Files whose decoders agree on a no-data sentinel that is not a bitmap.
SENTINELS = {"mrms.grib2": -999.0}

EARTH_RADIUS_KM = 6371.0


@dataclass
class Result:
    file: str
    label: str
    status: str = "PASS"  # PASS, FAIL, SKIP
    reason: str = ""
    probes: int = 0
    compared: int = 0
    both_valid: int = 0
    both_masked: int = 0
    mask_disagree: int = 0
    max_diff: float = 0.0
    max_where: str = ""
    value_bad: int = 0
    cell_bad: int = 0
    ties: int = 0
    oracle_errors: int = 0
    sentinel: int = 0
    first_bad: list[str] = field(default_factory=list)

    def merge(self, other: "Result") -> None:
        """Fold in the counters of another chunk of the same item."""
        if other.status == "SKIP":
            self.status, self.reason = "SKIP", other.reason
            return
        for k in ("probes", "compared", "both_valid", "both_masked", "mask_disagree", "value_bad",
                  "cell_bad", "ties", "oracle_errors", "sentinel"):
            setattr(self, k, getattr(self, k) + getattr(other, k))
        if other.max_diff > self.max_diff:
            self.max_diff, self.max_where = other.max_diff, other.max_where
        for b in other.first_bad:
            self.note_bad(b)

    def note_bad(self, text: str) -> None:
        if len(self.first_bad) < 3:
            self.first_bad.append(text)


# ---------------------------------------------------------------------------
# Oracles. Each answers two questions in Fieldglass's own (row j, column i)
# convention: which cell is nearest this point, and what is at a cell.
# ---------------------------------------------------------------------------


@dataclass
class Nearest:
    best: tuple[int, int] | None  # (j, i) of the oracle's nearest cell
    best_dist: float
    fg_dist: float | None  # distance to the cell Fieldglass named; None if unknown
    # The oracle's value at the cell Fieldglass named, when the lookup gave it
    # (fg_known); GRIB only. Grid oracles are read through `value` instead.
    fg_known: bool = False
    fg_value: float | None = None


class Skip(Exception):
    """The sweep cannot check this item; the message says why."""


def haversine_km(lat1, lon1, lat2, lon2):
    import numpy as np

    p1, p2 = np.radians(lat1), np.radians(lat2)
    dl = np.radians(lon2 - lon1)
    a = np.sin((p2 - p1) / 2) ** 2 + np.cos(p1) * np.cos(p2) * np.sin(dl / 2) ** 2
    return 2 * EARTH_RADIUS_KM * np.arcsin(np.minimum(1.0, np.sqrt(a)))


class GribOracle:
    """eccodes' nearest-point lookup.

    The value at a cell comes from the same lookup, not from the `values`
    array: for a message with alternative-row scanning (NBM) the `values` key
    returns the points in storage order, odd rows reversed, while the lookup
    (like eccodes' own iterator and `grib_get_data`) puts each value under its
    own latitude and longitude. The array is not a placement oracle for those
    messages, and the lookup is the one the issue names.

    The lookup is wrong the other way for a j-consecutive message
    (`jPointsAreConsecutive`, scanning-mode bit 3): eccodes 2.48 returns the
    row-major index and reads the value at that index of the column-major data,
    so the value belongs to another point (`grib1_names.grib1` at 45N 15E: the
    lookup says 25, the data at that latitude and longitude is 33). For those
    messages the oracle is eccodes' geo iterator, which `grib_get_data` uses,
    and which pairs each stored value with its own latitude and longitude.
    """

    ITERATOR_POINT_LIMIT = 8_000_000

    def __init__(self, ec, handle, grid):
        self.ec, self.msg = ec, handle
        if grid is None or grid.get("pointsPerRow"):
            raise Skip("reduced grid: gridI is a column of the expanded raster, not an eccodes index")
        self.ni, self.nj = grid["ni"], grid["nj"]
        points = ec.codes_get(handle, "numberOfPoints") if ec.codes_is_defined(handle, "numberOfPoints") else None
        if self.ni is None or self.nj is None or points != self.ni * self.nj:
            raise Skip(f"eccodes holds {points} points for a {self.ni}x{self.nj} grid")
        has_bitmap = bool(ec.codes_get(handle, "bitmapPresent")) if ec.codes_is_defined(handle, "bitmapPresent") else False
        self.missing = ec.codes_get(handle, "missingValue") if has_bitmap else None
        # Fieldglass transposes a j-consecutive message into a row-major raster
        # (core::scan), so its (j, i) is eccodes' index i * nj + j.
        self.j_consecutive = bool(ec.codes_get(handle, "jPointsAreConsecutive")) if ec.codes_is_defined(handle, "jPointsAreConsecutive") else False
        self.tol_step = self._step()
        self.iterated = None
        if self.j_consecutive:
            self.iterated = self._iterate()

    def _iterate(self):
        """(tree, flat, values) from the geo iterator, in stored order."""
        import numpy as np
        from scipy.spatial import cKDTree

        ec = self.ec
        if self.ni * self.nj > self.ITERATOR_POINT_LIMIT:
            raise Skip("j-consecutive grid too large to walk with the geo iterator")
        it = ec.codes_grib_iterator_new(self.msg, 0)
        rows = []
        try:
            while True:
                r = ec.codes_grib_iterator_next(it)
                if not r:
                    break
                rows.append(r)
        finally:
            ec.codes_grib_iterator_delete(it)
        a = np.asarray(rows, dtype="float64")
        if a.shape[0] != self.ni * self.nj:
            raise Skip(f"the geo iterator yields {a.shape[0]} points for a {self.ni}x{self.nj} grid")
        la, lo = np.radians(a[:, 0]), np.radians(a[:, 1])
        xyz = np.stack([np.cos(la) * np.cos(lo), np.cos(la) * np.sin(lo), np.sin(la)], axis=-1)
        return cKDTree(xyz), xyz, a[:, 2]

    def _step(self) -> float:
        ec, handle = self.ec, self.msg
        try:
            e = ec.codes_get(handle, "binaryScaleFactor")
            d = ec.codes_get(handle, "decimalScaleFactor")
            return (2.0**e) / (10.0**d)
        except Exception:
            return 0.0

    def flat(self, j: int, i: int) -> int:
        return i * self.nj + j if self.j_consecutive else j * self.ni + i

    def unflat(self, k: int) -> tuple[int, int]:
        if self.j_consecutive:
            return k % self.nj, k // self.nj
        return k // self.ni, k % self.ni

    def nearest(self, lat, lon, fg_cell):
        if self.iterated is not None:
            return self._nearest_iterated(lat, lon, fg_cell)
        # The caller catches what eccodes raises off its grid.
        near = self.ec.codes_grib_find_nearest(self.msg, lat, lon, False, 4)
        by_index = {n.index: n for n in near}
        best_k = min(by_index, key=lambda k: by_index[k].distance)
        mine = by_index.get(self.flat(*fg_cell))
        if mine is None:
            return Nearest(self.unflat(best_k), by_index[best_k].distance, None)
        masked = self.missing is not None and mine.value == self.missing
        return Nearest(self.unflat(best_k), by_index[best_k].distance, mine.distance, True, None if masked else float(mine.value))

    def _nearest_iterated(self, lat, lon, fg_cell):
        import numpy as np

        tree, xyz, values = self.iterated
        la, lo = math.radians(lat), math.radians(lon)
        p = np.array([math.cos(la) * math.cos(lo), math.cos(la) * math.sin(lo), math.sin(la)])
        d, k = tree.query(p)
        fg_k = self.flat(*fg_cell)
        if not 0 <= fg_k < len(values):
            return Nearest(self.unflat(int(k)), float(d), None)
        v = values[fg_k]
        masked = self.missing is not None and v == self.missing
        return Nearest(self.unflat(int(k)), float(d), float(np.linalg.norm(xyz[fg_k] - p)), True, None if masked else float(v))

    def tolerance(self, v):
        return 1e-3 * self.tol_step + 1e-9 * abs(v)


def kind_of(coord):
    a = coord.attrs
    units = str(a.get("units", "")).lower()
    sn = str(a.get("standard_name", "")).lower()
    name = str(coord.name).lower()
    if sn == "latitude" or units in ("degrees_north", "degree_north", "degree_n", "degrees_n", "degreen", "degreesn") or name in ("lat", "latitude"):
        return "lat"
    if sn == "longitude" or units in ("degrees_east", "degree_east", "degree_e", "degrees_e", "degreee", "degreese") or name in ("lon", "longitude"):
        return "lon"
    if sn == "projection_x_coordinate" or str(a.get("axis", "")).upper() == "X":
        return "x"
    if sn == "projection_y_coordinate" or str(a.get("axis", "")).upper() == "Y":
        return "y"
    return None


class GridOracle:
    """xarray, looking the cell up from the file's own coordinates."""

    def __init__(self, ds, da, ydim, xdim, plane, cache):
        import numpy as np

        self.plane = np.asarray(plane, dtype="float64")  # (y, x), NaN where masked
        self.ny, self.nx = self.plane.shape
        enc = da.encoding or {}
        off = enc.get("add_offset", 0.0) or 0.0
        self.offset = abs(float(off))
        self.mode = None
        # 1. Two-dimensional latitude and longitude over exactly these axes.
        lat2 = lon2 = None
        for c in da.coords.values():
            if set(c.dims) == {ydim, xdim}:
                k = kind_of(c)
                if k == "lat":
                    lat2 = c
                elif k == "lon":
                    lon2 = c
        if lat2 is not None and lon2 is not None:
            self.mode = "curvilinear"
            key = (lat2.name, lon2.name, self.ny, self.nx)
            if key not in cache:
                cache[key] = self._tree(lat2.transpose(ydim, xdim).values, lon2.transpose(ydim, xdim).values)
            self.tree, self.xyz, self.ok = cache[key]
            return
        # 2. One-dimensional coordinates on the two axes.
        cy, cx = da.coords.get(ydim), da.coords.get(xdim)
        if cy is None or cx is None or cy.ndim != 1 or cx.ndim != 1:
            raise Skip("no coordinate variables for the image axes")
        ky, kx = kind_of(cy), kind_of(cx)
        if (ky, kx) == ("lat", "lon"):
            self.mode = "latlon"
            self.lat = np.asarray(cy.values, dtype="float64")
            self.lon = np.asarray(cx.values, dtype="float64")
            return
        if (ky, kx) == ("y", "x") and "grid_mapping" in da.attrs:
            gm = ds[da.attrs["grid_mapping"]].attrs if da.attrs["grid_mapping"] in ds else {}
            if gm.get("grid_mapping_name") != "geostationary":
                raise Skip(f"grid mapping {gm.get('grid_mapping_name')!r} has no independent oracle in the sweep")
            if str(gm.get("sweep_angle_axis", "x")).lower() != "x":
                raise Skip("geostationary sweep_angle_axis y has no oracle in the sweep")
            h = float(gm["perspective_point_height"])
            self.req = float(gm["semi_major_axis"])
            self.rpol = float(gm["semi_minor_axis"])
            self.H = h + self.req
            self.lam0 = math.radians(float(gm["longitude_of_projection_origin"]))
            scale = 1.0 if str(cx.attrs.get("units", "rad")).lower().startswith("rad") else 1.0 / h
            self.sx = np.asarray(cx.values, dtype="float64") * scale
            self.sy = np.asarray(cy.values, dtype="float64") * scale
            self.mode = "geos"
            return
        raise Skip(f"coordinate kinds {ky}/{kx} on the image axes have no oracle in the sweep")

    @staticmethod
    def _tree(lat, lon):
        import numpy as np
        from scipy.spatial import cKDTree

        la, lo = np.radians(np.asarray(lat, dtype="float64")), np.radians(np.asarray(lon, dtype="float64"))
        ok = np.isfinite(la) & np.isfinite(lo)
        xyz = np.stack([np.cos(la) * np.cos(lo), np.cos(la) * np.sin(lo), np.sin(la)], axis=-1)
        xyz = np.where(ok[..., None], xyz, 0.0)
        flat = np.flatnonzero(ok.ravel())
        tree = cKDTree(xyz.reshape(-1, 3)[flat])
        return (tree, flat), xyz, ok

    # -- geometry per mode -------------------------------------------------

    def _geos_angles(self, lat, lon):
        phi, lam = math.radians(lat), math.radians(lon)
        e2 = (self.req**2 - self.rpol**2) / self.req**2
        phic = math.atan((self.rpol**2 / self.req**2) * math.tan(phi))
        rc = self.rpol / math.sqrt(1 - e2 * math.cos(phic) ** 2)
        sx = self.H - rc * math.cos(phic) * math.cos(lam - self.lam0)
        sy = -rc * math.cos(phic) * math.sin(lam - self.lam0)
        sz = rc * math.sin(phic)
        y = math.atan2(sz, sx)
        x = math.asin(-sy / math.sqrt(sx * sx + sy * sy + sz * sz))
        return x, y

    def nearest(self, lat, lon, fg_cell):
        import numpy as np

        j, i = fg_cell
        if self.mode == "latlon":
            dlat = np.abs(self.lat - lat)
            dlon = np.abs(((self.lon - lon) + 180.0) % 360.0 - 180.0)
            bj, bi = int(np.argmin(dlat)), int(np.argmin(dlon))
            best = float(haversine_km(lat, lon, self.lat[bj], self.lon[bi]))
            fg = float(haversine_km(lat, lon, self.lat[j], self.lon[i])) if 0 <= j < self.ny and 0 <= i < self.nx else None
            return Nearest((bj, bi), best, fg)
        if self.mode == "geos":
            x, y = self._geos_angles(lat, lon)
            bi, bj = int(np.argmin(np.abs(self.sx - x))), int(np.argmin(np.abs(self.sy - y)))
            best = math.hypot(self.sx[bi] - x, self.sy[bj] - y)
            fg = math.hypot(self.sx[i] - x, self.sy[j] - y) if 0 <= j < self.ny and 0 <= i < self.nx else None
            return Nearest((bj, bi), best, fg)
        # curvilinear: chord length on the unit sphere
        la, lo = math.radians(lat), math.radians(lon)
        p = np.array([math.cos(la) * math.cos(lo), math.cos(la) * math.sin(lo), math.sin(la)])
        tree, flat = self.tree
        d, k = tree.query(p)
        bj, bi = divmod(int(flat[k]), self.nx)
        fg = None
        if 0 <= j < self.ny and 0 <= i < self.nx and self.ok[j, i]:
            fg = float(np.linalg.norm(self.xyz[j, i] - p))
        return Nearest((bj, bi), float(d), fg)

    def value(self, cell):
        j, i = cell
        if not (0 <= j < self.ny and 0 <= i < self.nx):
            return None
        v = self.plane[j, i]
        return None if math.isnan(v) else float(v)

    def tolerance(self, v):
        return 1e-6 * (abs(v) + self.offset) + 1e-9


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------


def finalize(res: Result) -> Result:
    """Turn an item's counters into its verdict."""
    if res.status == "SKIP":
        return res
    fails = []
    if res.probes == 0:
        fails.append("no on-grid probe could be drawn")
    if res.cell_bad:
        fails.append(f"{res.cell_bad} cell disagreement(s)")
    if res.mask_disagree:
        fails.append(f"{res.mask_disagree} mask disagreement(s)")
    if res.value_bad:
        fails.append(f"{res.value_bad} value(s) beyond tolerance")
    if res.compared == 0 and res.probes:
        fails.append("the oracle answered no probe")
    if res.compared and res.ties > TIE_SHARE_LIMIT * res.compared:
        fails.append(f"{res.ties} of {res.compared} points are ties (> {TIE_SHARE_LIMIT:.0%})")
    if res.oracle_errors > 0.05 * max(res.probes, 1):
        fails.append(f"{res.oracle_errors} probe(s) the oracle could not answer")
    if fails:
        res.status, res.reason = "FAIL", "; ".join(fails)
    return res


def compare(res: Result, probes, oracle, sentinel=None):
    res.probes = len(probes)
    for p in probes:
        cell = (int(p["gridJ"]), int(p["gridI"]))
        where = f"lat {p['lat']:.4f} lon {p['lon']:.4f} (gridI {p['gridI']}, gridJ {p['gridJ']})"
        if p["lat"] is None or p["lon"] is None:
            res.oracle_errors += 1
            continue
        try:
            near = oracle.nearest(p["lat"], p["lon"], cell)
        except Exception as e:  # eccodes raises off its grid; keep the first words
            res.oracle_errors += 1
            res.note_bad(f"oracle could not answer at {where}: {str(e).splitlines()[0][:80]}")
            continue
        res.compared += 1
        # 1. Which cell.
        if near.best == cell:
            pass
        elif near.fg_dist is not None and near.fg_dist <= near.best_dist * TIE_RATIO + 1e-9:
            res.ties += 1
        else:
            res.cell_bad += 1
            res.note_bad(
                f"cell: Fieldglass (j={cell[0]}, i={cell[1]}), oracle nearest (j={near.best[0]}, i={near.best[1]}) at {where}"
            )
        # 2. What is there. The oracle is read at the cell Fieldglass named.
        if near.fg_known:
            ov = near.fg_value
        elif hasattr(oracle, "value"):
            ov = oracle.value(cell)
        else:
            continue  # a cell outside the lookup's neighbours: already a cell disagreement
        fv = p["value"]
        if sentinel is not None and fv == sentinel:
            res.sentinel += 1
        if fv is None and ov is None:
            res.both_masked += 1
        elif fv is None or ov is None:
            res.mask_disagree += 1
            res.note_bad(f"mask: Fieldglass {fv}, oracle {ov} at {where}")
        else:
            res.both_valid += 1
            diff = abs(fv - ov)
            if diff > res.max_diff:
                res.max_diff, res.max_where = diff, where
            if diff > oracle.tolerance(ov):
                res.value_bad += 1
                res.note_bad(f"value: Fieldglass {fv!r}, oracle {ov!r} (diff {diff:.3g}) at {where}")


# ---------------------------------------------------------------------------
# Files
# ---------------------------------------------------------------------------


def run_probe(path: Path, args, *extra: str) -> dict:
    cmd = ["node", str(PROBE_JS), str(path), "--n", str(args.n), "--seed", str(args.seed), "--slices", str(args.slices)]
    if args.addon:
        cmd += ["--addon", str(args.addon)]
    cmd += list(extra)
    out = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", check=False)
    if out.returncode != 0:
        raise RuntimeError(out.stderr.strip() or f"probe exited {out.returncode}")
    return json.loads(out.stdout)


def grib_task(path: Path, item: dict, probes: list, sentinel) -> Result:
    """One chunk of one message's probes against eccodes' nearest-point lookup."""
    import eccodes as ec

    res = Result(path.name, item["label"])
    handle = None
    try:
        with open(path, "rb") as fh:
            for _ in range(item["index"] + 1):
                handle = ec.codes_grib_new_from_file(fh)
                if handle is None:
                    res.status, res.reason = "SKIP", "eccodes sees fewer messages than Fieldglass"
                    return res
        try:
            oracle = GribOracle(ec, handle, item["grid"])
        except Skip as s:
            res.status, res.reason = "SKIP", str(s)
            return res
        compare(res, probes, oracle, sentinel)
        return res
    finally:
        if handle is not None:
            ec.codes_release(handle)


def open_dataset(path: Path, is_zarr: bool):
    import xarray as xr

    if is_zarr:
        return xr.open_dataset(path, engine="zarr", consolidated=False, decode_times=False, decode_timedelta=False)
    return xr.open_dataset(path, engine="netcdf4", decode_times=False, decode_timedelta=False)


def xarray_task(path: Path, args, variable: str) -> list[Result]:
    """Every sampled slice of one variable against xarray.

    The probe runs here, in the worker, because on a large curvilinear grid
    each `probePixel` call costs about a second and the variables of a file are
    independent of one another.
    """
    results: list[Result] = []
    try:
        doc = run_probe(path, args, "--variable", variable)
    except Exception as e:
        raise RuntimeError(f"{path.name}:{variable}: {e}") from e
    try:
        ds = open_dataset(path, doc["format"] == "zarr")
    except OSError as e:
        # netCDF4 carries an older libhdf5 than the one that wrote a
        # "latest format" file; the oracle cannot read it, which is not a
        # disagreement.
        for item in doc["items"]:
            results.append(Result(path.name, item["label"], "SKIP", "the oracle (netCDF4, with an older libhdf5) cannot open the file"))
        return results
    cache: dict = {}
    try:
        for item in doc["items"]:
            res = Result(path.name, item["label"])
            results.append(res)
            if item["status"] != "ok":
                res.status, res.reason = "SKIP", item["reason"]
                continue
            name = item["name"]
            if name not in ds.variables:
                res.status, res.reason = "SKIP", "variable lives in a group the sweep does not open"
                continue
            try:
                da = ds[name]
                ydim, xdim = da.dims[item["yDim"]], da.dims[item["xDim"]]
                idx = {d: item["sliceIndices"][k] for k, d in enumerate(da.dims) if d not in (ydim, xdim)}
                plane = da.isel(idx).transpose(ydim, xdim).values
                oracle = GridOracle(ds, da, ydim, xdim, plane, cache)
            except Skip as s:
                res.status, res.reason = "SKIP", str(s)
                continue
            compare(res, item["probes"], oracle)
            finalize(res)
    finally:
        ds.close()
    return results


def is_zarr_dir(p: Path) -> bool:
    return p.is_dir() and any((p / m).exists() for m in ZARR_MARKERS)


def collect(paths: list[Path]) -> list[Path]:
    found = []
    for p in paths:
        if p.is_file() or is_zarr_dir(p):
            found.append(p)
        elif p.is_dir():
            for c in sorted(p.iterdir()):
                if c.is_file() and c.suffix.lower() in GRIB_SUFFIXES | NC_SUFFIXES:
                    found.append(c)
                elif is_zarr_dir(c):
                    found.append(c)
        else:
            raise SystemExit(f"probe_parity_sweep: {p} does not exist")
    return found


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def report(results: list[Result], skipped_files: list[tuple[str, str]]) -> int:
    cols = ("file", "item", "probes", "cmp", "valid", "masked", "mask!=", "max|d|", "cell!=", "ties", "verdict")
    rows = []
    for r in results:
        if r.status == "SKIP":
            continue
        rows.append(
            (
                r.file, r.label, str(r.probes), str(r.compared), str(r.both_valid), str(r.both_masked),
                str(r.mask_disagree), f"{r.max_diff:.3g}", str(r.cell_bad), str(r.ties), r.status,
            )
        )
    widths = [max(len(c), *(len(row[k]) for row in rows)) if rows else len(c) for k, c in enumerate(cols)]
    line = lambda row: "  ".join(c.ljust(w) if k < 2 else c.rjust(w) for k, (c, w) in enumerate(zip(row, widths)))
    print(line(cols))
    print(line(tuple("-" * w for w in widths)))
    for row in rows:
        print(line(row))

    n_fail = sum(r.status == "FAIL" for r in results)
    n_pass = sum(r.status == "PASS" for r in results)
    skips = [r for r in results if r.status == "SKIP"]
    print()
    print(f"{n_pass} item(s) passed, {n_fail} failed, {len(skips)} skipped; "
          f"{sum(r.compared for r in results)} points compared.")

    notes = [r for r in results if r.sentinel]
    if notes:
        print("\nDocumented exceptions applied:")
        for r in notes:
            print(f"  {r.file} {r.label}: {r.sentinel} of {r.probes} probes read the -999 no-coverage "
                  "sentinel (a data value in both decoders, not a bitmap); compared as a value.")
    ties = [r for r in results if r.ties]
    if ties:
        print("\nTies (point on a cell boundary, nearest cell within 2%, not counted as disagreements):")
        for r in ties:
            print(f"  {r.file} {r.label}: {r.ties}")
    if skips or skipped_files:
        print("\nSkipped (not checked):")
        seen = {}
        for r in skips:
            seen.setdefault((r.file, r.reason), []).append(r.label)
        for (f, why), labels in seen.items():
            shown = ", ".join(labels[:3]) + (f", ... ({len(labels)} items)" if len(labels) > 3 else "")
            print(f"  {f}: {shown}: {why}")
        for f, why in skipped_files:
            print(f"  {f}: {why}")
    if n_fail:
        print("\nFAILURES:")
        for r in results:
            if r.status == "FAIL":
                print(f"  {r.file} {r.label}: {r.reason}")
                if r.max_where:
                    print(f"      largest value difference {r.max_diff:.6g} at {r.max_where}")
                for b in r.first_bad:
                    print(f"      {b}")
    return 1 if n_fail else 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Probe-parity sweep of sample files against eccodes and xarray.")
    ap.add_argument("paths", nargs="*", type=Path, help="files, Zarr stores or directories (default: samples/ and samples/0.6.0/)")
    ap.add_argument("--n", type=int, default=200, help="points per message or slice (default 200)")
    ap.add_argument("--seed", type=int, default=1, help="seed for the point draw (default 1)")
    ap.add_argument("--slices", type=int, default=3, help="slices tried per NetCDF/Zarr variable (default 3)")
    ap.add_argument("--addon", type=Path, default=None, help="native addon (default extension/bin/fieldglass.linux-x64-gnu.node)")
    ap.add_argument("--jobs", type=int, default=4, help="worker processes (default 4)")
    ap.add_argument("--only", default=None, help="only files whose name contains this text")
    args = ap.parse_args(argv)

    paths = args.paths or [REPO / "samples", REPO / "samples" / "0.6.0"]
    files = [f for f in collect([p for p in paths if p.exists() or args.paths]) if not args.only or args.only in f.name]
    if not files:
        print("probe_parity_sweep: no sample files found; run tools/fetch_samples.sh first", file=sys.stderr)
        return 2

    from concurrent.futures import ProcessPoolExecutor

    docs = []
    skipped_files: list[tuple[str, str]] = []
    for f in files:
        print(f"probing {f.name}", file=sys.stderr, flush=True)
        try:
            # GRIB probes are cheap and drawn here; a NetCDF/Zarr file only
            # lists its variables, which the pool then probes one by one.
            doc = run_probe(f, args, "--list-variables")
            if doc["format"] in ("grib1", "grib2"):
                doc = run_probe(f, args)
            docs.append((f, doc))
        except Exception as e:
            print(f"probe_parity_sweep: {f.name}: {e}", file=sys.stderr)
            return 2

    # Eccodes' nearest-point lookup walks the whole grid on every call for a
    # projected grid (about a second on NBM), so a message's probes go to the
    # pool in chunks, and a NetCDF/Zarr file goes whole (its slices share a
    # coordinate search tree).
    jobs: list = []  # (future, kind, item_key)
    ordered: list = []  # result slots in report order
    sentinel_of = SENTINELS.get
    try:
        with ProcessPoolExecutor(max_workers=args.jobs) as pool:
            for f, doc in docs:
                if doc["format"] in ("grib1", "grib2"):
                    for item in doc["items"]:
                        slot = {"file": f.name, "item": item, "parts": []}
                        ordered.append(slot)
                        if item["status"] != "ok":
                            continue
                        probes = item["probes"]
                        chunks = [probes[k:k + CHUNK] for k in range(0, len(probes), CHUNK)] or [[]]
                        for chunk in chunks:
                            slot["parts"].append(pool.submit(grib_task, f, item, chunk, sentinel_of(f.name)))
                elif doc["format"] in ("netcdf", "zarr"):
                    for name in doc["variables"]:
                        ordered.append({"file": f.name, "future": pool.submit(xarray_task, f, args, name)})
                else:
                    skipped_files.append((f.name, f"format {doc['format']!r}"))
            results: list[Result] = []
            for slot in ordered:
                if "future" in slot:
                    results.extend(slot["future"].result())
                    continue
                item = slot["item"]
                res = Result(slot["file"], item["label"])
                if item["status"] != "ok":
                    res.status, res.reason = "SKIP", item["reason"]
                else:
                    for part in slot["parts"]:
                        res.merge(part.result())
                        if res.status == "SKIP":
                            break
                    finalize(res)
                results.append(res)
    except ImportError as e:
        print(f"probe_parity_sweep: {e}; use the pre-commit venv's python3", file=sys.stderr)
        return 2
    return report(results, skipped_files)


if __name__ == "__main__":
    sys.exit(main())
