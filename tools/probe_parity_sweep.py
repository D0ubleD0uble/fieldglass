#!/usr/bin/env python3
"""Probe-parity sweep of real sample files against independent readers (#1029).

The value checks against eccodes run on committed fixtures, and the files
`tools/fetch_samples.sh` downloads are only checked for "decodes and renders"
(`tools/preflight_samples.js`). A placement or scan-order bug draws a plausible
picture with the right color range, so neither check sees it: NBM's
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

* GRIB: `eccodes.codes_grib_find_nearest` for the cell nearest `(lat, lon)`
  and the value eccodes holds there, at the cell Fieldglass named when it is
  one of the four neighbors the lookup returns. Lambert, polar, rotated and
  Gaussian grids are eccodes' business.
* NetCDF and Zarr: xarray, with the nearest cell found from the file's own
  coordinates. A 1-D latitude/longitude pair is searched per axis; a
  curvilinear pair (2-D latitude and longitude) by nearest neighbor over the
  whole grid; a GOES-R geostationary grid by the scan-angle formulas of the
  GOES-R Product User Guide (vol. 3, 5.1.2.8.1), so even the projection maths
  is not Fieldglass's.

Three checks per point, so a bug has to fool all of them to pass:

1. the cell Fieldglass resolved the pixel to (`gridI`, `gridJ`) against the
   oracle's nearest cell,
2. Fieldglass's value against the oracle's value at the cell Fieldglass named,
   and
3. the position `probePixel` reports for pixel (px, py) against where that
   pixel falls on the Equirectangular raster, from the raster size and the
   extent the render reports, to within 1e-3 of a pixel (the render's own
   mapping, so float noise passes and a half-pixel center-versus-edge mix-up
   does not). A shared offset in the render and the probe would not show
   here; the cell and value checks against the oracle are what catch that.

A cell that is not the oracle's nearest but is within 2% of its distance is a
tie (a point on a cell boundary, where the two readers round differently), not
a disagreement. Ties are counted and reported, and more than 10% of an item's
points being ties fails it, since a half-cell offset would sit there.

Exit status: 0 clean, 1 at least one disagreement beyond the packing's
precision, 2 the sweep itself could not run. Items it cannot check for one of a
short list of expected reasons (spectral messages, reduced grids, no CF image
axes, a grid Fieldglass itself says it cannot place, a file the oracle's
libhdf5 is too old to open, a grid mapping with no oracle here) are listed with
the reason and do not fail the run. A probe that threw, a message count that
differs between the two readers, or any other reason for not checking an item
is a failure: a skip must not be a way to go green. So is a message count that
differs in either direction (including Fieldglass finding none), a NetCDF or
Zarr file with no listed variables (unless pinned), and an item where fewer than
MIN_PROBE_SHARE of the requested `--n` probes landed on the grid.

Tolerances. A GRIB value must agree within 1/1000 of the packing's quantization
step (2^E / 10^D) plus a 1e-9 relative float term: two decoders reading the same
integers differ by rounding, not by a step. A NetCDF/Zarr value is held to
1e-6 relative to the larger of the value and the file's `add_offset`, which is
the float32 rounding xarray's decode carries.

Documented exceptions (samples/README.md, "Known oracle exceptions"):

* MRMS marks no coverage with a -999 sentinel that is not a bitmap. Both
  readers return it as a data value, so it is compared as one and the count is
  reported beside the row.
* eccodes' nearest-point lookup (seen on the 2.48 wheel; the report prints the
  installed version) is wrong for a j-consecutive message
  (it returns the row-major index and the value stored there, which belongs
  to another point; `grib1_names.grib1` at 45N 15E: 25 against the true 33).
  For those the oracle is eccodes' geo iterator instead, and the report names
  the file. The `values` array is no oracle for an alternate-row message
  (NBM): it keeps storage order, odd rows reversed, which is why the value is
  taken from the lookup and not the array.
* eccodes' known wrong decodes (the GRIB2 boustrophedonic reduced-grid minimum,
  CCSDS streams with SIGNED/PAD_RSI flags) live in committed fixtures, not in
  the downloaded samples. A reduced grid is skipped here for the reason below,
  so the first cannot arise; a sweep over a directory holding the second will
  report the disagreement, and that is the sweep working.

Reduced grids are skipped: Fieldglass resolves a pixel on the expanded raster,
so `gridI` is not an index into eccodes' value array.

Run it under a memory cap, like every reader of files you did not write:
    ( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py )

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
# An item must land at least this share of the requested `--n` probes. The
# probe's sampler draws up to max(50 n, 1000) pixels and, once 20 land, narrows
# to the box around them, so a thin swath still fills `--n`; an item below half
# of that is a grid the draw barely hit, and its verdict rests on too few points.
MIN_PROBE_SHARE = 0.5
# Probes per pool task for a GRIB message.
CHUNK = 50

# Files whose decoders agree on a no-data sentinel that is not a bitmap.
SENTINELS = {"mrms.grib2": -999.0}

EARTH_RADIUS_KM = 6371.0

# Reasons Fieldglass itself reports for not placing something. Each is allowed
# only for the (file, item) pairs below, so a file that stops placing, or an
# axis that stops being detected, fails instead of being listed as a skip.
# Add a pair here after deciding the new case really has no raster to probe.
PINNED_REASONS = (
    "grid is not reprojectable (unplaceable)",
    "grid is not reprojectable (no_raster)",
    "no CF image axes",
    "no variables listed",
)
PINNED_SKIPS = {
    ("tiny_earth_pair.grib2", "msg#0", "grid is not reprojectable (unplaceable)"),
    ("tiny_earth_pair.grib2", "msg#1", "grid is not reprojectable (unplaceable)"),
    ("spectral_pair.grib2", "msg#0", "grid is not reprojectable (no_raster)"),
    ("spectral_pair.grib2", "msg#1", "grid is not reprojectable (no_raster)"),
    ("mirs_swath.nc", "Qc", "no CF image axes"),
    ("wrf.nc", "XLAT", "no CF image axes"),
    ("wrf.nc", "XLONG", "no CF image axes"),
    ("wrf.nc", "T2", "no CF image axes"),
    ("air_copy", "time_bnds", "no CF image axes"),
    ("air_v2.zarr", "time_bnds", "no CF image axes"),
    ("air_v3.zarr", "time_bnds", "no CF image axes"),
    ("ncep_air_daily_2024.nc", "time_bnds", "no CF image axes"),
    ("ncep_air_levels_ltm.nc", "climatology_bounds", "no CF image axes"),
}

# The only other reasons an item may go unchecked without failing the run. Anything
# else (a probe that threw, eccodes holding another point count, a grid with no
# coordinates the sweep can find) is a failure.
EXPECTED_SKIPS = (
    "spectral field",
    "reduced grid",
    "the oracle (netCDF4, with an older libhdf5)",
    "grid mapping",
    "geostationary sweep_angle_axis",
)


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
    place_bad: int = 0
    ties: int = 0
    oracle_errors: int = 0
    sentinel: int = 0
    note: str = ""
    first_bad: list[str] = field(default_factory=list)

    def skip(self, reason: str) -> "Result":
        """Leave the item unchecked: a skip if the reason is an expected one, else a failure."""
        pinned = reason.startswith(PINNED_REASONS)
        if pinned and not any((self.file, self.label, r) in PINNED_SKIPS for r in PINNED_REASONS if reason.startswith(r)):
            self.status, self.reason = "FAIL", (
                f"Fieldglass reports '{reason}' for something not on the pinned list (PINNED_SKIPS): "
                "a regression, or a new case to review"
            )
        elif pinned or reason.startswith(EXPECTED_SKIPS):
            self.status, self.reason = "SKIP", reason
        else:
            self.status, self.reason = "FAIL", f"not checked, and not for an expected reason: {reason}"
        return self

    def fail(self, reason: str) -> "Result":
        self.status, self.reason = "FAIL", reason
        return self

    def merge(self, other: "Result") -> None:
        """Fold in the counters of another chunk of the same item."""
        if other.status != "PASS":
            self.status, self.reason = other.status, other.reason
            return
        for k in ("probes", "compared", "both_valid", "both_masked", "mask_disagree", "value_bad",
                  "cell_bad", "place_bad", "ties", "oracle_errors", "sentinel"):
            setattr(self, k, getattr(self, k) + getattr(other, k))
        if other.max_diff > self.max_diff:
            self.max_diff, self.max_where = other.max_diff, other.max_where
        self.note = self.note or other.note
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
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dl = math.radians(lon2 - lon1)
    a = math.sin((p2 - p1) / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * EARTH_RADIUS_KM * math.asin(min(1.0, math.sqrt(a)))


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
        # (core::storage_order), so its (j, i) is eccodes' index i * nj + j.
        self.j_consecutive = bool(ec.codes_get(handle, "jPointsAreConsecutive")) if ec.codes_is_defined(handle, "jPointsAreConsecutive") else False
        self.tol_step = self._step()
        self.iterated = None
        self.note = ""
        if self.j_consecutive:
            self.iterated = self._iterate()
            self.note = "j-consecutive message: eccodes' find_nearest returns the wrong cell and value, so the geo iterator is the oracle"

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


def finalize(res: Result, requested: int | None = None) -> Result:
    """Turn an item's counters into its verdict.

    `requested` is the `--n` the probes were drawn for; with it, an item that
    landed fewer than MIN_PROBE_SHARE of that fails.
    """
    if res.status != "PASS":
        return res
    fails = []
    if res.probes == 0:
        fails.append("no on-grid probe could be drawn")
    elif requested is not None and res.probes < MIN_PROBE_SHARE * requested:
        fails.append(f"only {res.probes} of {requested} requested probes landed on the grid (< {MIN_PROBE_SHARE:.0%})")
    if res.cell_bad:
        fails.append(f"{res.cell_bad} cell disagreement(s)")
    if res.place_bad:
        fails.append(f"{res.place_bad} pixel(s) whose reported position is off the raster's own mapping")
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


# A pixel must be where the raster's own mapping puts it, to within this share
# of a pixel. The mapping here is the one the render uses (no half-pixel
# offset), so a correct result matches to float noise; a center-versus-edge
# mix-up is exactly half a pixel and must not pass on the boundary.
PLACEMENT_TOLERANCE_PIXELS = 1e-3


def expected_position(px, py, raster, bounds, periodic):
    """(lat, lon, pixel height, pixel width), in degrees, of pixel (px, py) on the raster the render drew.

    `bounds` is the extent the render reports: latitude and longitude of the
    first and last pixel centers. When the source grid is periodic in
    longitude (`periodic` true) the last pixel is one step short of the full
    turn, so the step is span / width; otherwise it is span / (width - 1). The
    span alone cannot tell the two apart (a duplicated seam column spans
    exactly 360 degrees and is not periodic).
    """
    (w, h), (lat_min, lat_max, lon_min, lon_max) = raster, bounds
    span_lon = lon_max - lon_min
    step_lon = span_lon / w if periodic else span_lon / max(w - 1, 1)
    step_lat = (lat_max - lat_min) / max(h - 1, 1)
    return lat_max - py * step_lat, lon_min + px * step_lon, abs(step_lat), abs(step_lon)


def placement_ok(p, raster, bounds, periodic=None):
    """Whether the probe's position is where the pixel falls.

    The addon exposes `periodicX` for a GRIB message and not for a NetCDF or
    Zarr slice; with `periodic` None both steps are accepted rather than
    guessing from the span.
    """
    for per in ((periodic,) if periodic is not None else (True, False)):
        lat, lon, step_lat, step_lon = expected_position(p["px"], p["py"], raster, bounds, per)
        dlon = (p["lon"] - lon + 180.0) % 360.0 - 180.0
        if (abs(p["lat"] - lat) <= PLACEMENT_TOLERANCE_PIXELS * step_lat + 1e-9
                and abs(dlon) <= PLACEMENT_TOLERANCE_PIXELS * step_lon + 1e-9):
            return True
    return False


def compare(res: Result, probes, oracle, sentinel=None, geometry=None):
    """Check each probe against the oracle.

    `geometry` is the item's (raster size, render extent, periodic or None), for the check that
    the position `probePixel` reports is where the pixel falls. Without it the
    check cannot run, which is a failure rather than a pass.
    """
    res.probes = len(probes)
    res.note = getattr(oracle, "note", "")
    for p in probes:
        cell = (int(p["gridJ"]), int(p["gridI"]))
        if p["lat"] is None or p["lon"] is None:
            res.oracle_errors += 1  # a grid with no forward map reports a value but no position
            continue
        where = f"lat {p['lat']:.4f} lon {p['lon']:.4f} (gridI {p['gridI']}, gridJ {p['gridJ']})"
        if geometry is None or any(b is None for b in geometry[1]) or not placement_ok(p, *geometry):
            res.place_bad += 1
            res.note_bad(f"placement: pixel ({p['px']}, {p['py']}) is reported at {where}, not where the raster puts it")
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
            continue  # a cell outside the lookup's neighbors: already a cell disagreement
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


def apply_item_status(res: Result, item: dict) -> None:
    """Carry a probe-side verdict ("skipped", "failed") onto the result."""
    if item["status"] == "failed":
        res.fail(item["reason"])
    else:
        res.skip(item["reason"])


def eccodes_message_count(ec, path: Path) -> int:
    """Messages eccodes finds in the file, each handle released as it goes."""
    n = 0
    with open(path, "rb") as fh:
        while True:
            handle = ec.codes_grib_new_from_file(fh)
            if handle is None:
                return n
            ec.codes_release(handle)
            n += 1


def count_check(name: str, fieldglass: int, eccodes: int) -> list[Result]:
    """A failing result unless both readers find the same, non-zero, number of messages."""
    if fieldglass == eccodes and fieldglass > 0:
        return []
    why = (
        f"Fieldglass finds {fieldglass} message(s), eccodes {eccodes}: the readers disagree on the count"
        if fieldglass != eccodes
        else "neither reader finds a message in the file"
    )
    return [Result(name, "message count").fail(why)]


def grib_count_task(path: Path, fieldglass: int) -> list[Result]:
    import eccodes as ec

    return count_check(path.name, fieldglass, eccodes_message_count(ec, path))


def grib_task(path: Path, item: dict, probes: list, sentinel) -> Result:
    """One chunk of one message's probes against eccodes' nearest-point lookup."""
    import eccodes as ec

    res = Result(path.name, item["label"])
    handle = None
    try:
        with open(path, "rb") as fh:
            # One pass: every message before the target is released as soon as it is passed.
            for k in range(item["index"] + 1):
                handle = ec.codes_grib_new_from_file(fh)
                if handle is None:
                    return res.fail("eccodes finds fewer messages than Fieldglass does: the readers disagree on the count")
                if k < item["index"]:
                    ec.codes_release(handle)
                    handle = None
        try:
            oracle = GribOracle(ec, handle, item["grid"])
        except Skip as s:
            return res.skip(str(s))
        compare(res, probes, oracle, sentinel, (item["raster"], item["bounds"], item.get("periodic")))
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
    if not doc["items"]:
        return [Result(path.name, variable).fail("probe returned no items")]
    try:
        ds = open_dataset(path, doc["format"] == "zarr")
    except OSError as e:
        # netCDF4 carries an older libhdf5 than the one that wrote a
        # "latest format" file (Errno -101, "NetCDF: HDF error"); that is the
        # oracle's limit, not a disagreement. Any other failure to open is not.
        if "NetCDF: HDF error" not in str(e):
            raise
        # That error covers any HDF5 failure, corruption included. Tolerate it
        # only where h5py's newer libhdf5 does open the file.
        try:
            import h5py

            with h5py.File(path, "r"):
                pass
        except Exception as e2:
            for item in doc["items"]:
                results.append(Result(path.name, item["label"]).fail(f"neither netCDF4 nor h5py can open the file: {e2}"))
            return results
        for item in doc["items"]:
            results.append(Result(path.name, item["label"]).skip("the oracle (netCDF4, with an older libhdf5) cannot open the file; h5py can"))
        return results
    cache: dict = {}
    try:
        for item in doc["items"]:
            res = Result(path.name, item["label"])
            results.append(res)
            if item["status"] != "ok":
                apply_item_status(res, item)
                continue
            name = item["name"]
            if name not in ds.variables:
                res.skip("variable lives in a group the sweep does not open")
                continue
            try:
                da = ds[name]
                ydim, xdim = da.dims[item["yDim"]], da.dims[item["xDim"]]
                idx = {d: item["sliceIndices"][k] for k, d in enumerate(da.dims) if d not in (ydim, xdim)}
                plane = da.isel(idx).transpose(ydim, xdim).values
                oracle = GridOracle(ds, da, ydim, xdim, plane, cache)
            except Skip as s:
                res.skip(str(s))
                continue
            compare(res, item["probes"], oracle, None, (item["raster"], item["bounds"], None))
            finalize(res, args.n)
    finally:
        ds.close()
    return results


def no_variables_result(name: str) -> Result:
    """A NetCDF/Zarr file that lists no variable has nothing to check: a failure unless pinned."""
    return Result(name, "variables").skip("no variables listed: the file contributes no item")


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
            raise FileNotFoundError(f"{p} does not exist")
    return found


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def oracle_versions() -> str:
    parts = []
    for mod, label in (("eccodes", "eccodes"), ("xarray", "xarray"), ("netCDF4", "netCDF4")):
        try:
            m = __import__(mod)
            v = m.codes_get_api_version() if mod == "eccodes" else m.__version__
            parts.append(f"{label} {v}")
        except Exception:
            parts.append(f"{label} unavailable")
    return ", ".join(parts)


def report(results: list[Result], skipped_files: list[tuple[str, str]]) -> int:
    print(f"Oracles: {oracle_versions()}\n")
    cols = ("file", "item", "probes", "cmp", "valid", "masked", "mask!=", "max|d|", "cell!=", "place!=", "ties", "verdict")
    rows = []
    for r in results:
        if r.status == "SKIP":
            continue
        rows.append(
            (
                r.file, r.label, str(r.probes), str(r.compared), str(r.both_valid), str(r.both_masked),
                str(r.mask_disagree), f"{r.max_diff:.3g}", str(r.cell_bad), str(r.place_bad), str(r.ties), r.status,
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
    quirks = sorted({(r.file, r.note) for r in results if r.note})
    if quirks:
        print("\nOracle quirks worked around (eccodes is wrong here, not Fieldglass):")
        for f, note in quirks:
            print(f"  {f}: {note}")
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


def main(argv=None, executor=None) -> int:
    """`executor` is a factory `(max_workers=...)` for a context manager with `submit`; tests inject one."""
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
    try:
        found = collect([p for p in paths if p.exists() or args.paths])
    except FileNotFoundError as e:
        print(f"probe_parity_sweep: {e}", file=sys.stderr)
        return 2
    files = [f for f in found if not args.only or args.only in f.name]
    if not files:
        print("probe_parity_sweep: no sample files found; run tools/fetch_samples.sh first", file=sys.stderr)
        return 2

    if executor is None:
        from concurrent.futures import ProcessPoolExecutor as executor

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
        with executor(max_workers=args.jobs) as pool:
            for f, doc in docs:
                if doc["format"] in ("grib1", "grib2"):
                    ordered.append({"file": f.name, "future": pool.submit(grib_count_task, f, len(doc["items"]))})
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
                    if not doc["variables"]:
                        ordered.append({"file": f.name, "static": [no_variables_result(f.name)]})
                    for name in doc["variables"]:
                        ordered.append({"file": f.name, "future": pool.submit(xarray_task, f, args, name)})
                else:
                    # A file with a GRIB/NetCDF/Zarr name that Fieldglass cannot identify was not checked.
                    ordered.append({"file": f.name, "static": [
                        Result(f.name, "format").fail(f"Fieldglass does not recognize the format ({doc['format']!r}); not checked")
                    ]})
            results: list[Result] = []
            for slot in ordered:
                if "static" in slot:
                    results.extend(slot["static"])
                    continue
                if "future" in slot:
                    results.extend(slot["future"].result())
                    continue
                item = slot["item"]
                res = Result(slot["file"], item["label"])
                if item["status"] != "ok":
                    apply_item_status(res, item)
                else:
                    for part in slot["parts"]:
                        res.merge(part.result())
                        if res.status != "PASS":
                            break
                    finalize(res, args.n)
                results.append(res)
    except ImportError as e:
        print(f"probe_parity_sweep: {e}; use the pre-commit venv's python3", file=sys.stderr)
        return 2
    except Exception as e:  # a worker could not probe or open a file: the sweep did not run
        print(f"probe_parity_sweep: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    return report(results, skipped_files)


if __name__ == "__main__":
    sys.exit(main())
