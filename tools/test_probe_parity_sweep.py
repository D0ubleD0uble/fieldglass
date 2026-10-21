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


# An 11x11 raster over lat 0..10, lon 20..30: pixel (0, 0) is at 10N 20E,
# which is where `probe()` puts every point unless a test moves it.
GEOMETRY = ((11, 11), (0.0, 10.0, 20.0, 30.0))


def run(probes, oracle, sentinel=None, geometry=GEOMETRY):
    res = sweep.Result("f", "item")
    sweep.compare(res, probes, oracle, sentinel, geometry)
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
        # Chunks are compared but not finalized: only the merged item gets a verdict.
        a, b = sweep.Result("f", "item"), sweep.Result("f", "item")
        sweep.compare(a, [probe(0, 0, 1.0)], FakeOracle((0, 0), {(0, 0): 1.0}), None, GEOMETRY)
        sweep.compare(b, [probe(0, 0, 9.0)], FakeOracle((0, 0), {(0, 0): 1.0}), None, GEOMETRY)
        whole = sweep.Result("f", "item")
        whole.merge(a)
        whole.merge(b)
        sweep.finalize(whole)
        self.assertEqual((whole.probes, whole.value_bad, whole.status), (2, 1, "FAIL"))

    def test_a_skipped_chunk_skips_the_item(self):
        whole = sweep.Result("f", "item")
        whole.merge(sweep.Result("f", "item").skip("reduced grid"))
        self.assertEqual(sweep.finalize(whole).status, "SKIP")


class Placement(unittest.TestCase):
    ORACLE = FakeOracle((0, 0), {(0, 0): 1.0})

    def test_a_pixel_where_the_raster_puts_it_passes(self):
        p = dict(probe(0, 0, 1.0, lat=5.0, lon=25.0), px=5, py=5)
        self.assertEqual(run([p], self.ORACLE).place_bad, 0)

    def test_a_flipped_or_shifted_position_fails(self):
        flipped = dict(probe(0, 0, 1.0, lat=0.0, lon=25.0), px=5, py=0)  # row 0 is the north edge
        self.assertEqual(run([flipped], self.ORACLE).status, "FAIL")
        shifted = dict(probe(0, 0, 1.0, lat=5.0, lon=26.0), px=5, py=5)
        res = run([shifted], self.ORACLE)
        self.assertEqual((res.status, res.place_bad), ("FAIL", 1))

    def test_float_noise_passes_and_half_a_pixel_does_not(self):
        # The pixel is 1 degree wide here. A centre-versus-edge mix-up is half of it.
        noise = dict(probe(0, 0, 1.0, lat=5.0, lon=25.0004), px=5, py=5)
        half_lon = dict(probe(0, 0, 1.0, lat=5.0, lon=25.5), px=5, py=5)
        half_lat = dict(probe(0, 0, 1.0, lat=5.5, lon=25.0), px=5, py=5)
        self.assertEqual(run([noise], self.ORACLE).place_bad, 0)
        self.assertEqual(run([half_lon], self.ORACLE).place_bad, 1)
        self.assertEqual(run([half_lat], self.ORACLE).place_bad, 1)

    def test_a_global_raster_closes_the_circle(self):
        # 8 pixels over 0..360 are 45 degrees apart; the last is at 315, not 360.
        bounds = (-90.0, 90.0, 0.0, 360.0)
        lat, lon, _, _ = sweep.expected_position(7, 0, (8, 3), bounds, True)
        self.assertEqual((lat, lon), (90.0, 315.0))
        p = dict(probe(0, 0, 1.0, lat=90.0, lon=-45.0), px=7, py=0)  # 315E reported as -45
        self.assertTrue(sweep.placement_ok(p, (8, 3), bounds, True))
        # A duplicated seam column spans 360 degrees too, but is not periodic:
        # its last pixel is at 360, which is 0 again.
        seam = dict(probe(0, 0, 1.0, lat=90.0, lon=0.0), px=7, py=0)
        self.assertTrue(sweep.placement_ok(seam, (8, 3), bounds, False))
        self.assertFalse(sweep.placement_ok(seam, (8, 3), bounds, True))

    def test_without_the_periodic_flag_either_step_is_accepted(self):
        bounds = (-90.0, 90.0, 0.0, 360.0)
        periodic = dict(probe(0, 0, 1.0, lat=90.0, lon=-45.0), px=7, py=0)
        seam = dict(probe(0, 0, 1.0, lat=90.0, lon=0.0), px=7, py=0)
        wrong = dict(probe(0, 0, 1.0, lat=90.0, lon=100.0), px=7, py=0)
        self.assertTrue(sweep.placement_ok(periodic, (8, 3), bounds))
        self.assertTrue(sweep.placement_ok(seam, (8, 3), bounds))
        self.assertFalse(sweep.placement_ok(wrong, (8, 3), bounds))

    def test_no_extent_means_no_check_which_fails(self):
        self.assertEqual(run([probe(0, 0, 1.0)], self.ORACLE, geometry=None).status, "FAIL")
        self.assertEqual(run([probe(0, 0, 1.0)], self.ORACLE, geometry=((11, 11), (None, 10.0, 20.0, 30.0))).status, "FAIL")


class Skips(unittest.TestCase):
    def test_an_oracle_limit_skips(self):
        for why in ("spectral field, no grid to probe", "reduced grid: gridI is a column",
                    "grid mapping 'x' has no independent oracle in the sweep"):
            self.assertEqual(sweep.Result("f", "i").skip(why).status, "SKIP", why)

    def test_a_reason_fieldglass_reports_skips_only_where_pinned(self):
        for file, label, why in sorted(sweep.PINNED_SKIPS):
            self.assertEqual(sweep.Result(file, label).skip(why).status, "SKIP", (file, label))
        # The same reasons on a file that is not pinned are a regression, or a new case to review.
        for why in ("grid is not reprojectable (unplaceable)", "grid is not reprojectable (no_raster)",
                    "no CF image axes detected"):
            res = sweep.Result("nbm.grib2", "msg#0").skip(why)
            self.assertEqual(res.status, "FAIL", why)
            self.assertIn("PINNED_SKIPS", res.reason)
        # And a pinned file with another item, or another reason, is not covered.
        self.assertEqual(sweep.Result("tiny_earth_pair.grib2", "msg#7").skip("grid is not reprojectable (unplaceable)").status, "FAIL")
        self.assertEqual(sweep.Result("tiny_earth_pair.grib2", "msg#0").skip("grid is not reprojectable (no_raster)").status, "FAIL")

    def test_any_other_reason_fails(self):
        for why in ("eccodes holds 5 points for a 2x3 grid", "no coordinate variables for the image axes",
                    "probe failed: boom", "something new"):
            res = sweep.Result("f", "i").skip(why)
            self.assertEqual(res.status, "FAIL", why)
            self.assertIn(why, res.reason)

    def test_a_probe_that_threw_is_a_failure(self):
        res = sweep.Result("f", "i")
        sweep.apply_item_status(res, {"status": "failed", "reason": "probe failed: boom"})
        self.assertEqual(res.status, "FAIL")

    def test_a_probe_side_skip_follows_the_same_list(self):
        res = sweep.Result("spectral_pair.grib2", "msg#0")
        sweep.apply_item_status(res, {"status": "skipped", "reason": "grid is not reprojectable (no_raster)"})
        self.assertEqual(res.status, "SKIP")
        res = sweep.Result("f", "i")
        sweep.apply_item_status(res, {"status": "skipped", "reason": "grid is not reprojectable (other)"})
        self.assertEqual(res.status, "FAIL")

    def test_a_failed_chunk_fails_the_item(self):
        whole = sweep.Result("f", "item")
        whole.merge(sweep.Result("f", "item").fail("eccodes finds fewer messages"))
        self.assertEqual(sweep.finalize(whole).status, "FAIL")


class CountsAndCoverage(unittest.TestCase):
    """The ways a file or an item could contribute nothing and still go green (#1314)."""

    def test_equal_nonzero_counts_pass(self):
        self.assertEqual(sweep.count_check("f.grib2", 3, 3), [])

    def test_eccodes_finding_fewer_fails(self):
        (r,) = sweep.count_check("f.grib2", 3, 2)
        self.assertEqual(r.status, "FAIL")
        self.assertIn("3 message(s), eccodes 2", r.reason)

    def test_eccodes_finding_more_fails(self):
        (r,) = sweep.count_check("f.grib2", 2, 3)
        self.assertEqual(r.status, "FAIL")

    def test_fieldglass_finding_none_fails(self):
        (r,) = sweep.count_check("f.grib2", 0, 4)
        self.assertEqual(r.status, "FAIL")

    def test_neither_reader_finding_a_message_fails(self):
        (r,) = sweep.count_check("f.grib2", 0, 0)
        self.assertEqual(r.status, "FAIL")

    def test_a_file_with_no_variables_fails_unless_pinned(self):
        self.assertEqual(sweep.no_variables_result("empty.nc").status, "FAIL")
        sweep.PINNED_SKIPS.add(("pinned_empty.nc", "variables", "no variables listed"))
        try:
            self.assertEqual(sweep.no_variables_result("pinned_empty.nc").status, "SKIP")
        finally:
            sweep.PINNED_SKIPS.discard(("pinned_empty.nc", "variables", "no variables listed"))

    def test_an_item_with_few_probes_fails(self):
        probes = [probe(0, 0, 1.0)] * 99
        res = sweep.Result("f", "item")
        sweep.compare(res, probes, FakeOracle((0, 0), {(0, 0): 1.0}), None, GEOMETRY)
        self.assertEqual(sweep.finalize(res, 200).status, "FAIL")
        self.assertIn("99 of 200", res.reason)

    def test_half_of_the_requested_probes_is_enough(self):
        res = sweep.Result("f", "item")
        sweep.compare(res, [probe(0, 0, 1.0)] * 100, FakeOracle((0, 0), {(0, 0): 1.0}), None, GEOMETRY)
        self.assertEqual(sweep.finalize(res, 200).status, "PASS")


class FakeEccodes:
    """A file of `n` messages whose handles are counted in and out."""

    def __init__(self, n):
        self.n, self.read, self.released = n, 0, []

    def codes_grib_new_from_file(self, fh):
        if self.read >= self.n:
            return None
        self.read += 1
        return f"h{self.read - 1}"

    def codes_release(self, handle):
        self.released.append(handle)


class HandleRelease(unittest.TestCase):
    def setUp(self):
        import tempfile

        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name) / "f.grib2"
        self.path.write_bytes(b"x")
        self.saved = sys.modules.get("eccodes")

    def tearDown(self):
        if self.saved is None:
            sys.modules.pop("eccodes", None)
        else:
            sys.modules["eccodes"] = self.saved
        self.tmp.cleanup()

    def test_the_count_releases_every_handle(self):
        ec = FakeEccodes(4)
        self.assertEqual(sweep.eccodes_message_count(ec, self.path), 4)
        self.assertEqual(ec.released, ["h0", "h1", "h2", "h3"])

    def test_grib_task_releases_every_handle_it_opens(self):
        ec = FakeEccodes(5)
        sys.modules["eccodes"] = ec
        item = {"label": "msg#2", "index": 2, "grid": None, "raster": (1, 1), "bounds": (0, 1, 0, 1)}
        original = sweep.GribOracle
        sweep.GribOracle = lambda *a: (_ for _ in ()).throw(sweep.Skip("reduced grid"))
        try:
            res = sweep.grib_task(self.path, item, [], None)
        finally:
            sweep.GribOracle = original
        self.assertEqual(res.status, "SKIP")
        self.assertEqual(sorted(ec.released), ["h0", "h1", "h2"])
        self.assertEqual(ec.read, 3)  # one pass, stopping at the target

    def test_a_short_file_releases_what_it_opened(self):
        ec = FakeEccodes(1)
        sys.modules["eccodes"] = ec
        item = {"label": "msg#3", "index": 3}
        res = sweep.grib_task(self.path, item, [], None)
        self.assertEqual(res.status, "FAIL")
        self.assertEqual(ec.released, ["h0"])


class SyncPool:
    """A stand-in for the process pool: runs each task on submit."""

    def __init__(self, max_workers=None):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def submit(self, fn, *a):
        from concurrent.futures import Future

        fut = Future()
        try:
            fut.set_result(fn(*a))
        except Exception as e:
            fut.set_exception(e)
        return fut


class MainWiring(unittest.TestCase):
    """main() must route the new checks to a failing exit (#1314)."""

    def setUp(self):
        import contextlib
        import io
        import tempfile

        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.saved = (sweep.run_probe, sweep.grib_task, sweep.grib_count_task)
        self.io, self.cm = io, contextlib

    def tearDown(self):
        sweep.run_probe, sweep.grib_task, sweep.grib_count_task = self.saved
        self.tmp.cleanup()

    def sweep_exit(self, name, doc, grib_probes=None, eccodes_count=None):
        (self.dir / name).write_bytes(b"x")
        sweep.run_probe = lambda path, args, *extra: doc
        if grib_probes is not None:
            def task(path, item, probes, sentinel):
                r = sweep.Result(path.name, item["label"])
                r.probes = r.compared = r.both_valid = grib_probes
                return r
            sweep.grib_task = task
        if eccodes_count is not None:
            sweep.grib_count_task = lambda path, fg: sweep.count_check(path.name, fg, eccodes_count)
        out = self.io.StringIO()
        with self.cm.redirect_stdout(out), self.cm.redirect_stderr(self.io.StringIO()):
            code = sweep.main([str(self.dir), "--n", "200"], executor=SyncPool)
        return code, out.getvalue()

    def grib_doc(self, n_items):
        item = {"status": "ok", "probes": [], "grid": None, "raster": [1, 1], "bounds": [0, 1, 0, 1]}
        return {"format": "grib2", "items": [dict(item, label=f"msg#{k}", index=k) for k in range(n_items)]}

    def test_a_grib_file_fieldglass_reads_as_empty_fails(self):
        code, out = self.sweep_exit("a.grib2", self.grib_doc(0), eccodes_count=2)
        self.assertEqual(code, 1)
        self.assertIn("Fieldglass finds 0", out)

    def test_a_count_mismatch_fails(self):
        code, out = self.sweep_exit("a.grib2", self.grib_doc(2), grib_probes=200, eccodes_count=3)
        self.assertEqual(code, 1)
        self.assertIn("message count", out)

    def test_matching_counts_and_full_probes_pass(self):
        code, _ = self.sweep_exit("a.grib2", self.grib_doc(2), grib_probes=200, eccodes_count=2)
        self.assertEqual(code, 0)

    def test_a_netcdf_file_with_no_variables_fails(self):
        code, out = self.sweep_exit("b.nc", {"format": "netcdf", "variables": []})
        self.assertEqual(code, 1)
        self.assertIn("variables", out)

    def test_an_item_with_few_probes_fails_through_main(self):
        code, out = self.sweep_exit("a.grib2", self.grib_doc(1), grib_probes=10, eccodes_count=1)
        self.assertEqual(code, 1)
        self.assertIn("10 of 200", out)

    def test_an_unrecognised_format_fails(self):
        code, out = self.sweep_exit("c.grib", {"format": "unknown"})
        self.assertEqual(code, 1)
        self.assertIn("does not recognise", out)

    def test_a_slice_with_few_probes_fails_in_xarray_task(self):
        class DA:
            dims = ("y", "x")

            def isel(self, idx):
                return self

            def transpose(self, *d):
                return self

            values = None

        class DS:
            variables = {"v": 1}

            def __getitem__(self, name):
                return DA()

            def close(self):
                pass

        item = {"label": "v[0]", "status": "ok", "name": "v", "yDim": 0, "xDim": 1, "sliceIndices": [0, 0],
                "raster": GEOMETRY[0], "bounds": GEOMETRY[1], "probes": [probe(0, 0, 1.0)] * 10}
        saved = sweep.open_dataset, sweep.GridOracle
        sweep.run_probe = lambda path, args, *extra: {"format": "netcdf", "items": [item]}
        sweep.open_dataset = lambda path, is_zarr: DS()
        sweep.GridOracle = lambda *a: FakeOracle((0, 0), {(0, 0): 1.0})
        try:
            (res,) = sweep.xarray_task(self.dir / "b.nc", type("A", (), {"n": 200})(), "v")
        finally:
            sweep.open_dataset, sweep.GridOracle = saved
        self.assertEqual(res.status, "FAIL")
        self.assertIn("10 of 200", res.reason)

    def test_a_variable_with_no_items_fails(self):
        sweep.run_probe = lambda path, args, *extra: {"format": "netcdf", "items": []}
        (res,) = sweep.xarray_task(self.dir / "b.nc", type("A", (), {"n": 200})(), "v")
        self.assertEqual((res.status, res.reason), ("FAIL", "probe returned no items"))


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
