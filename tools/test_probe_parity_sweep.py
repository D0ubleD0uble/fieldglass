#!/usr/bin/env python3
"""Self-test for tools/probe_parity_sweep.py.

    python3 tools/test_probe_parity_sweep.py

The sweep is a gate that must not go green by doing nothing, so each way it
could pass vacuously has a test named for it: no probe at all, an oracle that
answers nothing, every point a tie. The other half checks the oracles that are
pure arithmetic (the grid search, the geostationary scan-angle formula) against
cases whose answer is known without running Fieldglass.

The real comparison against eccodes and xarray, and the mutation checks that
prove it fails when a scan-order fix is reverted, need the downloaded samples
and the native addon; they are described in RELEASE-TEST-PLAN.md section 0.
"""

from __future__ import annotations

import importlib.util
import math
import sys
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "probe_parity_sweep",
    Path(__file__).resolve().parent / "probe_parity_sweep.py",
)
assert _spec and _spec.loader
sweep = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = sweep  # dataclasses look the module up by name
_spec.loader.exec_module(sweep)


def probe(j, i, value, lat=10.0, lon=20.0):
    return {"px": 0, "py": 0, "lat": lat, "lon": lon, "value": value, "gridI": i, "gridJ": j}


class FakeOracle:
    """Answers from a table: cell -> value, with a fixed nearest cell."""

    def __init__(self, nearest, values, fg_dist=None, best_dist=1.0, tol=1e-6):
        self._nearest, self._values, self._fg_dist, self._best = nearest, values, fg_dist, best_dist
        self._tol = tol

    def nearest(self, lat, lon, fg_cell):
        return sweep.Nearest(self._nearest, self._best, self._fg_dist if fg_cell != self._nearest else self._best)

    def value(self, cell):
        return self._values.get(cell)

    def tolerance(self, v):
        return self._tol


def run(probes, oracle, sentinel=None):
    res = sweep.Result("f", "item")
    sweep.compare(res, probes, oracle, sentinel)
    return sweep.finalize(res)


class CompareVerdicts(unittest.TestCase):
    def test_agreement_passes(self):
        res = run([probe(1, 2, 5.0)], FakeOracle((1, 2), {(1, 2): 5.0}))
        self.assertEqual((res.status, res.both_valid, res.cell_bad), ("PASS", 1, 0))

    def test_a_wrong_value_fails_and_is_located(self):
        res = run([probe(1, 2, 5.5)], FakeOracle((1, 2), {(1, 2): 5.0}))
        self.assertEqual(res.status, "FAIL")
        self.assertEqual(res.value_bad, 1)
        self.assertIn("gridI 2", res.max_where)

    def test_a_value_inside_the_tolerance_passes(self):
        res = run([probe(1, 2, 5.0 + 1e-9)], FakeOracle((1, 2), {(1, 2): 5.0}, tol=1e-6))
        self.assertEqual(res.status, "PASS")

    def test_masked_on_one_side_only_fails(self):
        self.assertEqual(run([probe(0, 0, None)], FakeOracle((0, 0), {(0, 0): 3.0})).mask_disagree, 1)
        self.assertEqual(run([probe(0, 0, 3.0)], FakeOracle((0, 0), {})).mask_disagree, 1)

    def test_masked_on_both_sides_counts_as_agreement(self):
        res = run([probe(0, 0, None)], FakeOracle((0, 0), {}))
        self.assertEqual((res.status, res.both_masked), ("PASS", 1))

    def test_a_different_cell_far_away_fails(self):
        res = run([probe(0, 0, 1.0)], FakeOracle((3, 3), {(0, 0): 1.0}, fg_dist=50.0, best_dist=1.0))
        self.assertEqual((res.status, res.cell_bad), ("FAIL", 1))

    def test_a_cell_within_two_percent_is_a_tie_not_a_failure(self):
        probes = [probe(0, 0, 1.0)] * 19 + [probe(0, 1, 1.0)]
        res = run(probes, FakeOracle((0, 0), {(0, 0): 1.0, (0, 1): 1.0}, fg_dist=1.01, best_dist=1.0))
        self.assertEqual((res.status, res.cell_bad, res.ties), ("PASS", 0, 1))

    def test_a_cell_the_oracle_cannot_place_is_a_failure_not_a_tie(self):
        res = run([probe(0, 1, 1.0)], FakeOracle((0, 0), {(0, 1): 1.0}, fg_dist=None))
        self.assertEqual(res.status, "FAIL")

    def test_mostly_ties_fail_since_a_half_cell_offset_looks_like_that(self):
        probes = [probe(0, 1, 1.0)] * 20
        res = run(probes, FakeOracle((0, 0), {(0, 1): 1.0}, fg_dist=1.01, best_dist=1.0))
        self.assertEqual(res.status, "FAIL")
        self.assertIn("ties", res.reason)

    def test_no_probe_is_a_failure_not_a_pass(self):
        res = run([], FakeOracle((0, 0), {}))
        self.assertEqual(res.status, "FAIL")

    def test_an_oracle_that_answers_nothing_fails(self):
        class Mute:
            def nearest(self, *a):
                raise RuntimeError("off the grid")

        res = run([probe(0, 0, 1.0)] * 4, Mute())
        self.assertEqual(res.status, "FAIL")
        self.assertEqual(res.compared, 0)

    def test_the_sentinel_is_counted_and_still_compared(self):
        res = run([probe(0, 0, -999.0)], FakeOracle((0, 0), {(0, 0): -999.0}), sentinel=-999.0)
        self.assertEqual((res.status, res.sentinel), ("PASS", 1))
        bad = run([probe(0, 0, -999.0)], FakeOracle((0, 0), {(0, 0): 4.0}), sentinel=-999.0)
        self.assertEqual(bad.status, "FAIL")

    def test_chunks_merge_into_one_verdict(self):
        a = run([probe(0, 0, 1.0)], FakeOracle((0, 0), {(0, 0): 1.0}))
        b = run([probe(0, 0, 9.0)], FakeOracle((0, 0), {(0, 0): 1.0}))
        whole = sweep.Result("f", "item")
        whole.merge(a)
        whole.merge(b)
        sweep.finalize(whole)
        self.assertEqual((whole.probes, whole.value_bad, whole.status), (2, 1, "FAIL"))

    def test_a_skipped_chunk_skips_the_item(self):
        whole = sweep.Result("f", "item")
        whole.merge(sweep.Result("f", "item", "SKIP", "reduced grid"))
        self.assertEqual(sweep.finalize(whole).status, "SKIP")


class GridSearch(unittest.TestCase):
    def setUp(self):
        try:
            import numpy as np
            import xarray as xr
        except ImportError as e:
            # CI's push-stage hooks run on a bare pip Python. The verdict tests
            # above need nothing; these need the sweep's own dependencies, which
            # the pre-commit venv's python3 has.
            self.skipTest(f"needs numpy and xarray: {e}")
        self.np, self.xr = np, xr

    def dataset(self, lat, lon):
        np, xr = self.np, self.xr
        data = np.arange(len(lat) * len(lon), dtype="float32").reshape(len(lat), len(lon))
        return xr.Dataset(
            {"v": (("lat", "lon"), data)},
            coords={
                "lat": ("lat", lat, {"units": "degrees_north"}),
                "lon": ("lon", lon, {"units": "degrees_east"}),
            },
        )

    def test_a_regular_grid_finds_the_nearest_cell_across_the_seam(self):
        np = self.np
        ds = self.dataset(np.array([90.0, 45.0, 0.0, -45.0, -90.0]), np.array([0.0, 90.0, 180.0, 270.0]))
        o = sweep.GridOracle(ds, ds["v"], "lat", "lon", ds["v"].values, {})
        near = o.nearest(1.0, -2.0, (2, 0))  # 358E: nearer 0E than 270E
        self.assertEqual(near.best, (2, 0))
        self.assertAlmostEqual(near.fg_dist, near.best_dist)
        wrong = o.nearest(1.0, -2.0, (2, 3))
        self.assertGreater(wrong.fg_dist, 10 * wrong.best_dist)

    def test_descending_latitude_is_indexed_as_stored(self):
        np = self.np
        ds = self.dataset(np.array([60.0, 30.0, 0.0]), np.array([0.0, 10.0]))
        o = sweep.GridOracle(ds, ds["v"], "lat", "lon", ds["v"].values, {})
        self.assertEqual(o.nearest(29.0, 9.0, (1, 1)).best, (1, 1))
        self.assertEqual(o.value((1, 1)), float(ds["v"].values[1, 1]))

    def test_nan_is_masked(self):
        np = self.np
        ds = self.dataset(np.array([0.0, 10.0]), np.array([0.0, 10.0]))
        plane = ds["v"].values.copy()
        plane[0, 1] = np.nan
        o = sweep.GridOracle(ds, ds["v"], "lat", "lon", plane, {})
        self.assertIsNone(o.value((0, 1)))

    def test_a_curvilinear_grid_is_searched_in_two_dimensions(self):
        np, xr = self.np, self.xr
        lat2 = np.array([[10.0, 10.5, 11.0], [11.0, 11.5, 12.0]])
        lon2 = np.array([[20.0, 21.0, 22.0], [20.5, 21.5, 22.5]])
        ds = xr.Dataset(
            {"v": (("y", "x"), np.arange(6.0).reshape(2, 3))},
            coords={
                "Latitude": (("y", "x"), lat2, {"standard_name": "latitude"}),
                "Longitude": (("y", "x"), lon2, {"standard_name": "longitude"}),
            },
        )
        o = sweep.GridOracle(ds, ds["v"], "y", "x", ds["v"].values, {})
        self.assertEqual(o.mode, "curvilinear")
        self.assertEqual(o.nearest(11.5, 21.5, (0, 0)).best, (1, 1))
        self.assertEqual(o.nearest(10.0, 20.0, (0, 0)).best, (0, 0))

    def test_a_grid_with_no_coordinates_is_skipped_not_passed(self):
        np, xr = self.np, self.xr
        ds = xr.Dataset({"v": (("y", "x"), np.zeros((2, 2)))})
        with self.assertRaises(sweep.Skip):
            sweep.GridOracle(ds, ds["v"], "y", "x", ds["v"].values, {})

    def test_geostationary_scan_angles(self):
        np, xr = self.np, self.xr
        gm = {
            "grid_mapping_name": "geostationary",
            "perspective_point_height": 35786023.0,
            "semi_major_axis": 6378137.0,
            "semi_minor_axis": 6356752.31414,
            "longitude_of_projection_origin": -75.0,
            "sweep_angle_axis": "x",
        }
        xs = np.linspace(-0.05, 0.05, 11)
        ds = xr.Dataset(
            {"v": (("y", "x"), np.zeros((11, 11))), "goes_imager_projection": ((), 0, gm)},
            coords={
                "x": ("x", xs, {"standard_name": "projection_x_coordinate", "units": "rad"}),
                "y": ("y", xs[::-1].copy(), {"standard_name": "projection_y_coordinate", "units": "rad"}),
            },
        )
        ds["v"].attrs["grid_mapping"] = "goes_imager_projection"
        o = sweep.GridOracle(ds, ds["v"], "y", "x", ds["v"].values, {})
        self.assertEqual(o.mode, "geos")
        x, y = o._geos_angles(0.0, -75.0)  # the sub-satellite point
        self.assertAlmostEqual(x, 0.0, places=12)
        self.assertAlmostEqual(y, 0.0, places=12)
        x, y = o._geos_angles(0.0, -70.0)  # east of nadir: positive x, no y
        self.assertGreater(x, 0.0)
        self.assertAlmostEqual(y, 0.0, places=12)
        # The nearest column to that point is on the +x side; y = 0 is the middle row.
        self.assertEqual(o.nearest(0.0, -75.0, (5, 5)).best, (5, 5))
        self.assertGreater(o.nearest(0.0, -70.0, (5, 5)).best[1], 5)
        _, y_north = o._geos_angles(10.0, -75.0)
        self.assertGreater(y_north, 0.0)


class Haversine(unittest.TestCase):
    def test_a_degree_of_latitude_is_about_111_km(self):
        self.assertAlmostEqual(float(sweep.haversine_km(0.0, 0.0, 1.0, 0.0)), 111.19, delta=0.1)
        self.assertTrue(math.isclose(float(sweep.haversine_km(10.0, 20.0, 10.0, 20.0)), 0.0))


if __name__ == "__main__":
    unittest.main()
